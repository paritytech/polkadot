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
	pin_mut, poll, select,
	Future, FutureExt, StreamExt,
	channel::oneshot, stream::FuturesUnordered,
};
use futures_timer::Delay;
use log::{trace, warn};

use polkadot_primitives::v1::{
	Id as ParaId, CandidateReceipt, CollatorId, Hash, PoV,
};
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, Subsystem, SubsystemContext, SubsystemError, SpawnedSubsystem,
	messages::{
		AllMessages, CollatorProtocolMessage, NetworkBridgeMessage,
	},
};
use polkadot_node_network_protocol::{
	v1 as protocol_v1, View, PeerId, ReputationChange as Rep, RequestId,
	NetworkBridgeEvent,
};

use super::{
	modify_reputation,
	TARGET,
	Error, Result,
};

const COST_UNEXPECTED_MESSAGE: Rep = Rep::new(-10, "An unexpected message");
const COST_REPORT_BAD: Rep = Rep::new(-50, "A collator was reported by another subsystem");
const COST_REPORT_BAN: Rep = Rep::new(-100, "This peer should be banned");
const COST_FATAL: Rep = Rep::new_fatal("Collator has commited a fatal offence and should be disconnected from");

const BENEFIT_NOTIFY_GOOD: Rep = Rep::new(50, "A collator was noted good by another subsystem");

#[derive(Debug)]
enum CollationRequestResult {
	Received(RequestId),
	Timeout(RequestId),
}

struct CollationRequest {
	// The response for this request has been received successfully.
	received: oneshot::Receiver<()>,

	// The timeout of this request.
	timeout: Delay,

	// The id of this request.
	request_id: RequestId,
}

struct PerRequest {
	peer_id: PeerId,

	relay_parent: Hash,

	para_id: ParaId,

	// The sender side to signal the `CollationRequest` to resolve successfully.
	received: oneshot::Sender<()>,

	// Send result here.
	// `Some` in case the collation was requested by some other subsystem.
	// `None` in case the collation was requested by this subsystem.
	result: Option<oneshot::Sender<(CandidateReceipt, PoV)>>,
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

#[derive(Default)]
struct State {
	/// Our own view.
	view: View,

	/// Peers that are now connected to us.
	connected_peers: HashSet<PeerId>,

	/// Peers that have declared themselves as collators.
	known_collators: HashMap<PeerId, CollatorId>,

	/// Advertisments received from collators. We accept one advertisment
	/// per collator per source per relay-parent.
	advertisments: HashMap<(CollatorId, PeerId, Hash), ParaId>,

	/// Derive RequestIds from this.
	next_request_id: RequestId,

	/// The collations we have requested by relay parent and para id.
	requested_collations: HashMap<(Hash, ParaId), RequestId>,

	/// Info on requests that are currently in progress.
	requests_in_progress_info: HashMap<RequestId, PerRequest>,

	/// Collation requests that are currently in progress.
	requests_in_progress: FuturesUnordered<CollationRequest>,

	/// Banned peers.
	banned_peers: HashSet<PeerId>,
}

async fn request_collation<Context>(
	ctx: &mut Context,
	state: &mut State,
	peer_id: PeerId,
	relay_parent: Hash,
	para_id: ParaId,
	result: Option<oneshot::Sender<(CandidateReceipt, PoV)>>,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	let request_id = state.next_request_id;
	state.next_request_id += 1;

	let (tx, rx) = oneshot::channel();

	let per_request = PerRequest {
		peer_id: peer_id.clone(),
		relay_parent,
		para_id,
		received: tx,
		result,
	};

	let request = CollationRequest {
		received: rx,
		timeout: Delay::new(Duration::from_secs(1)), // TODO: make configurable.
		request_id,
	};

	state.requests_in_progress_info.insert(request_id, per_request);

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

async fn re_request_collation<Context>(ctx: &mut Context, state: &mut State, id: RequestId)
	-> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	if let Some(per_request) = state.requests_in_progress_info.remove(&id) {
		let PerRequest {
			peer_id,
			relay_parent,
			para_id,
			received: _,
			result,
		} = per_request;

		if state.view.0.contains(&relay_parent) {
			trace!(
				target: TARGET,
				"Re-requesting timed out collation request on {} for {}",
				relay_parent,
				para_id,
			);

			modify_reputation(ctx, peer_id.clone(), COST_REPORT_BAN).await?;
			request_collation(ctx, state, peer_id, relay_parent, para_id, result).await?;
		} else {
			trace!(
				target: TARGET,
				"The chain has moved on from {}, dropping collation request for {}",
				relay_parent, para_id,
			);
		}
	}

	Ok(())
}

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
			state.known_collators.entry(origin).or_insert(id);
		}
	    AdvertiseCollation(relay_parent, para_id) => {
			// TODO: deal with cloning
			match state.known_collators.get(&origin).cloned() {
				Some(collator_id) => {
					match state.advertisments.get(&(collator_id.clone(), origin.clone(), relay_parent)).cloned() {
						Some(pid) if para_id != pid => {
							// This peer advertised another paraid, should probably be punished.
						}
						Some(_para_id) => {
							// This is the advertisment with the expected CollatorId.
						}
						None => {
							// A previously unseen advertisment, should now act upon it and issue
							// a request for this collation.
							state.advertisments.insert((collator_id, origin.clone(), relay_parent), para_id);
							request_collation(ctx, state, origin, relay_parent, para_id, None).await?;
						}
					}
				}
				None => {
					// This is a collator that has not yet declared itself and probably
					// should be punished.
					return modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
				}
			};
		}
	    RequestCollation(_, _, _) => {
			// This is a validator side of the protocol, collation requests are not expected here.
			return modify_reputation(ctx, origin, COST_UNEXPECTED_MESSAGE).await;
		}
	    Collation(request_id, candidate_receipt, pov) => {
		}
	}

	Ok(())
}

async fn remove_relay_parent<Context>(
	ctx: &mut Context,
	state: &mut State,
	relay_parent: Hash,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	let mut requests_to_cancel = Vec::new();

	state.requested_collations.retain(|k, request_id| {
		if k.0 == relay_parent {
			requests_to_cancel.push(*request_id);
			false
		} else {
			true
		}
	});

	for request in requests_to_cancel.drain(..) {
		if let Some(per_request) = state.requests_in_progress_info.remove(&request) {
			per_request.received.send(());
		}
	}

	Ok(())
}

async fn handle_our_view_change<Context>(
	ctx: &mut Context,
	state: &mut State,
	view: View,
) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	let old_view = std::mem::replace(&mut (state.view), view);

	let cloned_view = state.view.clone();
	let removed = old_view.difference(&cloned_view).collect::<Vec<_>>();

	for removed in removed {
		remove_relay_parent(ctx, state, *removed).await?;
	}

	Ok(())
}

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
		PeerConnected(id, _role) => {
			state.connected_peers.insert(id);
		},
		PeerDisconnected(id) => {},
		PeerViewChange(_id, _view) => {},
		OurViewChange(view) => {
			handle_our_view_change(ctx, state, view).await?;
		},
		PeerMessage(remote, msg) => {
			process_incoming_peer_message(ctx, state, remote, msg).await?;
		}
	}

	Ok(())
}

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
		},
		FetchCollation(relay_parent, para_id, tx) => {
			// request_collation(ctx, state, Default::default(), relay_parent, para_id, Some(tx)).await?;
		},
		ReportCollator(id) => {
			let mut banned = Vec::new();

			state.known_collators.retain(|k, v| {
				if *v != id {
					true
				} else {
					// modify_reputation(ctx, peer_id.clone(), COST_REPORT_BAN).await?;
					banned.push(k.clone());
					false
				}
			});

			for peer_id in banned.iter() {
				modify_reputation(ctx, peer_id.clone(), COST_REPORT_BAD).await?;
			}
			state.banned_peers.extend(banned.into_iter());
		},
		NoteGoodCollation(id) => {
			for (peer_id, collator_id) in state.known_collators.iter() {
				if id == *collator_id {
					modify_reputation(ctx, peer_id.clone(), BENEFIT_NOTIFY_GOOD).await?;
				}
			}
		},
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
		},
	}

	Ok(())
}

pub(crate) async fn run<Context>(mut ctx: Context) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>
{
	use FromOverseer::*;
	use OverseerSignal::*;

	let mut state = State::default();

	loop {
		if let Poll::Ready(msg) = futures::poll!(ctx.recv()) {
			let msg = msg?;
			trace!(
				"> Received a message {:?}", msg,
			);

			match msg {
				Communication { msg } => process_msg(&mut ctx, msg, &mut state).await?,
				Signal(_) => {}
				Signal(BlockFinalized(_)) => {}
				Signal(Conclude) => break,
			}
			continue;
		}

		if let Poll::Ready(Some(request)) = futures::poll!(state.requests_in_progress.next()) {
			// Request has timed out, we need to penalize the collator and re-send the request
			// if the chain has not moved on yet.
			match request {
				CollationRequestResult::Timeout(id) => {
					trace!(target: TARGET, "Request timed out {}", id);
					re_request_collation(&mut ctx, &mut state, id).await?;
				}
				CollationRequestResult::Received(id) => {
					state.requests_in_progress_info.remove(&id);
				}
			}
			continue;
		}

		futures::pending!();
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::iter;
	use futures::{
		executor, future,
	};
	use sp_core::crypto::Pair;
	use smallvec::smallvec;
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
			.try_init();

		let pool = sp_core::testing::TaskExecutor::new();

		let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

		let subsystem = run(context);

		let test_fut = test(TestHarness { virtual_overseer });

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	const TIMEOUT: Duration = Duration::from_millis(100);

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

	async fn overseer_send(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
		msg: CollatorProtocolMessage,
	) {
		log::trace!("Sending message:\n{:?}", &msg);
		overseer
			.send(FromOverseer::Communication { msg })
			.timeout(TIMEOUT)
			.await
			.expect("Is more than enough for sending messages.");
	}

	async fn overseer_recv(
		overseer: &mut test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>,
	) -> AllMessages {
		let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
			.await
			.expect("is enough to receive messages.");

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
				Delay::new(Duration::from_secs(1)).await;
				assert_matches!(
					overseer_recv(&mut virtual_overseer).await,
					AllMessages::NetworkBridge(
						NetworkBridgeMessage::ReportPeer(peer, rep)
					) => {
						assert_eq!(peer, peer_b);
						assert_eq!(rep, COST_REPORT_BAN);
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

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(View(vec![test_state.relay_parent]))
				)
			).await;

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
					Duration::from_secs(2),
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
	fn view_change_cancels_ongoing_requests() {
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

			let (tx, rx) = oneshot::channel();

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::FetchCollation(
					test_state.relay_parent,
					test_state.chain_ids[0],
					tx,
				)
			).await;

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
			});

			overseer_send(
				&mut virtual_overseer,
				CollatorProtocolMessage::NetworkBridgeUpdateV1(
					NetworkBridgeEvent::OurViewChange(View(vec![Hash::repeat_byte(0x42)]))
				)
			).await;

			assert!(
				overseer_recv_with_timeout(
					&mut virtual_overseer,
					Duration::from_secs(2),
				).await.is_none()
			);
		});
	}
}
