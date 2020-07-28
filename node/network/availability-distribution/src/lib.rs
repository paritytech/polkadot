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
use polkadot_subsystem::messages::*;
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemResult,
};
use polkadot_erasure_coding::{obtain_chunks, branches, reconstruct};
use polkadot_primitives::v1::{CommittedCandidateReceipt, Hash, ErasureChunk, PoV, SigningContext, ValidatorId, ValidatorIndex};
use sc_network::ReputationChange;
use std::collections::{HashMap, HashSet};

const COST_SMTH: ReputationChange =
	ReputationChange::new(-100, "Smth bad");


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
	per_relay_parent : HashMap<Hash, PerRelayParent>,
}

impl ProtocolState {
}

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

/// Based on a iterator of relay heads
async fn live_candidates(relay_heads: impl IntoIterator<Hash>) -> HashSet<CommittedCandidateReceipt> {
	unimplemented!("use runtime availability API")
}


/// Deal with network bridge updates and track what needs to be tracked
/// which depends on the message type received.
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	bridge_message: NetworkBridgeEvent,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
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
			handle_our_view_change(state, view)?;
		}
		NetworkBridgeEvent::PeerMessage(remote, bytes) => {
			if let Ok(gossiped_bitfield) = AvailabilityGossipMessage::decode(&mut (bytes.as_slice())) {
				trace!(target: "avad", "Received bitfield gossip from peer {:?}", &remote);
				process_incoming_peer_message(ctx, state, remote, gossiped_bitfield).await?;
			} else {
				modify_reputation(ctx, remote, COST_MESSAGE_NOT_DECODABLE).await?;
			}
		}
	}
	Ok(())
}

/// Handle the changes necassary when our view changes.
fn handle_our_view_change(state: &mut ProtocolState, view: View) -> SubsystemResult<()> {
	let old_view = std::mem::replace(&mut (state.view), view);

	for added in state.view.difference(&old_view) {
		if !state.per_relay_parent.contains_key(&added) {

		}


		let validator_set = &state.validator_set;

		// @todo for all live candidates
		if let Some(pov) = query_proof_of_validity(ctx, added).await? {
			// perform erasure encoding
			let erasure_chunks : Vec<Vec<u8>> = obtain_chunks(validator_set.len(), pov)?;
			// create proofs for each erasure chunk

			let branches = branches(chunks.as_ref());
			let root = branches.root();

			let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();
		}
	}
	for removed in old_view.difference(&state.view) {
		// cleanup relay parents we are not interested in any more
		let _ = state.per_relay_parent.remove(&removed);
	}
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
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
{
	let current = state.peer_views.entry(origin.clone()).or_default();

	let delta_vec: Vec<Hash> = (*current).difference(&view).cloned().collect();

	*current = view;

	// Send all messages we've seen before and the peer is now interested
	// in to that peer.

	let delta_set: Vec<(ValidatorId, GossipMessage)> = delta_vec
		.into_iter()
		.filter_map(|new_relay_parent_interest| {
			if let Some(job_data) = (&*state).per_relay_parent.get(&new_relay_parent_interest) {
				// Send all jointly known messages for a validator (given the current relay parent)
				// to the peer `origin`...
				let one_per_validator = job_data.one_per_validator.clone();
				let origin = origin.clone();
				Some(
					one_per_validator
						.into_iter()
						.filter(move |(validator, _message)| {
							// ..except for the ones the peer already has
							job_data.message_from_validator_needed_by_peer(&origin, validator)
						}),
				)
			} else {
				// A relay parent is in the peers view, which is not in ours, ignore those.
				None
			}
		})
		.flatten()
		.collect();

	for (validator, message) in delta_set.into_iter() {
		send_tracked_gossip_message(ctx, state, origin.clone(), validator, message).await?;
	}

	Ok(())
}

/// Send a gossip message and track it in the per relay parent data.
async fn send_tracked_gossip_message<Context>(
	ctx: &mut Context,
	state: &mut ProtocolState,
	dest: PeerId,
	validator: ValidatorId,
	message: BitfieldGossipMessage,
) -> SubsystemResult<()>
where
	Context: SubsystemContext<Message = BitfieldDistributionMessage>,
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
					trace!(target: "avad", "Processing NetworkMessage");
					// a network message was received
					if let Err(e) = handle_network_msg(&mut ctx, &mut state, event).await {
					 	warn!(target: "avad", "Failed to handle incomming network messages: {:?}", e);
					}
				}
				FromOverseer::Communication {
					msg: AvailabilityDistributionMessage::DistributeChunk(hash, erasure_chunk),
				} => {
					trace!(target: "avad", "Processing incoming erasure chunk");
				}
				FromOverseer::Communication {
					msg: AvailabilityDistributionMessage::FetchChunk(hash, erasure_chunk),
				} => {
					trace!(target: "avad", "Processing incoming erasure chunk");
				}
				/// Event from the network bridge.
				FromOverseer::Signal(OverseerSignal::ActiveLeaves( ActiveLeavesUpdate{ activated, deactivated })) => {
					for relay_parent in activated {
						trace!(target: "avad", "Start {:?}", relay_parent);
					}
					for relay_parent in deactivated {
						trace!(target: "avad", "Stop {:?}", relay_parent);
					}

				}
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					trace!(target: "avad", "Conclude");
					return Ok(());
				}
			}
		}
	}
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
	trace!(target: "avad", "Reputation change of {:?} for peer {:?}", rep, peer);
	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep),
	))
	.await
}

async fn query_proof_of_validity<Context>(
	ctx: &mut Context, hash: Hash) -> SubsystemResult<Option<PoV>> where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(AvailabilityStoreMessage::QueryPoV(hash, tx))).await.expect("FIXME");
	let x = rx.await.expect("FIXME");
	Ok(x)
}

async fn query_stored_chunk<Context>(
	ctx: &mut Context, hash: Hash, validator_index: ValidatorIndex) -> SubsystemResult<Option<ErasureChunk>> where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(AvailabilityStoreMessage::QueryChunk(hash, validator_index, tx))).await.expect("FIXME");
	let x = rx.await.expect("FIXME");
	Ok(x)
}

async fn store_chunk<Context>(
	ctx: &mut Context, hash: Hash, validator_index: ValidatorIndex, erasure_chunk: ErasureChunk) -> SubsystemResult<()> where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage>,
{
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreChunk(hash, validator_index, erasure_chunk, tx))).await
	let x = rx.await.expect("FIXME");
	Ok(x)
}



/// Handle the changes necassary when our view changes.
fn handle_our_view_change(state: &mut ProtocolState, view: View) -> SubsystemResult<()> {
	let old_view = std::mem::replace(&mut (state.view), view);

	for added in state.view.difference(&old_view) {
	}
	for removed in old_view.difference(&state.view) {
		// cleanup relay parents we are not interested in any more
		unimplemented!("cleanup");
	}
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
	unimplemented!("peer view change");

	// @todo obtain the added difference

	// @todo check if we have messages, that we did not send to the peer just yet

	// @todo send those messages to the peer
	Ok(())
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
	use bitvec::bitvec;
	use futures::executor;
	use maplit::hashmap;
	use polkadot_primitives::v1::{Signed, ValidatorPair, AvailabilityBitfield};
	use polkadot_subsystem::test_helpers::make_subsystem_context;
	use smol_timeout::TimeoutExt;
	use sp_core::crypto::Pair;
	use std::time::Duration;
	use assert_matches::assert_matches;

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
			$fut
			.timeout(Duration::from_millis(10))
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
