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

//! The bitfield distribution subsystem spreading @todo .

use codec::{Codec, Decode, Encode};
use futures::{
    channel::oneshot,
    future::{abortable, AbortHandle, Abortable},
    stream::{FuturesUnordered, Stream, StreamExt},
    Future, FutureExt,
};

use node_primitives::{ProtocolId, SignedFullStatement, View};
use polkadot_network::protocol::Message;
use polkadot_node_subsystem::messages::*;
use polkadot_node_subsystem::{
    FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemResult,
};
use polkadot_primitives::parachain::SignedAvailabilityBitfield;
use polkadot_primitives::parachain::{SigningContext, ValidatorId};
use polkadot_primitives::Hash;
use sc_network::ReputationChange;
use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
};

const COST_SIGNATURE_INVALID: ReputationChange =
    ReputationChange::new(-100, "Bitfield signature invalid");
const COST_MISSING_PEER_SESSION_KEY: ReputationChange =
    ReputationChange::new(-133, "Missing peer session key");
const COST_MULTIPLE_BITFIELDS_FROM_PEER: ReputationChange =
    ReputationChange::new(-22, "Received more than once bitfield from peer");
const COST_NOT_INTERESTED: ReputationChange =
    ReputationChange::new(-51, "Not intersted in that parent hash");
const COST_MESSAGE_NOT_DECODABLE: ReputationChange =
    ReputationChange::new(-100, "Not intersted in that parent hash");

#[derive(Default, Clone)]
struct Tracker {
    // track all active peers and their views
    // to determine what is relevant to them
    peer_views: HashMap<PeerId, View>,

    // our current view
    view: View,

    // signing context for a particular relay_parent
    jobs: HashMap<Hash, JobData>,

    // set of validators for a particular relay_parent
    per_job: HashMap<Hash, JobData>,
}

/// Data for each relay parent
#[derive(Debug, Clone, Default)]
struct JobData {
    // set of validators which already sent a message
    validator_bitset_received: HashSet<ValidatorId>,

    // signing context for a particular relay_parent
    signing_context: SigningContext<Hash>,

    // set of validators for a particular relay_parent
    validator_set: Vec<ValidatorId>,

    // set of validators for a particular relay_parent and the number of messages
    // received authored by them
    received_bitsets_per_validator: HashSet<ValidatorId, usize>,
}

fn network_update_message(n: NetworkBridgeEvent) -> AllMessages {
    AllMessages::BitfieldDistribution(BitfieldDistributionMessage::NetworkBridgeUpdate(n))
}

pub struct BitfieldDistribution;

impl BitfieldDistribution {
    const PROTOCOL_ID: ProtocolId = *b"bitd";

    async fn run<Context>(mut ctx: Context) -> SubsystemResult<()>
    where
        Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
    {
        // startup: register the network protocol with the bridge.
        ctx.send_message(AllMessages::NetworkBridge(
            NetworkBridgeMessage::RegisterEventProducer(Self::PROTOCOL_ID, network_update_message),
        ))
        .await?;

        // set of active heads the overseer told us to work on with the connected
        // tasks abort handles
        let mut active_jobs = HashMap::<Hash, AbortHandle>::new();
        let mut futurama = HashMap::<Hash, Pin<Box<dyn Future<Output = ()>>>>::new();
        let mut tracker = Tracker::default();
        loop {
            {
                let message = {
                    let mut ctx = ctx.clone();
                    ctx.recv().await?
                };
                match message {
                    FromOverseer::Communication { msg } => {
                        // we signed this bitfield
                        match msg {
                            // Distribute a bitfield via gossip to other validators.
                            BitfieldDistributionMessage::DistributeBitfield(
                                hash,
                                signed_availability,
                            ) => {
                                let msg = BitfieldGossipMessage {
                                    relay_parent: hash,
                                    signed_availability,
                                };
                                distribute(ctx.clone(), &mut tracker, msg).await?;
                            }
                            BitfieldDistributionMessage::NetworkBridgeUpdate(event) => {
                                handle_network_msg(ctx.clone(), &mut tracker, event).await;
                            }
                        }
                    }
                    FromOverseer::Signal(OverseerSignal::StartWork(relay_parent)) => {
                        let (validator_set, signing_context) =
                            query_basics(ctx.clone(), relay_parent).await?;

                        let _ = tracker.per_job.insert(
                            relay_parent,
                            JobData {
                                signing_context,
                                validator_set: validator_set,
                                ..Default::default()
                            },
                        );
                        // futurama.insert(relay_parent.clone(), Box::pin(future.map(|_| ())));
                        // active_jobs.insert(relay_parent.clone(), abort_handle);
                    }
                    FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)) => {
                        // if let Some(abort_handle) = active_jobs.remove(&relay_parent) {
                        //     let _ = abort_handle.abort();
                        // }
                        // let _ = futurama.remove(&relay_parent);
                        let _ = tracker.per_job.remove(&relay_parent);
                    }
                    FromOverseer::Signal(OverseerSignal::Conclude) => {
                        tracker.per_job.clear();
                        // let unordered = futurama
                        //     .drain()
                        //     .map(|(_relay_parent, future)| future)
                        //     .zip(
                        //         active_jobs
                        //             .drain()
                        //             .map(|(_relay_parent, abort_handle)| abort_handle),
                        //     )
                        //     .map(|(future, abort_handle)| {
                        //         abort_handle.abort();
                        //         // TODO pipe to cleanup state
                        //         future
                        //     })
                        //     .collect::<FuturesUnordered<_>>();

                        // let _ = async move { unordered.into_future().await }.await;
                        return Ok(());
                    }
                }
            }
            // active_jobs
            //     .retain(|_, future| future.poll().is_pending());
        }
    }
}

/// modify the reputiation, good or bad
async fn modify_reputiation<Context>(
    mut ctx: Context,
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

/// Distribute a checked message, either originated by us or gossiped on from other peers.
async fn distribute<Context>(
    mut ctx: Context,
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
        .filter(|(_peerid, view)| view.contains(&relay_parent))
        .map(|(peerid, _)| peerid.clone())
        .collect::<Vec<PeerId>>();
    let message = BitfieldGossipMessage {
        relay_parent: relay_parent,
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

/// Handle an incoming message from a peer
async fn process_incoming_peer_message<Context>(
    mut ctx: Context,
    tracker: &mut Tracker,
    peerid: PeerId,
    message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
    let peer_view = if let Some(peer_view) = tracker.peer_views.get(&peerid) {
        peer_view
    } else {
        return modify_reputiation(ctx, peerid, COST_NOT_INTERESTED).await;
    };

    let job_data = if let Some(job_data) = tracker.per_job.get(&message.relay_parent) {
        job_data
    } else {
        return modify_reputiation(ctx, peerid, COST_NOT_INTERESTED).await;
    };

    // @todo should we only distribute availability messages to peer if they are relevant to us
    // or is the only discriminator if the peer cares about it?
    if !peer_view.contains(&message.relay_parent) {
        // we don't care about this, the other side should have known better
        return modify_reputiation(ctx, peerid, COST_NOT_INTERESTED).await;
    }

    let validator_set = &job_data.validator_set;
    if validator_set.len() == 0 {
        return modify_reputiation(ctx, peerid, COST_MISSING_PEER_SESSION_KEY).await;
    }

    // check all validators that could have signed this message
    if validator_set.iter().find(|validator| {
        message
            .signed_availability
            .check_signature(&job_data.signing_context, validator)
            .is_ok()
    }).is_none() {
        return modify_reputiation(ctx, peerid, COST_SIGNATURE_INVALID).await;
    }

    distribute(ctx, tracker, message).await?;

    Ok(())
}

/// A gossiped or gossipable signed availability bitfield for a particular relay hash
#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
pub struct BitfieldGossipMessage {
    relay_parent: Hash,
    signed_availability: SignedAvailabilityBitfield,
}

/// Deal with network bridge updates and track what needs to be tracked
async fn handle_network_msg<Context>(
    mut ctx: Context,
    tracker: &mut Tracker,
    bridge_message: NetworkBridgeEvent,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
    let peer_views = &mut tracker.peer_views;
    let per_job = &mut tracker.per_job;
    let ego = &((*tracker).view);
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
            let ego = ego.clone();
            let old_view = std::mem::replace(&mut (tracker.view), view);
            tracker
                .per_job
                .retain(move |hash, _job_data| ego.0.contains(hash));

            for new in tracker.view.difference(&old_view) {
                if !tracker.per_job.contains_key(&new) {
                    log::warn!("Active head running that's not active anymore, go catch it")
                    //@todo rephrase
                    //@todo should we get rid of that right here
                }
            }
        }
        NetworkBridgeEvent::PeerMessage(remote, mut bytes) => {
            log::info!("Got a peer message from {:?}", &remote);
            if let Ok(gossiped_bitfield) = BitfieldGossipMessage::decode(&mut (bytes.as_slice())) {
                let (future, _abort_handle) = abortable(process_incoming_peer_message(ctx, tracker, remote, gossiped_bitfield));
                future.await;
                // tracker.active_jobs.insert(&gossiped_bitfield.relay_parent, abort_handle);
                // let _future = ctx.spawn(Box::pin(async move {
                //     future.map(|_| ()).await
                // }));
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
    mut ctx: Context,
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

    fn generate_valid_message() -> AllMessages {}

    fn generate_invalid_message() -> AllMessages {}

    #[test]
    fn game_changer() {
        // @todo
    }
}
