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
	OverseerSignal, SubsystemContext, Subsystem, SubsystemResult, FromOverseer, SpawnedSubsystem,
};
use polkadot_subsystem::messages::{
	PoVDistributionMessage, NetworkBridgeEvent, ReputationChange as Rep, PeerId,
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
const COST_MALFORMED_MESSAGE: Rep = Rep::new(-500, "Peer sent us a malformed message");
const COST_AWAITED_NOT_IN_VIEW: Rep
	= Rep::new(-100, "Peer claims to be awaiting something outside of its view");

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

/// The PoV Distribution Subsystem.
pub struct PoVDistribution;

impl<C> Subsystem<C> for PoVDistribution
	where C: SubsystemContext<Message = PoVDistributionMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run`.
		SpawnedSubsystem(run(ctx).map(|_| ()).boxed())
	}
}

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

/// Handles the signal. If successful, returns `true` if the subsystem should conclude,
/// `false` otherwise.
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

/// Notify peers that we are awaiting a given PoV hash.
///
/// This only notifies peers who have the relay parent in their view.
async fn notify_all_we_are_awaiting(
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

/// Notify one peer about everything we're awaiting at a given relay-parent.
async fn notify_one_we_are_awaiting_many(
	peer: &PeerId,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	relay_parent_state: &HashMap<Hash, BlockBasedState>,
	relay_parent: Hash,
) -> SubsystemResult<()> {
	let awaiting_hashes = relay_parent_state.get(&relay_parent).into_iter().flat_map(|s| {
		// Send the peer everything we are fetching at this relay-parent
		s.fetching.iter()
			.filter(|(_, senders)| !senders.is_empty()) // that has not been completed already.
			.map(|(pov_hash, _)| *pov_hash)
	}).collect::<Vec<_>>();

	if awaiting_hashes.is_empty() { return Ok(()) }

	let payload = WireMessage::Awaiting(relay_parent, awaiting_hashes).encode();

	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendMessage(
		vec![peer.clone()],
		PROTOCOL_V1,
		payload,
	))).await
}

/// Distribute a PoV to peers who are awaiting it.
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

/// Handles a `FetchPoV` message.
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
	notify_all_we_are_awaiting(
		&mut state.peer_state,
		ctx,
		relay_parent,
		descriptor.pov_hash
	).await
}

/// Handles a `DistributePoV` message.
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

/// Report a reputation change for a peer.
async fn report_peer(
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	peer: PeerId,
	rep: Rep,
) -> SubsystemResult<()> {
	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(peer, rep))).await
}

/// Handle a notification from a peer that they are awaiting some PoVs.
async fn handle_awaiting(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	peer: PeerId,
	relay_parent: Hash,
	pov_hashes: Vec<Hash>,
) -> SubsystemResult<()> {
	if !state.our_view.0.contains(&relay_parent) {
		report_peer(ctx, peer, COST_AWAITED_NOT_IN_VIEW).await?;
		return Ok(());
	}

	let relay_parent_state = match state.relay_parent_state.get_mut(&relay_parent) {
		None => {
			log::warn!("PoV Distribution relay parent state out-of-sync with our view");
			return Ok(());
		}
		Some(s) => s,
	};

	let peer_awaiting = match
		state.peer_state.get_mut(&peer).and_then(|s| s.awaited.get_mut(&relay_parent))
	{
		None => {
			report_peer(ctx, peer, COST_AWAITED_NOT_IN_VIEW).await?;
			return Ok(());
		}
		Some(a) => a,
	};

	let will_be_awaited = peer_awaiting.len() + pov_hashes.len();
	if will_be_awaited <= 2 * relay_parent_state.n_validators {
		for pov_hash in pov_hashes {
			// For all requested PoV hashes, if we have it, we complete the request immediately.
			// Otherwise, we note that the peer is awaiting the PoV.
			if let Some(pov) = relay_parent_state.known.get(&pov_hash) {
				ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendMessage(
					vec![peer.clone()],
					PROTOCOL_V1,
					WireMessage::SendPoV(relay_parent, pov_hash, (&**pov).clone()).encode(),
				))).await?;
			} else {
				peer_awaiting.insert(pov_hash);
			}
		}
	} else {
		report_peer(ctx, peer, COST_APPARENT_FLOOD).await?;
	}

	Ok(())
}

/// Handle an incoming PoV from our peer. Reports them if unexpected, rewards them if not.
///
/// Completes any requests awaiting that PoV.
async fn handle_incoming_pov(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	peer: PeerId,
	relay_parent: Hash,
	pov_hash: Hash,
	pov: PoV,
) -> SubsystemResult<()> {
	let relay_parent_state = match state.relay_parent_state.get_mut(&relay_parent) {
		None =>	{
			report_peer(ctx, peer, COST_UNEXPECTED_POV).await?;
			return Ok(());
		},
		Some(r) => r,
	};

	let pov = {
		// Do validity checks and complete all senders awaiting this PoV.
		let fetching = match relay_parent_state.fetching.get_mut(&pov_hash) {
			None => {
				report_peer(ctx, peer, COST_UNEXPECTED_POV).await?;
				return Ok(());
			}
			Some(f) => f,
		};

		let hash = pov.hash();
		if hash != pov_hash {
			report_peer(ctx, peer, COST_UNEXPECTED_POV).await?;
			return Ok(());
		}

		let pov = Arc::new(pov);

		if fetching.is_empty() {
			// fetching is empty whenever we were awaiting something and
			// it was completed afterwards.
			report_peer(ctx, peer.clone(), BENEFIT_LATE_POV).await?;
		} else {
			// fetching is non-empty when the peer just provided us with data we needed.
			report_peer(ctx, peer.clone(), BENEFIT_FRESH_POV).await?;
		}

		for response_sender in fetching.drain(..) {
			let _ = response_sender.send(pov.clone());
		}

		pov
	};

	// make sure we don't consider this peer as awaiting that PoV anymore.
	if let Some(peer_state) = state.peer_state.get_mut(&peer) {
		peer_state.awaited.remove(&pov_hash);
	}

	// distribute the PoV to all other peers who are awaiting it.
	distribute_to_awaiting(
		&mut state.peer_state,
		ctx,
		relay_parent,
		pov_hash,
		&*pov,
	).await
}

/// Handles a network bridge update.
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
					if let Entry::Vacant(entry) = peer_state.awaited.entry(*relay_parent) {
						entry.insert(HashSet::new());

						// Notify the peer about everything we're awaiting at the new relay-parent.
						notify_one_we_are_awaiting_many(
							&peer_id,
							ctx,
							&state.relay_parent_state,
							*relay_parent,
						).await?;
					}
				}
			}

			Ok(())
		}
		NetworkBridgeEvent::PeerMessage(peer, bytes) => {
			match WireMessage::decode(&mut &bytes[..]) {
				Ok(msg) => match msg {
					WireMessage::Awaiting(relay_parent, pov_hashes) => handle_awaiting(
						state,
						ctx,
						peer,
						relay_parent,
						pov_hashes,
					).await,
					WireMessage::SendPoV(relay_parent, pov_hash, pov) => handle_incoming_pov(
						state,
						ctx,
						peer,
						relay_parent,
						pov_hash,
						pov,
					).await,
				},
				Err(_) => {
					report_peer(ctx, peer, COST_MALFORMED_MESSAGE).await?;
					Ok(())
				}
			}
		}
		NetworkBridgeEvent::OurViewChange(view) => {
			state.our_view = view;
			Ok(())
		}
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

#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor::{self, ThreadPool};
	use polkadot_primitives::parachain::BlockData;
	use assert_matches::assert_matches;

	fn make_pov(data: Vec<u8>) -> PoV {
		PoV { block_data: BlockData(data) }
	}

	fn make_peer_state(awaited: Vec<(Hash, Vec<Hash>)>)
		-> PeerState
	{
		PeerState {
			awaited: awaited.into_iter().map(|(rp, h)| (rp, h.into_iter().collect())).collect()
		}
	}

	#[test]
	fn distributes_to_those_awaiting_and_completes_local() {
		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into();

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();
		let peer_c = PeerId::random();

		let (pov_send, pov_recv) = oneshot::channel();
		let pov = make_pov(vec![1, 2, 3]);
		let pov_hash = pov.hash();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let mut b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				b.fetching.insert(pov_hash, vec![pov_send]);
				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				// peer A has hash_a in its view and is awaiting the PoV.
				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![pov_hash])]),
				);

				// peer B has hash_a in its view but is not awaiting.
				s.insert(
					peer_b.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				// peer C doesn't have hash_a in its view but is awaiting the PoV under hash_b.
				s.insert(
					peer_c.clone(),
					make_peer_state(vec![(hash_b, vec![pov_hash])]),
				);

				s
			},
			our_view: View(vec![hash_a, hash_b]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);
		let mut descriptor = CandidateDescriptor::default();
		descriptor.pov_hash = pov_hash;

		executor::block_on(async move {
			handle_distribute(
				&mut state,
				&mut ctx,
				hash_a,
				descriptor,
				Arc::new(pov.clone()),
			).await.unwrap();

			assert!(!state.peer_state[&peer_a].awaited[&hash_a].contains(&pov_hash));
			assert!(state.peer_state[&peer_c].awaited[&hash_b].contains(&pov_hash));

			// our local sender also completed
			assert_eq!(&*pov_recv.await.unwrap(), &pov);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendMessage(peers, protocol, message)
				) => {
					assert_eq!(peers, vec![peer_a.clone()]);
					assert_eq!(protocol, PROTOCOL_V1);
					assert_eq!(
						message,
						WireMessage::SendPoV(hash_a, pov_hash, pov.clone()).encode(),
					);
				}
			)
		});
	}

	#[test]
	fn we_inform_peers_with_same_view_we_are_awaiting() {
		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into();

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		let (pov_send, _) = oneshot::channel();
		let pov = make_pov(vec![1, 2, 3]);
		let pov_hash = pov.hash();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				// peer A has hash_a in its view.
				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				// peer B doesn't have hash_a in its view.
				s.insert(
					peer_b.clone(),
					make_peer_state(vec![(hash_b, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);
		let mut descriptor = CandidateDescriptor::default();
		descriptor.pov_hash = pov_hash;

		executor::block_on(async move {
			handle_fetch(
				&mut state,
				&mut ctx,
				hash_a,
				descriptor,
				pov_send,
			).await.unwrap();

			assert_eq!(state.relay_parent_state[&hash_a].fetching[&pov_hash].len(), 1);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendMessage(peers, protocol, message)
				) => {
					assert_eq!(peers, vec![peer_a.clone()]);
					assert_eq!(protocol, PROTOCOL_V1);
					assert_eq!(
						message,
						WireMessage::Awaiting(hash_a, vec![pov_hash]).encode(),
					);
				}
			)
		});
	}

	#[test]
	fn peer_view_change_leads_to_us_informing() {
		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into();

		let peer_a = PeerId::random();

		let (pov_a_send, _) = oneshot::channel();

		let pov_a = make_pov(vec![1, 2, 3]);
		let pov_a_hash = pov_a.hash();

		let pov_b = make_pov(vec![4, 5, 6]);
		let pov_b_hash = pov_b.hash();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let mut b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				// pov_a is still being fetched, whereas the fetch of pov_b has already
				// completed, as implied by the empty vector.
				b.fetching.insert(pov_a_hash, vec![pov_a_send]);
				b.fetching.insert(pov_b_hash, vec![]);

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				// peer A doesn't yet have hash_a in its view.
				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_b, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerViewChange(peer_a.clone(), View(vec![hash_a, hash_b])),
			).await.unwrap();

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendMessage(peers, protocol, message)
				) => {
					assert_eq!(peers, vec![peer_a.clone()]);
					assert_eq!(protocol, PROTOCOL_V1);
					assert_eq!(
						message,
						WireMessage::Awaiting(hash_a, vec![pov_a_hash]).encode(),
					);
				}
			)
		});
	}

	#[test]
	fn peer_complete_fetch_and_is_rewarded() {
		let hash_a: Hash = [0; 32].into();

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		let (pov_send, pov_recv) = oneshot::channel();

		let pov = make_pov(vec![1, 2, 3]);
		let pov_hash = pov.hash();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let mut b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				// pov is being fetched.
				b.fetching.insert(pov_hash, vec![pov_send]);

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				// peers A and B are functionally the same.
				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				s.insert(
					peer_b.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			// Peer A answers our request before peer B.
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::SendPoV(hash_a, pov_hash, pov.clone()).encode(),
				),
			).await.unwrap();

			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_b.clone(),
					WireMessage::SendPoV(hash_a, pov_hash, pov.clone()).encode(),
				),
			).await.unwrap();

			assert_eq!(&*pov_recv.await.unwrap(), &pov);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, BENEFIT_FRESH_POV);
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, BENEFIT_LATE_POV);
				}
			);
		});
	}

	#[test]
	fn peer_punished_for_sending_bad_pov() {
		let hash_a: Hash = [0; 32].into();

		let peer_a = PeerId::random();

		let (pov_send, _) = oneshot::channel();

		let pov = make_pov(vec![1, 2, 3]);
		let pov_hash = pov.hash();

		let bad_pov = make_pov(vec![6, 6, 6]);

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let mut b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				// pov is being fetched.
				b.fetching.insert(pov_hash, vec![pov_send]);

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			// Peer A answers our request: right relay parent, awaited hash, wrong PoV.
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::SendPoV(hash_a, pov_hash, bad_pov.clone()).encode(),
				),
			).await.unwrap();

			// didn't complete our sender.
			assert_eq!(state.relay_parent_state[&hash_a].fetching[&pov_hash].len(), 1);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_UNEXPECTED_POV);
				}
			);
		});
	}

	#[test]
	fn peer_punished_for_sending_unexpected_pov() {
		let hash_a: Hash = [0; 32].into();

		let peer_a = PeerId::random();

		let pov = make_pov(vec![1, 2, 3]);
		let pov_hash = pov.hash();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			// Peer A answers our request: right relay parent, awaited hash, wrong PoV.
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::SendPoV(hash_a, pov_hash, pov.clone()).encode(),
				),
			).await.unwrap();

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_UNEXPECTED_POV);
				}
			);
		});
	}

	#[test]
	fn peer_punished_for_sending_pov_out_of_our_view() {
		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into();

		let peer_a = PeerId::random();

		let pov = make_pov(vec![1, 2, 3]);
		let pov_hash = pov.hash();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			// Peer A answers our request: right relay parent, awaited hash, wrong PoV.
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::SendPoV(hash_b, pov_hash, pov.clone()).encode(),
				),
			).await.unwrap();

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_UNEXPECTED_POV);
				}
			);
		});
	}

	#[test]
	fn peer_reported_for_awaiting_too_much() {
		let hash_a: Hash = [0; 32].into();

		let peer_a = PeerId::random();
		let n_validators = 10;

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators,
				};

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			let max_plausibly_awaited = n_validators * 2;

			// The peer awaits a plausible (albeit unlikely) amount of PoVs.
			for i in 0..max_plausibly_awaited {
				let pov_hash = make_pov(vec![i as u8; 32]).hash();
				handle_network_update(
					&mut state,
					&mut ctx,
					NetworkBridgeEvent::PeerMessage(
						peer_a.clone(),
						WireMessage::Awaiting(hash_a, vec![pov_hash]).encode(),
					),
				).await.unwrap();
			}

			assert_eq!(state.peer_state[&peer_a].awaited[&hash_a].len(), max_plausibly_awaited);

			// The last straw:
			let last_pov_hash = make_pov(vec![max_plausibly_awaited as u8; 32]).hash();
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::Awaiting(hash_a, vec![last_pov_hash]).encode(),
				),
			).await.unwrap();

			// No more bookkeeping for you!
			assert_eq!(state.peer_state[&peer_a].awaited[&hash_a].len(), max_plausibly_awaited);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_APPARENT_FLOOD);
				}
			);
		});
	}

	#[test]
	fn peer_reported_for_awaiting_outside_their_view() {
		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into();

		let peer_a = PeerId::random();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				s.insert(hash_a, BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				});

				s.insert(hash_b, BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				});

				s
			},
			peer_state: {
				let mut s = HashMap::new();

				// Peer has only hash A in its view.
				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a, hash_b]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			let pov_hash = make_pov(vec![1, 2, 3]).hash();

			// Hash B is in our view but not the peer's
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::Awaiting(hash_b, vec![pov_hash]).encode(),
				),
			).await.unwrap();

			assert!(state.peer_state[&peer_a].awaited.get(&hash_b).is_none());

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_AWAITED_NOT_IN_VIEW);
				}
			);
		});
	}

	#[test]
	fn peer_reported_for_awaiting_outside_our_view() {
		let hash_a: Hash = [0; 32].into();
		let hash_b: Hash = [1; 32].into();

		let peer_a = PeerId::random();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				s.insert(hash_a, BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				});

				s
			},
			peer_state: {
				let mut s = HashMap::new();

				// Peer has hashes A and B in their view.
				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![]), (hash_b, vec![])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			let pov_hash = make_pov(vec![1, 2, 3]).hash();

			// Hash B is in peer's view but not ours.
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::Awaiting(hash_b, vec![pov_hash]).encode(),
				),
			).await.unwrap();

			// Illegal `awaited` is ignored.
			assert!(state.peer_state[&peer_a].awaited[&hash_b].is_empty());

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, COST_AWAITED_NOT_IN_VIEW);
				}
			);
		});
	}

	#[test]
	fn peer_complete_fetch_leads_to_us_completing_others() {
		let hash_a: Hash = [0; 32].into();

		let peer_a = PeerId::random();
		let peer_b = PeerId::random();

		let (pov_send, pov_recv) = oneshot::channel();

		let pov = make_pov(vec![1, 2, 3]);
		let pov_hash = pov.hash();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let mut b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				// pov is being fetched.
				b.fetching.insert(pov_hash, vec![pov_send]);

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![])]),
				);

				// peer B is awaiting peer A's request.
				s.insert(
					peer_b.clone(),
					make_peer_state(vec![(hash_a, vec![pov_hash])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::SendPoV(hash_a, pov_hash, pov.clone()).encode(),
				),
			).await.unwrap();

			assert_eq!(&*pov_recv.await.unwrap(), &pov);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, BENEFIT_FRESH_POV);
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendMessage(peers, protocol, message)
				) => {
					assert_eq!(peers, vec![peer_b.clone()]);
					assert_eq!(protocol, PROTOCOL_V1);
					assert_eq!(
						message,
						WireMessage::SendPoV(hash_a, pov_hash, pov.clone()).encode(),
					);
				}
			);

			assert!(!state.peer_state[&peer_b].awaited[&hash_a].contains(&pov_hash));
		});
	}

	// TODO [now] awaiting peer sending us something is no longer awaiting.
	#[test]
	fn peer_completing_request_no_longer_awaiting() {
		let hash_a: Hash = [0; 32].into();

		let peer_a = PeerId::random();

		let (pov_send, pov_recv) = oneshot::channel();

		let pov = make_pov(vec![1, 2, 3]);
		let pov_hash = pov.hash();

		let mut state = State {
			relay_parent_state: {
				let mut s = HashMap::new();
				let mut b = BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: 10,
				};

				// pov is being fetched.
				b.fetching.insert(pov_hash, vec![pov_send]);

				s.insert(hash_a, b);
				s
			},
			peer_state: {
				let mut s = HashMap::new();

				// peer A is registered as awaiting.
				s.insert(
					peer_a.clone(),
					make_peer_state(vec![(hash_a, vec![pov_hash])]),
				);

				s
			},
			our_view: View(vec![hash_a]),
		};

		let pool = ThreadPool::new().unwrap();
		let (mut ctx, mut handle) = subsystem_test::make_subsystem_context(pool);

		executor::block_on(async move {
			handle_network_update(
				&mut state,
				&mut ctx,
				NetworkBridgeEvent::PeerMessage(
					peer_a.clone(),
					WireMessage::SendPoV(hash_a, pov_hash, pov.clone()).encode(),
				),
			).await.unwrap();

			assert_eq!(&*pov_recv.await.unwrap(), &pov);

			assert_matches!(
				handle.recv().await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep)
				) => {
					assert_eq!(peer, peer_a);
					assert_eq!(rep, BENEFIT_FRESH_POV);
				}
			);

			// We received the PoV from peer A, so we do not consider it awaited by peer A anymore.
			assert!(!state.peer_state[&peer_a].awaited[&hash_a].contains(&pov_hash));
		});
	}
}
