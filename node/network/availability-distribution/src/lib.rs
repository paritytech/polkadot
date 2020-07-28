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
    /// The relay parent this message is relative to.
    pub relay_parent: Hash,
    /// The actual signed availability bitfield.
    pub erasure_chunk: ErasureChunk,
    // @todo incomplete!
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

    /// Data specific to one relay parent.
    per_relay_parent: HashMap<Hash, PerRelayParent>,

    /// Set of validators valid for a session.
    validator_set: Vec<ValidatorId>,
}

impl ProtocolState {}

#[derive(Default, Clone)]
struct PerRelayParent {
    /// The relay parent is backed but only
    backed_candidate: HashSet<ValidatorId>,

    ///
    erasure_chunks: HashMap<ValidatorIndex, ErasureChunk>,
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

    for added in state.view.difference(&old_view).cloned() {
        if !state.per_relay_parent.contains_key(&added) {}

        let validator_set = &state.validator_set;

        // @todo for all live candidates
        if let Some(pov) = query_proof_of_validity(ctx, added).await? {
            // perform erasure encoding
            // @todo is this only the `PoV` or more? must this not also contain

            let para = Default::default();
            let head_data = query_head_data(ctx, added, para).await?;

            let available_data = AvailableData {
                pov,
                omitted_validation: OmittedValidationData::default(), // @todo sane? correct?
            };
            let erasure_chunks: Vec<Vec<u8>> = obtain_chunks(validator_set.len(), &available_data)?;
            // create proofs for each erasure chunk

            let branches = branches(erasure_chunks.as_ref());
            let root = branches.root();

            let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();

            // use `branch_hash(root, )` to verify

            let candidate_hash = unimplemented!(); // @todo

            let validator_index = ValidatorIndex::default(); // @todo FIXME

            if let Some(erasure_chunk) = erasure_chunks.get(validator_index) {
                if let Err(e) =
                    store_chunk(ctx, candidate_hash, validator_index, erasure_chunk).await?
                {
                    warn!("Failed to store our own erasure chunk");
                }
            }
        }
    }
    for removed in old_view.difference(&state.view) {
        // cleanup relay parents we are not interested in any more
        let _ = state.per_relay_parent.remove(&removed);
    }
    Ok(())
}

async fn send_gossip_messages_to_peer<Context>(
    ctx: &mut Context,
    peer: PeerId,
    messages: Vec<AvailabilityGossipMessage>,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
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

    // Send all messages we've seen before and the peer is now interested
    // in to that peer.

    // let delta_set: Vec<(ValidatorId, AvailabilityGossipMessage)> = delta_vec
    //     .into_iter()
    //     .filter_map(|new_relay_parent_interest| {
    //         if let Some(job_data) = (&*state).per_relay_parent.get(&new_relay_parent_interest) {
    //             // Send all jointly known messages for a validator (given the current relay parent)
    // 			// to the peer `origin`...

    // 			// @todo
    //             // let one_per_validator = job_data.one_per_validator.clone();
    //             // let origin = origin.clone();
    //             // Some(
    //             //     one_per_validator
    //             //         .into_iter()
    //             //         .filter(move |(validator, _message)| {
    //             //             // ..except for the ones the peer already has
    //             //             job_data.message_from_validator_needed_by_peer(&origin, validator)
    //             //         }),
    // 			// )
    // 			Som(())
    //         } else {
    //             // A relay parent is in the peers view, which is not in ours, ignore those.
    //             None
    //         }
    //     })
    //     .flatten()
    //     .collect();

    // for (validator, message) in delta_set.into_iter() {
    //     send_tracked_gossip_message(ctx, state, origin.clone(), validator, message).await?;
    // }

    unimplemented!("peer view change");

    // @todo obtain the added difference

    // @todo check if we have messages, that we did not send to the peer just yet

    // @todo send those messages to the peer

    Ok(())
}

/// Send a gossip message and track it in the per relay parent data.
async fn send_tracked_gossip_message<Context>(
    ctx: &mut Context,
    state: &mut ProtocolState,
    dest: PeerId,
    validator: ValidatorId,
    message: AvailabilityGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let job_data = if let Some(job_data) = state.per_relay_parent.get_mut(&message.relay_parent) {
        job_data
    } else {
        return Ok(());
    };

    let message_sent_to_peer = &mut (job_data.message_sent_to_peer);
    message_sent_to_peer
        .entry(dest.clone())
        .or_default()
        .insert(validator.clone());

    ctx.send_message(AllMessages::NetworkBridge(
        NetworkBridgeMessage::SendMessage(
            vec![dest],
            AvailabilityDistribution::PROTOCOL_ID,
            message.encode(),
        ),
    ))
    .await?;

    Ok(())
}

async fn obtain_validator_set_and_our_index() -> SubsystemResult<()> {
    Ok(())
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
    // we don't care about this, not part of our view
    if !state.view.contains(&message.relay_parent) {
        return modify_reputation(ctx, origin, COST_NOT_IN_VIEW).await;
    }

    // Ignore anything the overseer did not tell this subsystem to work on
    let mut job_data = state.per_relay_parent.get_mut(&message.relay_parent);
    let job_data: &mut _ = if let Some(ref mut job_data) = job_data {
        job_data
    } else {
        return modify_reputation(ctx, origin, COST_NOT_IN_VIEW).await;
    };

    // @todo !?
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
                    }
                    for relay_parent in deactivated {
                        trace!(target: TARGET, "Stop {:?}", relay_parent);
                    }
                }
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

/// Based on a iterator of relay heads.
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
    let x = rx.await.expect("FIXME");
    Ok(x.filter_map(|core_state| {
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
    .await
    .expect("FIXME");
    let x = rx.await.expect("FIXME");
    Ok(x)
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
    .await
    .expect("FIXME");
    let x = rx.await.expect("FIXME");
    Ok(x)
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
    .await;
    let x = rx.await.expect("FIXME");
    Ok(x)
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
    .await;
    let x = rx.await.expect("FIXME");
    Ok(x)
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
    .await;
    let x = rx.await.expect("FIXME");
    Ok(x)
}

/// Query basic information from the runtime we need to operate.
async fn query_basics<Context>(
    ctx: &mut Context,
    relay_parent: Hash,
) -> SubsystemResult<(Vec<ValidatorId>, SigningContext)>
where
    Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
    let (validators_tx, validators_rx) = oneshot::channel();
    let (signing_tx, signing_rx) = oneshot::channel();

    let query_validators = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
        relay_parent.clone(),
        RuntimeApiRequest::Validators(validators_tx),
    ));

    let query_signing = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
        relay_parent.clone(),
        RuntimeApiRequest::SigningContext(signing_tx),
    ));

    ctx.send_messages(std::iter::once(query_validators).chain(std::iter::once(query_signing)))
        .await?;

    Ok((validators_rx.await?, signing_rx.await?))
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
