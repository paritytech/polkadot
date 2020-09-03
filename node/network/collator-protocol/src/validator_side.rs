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

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{
	Future, SinkExt, StreamExt,
	channel::{mpsc, oneshot}, stream::FuturesUnordered,
};
use futures_timer::Delay;
use log::{trace, warn};

use polkadot_primitives::v1::{
	Id as ParaId, CandidateReceipt, CollatorId, Hash, PoV,
};
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, SubsystemContext,
	messages::{
		AllMessages, CollatorProtocolMessage, NetworkBridgeMessage,
	},
};
use polkadot_node_network_protocol::{
	v1 as protocol_v1, View, PeerId, ReputationChange as Rep, RequestId,
	NetworkBridgeEvent,
};

use super::{modify_reputation, TARGET, Result};

const COST_UNEXPECTED_MESSAGE: Rep = Rep::new(-10, "An unexpected message");
const COST_REQUEST_TIMEDOUT: Rep = Rep::new(-20, "A collation request has timed out");
const COST_REPORT_BAD: Rep = Rep::new(-50, "A collator was reported by another subsystem");
const BENEFIT_NOTIFY_GOOD: Rep = Rep::new(50, "A collator was noted good by another subsystem");

#[derive(Debug)]
enum CollationRequestResult {
	Received(RequestId),
	Timeout(RequestId),
}

/// A Future representing an ongoing collation request.
/// It may timeout or end in a graceful fashion if a requested
/// collation has been received sucessfully or chain has moved on.
struct CollationRequest {
	// The response for this request has been received successfully or 
	// chain has moved forward and this request is no longer relevant.
	received: oneshot::Receiver<()>,

	// The timeout of this request.
	timeout: Delay,

	// The id of this request.
	request_id: RequestId,
}

impl Future for CollationRequest {
	type Output = CollationRequestResult;

	fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll::<Self::Output> {
		match Pin::new(&mut self.timeout).poll(cx) {
			Poll::Pending => (),
			Poll::Ready(_) => {
				return Poll::Ready(CollationRequestResult::Timeout(self.request_id));
			}
		};

		match Pin::new(&mut self.received).poll(cx) {
			Poll::Pending => (),
			Poll::Ready(_) => {
				return Poll::Ready(CollationRequestResult::Received(self.request_id));
			}
		};

		Poll::Pending
	}
}

struct PerRequest {
	// The sender side to signal the `CollationRequest` to resolve successfully.
	received: oneshot::Sender<()>,

	// Send result here.
	// `Some` in case the collation was requested by some other subsystem.
	// `None` in case the collation was requested by this subsystem.
	result: Option<mpsc::Sender<(CollatorId, CandidateReceipt, PoV)>>,
}

/// All state relevant for the validator side of the protocol lives here.
#[derive(Default)]
struct State {
	/// Our own view.
	view: View,

	/// Track all active collators and their views.
	peer_views: HashMap<PeerId, View>,

	/// Peers that have declared themselves as collators.
	known_collators: HashMap<PeerId, CollatorId>,

	/// Advertisments received from collators. We accept one advertisment
	/// per collator per source per relay-parent.
	advertisments: HashMap<PeerId, HashSet<(ParaId, Hash)>>,

	/// Derive RequestIds from this.
	next_request_id: RequestId,

	/// The collations we have requested by relay parent and para id.
	///
	/// For each relay parent and para id we may be connected to a number
	/// of collators each of those may have advertised a different collation.
	/// So we group such cases here.
	requested_collations: HashMap<(Hash, ParaId, PeerId), RequestId>,

	/// Housekeeping handles we need to have per request to:
	///  - cancel ongoing requests
	///  - reply with collations to other subsystems.
	requests_info: HashMap<RequestId, PerRequest>,

	/// Collation requests that are currently in progress.
	requests_in_progress: FuturesUnordered<CollationRequest>,

	/// Delay after which a collation request would time out.
	request_timeout: Duration, 

	/// Posessed collations.
	collations: HashMap<(Hash, ParaId), Vec<(CollatorId, CandidateReceipt, PoV)>>,
}

/// Another subsystem has requested to fetch collations on a particular leaf for some para.
async fn fetch_collations<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	para_id: ParaId,
	mut tx: mpsc::Sender<(CollatorId, CandidateReceipt, PoV)>
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	// First take a look if we have already stored some of the relevant collations.
	if let Some(collations) = state.collations.get(&(relay_parent, para_id)) {
		for collation in collations.iter() {
			if let Err(e) = tx.send(collation.clone()).await {
				// We do not want this to be fatal because the receving subsystem
				// may have closed the results channel for some reason.
				trace!(
					target: TARGET,
					"Failed to send collation: {:?}", e,
				);
			}
		}
	}

	// Dodge multiple references to `state`.
	let mut relevant_advertisers = Vec::new();

	// Who has advertised a relevant collation?
	for (k, v) in state.advertisments.iter() {
		if v.contains(&(para_id, relay_parent)) {
			relevant_advertisers.push(k.clone());
		}
	}

	// Request the collation.
	// Assume it is `request_collation`'s job to check and ignore duplicate requests.
	for peer_id in relevant_advertisers.into_iter() {
		request_collation(ctx, state, relay_parent, para_id, peer_id, Some(tx.clone())).await?;
	}

	Ok(())
}

/// Report a collator for some malicious actions.
async fn report_collator<Context>(
	ctx: &mut Context,
	state: &mut State,
	id: CollatorId,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	// Since we have a one way map of PeerId -> CollatorId we have to
	// iterate here. Since a huge amount of peers is not expected this
	// is a tolerable thing to do.
	for (k, v) in state.known_collators.iter() {
		if *v == id {
			modify_reputation(ctx, k.clone(), COST_REPORT_BAD).await?;
		}
	}

	Ok(())
}

/// Some other subsystem has reported a collator as a good one, bump reputation.
async fn note_good_collation<Context>(
	ctx: &mut Context,
	state: &mut State,
	id: CollatorId,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	for (peer_id, collator_id) in state.known_collators.iter() {
		if id == *collator_id {
			modify_reputation(ctx, peer_id.clone(), BENEFIT_NOTIFY_GOOD).await?;
		}
	}

	Ok(())
}

/// A peer's view has changed. A number of things should be done:
///  - Ongoing collation requests have to be cancelled.
///  - Advertisments by this peer that are no longer relevant have to be removed.
async fn handle_peer_view_change(
	state: &mut State,
	peer_id: PeerId,
	view: View,
) -> Result<()> {
	let current = state.peer_views.entry(peer_id.clone()).or_default();

	let removed: Vec<_> = current.difference(&view).cloned().collect();

	*current = view;

	if let Some(advertisments) = state.advertisments.get_mut(&peer_id) {
		advertisments.retain(|(_, relay_parent)| !removed.contains(relay_parent));
	}

	let mut requests_to_cancel = Vec::new();

	for removed in removed.into_iter() {
		state.requested_collations.retain(|k, v| {
			if k.0 == removed {
				requests_to_cancel.push(*v);
				false
			} else {
				true
			}
		});
	}

	for r in requests_to_cancel.into_iter() {
		if let Some(per_request) = state.requests_info.remove(&r) {
			per_request.received.send(()).map_err(|_| oneshot::Canceled)?;
		}
	}

	Ok(())
}

/// We have received a collation.
///  - Cancel all ongoing requests
///  - Reply to interested parties if any
///  - Store collation.
async fn received_collation<Context>(
	ctx: &mut Context,
	state: &mut State,
	origin: PeerId,
	request_id: RequestId,
	receipt: CandidateReceipt,
	pov: PoV,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	let relay_parent = receipt.descriptor.relay_parent;
	let para_id = receipt.descriptor.para_id;

	if let Some(id) = state.requested_collations.remove(
		&(relay_parent, para_id, origin.clone())
	) {
		if id == request_id {
			if let Some(mut per_request) = state.requests_info.remove(&id) {
				let _ = per_request.received.send(());
				if let Some(collator_id) = state.known_collators.get(&origin) {
					if let Some(ref mut result) = per_request.result {
						let _ = result.send((collator_id.clone(), receipt.clone(), pov.clone())).await;
					}

					state.collations
						.entry((relay_parent, para_id))
						.or_default()
						.push((collator_id.clone(), receipt, pov));
				}
			}
		}
	} else {
		// TODO: This is tricky. If our chain has moved on, we have already canceled
		// the relevant request and removed it from the map; so and we are not expecting
		// this reply although technically it is not a malicious behaviur.
		modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await?;
	}

	Ok(())
}

/// Request a collation from the network.
/// This function will
///  - Check for duplicate requests.
///  - Check if the requested collation is in our view.
///  - Update PerRequest records with the `result` field if necessary.
/// And as such invocations of this function may rely on that.
async fn request_collation<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
	para_id: ParaId,
	peer_id: PeerId,
	result: Option<mpsc::Sender<(CollatorId, CandidateReceipt, PoV)>>,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	if !state.view.contains(&relay_parent) {
		trace!(
			target: TARGET,
			"Collation by {} on {} on relay parent {} is no longer in view",
			peer_id, para_id, relay_parent,
		);
		return Ok(());
	}

	if let Some(request_id) = state.requested_collations.get(
		&(relay_parent, para_id.clone(), peer_id.clone())
	) {
		if let Some(peer_request) = state.requests_info.get_mut(request_id) {
			// This is a request initiated by us as an action to an advertisment.
			// In that case it has `result` field set to `None` so here we just
			// need to update this field with one provided by another subsystem.
			if peer_request.result.is_none() {
				peer_request.result = result;
			}
		}

		return Ok(());
	}

	let request_id = state.next_request_id;
	state.next_request_id += 1;

	let (tx, rx) = oneshot::channel();

	let per_request = PerRequest {
		received: tx,
		result,
	};

	let request = CollationRequest {
		received: rx,
		timeout: Delay::new(state.request_timeout),
		request_id,
	};

	state.requested_collations.insert((relay_parent, para_id.clone(), peer_id.clone()), request_id);

	state.requests_info.insert(request_id, per_request);

	state.requests_in_progress.push(request);

	let wire_message = protocol_v1::CollatorProtocolMessage::RequestCollation(
		request_id,
		relay_parent,
		para_id,
	);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::SendCollationMessage(
			vec![peer_id],
			protocol_v1::CollationProtocol::CollatorProtocol(wire_message),
		)
	)).await?;

	Ok(())
}

/// Networking message has been received.
async fn process_incoming_peer_message<Context>(
	ctx: &mut Context,
	state: &mut State,
	origin: PeerId,
	msg: protocol_v1::CollatorProtocolMessage,
)-> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use protocol_v1::CollatorProtocolMessage::*;

	match msg {
	    Declare(id) => {
			state.known_collators.insert(origin.clone(), id);
			state.peer_views.entry(origin).or_default();
		}
	    AdvertiseCollation(relay_parent, para_id) => {
			state.advertisments.entry(origin.clone()).or_default().insert((para_id, relay_parent));
			request_collation(ctx, state, relay_parent, para_id, origin, None).await?;
		}
	    RequestCollation(_, _, _) => {
			// This is a validator side of the protocol, collation requests are not expected here.
			return modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
		}
	    Collation(request_id, receipt, pov) => {
			received_collation(ctx, state, origin, request_id, receipt, pov).await?;
		}
	}

	Ok(())
}

/// A leaf has become inactive so we want to
///   - Cancel all ongoing collation requests that are on top of that leaf.
///   - Remove all stored collations relevant to that leaf.
async fn remove_relay_parent(
	state: &mut State,
	relay_parent: Hash,
) -> Result<()> {
	let mut remove_these = Vec::new();

	state.requested_collations.retain(|k, v| {
		if k.0 == relay_parent {
			remove_these.push(*v);
			false
		} else {
			true
		}
	});

	for id in remove_these.into_iter() {
		if let Some(info) = state.requests_info.remove(&id) {
			info.received.send(()).map_err(|_| oneshot::Canceled)?;
		}
	}

	state.collations.retain(|k, _| {
		if k.0 == relay_parent {
			false
		} else {
			true
		}
	});

	Ok(())
}

/// Our view has changed.
async fn handle_our_view_change(
	state: &mut State,
	view: View,
) -> Result<()> {
	let old_view = std::mem::replace(&mut (state.view), view);

	let cloned_view = state.view.clone();
	let removed = old_view.difference(&cloned_view).collect::<Vec<_>>();

	for removed in removed {
		remove_relay_parent(state, *removed).await?;
	}

	Ok(())
}

/// A request has timed out.
async fn request_timed_out<Context>(
	ctx: &mut Context,
	state: &mut State,
	id: RequestId,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	// We have to go backwards in the map, again.
	if let Some(key) = find_val_in_map(&state.requested_collations, &id) {
		if let Some(_) = state.requested_collations.remove(&key) {
			if let Some(request_info) = state.requests_info.remove(&id) {
				let (relay_parent, para_id, peer_id) = key;

				modify_reputation(ctx, peer_id.clone(), COST_REQUEST_TIMEDOUT).await?;

				// the callee will check if the parent is still in view, so do no checks here.
				request_collation(
					ctx,
					state,
					relay_parent,
					para_id,
					peer_id,
					request_info.result,
				).await?;
			}
		}
	}

	Ok(())
}

/// Bridge event switch.
async fn handle_network_msg<Context>(
	ctx: &mut Context,
	state: &mut State,
	bridge_message: NetworkBridgeEvent<protocol_v1::CollatorProtocolMessage>,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use NetworkBridgeEvent::*;

	match bridge_message {
		PeerConnected(_id, _role) => {
			// A peer has connected. Until it issues a `Declare` message we do not
			// want to track it's view or take any other actions.
		},
		PeerDisconnected(peer_id) => {
			state.peer_views.remove(&peer_id);
		},
		PeerViewChange(peer_id, view) => {
			handle_peer_view_change(state, peer_id, view).await?;
		},
		OurViewChange(view) => {
			handle_our_view_change(state, view).await?;
		},
		PeerMessage(remote, msg) => {
			process_incoming_peer_message(ctx, state, remote, msg).await?;
		}
	}

	Ok(())
}

/// The main message receiver switch.
async fn process_msg<Context>(
	ctx: &mut Context,
	msg: CollatorProtocolMessage,
	state: &mut State,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use CollatorProtocolMessage::*;

	match msg {
		CollateOn(id) => {
			warn!(
				target: TARGET,
				"CollateOn({}) message is not expected on the validator side of the protocol", id,
			);
		}
		DistributeCollation(_, _) => {
			warn!(
				target: TARGET,
				"DistributeCollation message is not expected on the validator side of the protocol",
			);
		}
		FetchCollations(relay_parent, para_id, tx) => {
			fetch_collations(ctx, state, relay_parent, para_id, tx).await?;
		}
		ReportCollator(id) => {
			report_collator(ctx, state, id).await?;
		}
		NoteGoodCollation(id) => {
			note_good_collation(ctx, state, id).await?;
		}
		NetworkBridgeUpdateV1(event) => {
			if let Err(e) = handle_network_msg(
				ctx,
				state,
				event,
			).await {
				warn!(
					target: TARGET,
					"Failed to handle incoming network message: {:?}", e,
				);
			}
		}
	}

	Ok(())
}

/// The main run loop.
pub(crate) async fn run<Context>(mut ctx: Context, request_timeout: Duration) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use FromOverseer::*;
	use OverseerSignal::*;

	let mut state = State {
		request_timeout,
		..Default::default()
	};

	loop {
		if let Poll::Ready(msg) = futures::poll!(ctx.recv()) {
			let msg = msg?;
			trace!(
				"> Received a message {:?}", msg,
			);

			match msg {
				Communication { msg } => process_msg(&mut ctx, msg, &mut state).await?,
				Signal(BlockFinalized(_)) => {}
				Signal(ActiveLeaves(_)) => {}
				Signal(Conclude) => { break }
			}
			continue;
		}

		if let Poll::Ready(Some(request)) = futures::poll!(state.requests_in_progress.next()) {
			// Request has timed out, we need to penalize the collator and re-send the request
			// if the chain has not moved on yet.
			match request {
				CollationRequestResult::Timeout(id) => {
					trace!(target: TARGET, "Request timed out {}", id);
					request_timed_out(&mut ctx, &mut state, id).await?;
				}
				CollationRequestResult::Received(id) => {
					state.requests_info.remove(&id);
				}
			}
			continue;
		}

		futures::pending!();
	}

	Ok(())
}

fn find_val_in_map<K: Clone, V: Eq>(map: &HashMap<K, V>, val: &V) -> Option<K> {
	map
		.iter()
		.find_map(|(k, v)| if v == val { Some(k.clone()) } else { None })
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::iter;
	use futures::{executor, future};
	use sp_core::crypto::Pair;
	use assert_matches::assert_matches;

	use polkadot_primitives::v1::{BlockData, CollatorPair};
	use polkadot_subsystem_testhelpers::{self as test_helpers, TimeoutExt};

	#[derive(Clone)]
	struct TestState {
		chain_ids: Vec<ParaId>,
		relay_parent: Hash,
		collators: Vec<CollatorPair>,
	}

	impl Default for TestState {
		fn default() -> Self {
			let chain_a = ParaId::from(1);
			let chain_b = ParaId::from(2);

			let chain_ids = vec![chain_a, chain_b];
			let relay_parent = Hash::repeat_byte(0x05);
			let collators = iter::repeat(())
				.map(|_| CollatorPair::generate().0)
				.take(4)
				.collect();

			Self {
				chain_ids,
				relay_parent,
				collators,
			}
		}
	}

	struct TestHarness {
		virtual_overseer: test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
	}

	fn test_harness<T: Future<Output = ()>>(test: impl FnOnce(TestHarness) -> T) {
		let _ = env_logger::builder()
			.is_test(true)
			.filter(
				Some("polkadot_collator_protocol"),
				log::LevelFilter::Trace,
			)
			.filter(
				Some(TARGET),
				log::LevelFilter::Trace,
			)
			.try_init();

		let pool = sp_core::testing::TaskExecutor::new();

		let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

		let subsystem = run(context, Duration::from_millis(50));

		let test_fut = test(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	const TIMEOUT: Duration = Duration::from_millis(100);

	/*
	async fn overseer_signal(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
		signal: OverseerSignal,
	) {
		log::trace!("Sending signal:\n{:?}", signal);
		overseer
			.send(FromOverseer::Signal(signal))
			.timeout(TIMEOUT)
			.await
			.expect("Is more than enough for sending signals");
	}
	*/

	async fn overseer_send(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
		msg: CollatorProtocolMessage,
	) {
		log::trace!("Sending message:\n{:?}", &msg);
		overseer
			.send(FromOverseer::Communication { msg })
			.timeout(TIMEOUT)
			.await
			.expect(&format!("{:?} is enough for sending messages.", TIMEOUT));
	}

	async fn overseer_recv(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
	) -> AllMessages {
		let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
			.await
			.expect(&format!("{:?} is enough to receive messages.", TIMEOUT));

		log::trace!("Received message:\n{:?}", &msg);

		msg
	}

	async fn overseer_recv_with_timeout(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
		timeout: Duration,
	) -> Option<AllMessages> {
		log::trace!("Waiting for message...");
		overseer
			.recv()
			.timeout(timeout)
			.await
	}

	#[test]
	fn act_on_advertisment() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			let pair = CollatorPair::generate().0;
			log::trace!("activating");

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(View(vec![test_state.relay_parent]))
				)
			).await;


			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(pair.public()),
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			let request_id = assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendCollationMessage(
					peers,
					protocol_v1::CollationProtocol::CollatorProtocol(
						protocol_v1::CollatorProtocolMessage::RequestCollation(
							id,
							relay_parent,
							para_id,
						)
					)
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(peers, vec![peer_b.clone()]);
				assert_eq!(para_id, test_state.chain_ids[0]);
				id
			});

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Collation(
							request_id,
							Default::default(),
							PoV {
								block_data: BlockData(vec![]),
							},
						)
					)
				)
			).await;
		});
	}

	#[test]
	fn collation_request_times_out() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(View(vec![test_state.relay_parent]))
				)
			).await;

			let peer_b = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[0].public(),
						),
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			let request_id = assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendCollationMessage(
					peers,
					protocol_v1::CollationProtocol::CollatorProtocol(
						protocol_v1::CollatorProtocolMessage::RequestCollation(
							id,
							relay_parent,
							para_id,
						)
					)
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(peers, vec![peer_b.clone()]);
				assert_eq!(para_id, test_state.chain_ids[0]);
				id
			});

			// Don't send a response and we shoud see reputation penalties to the
			// collator.
			for _ in 0..3usize {
				Delay::new(Duration::from_millis(50)).await;
				assert_matches!(
					overseer_recv(&mut virtual_overseer).await,
					AllMessages::NetworkBridge(
						NetworkBridgeMessage::ReportPeer(peer, rep)
					) => {
						assert_eq!(peer, peer_b);
						assert_eq!(rep, COST_REQUEST_TIMEDOUT);
					}
				);

				assert_matches!(
					overseer_recv(&mut virtual_overseer).await,
					AllMessages::NetworkBridge(NetworkBridgeMessage::SendCollationMessage(
						peers,
						protocol_v1::CollationProtocol::CollatorProtocol(
							protocol_v1::CollatorProtocolMessage::RequestCollation(
								req_id,
								relay_parent,
								para_id,
							)
						)
					)
				) => {
					assert_eq!(relay_parent, test_state.relay_parent);
					assert_eq!(peers, vec![peer_b.clone()]);
					assert_eq!(para_id, test_state.chain_ids[0]);
					assert_ne!(request_id, req_id);
				});
			}

			// Deactivate the relay parent in question.
			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(View(vec![Hash::repeat_byte(0x42)]))
				)
			).await;

			// After we've deactivated it we are not expecting any more requests
			// for timed out collations.
			assert!(
				overseer_recv_with_timeout(
					&mut virtual_overseer,
					Duration::from_secs(1),
				).await.is_none()
			);
		});
	}

	#[test]
	fn collator_reporting_works() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(View(vec![test_state.relay_parent]))
				)
			).await;

			let peer_b = PeerId::random();
			let peer_c = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[0].public(),
						),
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[1].public(),
						),
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::ReportCollator(test_state.collators[0].public()),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep),
				) => {
					assert_eq!(peer, peer_b);
					assert_eq!(rep, COST_REPORT_BAD);
				}
			);

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NoteGoodCollation(test_state.collators[1].public()),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(
					NetworkBridgeMessage::ReportPeer(peer, rep),
				) => {
					assert_eq!(peer, peer_c);
					assert_eq!(rep, BENEFIT_NOTIFY_GOOD);
				}
			);
		});
	}

	#[test]
	fn fetch_collations_works() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(View(vec![test_state.relay_parent]))
				)
			).await;

			let peer_b = PeerId::random();
			let peer_c = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[0].public(),
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[1].public(),
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			let (request_id, peer_id) = assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendCollationMessage(
					peers,
					protocol_v1::CollationProtocol::CollatorProtocol(
						protocol_v1::CollatorProtocolMessage::RequestCollation(
							id,
							relay_parent,
							para_id,
						)
					)
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				(id, peers[0].clone())
			});

			let mut candidate_a = CandidateReceipt::default();
			candidate_a.descriptor.para_id = test_state.chain_ids[0];
			candidate_a.descriptor.relay_parent = test_state.relay_parent;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_id,
						protocol_v1::CollatorProtocolMessage::Collation(
							request_id,
							candidate_a,
							PoV {
								block_data: BlockData(vec![]),
							},
						)
					)
				)
			).await;

			let (request_id, peer_id) = assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendCollationMessage(
					peers,
					protocol_v1::CollationProtocol::CollatorProtocol(
						protocol_v1::CollatorProtocolMessage::RequestCollation(
							id,
							relay_parent,
							para_id,
						)
					)
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				(id, peers[0].clone())
			});

			let mut candidate_b = CandidateReceipt::default();
			candidate_b.descriptor.para_id = test_state.chain_ids[0];
			candidate_b.descriptor.relay_parent = test_state.relay_parent;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_id,
						protocol_v1::CollatorProtocolMessage::Collation(
							request_id,
							candidate_b,
							PoV {
								block_data: BlockData(vec![1, 2, 3]),
							},
						)
					)
				)
			).await;

			let (tx, mut rx) = mpsc::channel(16);

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::FetchCollations(
					test_state.relay_parent,
					test_state.chain_ids[0],
					tx,
				)
			).await;

			let mut received_collations = Vec::new();

			for _ in 0..2 {
				received_collations.push(rx.next().await.unwrap());
			}
		});
	}

	#[test]
	fn fetch_collation_and_delay_works() {
		let test_state = TestState::default();

		test_harness(|test_harness| async move {
			let TestHarness {
				mut virtual_overseer,
			} = test_harness;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(View(vec![test_state.relay_parent]))
				)
			).await;

			let peer_b = PeerId::random();
			let peer_c = PeerId::random();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[0].public(),
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::CollatorProtocolMessage::Declare(
							test_state.collators[1].public(),
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_b.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_c.clone(),
						protocol_v1::CollatorProtocolMessage::AdvertiseCollation(
							test_state.relay_parent,
							test_state.chain_ids[0],
						)
					)
				)
			).await;

			let (request_id_1, peer_id_1) = assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendCollationMessage(
					peers,
					protocol_v1::CollationProtocol::CollatorProtocol(
						protocol_v1::CollatorProtocolMessage::RequestCollation(
							id,
							relay_parent,
							para_id,
						)
					)
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				(id, peers[0].clone())
			});

			let (request_id_2, peer_id_2) = assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::NetworkBridge(NetworkBridgeMessage::SendCollationMessage(
					peers,
					protocol_v1::CollationProtocol::CollatorProtocol(
						protocol_v1::CollatorProtocolMessage::RequestCollation(
							id,
							relay_parent,
							para_id,
						)
					)
				)
			) => {
				assert_eq!(relay_parent, test_state.relay_parent);
				assert_eq!(para_id, test_state.chain_ids[0]);
				(id, peers[0].clone())
			});

			let (tx, mut rx) = mpsc::channel(16);

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::FetchCollations(
					test_state.relay_parent,
					test_state.chain_ids[0],
					tx,
				)
			).await;


			let mut candidate_b = CandidateReceipt::default();
			candidate_b.descriptor.para_id = test_state.chain_ids[0];
			candidate_b.descriptor.relay_parent = test_state.relay_parent;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_id_2,
						protocol_v1::CollatorProtocolMessage::Collation(
							request_id_2,
							candidate_b,
							PoV {
								block_data: BlockData(vec![1, 2, 3]),
							},
						)
					)
				)
			).await;

			let mut candidate_a = CandidateReceipt::default();
			candidate_a.descriptor.para_id = test_state.chain_ids[0];
			candidate_a.descriptor.relay_parent = test_state.relay_parent;

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::PeerMessage(
						peer_id_1,
						protocol_v1::CollatorProtocolMessage::Collation(
							request_id_1,
							candidate_a,
							PoV {
								block_data: BlockData(vec![]),
							},
						)
					)
				)
			).await;

			let mut received_collations = Vec::new();

			for _ in 0..2 {
				received_collations.push(rx.next().await.unwrap());
			}
		});
	}
}
