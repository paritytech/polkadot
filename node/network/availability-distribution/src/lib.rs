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
use polkadot_primitives::v1::{Hash, ErasureChunk, PoV, SigningContext, ValidatorId, ValidatorIndex};
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
	/// Our own view.
	view: View,
}



fn network_update_message(n: NetworkBridgeEvent) -> AllMessages {
	AllMessages::AvailabilityDistribution(AvailabilityDistributionMessage::NetworkBridgeUpdate(n))
}

/// The bitfield distribution subsystem.
pub struct AvailabilityDistribution;

impl AvailabilityDistribution {
	/// The protocol identifier for bitfield distribution.
	const PROTOCOL_ID: ProtocolId = *b"avad";

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
					// if let Err(e) = handle_network_msg(&mut ctx, &mut state, event).await {
					// 	warn!(target: "avad", "Failed to handle incomming network messages: {:?}", e);
					// }
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
				FromOverseer::Signal(OverseerSignal::StartWork(relay_parent)) => {
					trace!(target: "avad", "Start {:?}", relay_parent);
				}
				FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)) => {
					trace!(target: "avad", "Stop {:?}", relay_parent);
					// defer the cleanup to the view change
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
	ctx.send_message(AllMessages::AvailabilityStore(AvailabilityStoreMessage::StoreChunk(hash, validator_index, erasure_chunk))).await
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
