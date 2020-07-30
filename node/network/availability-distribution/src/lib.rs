// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! The bitfield distribution
//!
//! In case this node is a validator, gossips its own signed availability bitfield
//! for a particular relay parent.
//! Independently of that, gossips on received messages from peers to other interested peers.

use codec::{Decode, Encode};
use futures::{channel::oneshot, FutureExt};

use node_primitives::{ProtocolId, View};

use log::{trace, warn};
use polkadot_erasure_coding::{
    branch_hash, branches, obtain_chunks_v1 as obtain_chunks, reconstruct_v1 as reconstruct,
};
use polkadot_primitives::v1::{
    AvailableData, CommittedCandidateReceipt, CoreState, ErasureChunk, Hash, PoV, SigningContext,
    ValidatorId, ValidatorIndex,
};
use polkadot_subsystem::messages::*;
use polkadot_subsystem::{
    ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem,
    SubsystemContext, SubsystemResult,
};
use sc_network::ReputationChange;
use std::collections::{HashMap, HashSet};

const TARGET: &'static str = "avad";

const COST_MERKLE_PROOF_INVALID: ReputationChange =
    ReputationChange::new(-100, "Bitfield signature invalid");
const COST_NOT_A_LIVE_CANDIDATE: ReputationChange =
    ReputationChange::new(-51, "Candidate is not part of the live candidates");
const COST_VALIDATOR_INDEX_INVALID: ReputationChange =
    ReputationChange::new(-100, "Bitfield validator index invalid");
const COST_MISSING_PEER_SESSION_KEY: ReputationChange =
    ReputationChange::new(-133, "Missing peer session key");
const COST_NOT_IN_VIEW: ReputationChange =
    ReputationChange::new(-51, "Not interested in that parent hash");
const COST_MESSAGE_NOT_DECODABLE: ReputationChange =
    ReputationChange::new(-100, "Not interested in that parent hash");
const COST_PEER_DUPLICATE_MESSAGE: ReputationChange =
    ReputationChange::new(-500, "Peer sent the same message multiple times");
const BENEFIT_VALID_MESSAGE_FIRST: ReputationChange =
    ReputationChange::new(15, "Valid message with new information");
const BENEFIT_VALID_MESSAGE: ReputationChange = ReputationChange::new(10, "Valid message");

/// Checked signed availability bitfield that is distributed
/// to other peers.
#[derive(Encode, Decode, Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityGossipMessage {
    /// Ankor hash of the candidate this is associated to.
    pub candidate_hash: Hash,
    /// The actual signed availability bitfield.
    pub erasure_chunk: ErasureChunk,
    // The accompanying merkle proof of the erasure chunk.
    pub merkle_proof: Vec<Vec<u8>>,
}

/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Clone)]
struct ProtocolState {
    /// Track all active peers and their views
    /// to determine what is relevant to them.
    peer_views: HashMap<PeerId, View>,

    /// Our own view.
    view: View,

    /// Caches a mapping of relay parents to live candidates
    /// and allows fast + intersection free obtaining of active heads set by unionizing.
    // relay parent -> live candidates
    cache: HashMap<Hash, HashSet<CommittedCandidateReceipt>>,

    /// allow reverse caching of view checks
    /// candidate hash -> relay parent
    reverse: HashMap<Hash, Hash>,

    /// Obtain the set of validators
    // @todo clarify if this needs to be a hash map over relay parents or not? iiuc it's not necessary, but also complicates things
    // relay parent -> validator set / our index
    validators: HashMap<Hash, (Vec<ValidatorId>, Option<ValidatorIndex>)>,

    /// Track things per relay parent
    per_relay_parent: HashMap<Hash, PerRelayParent>,
}

#[derive(Debug, Clone, Default)]
struct PerRelayParent {

    /// Track received candidate hashes and chunk indices from peers.
    received_candidates: HashMap<PeerId, HashSet<(Hash, u32)>>,

    /// Track already sent candidate hashes and the erasure chunk index to the peers.
    sent_candidates: HashMap<PeerId, HashSet<(Hash, u32)>>,

    /// A Candidate and a set of known erasure chunks in form of messages to be gossiped / distributed if the peer view wants that.
    /// This is _across_ peers and not specific to a particular one.
    /// candidate hash + erasure chunk index -> gossip message
    message_vault: HashMap<(Hash, u32), AvailabilityGossipMessage>,
}

impl ProtocolState {

    fn add_relay_parent(&mut self, relay_parent: Hash) -> &mut PerRelayParent {
        let prp = self.state.per_relay_parent.entry(relay_parent).or_default();
        prp
    }


    fn remove_relay_parent(&mut self, relay_parent: Hash) {
        let _x = self.state.per_relay_parent.remove(&relay_parent);
        self.state.validators.remove(&relay_parent);
        // @todo

        self.state.cache.remove(&relay_parent);
    }

    /// Obtain an iterator over the actively processed relay parents.
    /// Should be equivalent to the set of relay parents stored in view View
    fn cached_relay_parents<'a> (&'a self) -> impl Iterator<Item=&'a Hash> + 'a {
        debug_assert_eq!(self.cache.len(), self.view.0.len());
        self.cache.keys()
    }

    /// Unionize all cached entries for the given relay parents
    /// Ignores all non existant relay parents, so this can be used directly with a peers view.
    fn cached_relay_parents_to_live_candidates_unioned<'a>(&'a self, relay_parents: impl IntoIterator<Item = &Hash> + 'a) -> HashSet<CommittedCandidateReceipt> {
        relay_parents
            .into_iter()
            .filter_map(|relay_parent| {
                self.cache.get(relay_parent)
            })
            .map(|x| x.into_iter())
            .flatten()
            .collect::<HashSet<_>>()
    }
}

fn network_update_message(n: NetworkBridgeEvent) -> AllMessages {
    AllMessages::AvailabilityDistribution(AvailabilityDistributionMessage::NetworkBridgeUpdate(n))
}

/// Deal with network bridge updates and track what needs to be tracked
/// which depends on the message type received.
async fn handle_network_msg<Context>(
    ctx: &mut Context,
    state: &mut ProtocolState,
    bridge_message: NetworkBridgeEvent,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    match bridge_message {
        NetworkBridgeEvent::PeerConnected(peerid, _role) => {
            // insert if none already present
            state.peer_views.entry(peerid).or_default();
        }
        NetworkBridgeEvent::PeerDisconnected(peerid) => {
            // get rid of superfluous data
            state.peer_views.remove(&peerid);
        }
        NetworkBridgeEvent::PeerViewChange(peerid, view) => {
            handle_peer_view_change(ctx, state, peerid, view).await?;
        }
        NetworkBridgeEvent::OurViewChange(view) => {
            handle_our_view_change(ctx, state, view).await?;
        }
        NetworkBridgeEvent::PeerMessage(remote, bytes) => {
            if let Ok(gossiped_bitfield) =
                AvailabilityGossipMessage::decode(&mut (bytes.as_slice()))
            {
                trace!(
                    target: TARGET,
                    "Received bitfield gossip from peer {:?}",
                    &remote
                );
                process_incoming_peer_message(ctx, state, remote, gossiped_bitfield).await?;
            } else {
                modify_reputation(ctx, remote, COST_MESSAGE_NOT_DECODABLE).await?;
            }
        }
    }
    Ok(())
}

/// Handle the changes necassary when our view changes.
async fn handle_our_view_change<Context>(
    ctx: &mut Context,
    state: &mut ProtocolState,
    view: View,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let old_view = std::mem::replace(&mut (state.view), view);

    let added = state.view.difference(&old_view).collect::<Vec<_>>();

    // cache the pre relay parent
    for added in added.iter().cloned() {
        let candidates = live_candidates(ctx, std::iter::once(added.clone())).await?;

        if cfg!(debug_assert) {
            for receipt in candidates.iter() {
                debug_assert!(receipt.desc().relay_parent, added);
            }
        }

        state.cache.insert(added, candidates);

        for candidate in candidates {
            state.reverse.insert(candidate.commitment().commitments_hash, added);
        }
    }

    for candidate_receipt in state.cached_relay_parents_to_live_candidates_unioned(added) {
        let desc = candidate_receipt.descriptor();
        let para = desc.para_id;

        debug_assert!(desc.relay_parent, added);

        if let Some(pov) = query_proof_of_validity(ctx, added).await? {

            let per_relay_parent = state.per_relay_parent.get(&desc.relay_parent).expect("Must exist QED");

            // perform erasure encoding
            let head_data = query_head_data(ctx, added, para).await?;

            let available_data = AvailableData {
                pov,
                omitted_validation: OmittedValidationData::default(), // @todo sane? probably not, need two more runtime calls I guess
            };
            let erasure_chunks: Vec<Vec<u8>> = obtain_chunks(per_relay_parent.validator_set.len(), &available_data)?;

            // create proofs for each erasure chunk
            let branches = branches(erasure_chunks.as_ref());

            let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();

            let candidate_hash = pov.commitments().commitments_hash; // @todo is this correct?

            // we are a validator
            if let Some(validator_index) = state.validator_index {

                if let Some(erasure_chunk) = erasure_chunks.get(validator_index) {
                    if let Err(e) =
                        store_chunk(ctx, candidate_hash, validator_index, erasure_chunk).await?
                    {
                        warn!(target: TARGET, "Failed to store our own erasure chunk");
                    }
                } else {
                    warn!(target: TARGET, "Our validation index is out of bounds, no associated message");
                }
            }

            // obtain interested pHashMapeers in the candidate hash
            let peers: Vec<PeerId> = state.peer_views
                .iter()
                .filter(|(peer, view)| view.contains(desc.relay_parent))
                .map(|(peer, _view)| peer.clone())
                .collect();

            // distribute all erasure messages to interested peers
            for (erasure_chunk, merkle_proof) in erasure_chunks.zip(proofs) {

                // only the peers which did not receive this particular erasure chunk
                let peers = peers.iter().filter(|peer| {
                        !per_relay_parent.sent_candidates.contains(&(candidate_hash.clone(), erasure_chunk.index))
                    }).collect::<Vec<_>>();
                let message = AvailabilityGossipMessage {
                    candidate_hash,
                    erasure_chunk,
                    merkle_proof,
                };
                send_tracked_gossip_message_to_peers(ctx, &mut state, peers, message).await?;
            }
        }
    }

    let removed = old_view.difference(&state.view).collect::<Vec<_>>();
    for removed in removed {
        // cleanup relay parents we are not interested in any more
        // and drop their associated
        if let Some(candidates) = state.cache.remove(&removed) {
            for candidate in candidates {
                state.reverse.remove(candidate.commitment().commitments_hash);
            }
        }
    }
    Ok(())
}



#[inline(always)]
async fn send_tracked_gossip_message_to_peers<Context>(
    ctx: &mut Context,
    state: &mut ProtocolState,
    peer: Vec<PeerId>,
    message: AvailabilityGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    send_tracked_gossip_messages_to_peers(ctx, state, peers, iter::once(message)).await
}


async fn send_tracked_gossip_messages_to_peers<Context>(
    ctx: &mut Context,
    state: &mut ProtocolState,
    peers: Vec<PeerId>,
    message_iter: IntoIterator<Item=AvailabilityGossipMessage>,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    // @todo properly track
    // let job_data = if let Some(job_data) = state.per_relay_parent.get_mut(&message.relay_parent) {
	// 	job_data
	// } else {
	// 	return Ok(());
	// };

	// let message_sent_to_peer = &mut (job_data.message_sent_to_peer);
	// state.message_sent_to_peer
	// 	.entry(dest.clone())
	// 	.or_default()
	// 	.insert(validator.clone());

    ctx.send_messages(AllMessages::NetworkBridge(NetworkBridgeMessage::SendMessage(
        peers,
        AvailabilityDistribution::PROTOCOL_ID,
        message_iter.map(|message| message.encode())
    ))).await?;

    Ok(())
}

// Send the difference between two views which were not sent
// to that particular peer.
async fn handle_peer_view_change<Context>(
    ctx: &mut Context,
    state: &mut ProtocolState,
    origin: PeerId,
    view: View,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let current = state.peer_views.entry(origin.clone()).or_default();

    let delta_vec: Vec<Hash> = (*current).difference(&view).cloned().collect();

    *current = view;

    // only contains the intersection of what we are interested and
    // the union of all relay parent's candidates.
    let delta_candidates = state.cached_relay_parents_to_live_candidates_unioned(delta_vec.clone());


    // Send all messages we've seen before and the peer is now interested
    // in to that peer.
    let messages: Vec<_> = delta_candidates
        .into_iter()
        .filter_map(|receipt: CommittedCandidateReceipt| -> Option<HashSet<AvailabilityGossipMessage>> {
            let candidate_hash = receipt.commitments().commitments_hash; // @todo verify this is correct
            state
                .per_relay_parent
                .get(receipt.descriptor().relay_parent)
                .map(|per_relay_parent: PerRelayParent| {
                    // obtain the relevant chunk indices not sent yet
                    (0..per_relay_parent.validator_set.len())
                        .into_iter()
                        .filter(|erasure_chunk_index| {
                            // check if that erasure chunk was already sent before
                            if let Some(sent_set) = per_relay_parent.sent_candidates.get(origin) {
                                !sent_set.contains(&(candidate_hash, erasure_chunk_index))
                            } else {
                                true
                            }
                        })
                        .filter_map(|erasure_chunk_index| {
                            // try to pick up the message from the message vault
                            per_relay_parent
                                .message_vault
                                .get(&(candidate_hash, erasure_chunk_index))
                                .cloned()
                        })
                })
        })
        .flatten()
        .collect(); // must collect, since we need to borrow state mutabliy for tracking sends

    send_tracked_gossip_messages_to_peers(ctx, state, origin.clone(), messages).await?;

    Ok(())
}

/// Obtain the first key with a signing key, which must be ours. We obtain the index as `ValidatorIndex`
/// If we cannot find a key in the validator set, which we could use.
fn obtain_our_validator_index<Context>(validators: &[ValidatorId], keystore: &KeyStorePtr) -> Option<ValidatorIndex> {
	let keystore = keystore.read();
	validators.iter()
		.enumerate()
		.find_map(|(idx, v)| {
			keystore.key_pair::<ValidatorPair>(&v).ok().map(|_| idx)
		});
}

/// Handle an incoming message from a peer.
async fn process_incoming_peer_message<Context>(
    ctx: &mut Context,
    state: &mut ProtocolState,
    origin: PeerId,
    message: AvailabilityGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{

    // reverse lookup of the relay parent
    let relay_parent = if let Some(relay_parent) = state.reverse.get(&message.candidate_hash) {
        relay_parent
    } else {
        // not in reverse lookup means nobody has it in their view
        return modify_reputation(ctx, origin, COST_NOT_IN_VIEW).await;
    };

    if !state.view.contains(&relay_parent) {
        // we don't care about this, not part of our view
        // and what is not in our view shall not be gossiped to peers
        return modify_reputation(ctx, origin, COST_NOT_IN_VIEW).await;
    }

    // obtain the set of candidates we are interested in based on our current view
    let live_candidates = state.cached_relay_parents_to_live_candidates_unioned(state.view.0.iter());

    // check if the candidate is of interest
    let live_candidate = if let Some(live_candidate) = live_candidates.get(&message.candidate_hash) {
        live_candidate
    } else {
        return modify_reputation(ctx, origin, COST_NOT_A_LIVE_CANDIDATE).await;
    };

    // check the merkle proof
    let root = live_candidates.commitments().erasure_root;
    let anticipated_hash = if let Ok(hash) = branch_hash(root, &message.proof, message.erasure_chunk.index) {
        hash
    } else {
        return modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
    };

    let erasure_chunk_hash = BlakeTwo256::hash(&message.erasure_chunk);
    if anticipated_hash != erasure_chunk_hash {
        return modify_reputation(ctx, origin, COST_MERKLE_PROOF_INVALID).await;
    }

    let per_relay_parent = if let Some(ref mut per_relay_parent) = state.per_relay_parent.get_mut(&relay_parent) {
        per_relay_parent
    } else {
        warn!(target: TARGET, "Missing relay parent data");
        return Ok(())
    };

    let received_candidates = per_relay_parent
        .received_candidates
        .entry(origin)
        .or_default();

    // an internal unique identifier of this message
    let message_id = (message.candidate_hash, message.erasure_chunk.index);

    // check if this particular erasure chunk was already sent by that peer before
    let received_set = received_candidates.entry(origin.clone()).or_default();
    if received_set.contains(&message_id) {
        return modify_reputation(ctx, origin, COST_PEER_DUPLICATE_MESSAGE).await;
    } else {
        received_set.insert(message_id.clone());
    }

    // insert into known messages and change reputation
    if per_relay_parent.message_vault.insert(message_id.clone(), message.clone()).is_none() {
        modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE).await?;
    } else {
        modify_reputation(ctx, origin, BENEFIT_VALID_MESSAGE_FIRST).await?;
    };

    // condense the peers to the peers with interest on the candidate
    let peers = state
        .peer_views
        .iter()
        .filter(|(peer, view)| {
            // peers view must contain the relay parent that is associated to the candidate hash
            view.contains(&relay_parent)
        })
        .filter(|(peer, view)| {
            // avoid sending duplicate messages
            per_relay_parent
                .sent_candidates
                .entry(peer)
                .or_default()
                .contains(&message_id)
        })
        .collect();

    // gossip that message to interested peers
    send_tracked_gossip_message_to_peers(ctx, state, peers, message).await
}

/// The bitfield distribution subsystem.
pub struct AvailabilityDistribution;

impl AvailabilityDistribution {
    /// The protocol identifier for bitfield distribution.
    const PROTOCOL_ID: ProtocolId = *b"avad";

    /// Number of ancestors to keep around for the relay-chain heads.
    const K: usize = 3;

    /// Start processing work as passed on from the Overseer.
    async fn run<Context>(mut ctx: Context) -> SubsystemResult<()>
    where
        Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
    {
        // startup: register the network protocol with the bridge.
        ctx.send_message(AllMessages::NetworkBridge(
            NetworkBridgeMessage::RegisterEventProducer(Self::PROTOCOL_ID, network_update_message),
        ))
        .await?;

        // work: process incoming messages from the overseer.
        let mut state = ProtocolState::default();
        loop {
            let message = ctx.recv().await?;
            match message {
                FromOverseer::Communication {
                    msg: AvailabilityDistributionMessage::NetworkBridgeUpdate(event),
                } => {
                    trace!(target: TARGET, "Processing NetworkMessage");
                    // a network message was received
                    if let Err(e) = handle_network_msg(&mut ctx, &mut state, event).await {
                        warn!(
                            target: TARGET,
                            "Failed to handle incomming network messages: {:?}", e
                        );
                    }
                }
                FromOverseer::Communication {
                    msg: AvailabilityDistributionMessage::DistributeChunk(hash, erasure_chunk),
                } => {
                    trace!(target: TARGET, "Processing incoming erasure chunk");
                }
                FromOverseer::Communication {
                    msg: AvailabilityDistributionMessage::FetchChunk(hash, erasure_chunk),
                } => {
                    trace!(target: TARGET, "Processing incoming erasure chunk");
                }
                FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
                    activated,
                    deactivated,
                })) => {
                    for relay_parent in activated {
                        trace!(target: TARGET, "Start {:?}", relay_parent);
                        let validators = query_validators(ctx, added).await?;
                        let validator_index = obtain_our_validator_index(&validators, keystore);
                        state.validators.insert(relay_parent, (validators, validator_index));
                    }
                    for relay_parent in deactivated {
                        trace!(target: TARGET, "Stop {:?}", relay_parent);
                        state.validators.remove(&relay_parent);
                    }
                }
                FromOverseer::Signal(OverseerSignal::BlockFinalized(_)) => {}
                FromOverseer::Signal(OverseerSignal::Conclude) => {
                    trace!(target: TARGET, "Conclude");
                    return Ok(());
                }
            }
        }
    }
}

impl<C> Subsystem<C> for AvailabilityDistribution
where
    C: SubsystemContext<Message = AvailabilityDistributionMessage> + Sync + Send,
{
    fn start(self, ctx: C) -> SpawnedSubsystem {
        SpawnedSubsystem {
            name: "availability-distribution",
            future: Box::pin(async move { Self::run(ctx) }.map(|_| ())),
        }
    }
}

/// Obtain all live candidates based on a iterator of relay heads.
async fn live_candidates(
    ctx: &mut Context,
    relay_parents: impl IntoIterator<Item = Hash>,
) -> SubsystemResult<HashSet<CommittedCandidateReceipt>>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let paras = vec![]; // @todo how to obtain all the paras
    let mut iter = relay_parents.into_iter();
    let hint = iter.size_hint();

    let mut acc = HashSet::with_capacity(hint.1.unwrap_or(hint.0));
    for relay_parent in iter {
        for para in paras {
            let ccr = query_pending_availability(ctx, relay_parent, para).await?;
            acc.insert(ccr);
        }
    }
    Ok(acc)
}

// @todo move these into util.rs

async fn query_para_ids<Context>(
    ctx: &mut Context,
    relay_parent: Hash,
) -> SubsystemResult<Vec<ParaId>>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let (tx, rx) = oneshot::channel();
    ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
        relay_parent,
        RuntimeApiRequest::AvailabilityCores(tx),
    )))
    .await;
    let all_para_ids = rx.await.map_err(Into::into)?;
    Ok(all_para_ids.filter_map(|core_state| {
        if let CoreState::Occupied(occupied) = core_state {
            Some(occupied.pada_id)
        } else {
            None
        }
    }))
}

/// Modify the reputation of a peer based on its behaviour.
async fn modify_reputation<Context>(
    ctx: &mut Context,
    peer: PeerId,
    rep: ReputationChange,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    trace!(
        target: TARGET,
        "Reputation change of {:?} for peer {:?}",
        rep,
        peer
    );
    ctx.send_message(AllMessages::NetworkBridge(
        NetworkBridgeMessage::ReportPeer(peer, rep),
    ))
    .await
}

async fn query_proof_of_validity<Context>(
    ctx: &mut Context,
    hash: Hash,
) -> SubsystemResult<Option<PoV>>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let (tx, rx) = oneshot::channel();
    ctx.send_message(AllMessages::AvailabilityStore(
        AvailabilityStoreMessage::QueryPoV(hash, tx),
    ))
    .await.map_err(Into::into)?;
    rx.await.map_err(Into::into)
}

async fn query_stored_chunk<Context>(
    ctx: &mut Context,
    hash: Hash,
    validator_index: ValidatorIndex,
) -> SubsystemResult<Option<ErasureChunk>>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let (tx, rx) = oneshot::channel();
    ctx.send_message(AllMessages::AvailabilityStore(
        AvailabilityStoreMessage::QueryChunk(hash, validator_index, tx),
    ))
    .await.map_err(Into::into)?;
    rx.await.map_err(Into::into)
}

async fn store_chunk<Context>(
    ctx: &mut Context,
    hash: Hash,
    validator_index: ValidatorIndex,
    erasure_chunk: ErasureChunk,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let (tx, rx) = oneshot::channel();
    ctx.send_message(AllMessages::AvailabilityStore(
        AvailabilityStoreMessage::StoreChunk(hash, validator_index, erasure_chunk, tx),
    ))
    .await.map_err(Into::into)?;
    rx.await.map_err(Into::into)
}

/// Request the head data for a particular para.
async fn query_head_data<Context>(
    ctx: &mut Context,
    relay_parent: Hash,
    para: ParaId,
) -> SubsystemResult<HeadData>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let (tx, rx) = oneshot::channel();
    ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
        relay_parent,
        RuntimeApiRequest::HeadData(para, tx),
    )))
    .await.map_err(Into::into)?;
    rx.await.map_err(Into::into)
}

/// Request the head data for a particular para.
async fn query_pending_availability<Context>(
    ctx: &mut Context,
    relay_parent: Hash,
    para: ParaId,
) -> SubsystemResult<Option<CommittedCandidateReceipt>>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let (tx, rx) = oneshot::channel();
    ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
        relay_parent,
        RuntimeApiRequest::CandidatePendingAvailability(para, tx),
    )))
    .await.map_err(Into::into)?;
    rx.await.map_err(Into::into)
}

/// Query the validator set.
async fn query_validators<Context>(
    ctx: &mut Context,
    relay_parent: Hash,
) -> SubsystemResult<Vec<ValidatorId>>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let (tx, rx) = oneshot::channel();
    let query_validators = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
        relay_parent.clone(),
        RuntimeApiRequest::Validators(tx),
    ));

    ctx.send_message(query_validators)
    .await.map_err(Into::into)?;
    rx.await.map_err(Into::into)
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use bitvec::bitvec;
    use futures::executor;
    use maplit::hashmap;
    use polkadot_primitives::v1::{AvailabilityBitfield, Signed, ValidatorPair};
    use polkadot_subsystem::test_helpers::make_subsystem_context;
    use smol_timeout::TimeoutExt;
    use sp_core::crypto::Pair;
    use std::time::Duration;

    macro_rules! view {
		( $( $hash:expr ),* $(,)? ) => [
			View(vec![ $( $hash.clone() ),* ])
		];
	}

    macro_rules! peers {
		( $( $peer:expr ),* $(,)? ) => [
			vec![ $( $peer.clone() ),* ]
		];
	}

    macro_rules! launch {
        ($fut:expr) => {
            $fut.timeout(Duration::from_millis(10))
                .await
                .expect("10ms is more than enough for sending messages.")
                .expect("Error values should really never occur.")
        };
    }

    #[test]
    fn some_test() {
        unimplemented!("No tests yet :(");
    }
}
