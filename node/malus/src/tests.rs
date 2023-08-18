// Copyright (C) Parity Technologies (UK) Ltd.
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

use super::*;

use polkadot_node_subsystem_test_helpers::*;

use polkadot_node_subsystem::{
	messages::AvailabilityStoreMessage,
	overseer::{dummy::DummySubsystem, gen::TimeoutExt, Subsystem, AssociateOutgoing},
	SubsystemError,
};

#[derive(Clone, Debug)]
struct BlackHoleInterceptor;

impl<Sender> MessageInterceptor<Sender> for BlackHoleInterceptor
where
	Sender: overseer::AvailabilityStoreSenderTrait
		+ Clone
		+ 'static,
{
	type Message = AvailabilityStoreMessage;
	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
		msg: FromOrchestra<Self::Message>,
	) -> Option<FromOrchestra<Self::Message>> {
		match msg {
			FromOrchestra::Communication { msg: _msg } => None,
			// to conclude the test cleanly
			sig => Some(sig),
		}
	}
}

#[derive(Clone, Debug)]
struct PassInterceptor;

impl<Sender> MessageInterceptor<Sender> for PassInterceptor
where
	Sender: overseer::AvailabilityStoreSenderTrait
		+ Clone
		+ 'static,
{
	type Message = AvailabilityStoreMessage;
}

async fn overseer_send<T: Into<AllMessages>>(overseer: &mut TestSubsystemContextHandle<T>, msg: T) {
	overseer.send(FromOrchestra::Communication { msg }).await;
}

use sp_core::testing::TaskExecutor;

fn launch_harness<F, M, Sub, G>(test_gen: G)
where
	F: Future<Output = TestSubsystemContextHandle<M>> + Send,
	M: AssociateOutgoing + std::fmt::Debug + Send + 'static,
	// <M as AssociateOutgoing>::OutgoingMessages: From<M>,
	Sub: Subsystem<TestSubsystemContext<M, SpawnGlue<TaskExecutor>>, SubsystemError>,
	G: Fn(TestSubsystemContextHandle<M>) -> (F, Sub),
{
	let pool = TaskExecutor::new();
	let (context, overseer) = make_subsystem_context(pool);

	let (test_fut, subsystem) = test_gen(overseer);
	let subsystem = async move {
		subsystem.start(context).future.await.unwrap();
	};
	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	futures::executor::block_on(futures::future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
		},
		subsystem,
	))
	.1;
}

#[test]
fn integrity_test_intercept() {
	launch_harness(|mut overseer| {
		let sub = DummySubsystem;

		let sub_intercepted = InterceptedSubsystem::new(sub, BlackHoleInterceptor);

		(
			async move {
				let (tx, rx) = futures::channel::oneshot::channel();
				overseer_send(
					&mut overseer,
					AvailabilityStoreMessage::QueryChunk(Default::default(), 0.into(), tx),
				)
				.await;
				let _ = rx.timeout(std::time::Duration::from_millis(100)).await.unwrap();
				overseer
			},
			sub_intercepted,
		)
	})
}

#[test]
fn integrity_test_pass() {
	launch_harness(|mut overseer| {
		let sub = DummySubsystem;

		let sub_intercepted = InterceptedSubsystem::new(sub, PassInterceptor);

		(
			async move {
				let (tx, rx) = futures::channel::oneshot::channel();
				overseer_send(
					&mut overseer,
					AvailabilityStoreMessage::QueryChunk(Default::default(), 0.into(), tx),
				)
				.await;
				let _ = rx.timeout(std::time::Duration::from_millis(100)).await.unwrap();
				overseer
			},
			sub_intercepted,
		)
	})
}
