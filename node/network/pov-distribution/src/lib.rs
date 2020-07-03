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
	RuntimeApiMessage, RuntimeApiRequest, AllMessages, NetworkBridgeMessage,
};
use node_primitives::{View, ProtocolId};

use futures::prelude::*;
use futures::channel::oneshot;
use parity_scale_codec::{Encode, Decode};

use std::collections::{hash_map::{Entry, HashMap}, HashSet};
use std::sync::Arc;

const COST_APPARENT_FLOOD: Rep = Rep::new(-500, "Peer appears to be flooding us with PoV requests");
const COST_UNEXPECTED_POV: Rep = Rep::new(-500, "Peer sent us an unexpected PoV");

const BENEFIT_FRESH_POV: Rep = Rep::new(25, "Peer supplied us with an awaited PoV");
const BENEFIT_LATE_POV: Rep = Rep::new(10, "Peer supplied us with an awaited PoV, \
	but was not the first to do so");

const PROTOCOL_V1: ProtocolId = *b"pvd1";

#[derive(Encode, Decode)]
enum WireMessage {
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
	known: HashMap<Hash, Arc<PoV>>,
	/// All the PoVs we are or were fetching, coupled with channels expecting the data.
	///
	/// This may be an empty list, which indicates that we were once awaiting this PoV but have
	/// received it already.
	fetching: HashMap<Hash, Vec<oneshot::Sender<Arc<PoV>>>>,
	n_validators: usize,
}

#[derive(Default)]
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

// Notify peers that we are awaiting a given PoV hash.
//
// This only notifies peers who have the relay parent in their view.
async fn notify_we_are_awaiting(
	peers: &mut HashMap<PeerId, PeerState>,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	relay_parent: Hash,
	pov_hash: Hash,
) -> SubsystemResult<()> {
	// We use `awaited` as a proxy for which heads are in the peer's view.
	let peers_to_send: Vec<_> = peers.iter()
		.filter_map(|(peer, state)| if state.awaited.contains_key(&relay_parent) {
			Some(peer.clone())
		} else {
			None
		})
		.collect();

	if peers_to_send.is_empty() { return Ok(()) }

	let payload = WireMessage::Awaiting(relay_parent, vec![pov_hash]).encode();

	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendMessage(
		peers_to_send,
		PROTOCOL_V1,
		payload,
	))).await
}

// Distribute a PoV to peers who are awaiting it.
async fn distribute_to_awaiting(
	peers: &mut HashMap<PeerId, PeerState>,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	relay_parent: Hash,
	pov_hash: Hash,
	pov: &PoV,
) -> SubsystemResult<()> {
	// Send to all peers who are awaiting the PoV and have that relay-parent in their view.
	//
	// Also removes it from their awaiting set.
	let peers_to_send: Vec<_> = peers.iter_mut()
		.filter_map(|(peer, state)| state.awaited.get_mut(&relay_parent).and_then(|awaited| {
			if awaited.remove(&pov_hash) {
				Some(peer.clone())
			} else {
				None
			}
		}))
		.collect();

	if peers_to_send.is_empty() { return Ok(()) }

	let payload = WireMessage::SendPoV(relay_parent, pov_hash, pov.clone()).encode();

	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendMessage(
		peers_to_send,
		PROTOCOL_V1,
		payload,
	))).await
}

// Handles a `FetchPoV` message.
async fn handle_fetch(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	relay_parent: Hash,
	descriptor: CandidateDescriptor,
	response_sender: oneshot::Sender<Arc<PoV>>,
) -> SubsystemResult<()> {
	let relay_parent_state = match state.relay_parent_state.get_mut(&relay_parent) {
		Some(s) => s,
		None => return Ok(()),
	};

	if let Some(pov) = relay_parent_state.known.get(&descriptor.pov_hash) {
		let _  = response_sender.send(pov.clone());
		return Ok(());
	}

	{
		match relay_parent_state.fetching.entry(descriptor.pov_hash) {
			Entry::Occupied(mut e) => {
				// we are already awaiting this PoV if there is an entry.
				e.get_mut().push(response_sender);
				return Ok(());
			}
			Entry::Vacant(e) => {
				e.insert(vec![response_sender]);
			}
		}
	}

	if relay_parent_state.fetching.len() > 2 * relay_parent_state.n_validators {
		log::warn!("Other subsystems have requested PoV distribution to \
			fetch more PoVs than reasonably expected: {}", relay_parent_state.fetching.len());
		return Ok(());
	}

	// Issue an `Awaiting` message to all peers with this in their view.
	notify_we_are_awaiting(
		&mut state.peer_state,
		ctx,
		relay_parent,
		descriptor.pov_hash
	).await
}

// Handles a `DistributePoV` message.
async fn handle_distribute(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	relay_parent: Hash,
	descriptor: CandidateDescriptor,
	pov: Arc<PoV>,
) -> SubsystemResult<()> {
	let relay_parent_state = match state.relay_parent_state.get_mut(&relay_parent) {
		None => return Ok(()),
		Some(s) => s,
	};

	if let Some(our_awaited) = relay_parent_state.fetching.get_mut(&descriptor.pov_hash) {
		// Drain all the senders, but keep the entry in the map around intentionally.
		//
		// It signals that we were at one point awaiting this, so we will be able to tell
		// why peers are sending it to us.
		for response_sender in our_awaited.drain(..) {
			let _ = response_sender.send(pov.clone());
		}
	}

	relay_parent_state.known.insert(descriptor.pov_hash, pov.clone());

	distribute_to_awaiting(
		&mut state.peer_state,
		ctx,
		relay_parent,
		descriptor.pov_hash,
		&*pov,
	).await
}

// Handles a network bridge update.
async fn handle_network_update(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	update: NetworkBridgeEvent,
) -> SubsystemResult<()> {
	match update {
		NetworkBridgeEvent::PeerConnected(peer, _observed_role) => {
			state.peer_state.insert(peer, PeerState { awaited: HashMap::new() });
			Ok(())
		}
		NetworkBridgeEvent::PeerDisconnected(peer) => {
			state.peer_state.remove(&peer);
			Ok(())
		}
		NetworkBridgeEvent::PeerViewChange(peer_id, view) => {
			if let Some(peer_state) = state.peer_state.get_mut(&peer_id) {
				// prune anything not in the new view.
				peer_state.awaited.retain(|relay_parent, _| view.0.contains(&relay_parent));

				// introduce things from the new view.
				for relay_parent in view.0.iter() {
					peer_state.awaited.entry(*relay_parent).or_default();
				}
			}

			Ok(())
		}
		_ => Ok(()), // TODO [now] exhaustive match
	}
}

fn network_update_message(update: NetworkBridgeEvent) -> AllMessages {
	AllMessages::PoVDistribution(PoVDistributionMessage::NetworkBridgeUpdate(update))
}

async fn run(
	mut ctx: impl SubsystemContext<Message = PoVDistributionMessage>,
) -> SubsystemResult<()> {
	// startup: register the network protocol with the bridge.
	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::RegisterEventProducer(
		PROTOCOL_V1,
		network_update_message,
	))).await?;

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
