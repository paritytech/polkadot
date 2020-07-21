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
use futures::{
    channel::oneshot,
    FutureExt,
};

use node_primitives::{ProtocolId, View};

use polkadot_node_subsystem::messages::*;
use polkadot_node_subsystem::{
    FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemResult,
};
use polkadot_primitives::v1::{Hash, SignedAvailabilityBitfield};
use polkadot_primitives::v0::{SigningContext, ValidatorId};
use sc_network::ReputationChange;
use std::collections::{HashMap, HashSet};

const COST_SIGNATURE_INVALID: ReputationChange =
    ReputationChange::new(-100, "Bitfield signature invalid");
const COST_MISSING_PEER_SESSION_KEY: ReputationChange =
    ReputationChange::new(-133, "Missing peer session key");
const COST_NOT_INTERESTED: ReputationChange =
    ReputationChange::new(-51, "Not intersted in that parent hash");
const COST_MESSAGE_NOT_DECODABLE: ReputationChange =
    ReputationChange::new(-100, "Not intersted in that parent hash");


/// Checked signed availability bitfield that is distributed
/// to other peers.
#[derive(Encode, Decode, Debug, Clone)]
pub struct BitfieldGossipMessage {
	/// The relay parent this message is relative to.
	pub relay_parent: Hash,
	/// The actual signed availability bitfield.
    pub signed_availability: SignedAvailabilityBitfield,
}


/// Data used to track information of peers and relay parents the
/// overseer ordered us to work on.
#[derive(Default, Clone)]
struct Tracker {
    /// track all active peers and their views
    /// to determine what is relevant to them.
    peer_views: HashMap<PeerId, View>,

    /// Our current view.
    view: View,

    /// Additional data particular to a relay parent.
    per_relay_parent: HashMap<Hash, PerRelayParentData>,
}

/// Data for a particular relay parent.
#[derive(Debug, Clone, Default)]
struct PerRelayParentData {
    /// Signing context for a particular relay parent.
    signing_context: SigningContext<Hash>,

    /// Set of validators for a particular relay parent.
    validator_set: Vec<ValidatorId>,

    /// Set of validators for a particular relay parent for which we
    /// received a valid `BitfieldGossipMessage`.
    /// Also serves as the list of known messages for peers connecting
    /// after bitfield gossips were already received.
    one_per_validator: HashMap<ValidatorId, BitfieldGossipMessage>,

    /// which messages of which validators were already sent
    message_sent_to_peer: HashMap<PeerId, HashSet<ValidatorId>>,
}

impl PerRelayParentData {
    /// Determines if that particular message signed by a validator is needed by the given peer.
    fn message_from_validator_needed_by_peer(&self, peer: &PeerId, validator: &ValidatorId) -> bool {
        if let Some(set) = self.message_sent_to_peer.get(peer) {
            !set.contains(validator)
        } else {
            false
        }
    }
}

fn network_update_message(n: NetworkBridgeEvent) -> AllMessages {
    AllMessages::BitfieldDistribution(BitfieldDistributionMessage::NetworkBridgeUpdate(n))
}

/// The bitfield distribution subsystem.
pub struct BitfieldDistribution;

impl BitfieldDistribution {
    /// The protocol identifier for bitfield distribution.
    const PROTOCOL_ID: ProtocolId = *b"bitd";

    /// Start processing work as passed on from the Overseer.
    async fn run<Context>(mut ctx: Context) -> SubsystemResult<()>
    where
        Context: SubsystemContext<Message = BitfieldDistributionMessage>,
    {
        // startup: register the network protocol with the bridge.
        ctx.send_message(AllMessages::NetworkBridge(
            NetworkBridgeMessage::RegisterEventProducer(Self::PROTOCOL_ID, network_update_message),
        ))
        .await?;

        // work: process incoming messages from the overseer and process accordingly.
        let mut tracker = Tracker::default();
        loop {
            {
                let message = ctx.recv().await?;
                match message {
                    FromOverseer::Communication { msg } => {
                        // another subsystem created this signed availability bitfield messages
                        match msg {
                            // relay_message a bitfield via gossip to other validators
                            BitfieldDistributionMessage::DistributeBitfield(
                                hash,
                                signed_availability,
                            ) => {
                                let msg = BitfieldGossipMessage {
                                    relay_parent: hash,
                                    signed_availability,
                                };
                                relay_message(&mut ctx, &mut tracker, msg).await?;
                            }
                            BitfieldDistributionMessage::NetworkBridgeUpdate(event) => {
                                // a network message was received
                                if let Err(e) = handle_network_msg(&mut ctx, &mut tracker, event).await {
                                    log::warn!(target: "bitd", "Failed to handle incomming network messages: {:?}", e);
                                }
                            }
                        }
                    }
                    FromOverseer::Signal(OverseerSignal::StartWork(relay_parent)) => {
                        // query basic system parameters once
                        // @todo assumption: these cannot change within a session
                        let (validator_set, signing_context) =
                            query_basics(&mut ctx, relay_parent).await?;

                        let _ = tracker.per_relay_parent.insert(
                            relay_parent,
                            PerRelayParentData {
                                signing_context,
                                validator_set,
                                ..Default::default()
                            },
                        );
                    }
                    FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)) => {
                        // @todo assumption: it is good enough to prevent additional work from being
                        // scheduled, the individual futures are supposedly completed quickly
                        let _ = tracker.per_relay_parent.remove(&relay_parent);
                    }
                    FromOverseer::Signal(OverseerSignal::Conclude) => {
                        tracker.per_relay_parent.clear();
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Modify the reputation of peer based on their behaviour.
async fn modify_reputation<Context>(
    ctx: &mut Context,
    peerid: PeerId,
    rep: ReputationChange,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
    ctx.send_message(AllMessages::NetworkBridge(
        NetworkBridgeMessage::ReportPeer(peerid, rep),
    ))
    .await
}

/// Distribute a given valid bitfield message.
///
/// Can be originated by another subsystem or received via network from another peer.
async fn relay_message<Context>(
    ctx: &mut Context,
    tracker: &mut Tracker,
    message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
    // concurrently pass on the bitfield distribution to all interested peers
    let interested_peers = tracker
        .peer_views
        .iter()
        .filter_map(|(peerid, view)| {
            if view.contains(&message.relay_parent) {
                Some(peerid.clone())
            } else {
                None
            }
        })
        .collect::<Vec<PeerId>>();

    let bytes = Encode::encode(&message);
    ctx.send_message(AllMessages::NetworkBridge(
        NetworkBridgeMessage::SendMessage(
            interested_peers,
            BitfieldDistribution::PROTOCOL_ID,
            bytes,
        ),
    ))
    .await?;
    Ok(())
}


/// Handle an incoming message from a peer.
async fn process_incoming_peer_message<Context>(
    ctx: &mut Context,
    tracker: &mut Tracker,
    origin: PeerId,
    message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
    // we don't care about this, not part of our view
    if !tracker.view.contains(&message.relay_parent) {
        return modify_reputation(ctx, origin, COST_NOT_INTERESTED).await;
    }

    // Ignore anything the overseer did not tell this subsystem to work on
    let mut job_data = tracker.per_relay_parent.get_mut(&message.relay_parent);
    let job_data: &mut _ = if let Some(ref mut job_data) = job_data {
        job_data
    } else {
        return modify_reputation(ctx, origin, COST_NOT_INTERESTED).await;
    };

    let validator_set = &job_data.validator_set;
    if validator_set.len() == 0 {
        return modify_reputation(ctx, origin, COST_MISSING_PEER_SESSION_KEY).await;
    }

    // check all validators that could have signed this message
    // @todo there must be a better way figuring this out cheaply
    let signing_context = job_data.signing_context.clone();
    if let Some(validator) = validator_set.iter().find(|validator| {
        message
            .signed_availability
            .check_signature(&signing_context, validator)
            .is_ok()
    }) {
        let one_per_validator = &mut (job_data.one_per_validator);
        // only relay_message a message of a validator once
        if one_per_validator.get(validator).is_some() {
            return Ok(());
        }
        one_per_validator.insert(validator.clone(), message.clone());


        // track which messages that peer already received
        let message_sent_to_peer = &mut (job_data.message_sent_to_peer);
        message_sent_to_peer
            .entry(origin)
            .or_insert_with(|| {
                HashSet::default()
            })
            .insert(validator.clone());
    } else {
        return modify_reputation(ctx, origin, COST_SIGNATURE_INVALID).await;
    }

    // passed all conditions, distribute to peers!
    relay_message(ctx, tracker, message).await?;

    Ok(())
}

/// Deal with network bridge updates and track what needs to be tracked
async fn handle_network_msg<Context>(
    ctx: &mut Context,
    tracker: &mut Tracker,
    bridge_message: NetworkBridgeEvent,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
    match bridge_message {
        NetworkBridgeEvent::PeerConnected(peerid, _role) => {
            // insert if none already present
            tracker.peer_views.entry(peerid).or_insert(View::default());
        }
        NetworkBridgeEvent::PeerDisconnected(peerid) => {
            // get rid of superfluous data
            tracker.peer_views.remove(&peerid);
        }
        NetworkBridgeEvent::PeerViewChange(peerid, view) => {
            catch_up_messages(ctx, tracker, peerid, view).await?;
        }
        NetworkBridgeEvent::OurViewChange(view) => {
            let old_view = std::mem::replace(&mut (tracker.view), view);

            for new in tracker.view.difference(&old_view) {
                if !tracker.per_relay_parent.contains_key(&new) {
                    log::warn!(target: "bitd", "Our view contains {} but the overseer never told use we should work on this", &new);
                }
            }
        }
        NetworkBridgeEvent::PeerMessage(remote, bytes) => {
            if let Ok(gossiped_bitfield) = BitfieldGossipMessage::decode(&mut (bytes.as_slice())) {
                log::trace!(target: "bitd", "Received bitfield gossip from peer {:?}", &remote);
                process_incoming_peer_message(ctx, tracker, remote, gossiped_bitfield).await?;
            } else {
                return modify_reputation(ctx, remote, COST_MESSAGE_NOT_DECODABLE).await;
            }
        }
    }
    Ok(())
}

// Send the difference between two views which were not sent
// to that particular peer.
async fn catch_up_messages<Context>(
    ctx: &mut Context,
    tracker: &mut Tracker,
    origin: PeerId,
    view: View,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
    use std::collections::hash_map::Entry;
    let current = tracker
        .peer_views
        .entry(origin.clone())
        .or_default();


    let delta_vec: Vec<Hash> = (*current).difference(&view).cloned().collect();

    *current = view;

    // Send all messages we've seen before and the peer is now interested
    // in to that peer.

    let delta_set: HashMap<ValidatorId, BitfieldGossipMessage> = delta_vec.into_iter()
        .filter_map(|new_relay_parent_interest| {
            if let Some(per_job) = (&*tracker).per_relay_parent.get(&new_relay_parent_interest) {
                // send all messages
                let one_per_validator = per_job.one_per_validator.clone();
                let origin = origin.clone();
                Some(one_per_validator.into_iter().filter(move |(validator, _message)| {
                    // except for the ones the peer already has
                    // let validator = validator.clone();
                    per_job.message_from_validator_needed_by_peer(&origin, validator)
                }))
            } else {
                // A relay parent is in the peers view, which is not in ours, ignore those.
                None
            }
        })
        .flatten()
        .collect();

    for (validator, message) in delta_set.into_iter() {
        send_tracked_gossip_message(ctx, tracker, origin.clone(), validator, message).await?;
    }

    Ok(())
}


/// Send a gossip message and track it in the per relay parent data.
async fn send_tracked_gossip_message<Context>(
    ctx: &mut Context,
    tracker: &mut Tracker,
    dest: PeerId,
    validator: ValidatorId,
    message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
    let per_job = if let Some(per_job) = tracker.per_relay_parent.get_mut(&message.relay_parent) {
        per_job
    } else {
        // TODO punishing here seems unreasonable
        return Ok(());
    };

    let message_sent_to_peer = &mut (per_job.message_sent_to_peer);
    message_sent_to_peer
        .entry(dest.clone())
        .or_default()
        .insert(validator.clone());


    let bytes = Encode::encode(&message);
    ctx.send_message(AllMessages::NetworkBridge(
            NetworkBridgeMessage::SendMessage(
                vec![dest],
                BitfieldDistribution::PROTOCOL_ID,
                bytes,
            ),
        ))
        .await?;
    Ok(())
}

impl<C> Subsystem<C> for BitfieldDistribution
where
    C: SubsystemContext<Message = BitfieldDistributionMessage> + Sync + Send,
{
    fn start(self, ctx: C) -> SpawnedSubsystem {
        SpawnedSubsystem {
            name: "bitfield-distribution",
            future: Box::pin(async move { Self::run(ctx) }.map(|_| ())),
        }
    }
}

/// query the validator set and signing context
async fn query_basics<Context>(
    ctx: &mut Context,
    relay_parent: Hash,
) -> SubsystemResult<(Vec<ValidatorId>, SigningContext)>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage>,
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
    use bitvec::{bitvec, vec::BitVec};
    use polkadot_primitives::v1::AvailabilityBitfield;
    use polkadot_primitives::v0::{Signed, ValidatorPair};
    use sp_core::crypto::Pair;
	use futures::executor;

    fn generate_valid_message() -> AllMessages {
        // AllMessages::BitfieldDistribution(BitfieldDistributionMessage::DistributeBitfield())
        unimplemented!()
    }

    fn generate_invalid_message() -> AllMessages {
        unimplemented!()
    }

    macro_rules! msg_sequence {
        ($( $input:expr ),+ $(,)? ) => [
            vec![ $( FromOverseer::Communication { msg: $input } ),+ ]
        ];
    }

    macro_rules! view {
        ( $( $hash:expr ),+ $(,)? ) => [
            View(vec![ $( $hash.clone() ),+ ])
        ];
    }

    #[test]
    fn relay_must_work() {
		let hash_a: Hash = [0; 32].into(); // us
		let hash_b: Hash = [1; 32].into(); // other

		let peer_a = PeerId::random();
        let peer_b = PeerId::random();


        let context = SigningContext {
            session_index: 1,
            parent_hash: hash_a.clone(),
        };

        // validator 0 key pair
        let (validator_pair, _seed) = ValidatorPair::generate();
        let validator = validator_pair.public();

        let payload = AvailabilityBitfield(bitvec![bitvec::order::Lsb0, u8; 1u8; 32]);
        let signed = Signed::<AvailabilityBitfield>::sign(
            payload, &context, 0, &validator_pair);

        let input = msg_sequence![
            BitfieldDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::OurViewChange(view![hash_a, hash_b])),
            BitfieldDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerConnected(peer_b.clone(), ObservedRole::Full)),
            BitfieldDistributionMessage::NetworkBridgeUpdate(NetworkBridgeEvent::PeerViewChange(peer_b.clone(), view![hash_a, hash_b])),
            BitfieldDistributionMessage::DistributeBitfield(hash_b.clone(), signed.clone()),
        ];

        let mut tracker = Tracker::default();

        let pool = sp_core::testing::SpawnBlockingExecutor::new();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context::<BitfieldDistributionMessage, _>(pool);

        //
		executor::block_on(async move {

            println!("11111111111");
            for input in input {
                println!("msg");
                handle.send(input.into());
            }
            let _ = BitfieldDistribution::start(BitfieldDistribution, ctx).future.await;

            let msg = BitfieldGossipMessage {
                relay_parent: hash_a.clone(),
                signed_availability: signed.clone(),
            };
            println!("tx");
            let x = dbg!(send_tracked_gossip_message(&mut ctx, &mut tracker, peer_b, validator, msg).await);

            println!("wait for rx");

            while let Ok(rxd) = ctx.recv().await {
                dbg!(rxd);
            }
        });

    }
}
