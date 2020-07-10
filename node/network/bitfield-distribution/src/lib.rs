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
    Future, FutureExt,
};
use node_primitives::{ProtocolId, SignedFullStatement, View};
use polkadot_network::protocol::Message;
use polkadot_node_subsystem::messages::*;
use polkadot_node_subsystem::{
    FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemResult,
};
use polkadot_primitives::parachain::{SigningContext, ValidatorId};
use polkadot_primitives::Hash;
use sc_network::ReputationChange;
use std::{
    collections::{HashMap, HashSet},
    pin::Pin,
};

const COST_SIGNATURE_INVALID: ReputationChange =
    ReputationChange::new(-10000, "Bitfield signature invalid");
const COST_MISSING_PEER_SESSION_KEY: ReputationChange =
    ReputationChange::new(-1337, "Missing peer session key");
const COST_MULTIPLE_BITFIELDS_FROM_PEER: ReputationChange =
    ReputationChange::new(-10000, "Received more than once bitfield from peer");
const COST_NOT_INTERESTED: ReputationChange =
    ReputationChange::new(-100, "Not intersted in that parent hash");
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
        // @todo do we need Box<dyn Future<Output = ()>>) for anything?
        let mut active_jobs = HashMap::<Hash, AbortHandle>::new();
        let mut tracker = Tracker::default();
        loop {
            {
                let message = ctx.recv().await?;
                match message {
                    FromOverseer::Communication { msg } => {
                        let peerid = PeerId::random(); // @todo
                        process_incoming(ctx.clone(), &mut tracker, peerid, msg).await?;
                    }
                    FromOverseer::Signal(OverseerSignal::StartWork(relay_parent)) => {
                        let (validators, signing_context) =
                            query_basics(ctx.clone(), relay_parent).await?;

                            let _ = tracker.per_job.insert(relay_parent, JobData {
                                validator_bitset_received: HashSet::new(),
                                signing_context,
                                validator_set: Vec::new(),
                            });

                        let future = processor_per_relay_parent(ctx.clone(), relay_parent.clone());
                        // let (future, abort_handle) =
                        //     abortable(future);

                        let future = ctx.spawn(Box::pin(
                            future.map(|_| { () })
                        ));
                        // active_jobs
                        //     .insert(relay_parent.clone(), abort_handle);
                    }
                    FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)) => {
                        if let Some(abort_handle) =
                            active_jobs.remove(&relay_parent)
                        {
                            let _ = abort_handle.abort();
                        }
                    }
                    FromOverseer::Signal(OverseerSignal::Conclude) => {
                        // @todo cannot store the future
                        // return futures::future::join_all(
                        //     active_jobs
                        //         .drain()
                        //         .map(|(_relay_parent, (cancellation, future))| future),
                        // )
                        // .await;
                    }
                }
            }
            // active_jobs
            //     .retain(|_, future| future.poll().is_pending());
        }
    }
}


/// Process all requests related to one relay parent hash
async fn processor_per_relay_parent<Context>(mut ctx: Context, relay_parent: Hash) -> SubsystemResult<()> {
    let mut tracker = Tracker::default();
    loop {
        // todo!("consume relay parents")
    }
    Ok(())
}

/// modify the reputiation, good or bad
async fn modify_reputiation<Context>(mut ctx: Context, peerid: PeerId, rep: ReputationChange) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
    ctx.send_message(AllMessages::NetworkBridge(
        NetworkBridgeMessage::ReportPeer(peerid, rep),
    ))
    .await
}

/// Handle an incoming message from a peer
async fn process_incoming<Context>(
    mut ctx: Context,
    tracker: &mut Tracker,
    peerid: PeerId,
    message: BitfieldDistributionMessage,
) -> SubsystemResult<()>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
    let peer_view = tracker.peer_views.get(&peerid).expect("TODO");
    match message {
        // Distribute a bitfield via gossip to other validators.
        BitfieldDistributionMessage::DistributeBitfield(hash, signed_availability) => {
            let job_data = if let Some(job_data) = tracker.per_job.get(&hash) {
                job_data
            } else {
                return modify_reputiation(ctx, peerid, COST_NOT_INTERESTED).await;
            };

            // @todo should we only distribute availability messages to peer if they are relevant to us
            // or is the only discriminator if the peer cares about it?
            if !peer_view.contains(&hash) {
                // we don't care about this, the other side should have known better
                return modify_reputiation(ctx, peerid, COST_NOT_INTERESTED).await;
            }

            let validator_set = &job_data.validator_set;
            if validator_set.len() == 0 {
                return modify_reputiation(ctx.clone(), peerid, COST_MISSING_PEER_SESSION_KEY).await
            }

            // check all validators that could have signed this message
            if let Some(_) = validator_set.iter().find(|validator| { signed_availability.check_signature(&job_data.signing_context, validator).is_ok() }) {
                return modify_reputiation(ctx.clone(), peerid, COST_SIGNATURE_INVALID)
                    .await
            }

            // @todo verify sequential execution is ok or if spawning tasks is better
            // Send peers messages which are interesting to them
            for (peerid, view) in tracker.peer_views.iter()
                .filter(|(_peerid, view)| view.contains(&hash))
            {
                    // @todo shall we assure these complete or just let them be?
                    let _ = ctx.send_message(
                        AllMessages::BitfieldDistribution(
                            BitfieldDistributionMessage::DistributeBitfield(hash.clone(), signed_availability.clone()),
                    )).await;
            }
        }
        BitfieldDistributionMessage::NetworkBridgeUpdate(event) => {
            handle_network_msg(ctx, tracker, event).await?;
        }
    }
    Ok(())
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
            tracker.per_job.retain(move |hash, _job_data| ego.0.contains(hash));

            for new in tracker.view.difference(&old_view) {
                if !tracker.per_job.contains_key(&new) {
                    log::warn!("Active head running that's not active anymore, go catch it")
                    //@todo rephrase
                    //@todo should we get rid of that right here
                }
            }
        }
        NetworkBridgeEvent::PeerMessage(remote, bytes) => {
            log::info!("Got a peer message from {:?}", &remote);
            // @todo what would we receive here?
            match Message::decode(&mut bytes.as_ref()) {
                Ok(message) => {
                    match message {
                        // a new session key
                        Message::ValidatorId(session_key) => {
                            // @todo update all Validator ids I guess?
                            // let _ = tracker
                            //     .per_job.
                            //     .insert(remote.clone(), session_key);
                        }
                        _ => {}
                    }
                }
                Err(_) => unimplemented!("Invalid format shall be punished I guess"),
            }
        }
    }
    Ok(())
}

impl<C> Subsystem<C> for BitfieldDistribution
where
    C: SubsystemContext<Message = BitfieldDistributionMessage> + Clone + Sync,
{
    fn start(self, ctx: C) -> SpawnedSubsystem {
        SpawnedSubsystem(Box::pin(async move {
            Self::run(ctx).await;
        }))
    }
}

/// query the validator set
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

    #[test]
    fn x() {
        // @todo
    }
}
