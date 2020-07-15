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
    /// received a valid `BitfieldGossipMessage` and gossiped it to
    /// interested peers.
    one_per_validator: HashSet<ValidatorId>,
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
        Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
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
                            // distribute a bitfield via gossip to other validators.
                            BitfieldDistributionMessage::DistributeBitfield(
                                hash,
                                signed_availability,
                            ) => {
                                let msg = BitfieldGossipMessage {
                                    relay_parent: hash,
                                    signed_availability,
                                };
                                // @todo are we subject to sending something multiple times?
                                // @todo this also apply to ourself?
                                distribute(&mut ctx, &mut tracker, msg).await?;
                            }
                            BitfieldDistributionMessage::NetworkBridgeUpdate(event) => {
                                // a network message was received
                                if let Err(e) = handle_network_msg(&mut ctx, &mut tracker, event).await {
                                    log::warn!("Failed to handle incomming network messages: {:?}", e);
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
                        // @todo assumption: it is good enough to prevent addition work from being
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

/// Modify the reputiation of peer based on their behaviour.
async fn modify_reputiation<Context>(
    ctx: &mut Context,
    peerid: PeerId,
    rep: ReputationChange,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
    ctx.send_message(AllMessages::NetworkBridge(
        NetworkBridgeMessage::ReportPeer(peerid, rep),
    ))
    .await
}

/// Distribute a given valid bitfield message.
///
/// Can be originated by another subsystem or received via network from another peer.
async fn distribute<Context>(
    ctx: &mut Context,
    tracker: &mut Tracker,
    message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
    let BitfieldGossipMessage {
        relay_parent,
        signed_availability,
    } = message;
    // concurrently pass on the bitfield distribution to all interested peers
    let interested_peers = tracker
        .peer_views
        .iter()
        .filter_map(|(peerid, view)| {
            if view.contains(&relay_parent) {
                Some(peerid.clone())
            } else {
                None
            }
        })
        .collect::<Vec<PeerId>>();

    let message = BitfieldGossipMessage {
        relay_parent,
        signed_availability,
    };
    let bytes = message.encode();
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
    peerid: PeerId,
    message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
    // we don't care about this, not part of our view
    if !tracker.view.contains(&message.relay_parent) {
        return modify_reputiation(ctx, peerid, COST_NOT_INTERESTED).await;
    }

    // Ignore anything the overseer did not tell this subsystem to work on
    let mut job_data = tracker.per_relay_parent.get_mut(&message.relay_parent);
    let job_data: &mut _ = if let Some(ref mut job_data) = job_data {
        job_data
    } else {
        return modify_reputiation(ctx, peerid, COST_NOT_INTERESTED).await;
    };

    let validator_set = &job_data.validator_set;
    if validator_set.len() == 0 {
        return modify_reputiation(ctx, peerid, COST_MISSING_PEER_SESSION_KEY).await;
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
        // only distribute a message of a validator once
        if one_per_validator.contains(validator) {
            return Ok(());
        }
        one_per_validator.insert(validator.clone());
    } else {
        return modify_reputiation(ctx, peerid, COST_SIGNATURE_INVALID).await;
    }

    // passed all conditions, distribute!
    distribute(ctx, tracker, message).await?;

    Ok(())
}

/// A gossiped or gossipable signed availability bitfield for a particular relay parent.
#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
pub struct BitfieldGossipMessage {
    relay_parent: Hash,
    signed_availability: SignedAvailabilityBitfield,
}

/// Deal with network bridge updates and track what needs to be tracked
async fn handle_network_msg<Context>(
    ctx: &mut Context,
    tracker: &mut Tracker,
    bridge_message: NetworkBridgeEvent,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
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
            tracker
                .peer_views
                .entry(peerid)
                .and_modify(|val| *val = view);
        }
        NetworkBridgeEvent::OurViewChange(view) => {
            let old_view = std::mem::replace(&mut (tracker.view), view);

            for new in tracker.view.difference(&old_view) {
                if !tracker.per_relay_parent.contains_key(&new) {
                    log::warn!("Our view contains {} but the overseer never told use we should work on this", &new);
                }
            }
        }
        NetworkBridgeEvent::PeerMessage(remote, bytes) => {
            log::info!("Got a peer message from {:?}", &remote);
            if let Ok(gossiped_bitfield) = BitfieldGossipMessage::decode(&mut (bytes.as_slice())) {
                process_incoming_peer_message(ctx, tracker, remote, gossiped_bitfield).await?;
            } else {
                return modify_reputiation(ctx, remote, COST_MESSAGE_NOT_DECODABLE).await;
            }
        }
    }
    Ok(())
}

impl<C> Subsystem<C> for BitfieldDistribution
where
    C: SubsystemContext<Message = BitfieldDistributionMessage> + Clone + Sync + Send,
{
    fn start(self, ctx: C) -> SpawnedSubsystem {
        SpawnedSubsystem(Box::pin(async move { Self::run(ctx) }.map(|_| ())))
    }
}

/// query the validator set and signing context
async fn query_basics<Context>(
    ctx: &mut Context,
    relay_parent: Hash,
) -> SubsystemResult<(Vec<ValidatorId>, SigningContext)>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
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

    fn generate_valid_message() -> AllMessages {
        // AllMessages::BitfieldDistribution(BitfieldDistributionMessage::DistributeBitfield())
        unimplemented!()
    }

    fn generate_invalid_message() -> AllMessages {
        unimplemented!()
    }

    #[test]
    fn game_changer() {
        // @todo
    }
}
