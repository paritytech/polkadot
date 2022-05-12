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
	messages::AllMessages, overseer, FromOverseer, OverseerSignal, SpawnedSubsystem,
	SubsystemContext, SubsystemError, SubsystemResult,
};
use polkadot_node_subsystem_util::TimeoutExt;

use futures::{channel::mpsc, poll, prelude::*};
use parking_lot::Mutex;
use sp_core::{testing::TaskExecutor, traits::SpawnNamed};

use std::{
	convert::Infallible,
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
impl<T> overseer::SubsystemSender<T> for TestSubsystemSender
where
	T: Into<AllMessages> + Send + 'static,
{
	async fn send_message(&mut self, msg: T) {
		self.tx.send(msg.into()).await.expect("test overseer no longer live");
	}

	async fn send_messages<I>(&mut self, msgs: I)
	where
		I: IntoIterator<Item = T> + Send,
		I::IntoIter: Send,
	{
		let mut iter = stream::iter(msgs.into_iter().map(|msg| Ok(msg.into())));
		self.tx.send_all(&mut iter).await.expect("test overseer no longer live");
	}

	fn send_unbounded_message(&mut self, msg: T) {
		self.tx.unbounded_send(msg.into()).expect("test overseer no longer live");
	}
}

/// A test subsystem context.
pub struct TestSubsystemContext<M, S> {
	tx: TestSubsystemSender,
	rx: SingleItemStream<FromOverseer<M>>,
	spawn: S,
}

#[async_trait::async_trait]
impl<M, Spawner> overseer::SubsystemContext for TestSubsystemContext<M, Spawner>
where
	M: std::fmt::Debug + Send + 'static,
	AllMessages: From<M>,
	Spawner: SpawnNamed + Send + 'static,
{
	type Message = M;
	type Sender = TestSubsystemSender;
	type Signal = OverseerSignal;
	type OutgoingMessages = AllMessages;
	type Error = SubsystemError;

	async fn try_recv(&mut self) -> Result<Option<FromOverseer<M>>, ()> {
		match poll!(self.rx.next()) {
			Poll::Ready(Some(msg)) => Ok(Some(msg)),
			Poll::Ready(None) => Err(()),
			Poll::Pending => Ok(None),
		}
	}

	async fn recv(&mut self) -> SubsystemResult<FromOverseer<M>> {
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
	pub tx: SingleItemSink<FromOverseer<M>>,

	/// Direct access to the receiver.
	pub rx: mpsc::UnboundedReceiver<AllMessages>,
}

impl<M> TestSubsystemContextHandle<M> {
	/// Send a message or signal to the subsystem. This resolves at the point in time when the
	/// subsystem has _read_ the message.
	pub async fn send(&mut self, from_overseer: FromOverseer<M>) {
		self.tx.send(from_overseer).await.expect("Test subsystem no longer live");
	}

	/// Receive the next message from the subsystem.
	pub async fn recv(&mut self) -> AllMessages {
		self.try_recv().await.expect("Test subsystem no longer live")
	}

	/// Receive the next message from the subsystem, or `None` if the channel has been closed.
	pub async fn try_recv(&mut self) -> Option<AllMessages> {
		self.rx.next().await
	}
}

/// Make a test subsystem context.
pub fn make_subsystem_context<M, S>(
	spawn: S,
) -> (TestSubsystemContext<M, S>, TestSubsystemContextHandle<M>) {
	let (overseer_tx, overseer_rx) = single_item_sink();
	let (all_messages_tx, all_messages_rx) = mpsc::unbounded();

	(
		TestSubsystemContext {
			tx: TestSubsystemSender { tx: all_messages_tx },
			rx: overseer_rx,
			spawn,
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
	TestFactory: FnOnce(TestSubsystemContext<M, TaskExecutor>) -> Test,
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
	M: std::fmt::Debug + Send + 'static,
	Context:
		overseer::SubsystemContext<Message = M, Signal = OverseerSignal, Error = SubsystemError>,
	<Context as overseer::SubsystemContext>::OutgoingMessages: From<M> + Send,
{
	fn start(mut self, mut ctx: Context) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			loop {
				match ctx.recv().await {
					Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => return Ok(()),
					Ok(FromOverseer::Communication { msg }) => {
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

#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor::block_on;
	use polkadot_node_subsystem::messages::CollatorProtocolMessage;
	use polkadot_overseer::{dummy::dummy_overseer_builder, Handle, HeadSupportsParachains};
	use polkadot_primitives::v2::Hash;

	struct AlwaysSupportsParachains;
	impl HeadSupportsParachains for AlwaysSupportsParachains {
		fn head_supports_parachains(&self, _head: &Hash) -> bool {
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
