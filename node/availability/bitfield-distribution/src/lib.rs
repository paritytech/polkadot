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

use futures::{
    channel::oneshot,
    future::{abortable, AbortHandle, Abortable},
    Future,
};
use node_primitives::{ProtocolId, SignedFullStatement, View};
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
use polkadot_network::protocol::Message;
use codec::{Encode, Decode, Codec};

const COST_SIGNATURE_INVALID: ReputationChange =
    ReputationChange::new(-10000, "Bitfield signature invalid");
const COST_MISSING_PEER_SESSION_KEY: ReputationChange =
    ReputationChange::new(-1337, "Missing peer session key");
const COST_MULTIPLE_BITFIELDS_FROM_PEER: ReputationChange =
    ReputationChange::new(-10000, "Received more than once bitfield from peer");
const COST_NOT_INTERESTED: ReputationChange =
    ReputationChange::new(-100, "Not intersted in that parent hash");


#[derive(Default, Clone)]
struct Tracker {
    // track all active peers and their views
    // to determine what is relevant to them
	peer_views: HashMap<PeerId, View>,

	// keys used for verifying signatures of that peer
	peer_session_keys: HashMap<PeerId, ValidatorId>,

    // set of active heads the overseer told us to work on
    active_jobs: HashMap<Hash, (AbortHandle, Box<dyn Future<Output=()>>)>,

    // set of validators which already sent a message
    validator_bitset_received: HashSet<ValidatorId>,

    // our current view
	view: View,

	// signing context
	signing_context: SigningContext<Hash>,

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
            NetworkBridgeMessage::RegisterEventProducer(Self::PROTOCOL_ID, handle_network_msg),
        ))
        .await?;

        let mut tracker = Tracker::default();
        loop {
            {
                let message = ctx.recv().await?;
                match message {
                    FromOverseer::Communication { msg: _ } => {
                        unreachable!("BitfieldDistributionMessage does not exist; qed")
                    }
                    FromOverseer::Signal(OverseerSignal::StartWork(relay_parent)) => {
                        let (validators, signing_context) = query_basics(ctx.clone(), relay_parent).await?;

                        let (future, abort_handle) =
                            abortable(process_incoming_all_incoming( ctx.clone(), relay_parent.clone()));

						let spawn = ctx.spawn(Box::pin(future))?;
                        let future = Box::new();
                        tracker
                            .active_jobs
                            .insert(relay_parent.clone(), (abort_handle, future));
                    }
                    FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)) => {
                        if let Some((future, abort_handle)) = tracker.active_jobs.take(&relay_parent) {
							let _ = abort_handle.abort();
						}
                    }
                    FromOverseer::Signal(OverseerSignal::Conclude) => {
                        // @todo add a timeout here?
                        return futures::future::join_all(
                            tracker
                                .active_jobs
                                .drain()
                                .map(|(_relay_parent, (cancellation, future))| future)
                        )
                        .await;
                    }
                }
            }
            tracker
                .active_jobs
                .retain(|_, future| future.poll().is_pending());
        }
    }
}


/// query the validator set
async fn query_basics<Context>(mut ctx: Context, relay_parent: Hash) -> SubsystemResult<(Vec<Hash>, SigningContext)>
where
    Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone, {
		let (validators_tx, validators_rx) = oneshot::channel();
		let (signing_tx, signing_rx) = oneshot::channel();

		let query_validators =
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Validators(validators_tx),
			));

		let query_signing =
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SigningContext(signing_tx),
			));

		ctx.send_messages(
			std::iter::once(query_validators)
				.chain(std::iter::once(query_signing)),
		)
		.await?;

		Ok((validators_rx.await?, signing_rx.await?))
}


async fn process_incoming_all_incoming<Context>(mut ctx: Context, relay_parent: Hash) -> SubsystemResult<()>
where
Context: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
	loop {
		// @todo shall these be spawned? scheduling
		// @todo consume messages via a channel?
		// @todo obtain all input args
		process_incoming(&mut ctx, relay_parent, peerid, message).await?;
	}
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
    match message {
        /// Distribute a bitfield via gossip to other validators.
        BitfieldDistributionMessage::DistributeBitfield(hash, signed_availability) => {
            // @todo should we only distribute availability messages to peer if they are relevant to us
            // or is the only discriminator if the peer cares about it?
            if !tracker.view.contains(hash) {
                // we don't care about this one
                // @todo should this be a penality?
                return ctx
                    .send_message(AllMessages::NetworkBridge(
                        NetworkBridgeMessage::ReportPeer(peerid, COST_NOT_INTERESTED),
                    ))
                    .await;
            }

			let signing_ctx = &tracker.signing_context.as_ref().unwrap();

			let session_key = tracker.peer_session_keys.get(peerid);
			if session_key.is_none() {
				return ctx
                    .send_message(AllMessages::NetworkBridge(
                        NetworkBridgeMessage::ReportPeer(peerid, COST_MISSING_PEER_SESSION_KEY),
                    ))
                    .await;
			}
			let session_key = session_key.expect("Just proved it is not `None`; qed");

            if signed_availability.check_signature(&signing_ctx, session_key)? {
                return ctx
                    .send_message(AllMessages::NetworkBridge(
                        NetworkBridgeMessage::ReportPeer(peerid, COST_SIGNATURE_INVALID),
                    ))
                    .await;
            }

            // @todo verify sequential execution is ok or if spawning tasks is better
            // Send peers messages which are interesting to them
            for (peerid, view) in tracker
                .peer_views
                .iter()
                .filter(|(_peerid, view)| view.contains(hash))
            {
                // @todo shall we assure these complete or just let them be?
                ctx.spawn(Box::new(ctx.send_message(
                    AllMessages::BitfieldDistribution(
                        BitfieldDistributionMessage::DistributeBitfield(hash, signed_availability),
                    ),
                )))
				.await?;
            }
        }
        BitfieldDistributionMessage::NetworkBridgeUpdate(event) => {
            handle_network_msg(ctx, &mut tracker, event).await?;
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
	let active_jobs = &mut tracker.active_jobs;
	let ego = &((*tracker).peer_views);
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
			let old_view = std::mem::replace(&mut tracker.view, view);
            active_jobs
                .retain(|head, _| ego.contains(head));

            for new in tracker.view.difference(&old_view) {
                if !tracker.active_jobs.contains_key(&new) {
                    log::warn!("Active head running that's not active anymore, go catch it")
                    //@todo rephrase
                    //@todo should we get rid of that right here
                }
            }
        }
        NetworkBridgeEvent::PeerMessage(remote, bytes) => {
            // @todo what would we receive here?
			match Message::decode(&mut bytes.as_ref()) {
				Ok(message) => {
					match message {
						// a new session key
						Message::ValidatorId(session_key) => {
							tracker.peer_session_keys.insert(remote.clone(), session_key);
						}
						_ => {}
					}
				},
				Err(_) => unimplemented!("Invalid format shall be punished I guess"),
			}
        }
    }
    Ok(())
}

impl<C> Subsystem<C> for BitfieldDistribution
where
    C: SubsystemContext<Message = BitfieldDistributionMessage> + Clone,
{
    fn start(self, ctx: C) -> SpawnedSubsystem {
        SpawnedSubsystem(Box::pin(async move {
            Self::run(ctx).await;
        }))
    }
}
