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

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]

use polkadot_primitives::v1::{Hash, PoV, CandidateDescriptor};
use polkadot_subsystem::{
	ActiveLeavesUpdate, OverseerSignal, SubsystemContext, Subsystem, SubsystemResult, SubsystemError,
	FromOverseer, SpawnedSubsystem,
	messages::{
		PoVDistributionMessage, RuntimeApiMessage, RuntimeApiRequest, AllMessages, NetworkBridgeMessage,
	},
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_node_network_protocol::{v1 as protocol_v1, ReputationChange as Rep, NetworkBridgeEvent, PeerId, View};

use futures::prelude::*;
use futures::channel::oneshot;

use std::collections::{hash_map::{Entry, HashMap}, HashSet};
use std::sync::Arc;

#[cfg(test)]
mod tests;

const COST_APPARENT_FLOOD: Rep = Rep::new(-500, "Peer appears to be flooding us with PoV requests");
const COST_UNEXPECTED_POV: Rep = Rep::new(-500, "Peer sent us an unexpected PoV");
const COST_AWAITED_NOT_IN_VIEW: Rep
	= Rep::new(-100, "Peer claims to be awaiting something outside of its view");

const BENEFIT_FRESH_POV: Rep = Rep::new(25, "Peer supplied us with an awaited PoV");
const BENEFIT_LATE_POV: Rep = Rep::new(10, "Peer supplied us with an awaited PoV, \
	but was not the first to do so");

const LOG_TARGET: &str = "pov_distribution";

/// The PoV Distribution Subsystem.
pub struct PoVDistribution {
	// Prometheus metrics
	metrics: Metrics,
}

impl<C> Subsystem<C> for PoVDistribution
	where C: SubsystemContext<Message = PoVDistributionMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		// Swallow error because failure is fatal to the node and we log with more precision
		// within `run`.
		let future = self.run(ctx)
			.map_err(|e| SubsystemError::with_origin("pov-distribution", e))
			.boxed();
		SpawnedSubsystem {
			name: "pov-distribution-subsystem",
			future,
		}
	}
}

struct State {
	relay_parent_state: HashMap<Hash, BlockBasedState>,
	peer_state: HashMap<PeerId, PeerState>,
	our_view: View,
	metrics: Metrics,
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

fn awaiting_message(relay_parent: Hash, awaiting: Vec<Hash>)
	-> protocol_v1::ValidationProtocol
{
	protocol_v1::ValidationProtocol::PoVDistribution(
		protocol_v1::PoVDistributionMessage::Awaiting(relay_parent, awaiting)
	)
}

fn send_pov_message(relay_parent: Hash, pov_hash: Hash, pov: PoV)
	-> protocol_v1::ValidationProtocol
{
	protocol_v1::ValidationProtocol::PoVDistribution(
		protocol_v1::PoVDistributionMessage::SendPoV(relay_parent, pov_hash, pov)
	)
}

/// Handles the signal. If successful, returns `true` if the subsystem should conclude,
/// `false` otherwise.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_signal(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	signal: OverseerSignal,
) -> SubsystemResult<bool> {
	match signal {
		OverseerSignal::Conclude => Ok(true),
		OverseerSignal::ActiveLeaves(ActiveLeavesUpdate { activated, deactivated }) => {
			for relay_parent in activated {
				let (vals_tx, vals_rx) = oneshot::channel();
				ctx.send_message(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::Validators(vals_tx),
				))).await?;

				let n_validators = match vals_rx.await? {
					Ok(v) => v.len(),
					Err(e) => {
						tracing::warn!(
							target: LOG_TARGET,
							err = ?e,
							"Error fetching validators from runtime API for active leaf",
						);

						// Not adding bookkeeping here might make us behave funny, but we
						// shouldn't take down the node on spurious runtime API errors.
						//
						// and this is "behave funny" as in be bad at our job, but not in any
						// slashable or security-related way.
						continue;
					}
				};

				state.relay_parent_state.insert(relay_parent, BlockBasedState {
					known: HashMap::new(),
					fetching: HashMap::new(),
					n_validators: n_validators,
				});
			}

			for relay_parent in deactivated {
				state.relay_parent_state.remove(&relay_parent);
			}

			Ok(false)
		}
		OverseerSignal::BlockFinalized(_) => Ok(false),
	}
}

/// Notify peers that we are awaiting a given PoV hash.
///
/// This only notifies peers who have the relay parent in their view.
#[tracing::instrument(level = "trace", skip(peers, ctx), fields(subsystem = LOG_TARGET))]
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

	let payload = awaiting_message(relay_parent, vec![pov_hash]);

	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
		peers_to_send,
		payload,
	))).await
}

/// Notify one peer about everything we're awaiting at a given relay-parent.
#[tracing::instrument(level = "trace", skip(ctx, relay_parent_state), fields(subsystem = LOG_TARGET))]
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

	let payload = awaiting_message(relay_parent, awaiting_hashes);

	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
		vec![peer.clone()],
		payload,
	))).await
}

/// Distribute a PoV to peers who are awaiting it.
#[tracing::instrument(level = "trace", skip(peers, ctx, metrics, pov), fields(subsystem = LOG_TARGET))]
async fn distribute_to_awaiting(
	peers: &mut HashMap<PeerId, PeerState>,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	metrics: &Metrics,
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

	let payload = send_pov_message(relay_parent, pov_hash, pov.clone());

	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::SendValidationMessage(
		peers_to_send,
		payload,
	))).await?;

	metrics.on_pov_distributed();

	Ok(())
}

/// Handles a `FetchPoV` message.
#[tracing::instrument(level = "trace", skip(ctx, state, response_sender), fields(subsystem = LOG_TARGET))]
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
		tracing::warn!(
			relay_parent_state.fetching.len = relay_parent_state.fetching.len(),
			"other subsystems have requested PoV distribution to fetch more PoVs than reasonably expected",
		);
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
#[tracing::instrument(level = "trace", skip(ctx, state, pov), fields(subsystem = LOG_TARGET))]
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
		&state.metrics,
		relay_parent,
		descriptor.pov_hash,
		&*pov,
	).await
}

/// Report a reputation change for a peer.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn report_peer(
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	peer: PeerId,
	rep: Rep,
) -> SubsystemResult<()> {
	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(peer, rep))).await
}

/// Handle a notification from a peer that they are awaiting some PoVs.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
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
			tracing::warn!("PoV Distribution relay parent state out-of-sync with our view");
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
				let payload = send_pov_message(relay_parent, pov_hash, (&**pov).clone());
				ctx.send_message(AllMessages::NetworkBridge(
					NetworkBridgeMessage::SendValidationMessage(vec![peer.clone()], payload)
				)).await?;
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
#[tracing::instrument(level = "trace", skip(ctx, state, pov), fields(subsystem = LOG_TARGET))]
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
		&state.metrics,
		relay_parent,
		pov_hash,
		&*pov,
	).await
}

/// Handles a network bridge update.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_network_update(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = PoVDistributionMessage>,
	update: NetworkBridgeEvent<protocol_v1::PoVDistributionMessage>,
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
		NetworkBridgeEvent::PeerMessage(peer, message) => {
			match message {
				protocol_v1::PoVDistributionMessage::Awaiting(relay_parent, pov_hashes)
					=> handle_awaiting(
						state,
						ctx,
						peer,
						relay_parent,
						pov_hashes,
					).await,
				protocol_v1::PoVDistributionMessage::SendPoV(relay_parent, pov_hash, pov)
					=> handle_incoming_pov(
						state,
						ctx,
						peer,
						relay_parent,
						pov_hash,
						pov,
					).await,
			}
		}
		NetworkBridgeEvent::OurViewChange(view) => {
			state.our_view = view;
			Ok(())
		}
	}
}

impl PoVDistribution {
	/// Create a new instance of `PovDistribution`.
	pub fn new(metrics: Metrics) -> Self {
		Self { metrics }
	}

	#[tracing::instrument(skip(self, ctx), fields(subsystem = LOG_TARGET))]
	async fn run(
		self,
		mut ctx: impl SubsystemContext<Message = PoVDistributionMessage>,
	) -> SubsystemResult<()> {
		let mut state = State {
			relay_parent_state: HashMap::new(),
			peer_state: HashMap::new(),
			our_view: View(Vec::new()),
			metrics: self.metrics,
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
					PoVDistributionMessage::NetworkBridgeUpdateV1(event) =>
						handle_network_update(
							&mut state,
							&mut ctx,
							event,
						).await?,
				},
			}
		}
	}
}



#[derive(Clone)]
struct MetricsInner {
	povs_distributed: prometheus::Counter<prometheus::U64>,
}

/// Availability Distribution metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_pov_distributed(&self) {
		if let Some(metrics) = &self.0 {
			metrics.povs_distributed.inc();
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> std::result::Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			povs_distributed: prometheus::register(
				prometheus::Counter::new(
					"parachain_povs_distributed_total",
					"Number of PoVs distributed to other peers."
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
