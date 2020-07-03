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

//! PoV Distribution Subsystem of Polkadot.
//!
//! This is a gossip implementation of code that is responsible for distributing PoVs
//! among validators.

use polkadot_primitives::Hash;
use polkadot_primitives::parachain::{PoVBlock as PoV, CandidateDescriptor};
use polkadot_subsystem::{
	OverseerSignal, SubsystemContext, Subsystem, SubsystemResult, FromOverseer,
};
use polkadot_subsystem::messages::{
	PoVDistributionMessage, NetworkBridgeEvent, ObservedRole, ReputationChange as Rep, PeerId,
	RuntimeApiMessage, RuntimeApiRequest, AllMessages,
};
use node_primitives::View;

use futures::prelude::*;
use futures::channel::oneshot;
use parity_scale_codec::{Encode, Decode};

use std::collections::{HashMap, HashSet};

const COST_APPARENT_FLOOD: Rep = Rep::new(-500, "Peer appears to be flooding us with PoV requests");
const COST_UNEXPECTED_POV: Rep = Rep::new(-500, "Peer sent us an unexpected PoV");

const BENEFIT_FRESH_POV: Rep = Rep::new(25, "Peer supplied us with an awaited PoV");
const BENEFIT_LATE_POV: Rep = Rep::new(10, "Peer supplied us with an awaited PoV, \
	but was not the first to do so");

#[derive(Encode, Decode)]
enum NetworkMessage {
    /// Notification that we are awaiting the given PoVs (by hash) against a
	/// specific relay-parent hash.
	#[codec(index = "0")]
    Awaiting(Hash, Vec<Hash>),
    /// Notification of an awaited PoV, in a given relay-parent context.
    /// (relay_parent, pov_hash, pov)
	#[codec(index = "1")]
    SendPoV(Hash, Hash, PoV),
}

pub struct PoVDistributionSubsystem;

struct State {
	relay_parent_state: HashMap<Hash, BlockBasedState>,
	peer_state: HashMap<PeerId, PeerState>,
	our_view: View,
}

struct BlockBasedState {
	known: HashMap<Hash, PoV>,
	/// All the PoVs we are or were fetching, coupled with channels expecting the data.
	///
	/// This may be an empty list, which indicates that we were once awaiting this PoV but have
	/// received it already.
	fetching: HashMap<Hash, Vec<oneshot::Sender<PoV>>>,
	n_validators: usize,
}

struct PeerState {
	/// A set of awaited PoV-hashes for each relay-parent in the peer's view.
	awaited: HashMap<Hash, HashSet<Hash>>,
}

// Handles the signal. If successful, returns `true` if the subsystem should conclude, `false` otherwise.
async fn handle_signal(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	signal: OverseerSignal,
) -> SubsystemResult<bool> {
	match signal {
		OverseerSignal::Conclude => Ok(true),
		OverseerSignal::StartWork(relay_parent) => {
			let (vals_tx, vals_rx) = oneshot::channel();
			ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Validators(vals_tx),
			))).await?;

			state.relay_parent_state.insert(relay_parent, BlockBasedState {
				known: HashMap::new(),
				fetching: HashMap::new(),
				n_validators: vals_rx.await?.len(),
			});

			Ok(false)
		}
		OverseerSignal::StopWork(relay_parent) => {
			state.relay_parent_state.remove(&relay_parent);

			Ok(false)
		}
	}
}

// Handles a `FetchPoV` message.
async fn handle_fetch(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	relay_parent: Hash,
	descriptor: CandidateDescriptor,
	response_sender: oneshot::Sender<PoV>,
) -> SubsystemResult<()> {
	unimplemented!()
}

// Handles a `DistributePoV` message.
async fn handle_distribute(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	relay_parent: Hash,
	descriptor: CandidateDescriptor,
	pov: PoV,
) -> SubsystemResult<()> {
	unimplemented!()
}

// Handles a network bridge update.
async fn handle_network_update(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	update: NetworkBridgeEvent,
) -> SubsystemResult<()> {
	unimplemented!()
}

async fn run(
	mut ctx: impl SubsystemContext<Message = PoVDistributionMessage>,
) -> SubsystemResult<()> {
	let mut state = State {
		relay_parent_state: HashMap::new(),
		peer_state: HashMap::new(),
		our_view: View(Vec::new()),
	};

	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(signal) => if handle_signal(&mut state, &mut ctx, signal).await? {
				return Ok(());
			},
			FromOverseer::Communication { msg } => match msg {
				PoVDistributionMessage::FetchPoV(relay_parent, descriptor, response_sender) =>
					handle_fetch(
						&mut state,
						&mut ctx,
						relay_parent,
						descriptor,
						response_sender,
					).await?,
				PoVDistributionMessage::DistributePoV(relay_parent, descriptor, pov) =>
					handle_distribute(
						&mut state,
						&mut ctx,
						relay_parent,
						descriptor,
						pov,
					).await?,
				PoVDistributionMessage::NetworkBridgeUpdate(event) =>
					handle_network_update(
						&mut state,
						&mut ctx,
						event,
					).await?,
			},
		}
	}
}
