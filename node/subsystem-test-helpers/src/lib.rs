// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Utilities for testing subsystems.

#![warn(missing_docs)]

use polkadot_node_subsystem::{
	messages::AllMessages, overseer, FromOrchestra, OverseerSignal, SpawnGlue, SpawnedSubsystem,
	SubsystemError, SubsystemResult,
};
use polkadot_node_subsystem_util::TimeoutExt;

use futures::{channel::mpsc, poll, prelude::*};
use parking_lot::Mutex;
use sp_core::testing::TaskExecutor;

use std::{
	convert::Infallible,
	future::Future,
	pin::Pin,
	sync::Arc,
	task::{Context, Poll, Waker},
	time::Duration,
};

/// Generally useful mock data providers for unit tests.
pub mod mock;

enum SinkState<T> {
	Empty { read_waker: Option<Waker> },
	Item { item: T, ready_waker: Option<Waker>, flush_waker: Option<Waker> },
}

/// The sink half of a single-item sink that does not resolve until the item has been read.
pub struct SingleItemSink<T>(Arc<Mutex<SinkState<T>>>);

// Derive clone not possible, as it puts `Clone` constraint on `T` which is not sensible here.
impl<T> Clone for SingleItemSink<T> {
	fn clone(&self) -> Self {
		Self(self.0.clone())
	}
}

/// The stream half of a single-item sink.
pub struct SingleItemStream<T>(Arc<Mutex<SinkState<T>>>);

impl<T> Sink<T> for SingleItemSink<T> {
	type Error = Infallible;

	fn poll_ready(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Infallible>> {
		let mut state = self.0.lock();
		match *state {
			SinkState::Empty { .. } => Poll::Ready(Ok(())),
			SinkState::Item { ref mut ready_waker, .. } => {
				*ready_waker = Some(cx.waker().clone());
				Poll::Pending
			},
		}
	}

	fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), Infallible> {
		let mut state = self.0.lock();

		match *state {
			SinkState::Empty { ref mut read_waker } =>
				if let Some(waker) = read_waker.take() {
					waker.wake();
				},
			_ => panic!("start_send called outside of empty sink state ensured by poll_ready"),
		}

		*state = SinkState::Item { item, ready_waker: None, flush_waker: None };

		Ok(())
	}

	fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Infallible>> {
		let mut state = self.0.lock();
		match *state {
			SinkState::Empty { .. } => Poll::Ready(Ok(())),
			SinkState::Item { ref mut flush_waker, .. } => {
				*flush_waker = Some(cx.waker().clone());
				Poll::Pending
			},
		}
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Result<(), Infallible>> {
		self.poll_flush(cx)
	}
}

impl<T> Stream for SingleItemStream<T> {
	type Item = T;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
		let mut state = self.0.lock();

		let read_waker = Some(cx.waker().clone());

		match std::mem::replace(&mut *state, SinkState::Empty { read_waker }) {
			SinkState::Empty { .. } => Poll::Pending,
			SinkState::Item { item, ready_waker, flush_waker } => {
				if let Some(waker) = ready_waker {
					waker.wake();
				}

				if let Some(waker) = flush_waker {
					waker.wake();
				}

				Poll::Ready(Some(item))
			},
		}
	}
}

/// Create a single-item Sink/Stream pair.
///
/// The sink's send methods resolve at the point which the stream reads the item,
/// not when the item is buffered.
pub fn single_item_sink<T>() -> (SingleItemSink<T>, SingleItemStream<T>) {
	let inner = Arc::new(Mutex::new(SinkState::Empty { read_waker: None }));
	(SingleItemSink(inner.clone()), SingleItemStream(inner))
}

/// A test subsystem sender.
#[derive(Clone)]
pub struct TestSubsystemSender {
	tx: mpsc::UnboundedSender<AllMessages>,
}

/// Construct a sender/receiver pair.
pub fn sender_receiver() -> (TestSubsystemSender, mpsc::UnboundedReceiver<AllMessages>) {
	let (tx, rx) = mpsc::unbounded();
	(TestSubsystemSender { tx }, rx)
}

#[async_trait::async_trait]
impl<OutgoingMessage> overseer::SubsystemSender<OutgoingMessage> for TestSubsystemSender
where
	AllMessages: From<OutgoingMessage>,
	OutgoingMessage: Send + 'static,
{
	async fn send_message(&mut self, msg: OutgoingMessage) {
		self.tx.send(msg.into()).await.expect("test overseer no longer live");
	}

	async fn send_messages<I>(&mut self, msgs: I)
	where
		I: IntoIterator<Item = OutgoingMessage> + Send,
		I::IntoIter: Send,
	{
		let mut iter = stream::iter(msgs.into_iter().map(|msg| Ok(msg.into())));
		self.tx.send_all(&mut iter).await.expect("test overseer no longer live");
	}

	fn send_unbounded_message(&mut self, msg: OutgoingMessage) {
		self.tx.unbounded_send(msg.into()).expect("test overseer no longer live");
	}
}

/// A test subsystem context.
pub struct TestSubsystemContext<M, S> {
	tx: TestSubsystemSender,
	rx: mpsc::Receiver<FromOrchestra<M>>,
	spawn: S,
}

#[async_trait::async_trait]
impl<M, Spawner> overseer::SubsystemContext for TestSubsystemContext<M, Spawner>
where
	M: overseer::AssociateOutgoing + std::fmt::Debug + Send + 'static,
	AllMessages: From<<M as overseer::AssociateOutgoing>::OutgoingMessages>,
	AllMessages: From<M>,
	Spawner: overseer::gen::Spawner + Send + 'static,
{
	type Message = M;
	type Sender = TestSubsystemSender;
	type Signal = OverseerSignal;
	type OutgoingMessages = <M as overseer::AssociateOutgoing>::OutgoingMessages;
	type Error = SubsystemError;

	async fn try_recv(&mut self) -> Result<Option<FromOrchestra<M>>, ()> {
		match poll!(self.rx.next()) {
			Poll::Ready(Some(msg)) => Ok(Some(msg)),
			Poll::Ready(None) => Err(()),
			Poll::Pending => Ok(None),
		}
	}

	async fn recv(&mut self) -> SubsystemResult<FromOrchestra<M>> {
		self.rx
			.next()
			.await
			.ok_or_else(|| SubsystemError::Context("Receiving end closed".to_owned()))
	}

	fn spawn(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> SubsystemResult<()> {
		self.spawn.spawn(name, None, s);
		Ok(())
	}

	fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> SubsystemResult<()> {
		self.spawn.spawn_blocking(name, None, s);
		Ok(())
	}

	fn sender(&mut self) -> &mut TestSubsystemSender {
		&mut self.tx
	}
}

/// A handle for interacting with the subsystem context.
pub struct TestSubsystemContextHandle<M> {
	/// Direct access to sender of messages.
	///
	/// Useful for shared ownership situations (one can have multiple senders, but only one
	/// receiver.
	pub tx: mpsc::Sender<FromOrchestra<M>>,

	/// Direct access to the receiver.
	pub rx: mpsc::UnboundedReceiver<AllMessages>,
}

impl<M> TestSubsystemContextHandle<M> {
	/// Fallback timeout value used to never block test execution
	/// indefinitely.
	pub const TIMEOUT: Duration = Duration::from_secs(120);

	/// Send a message or signal to the subsystem. This resolves at the point in time when the
	/// subsystem has _read_ the message.
	pub async fn send(&mut self, from_overseer: FromOrchestra<M>) {
		self.tx
			.send(from_overseer)
			.timeout(Self::TIMEOUT)
			.await
			.expect("`fn send` does not timeout")
			.expect("Test subsystem no longer live");
	}

	/// Receive the next message from the subsystem.
	pub async fn recv(&mut self) -> AllMessages {
		self.try_recv()
			.timeout(Self::TIMEOUT)
			.await
			.expect("`fn recv` does not timeout")
			.expect("Test subsystem no longer live")
	}

	/// Receive the next message from the subsystem, or `None` if the channel has been closed.
	pub async fn try_recv(&mut self) -> Option<AllMessages> {
		self.rx
			.next()
			.timeout(Self::TIMEOUT)
			.await
			.expect("`try_recv` does not timeout")
	}
}

/// Make a test subsystem context with `buffer_size == 0`. This is used by most
/// of the tests.
pub fn make_subsystem_context<M, S>(
	spawner: S,
) -> (TestSubsystemContext<M, SpawnGlue<S>>, TestSubsystemContextHandle<M>) {
	make_buffered_subsystem_context(spawner, 0)
}

/// Make a test subsystem context with buffered overseer channel. Some tests (e.g.
/// `dispute-coordinator`) create too many parallel operations and deadlock unless
/// the channel is buffered. Usually `buffer_size=1` is enough.
pub fn make_buffered_subsystem_context<M, S>(
	spawner: S,
	buffer_size: usize,
) -> (TestSubsystemContext<M, SpawnGlue<S>>, TestSubsystemContextHandle<M>) {
	let (overseer_tx, overseer_rx) = mpsc::channel(buffer_size);
	let (all_messages_tx, all_messages_rx) = mpsc::unbounded();

	(
		TestSubsystemContext {
			tx: TestSubsystemSender { tx: all_messages_tx },
			rx: overseer_rx,
			spawn: SpawnGlue(spawner),
		},
		TestSubsystemContextHandle { tx: overseer_tx, rx: all_messages_rx },
	)
}

/// Test a subsystem, mocking the overseer
///
/// Pass in two async closures: one mocks the overseer, the other runs the test from the perspective of a subsystem.
///
/// Times out in 5 seconds.
pub fn subsystem_test_harness<M, OverseerFactory, Overseer, TestFactory, Test>(
	overseer_factory: OverseerFactory,
	test_factory: TestFactory,
) where
	OverseerFactory: FnOnce(TestSubsystemContextHandle<M>) -> Overseer,
	Overseer: Future<Output = ()>,
	TestFactory: FnOnce(TestSubsystemContext<M, overseer::SpawnGlue<TaskExecutor>>) -> Test,
	Test: Future<Output = ()>,
{
	let pool = TaskExecutor::new();
	let (context, handle) = make_subsystem_context(pool);
	let overseer = overseer_factory(handle);
	let test = test_factory(context);

	futures::pin_mut!(overseer, test);

	futures::executor::block_on(async move {
		future::join(overseer, test)
			.timeout(Duration::from_secs(5))
			.await
			.expect("test timed out instead of completing")
	});
}

/// A forward subsystem that implements [`Subsystem`].
///
/// It forwards all communication from the overseer to the internal message
/// channel.
///
/// This subsystem is useful for testing functionality that interacts with the overseer.
pub struct ForwardSubsystem<M>(pub mpsc::Sender<M>);

impl<M, Context> overseer::Subsystem<Context, SubsystemError> for ForwardSubsystem<M>
where
	M: overseer::AssociateOutgoing + std::fmt::Debug + Send + 'static,
	Context: overseer::SubsystemContext<
		Message = M,
		Signal = OverseerSignal,
		Error = SubsystemError,
		OutgoingMessages = <M as overseer::AssociateOutgoing>::OutgoingMessages,
	>,
{
	fn start(mut self, mut ctx: Context) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			loop {
				match ctx.recv().await {
					Ok(FromOrchestra::Signal(OverseerSignal::Conclude)) => return Ok(()),
					Ok(FromOrchestra::Communication { msg }) => {
						let _ = self.0.send(msg).await;
					},
					Err(_) => return Ok(()),
					_ => (),
				}
			}
		});

		SpawnedSubsystem { name: "forward-subsystem", future }
	}
}

/// Asserts that two patterns match, yet only one
#[macro_export]
macro_rules! arbitrary_order {
	($rx:expr; $p1:pat => $e1:expr; $p2:pat => $e2:expr) => {
		// If i.e. a enum has only two variants, `_` is unreachable.
		match $rx {
			$p1 => {
				let __ret1 = { $e1 };
				let __ret2 = match $rx {
					$p2 => $e2,
					#[allow(unreachable_patterns)]
					_ => unreachable!("first pattern matched, second pattern did not"),
				};
				(__ret1, __ret2)
			},
			$p2 => {
				let __ret2 = { $e2 };
				let __ret1 = match $rx {
					$p1 => $e1,
					#[allow(unreachable_patterns)]
					_ => unreachable!("second pattern matched, first pattern did not"),
				};
				(__ret1, __ret2)
			},
			#[allow(unreachable_patterns)]
			_ => unreachable!("neither first nor second pattern matched"),
		}
	};
}

/// Future that yields the execution once and resolves
/// immediately after.
///
/// Useful when one wants to poll the background task to completion
/// before sending messages to it in order to avoid races.
pub struct Yield(bool);

impl Yield {
	/// Returns new `Yield` future.
	pub fn new() -> Self {
		Self(false)
	}
}

impl Future for Yield {
	type Output = ();

	fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		if !self.0 {
			self.0 = true;
			cx.waker().wake_by_ref();
			Poll::Pending
		} else {
			Poll::Ready(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor::block_on;
	use polkadot_node_subsystem::messages::CollatorProtocolMessage;
	use polkadot_overseer::{dummy::dummy_overseer_builder, Handle, HeadSupportsParachains};
	use polkadot_primitives::v2::Hash;
	use sp_core::traits::SpawnNamed;

	struct AlwaysSupportsParachains;

	#[async_trait::async_trait]
	impl HeadSupportsParachains for AlwaysSupportsParachains {
		async fn head_supports_parachains(&self, _head: &Hash) -> bool {
			true
		}
	}

	#[test]
	fn forward_subsystem_works() {
		let spawner = sp_core::testing::TaskExecutor::new();
		let (tx, rx) = mpsc::channel(2);
		let (overseer, handle) =
			dummy_overseer_builder(spawner.clone(), AlwaysSupportsParachains, None)
				.unwrap()
				.replace_collator_protocol(|_| ForwardSubsystem(tx))
				.leaves(vec![])
				.build()
				.unwrap();

		let mut handle = Handle::new(handle);

		spawner.spawn("overseer", None, overseer.run().then(|_| async { () }).boxed());

		block_on(handle.send_msg_anon(CollatorProtocolMessage::CollateOn(Default::default())));
		assert!(matches!(
			block_on(rx.into_future()).0.unwrap(),
			CollatorProtocolMessage::CollateOn(_)
		));
	}

	#[test]
	fn macro_arbitrary_order() {
		let mut vals = vec![Some(15_usize), None];
		let (first, second) = arbitrary_order!(vals.pop().unwrap(); Some(fx) => fx; None => 0);
		assert_eq!(first, 15_usize);
		assert_eq!(second, 0_usize);
	}

	#[test]
	fn macro_arbitrary_order_swapped() {
		let mut vals = vec![None, Some(11_usize)];
		let (first, second) = arbitrary_order!(vals.pop().unwrap(); Some(fx) => fx; None => 0);
		assert_eq!(first, 11_usize);
		assert_eq!(second, 0);
	}
}
