// Copyright 2021 Parity Technologies (UK) Ltd.
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
	messages::{AllMessages, AvailabilityStoreMessage},
	overseer::{gen::TimeoutExt, Subsystem},
	DummySubsystem,
};

#[derive(Clone, Debug)]
struct BlackHoleInterceptor;

impl<Sender> MessageInterceptor<Sender> for BlackHoleInterceptor
where
	Sender: overseer::SubsystemSender<AllMessages>
		+ overseer::SubsystemSender<AvailabilityStoreMessage>
		+ Clone
		+ 'static,
{
	type Message = AvailabilityStoreMessage;
	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		match msg {
			FromOverseer::Communication { msg: _msg } => None,
			// to conclude the test cleanly
			sig => Some(sig),
		}
	}
}

async fn overseer_send<T: Into<AllMessages>>(overseer: &mut TestSubsystemContextHandle<T>, msg: T) {
	overseer.send(FromOverseer::Communication { msg }).await;
}

#[test]
fn integrity_test() {
	let pool = sp_core::testing::TaskExecutor::new();
	let (context, mut overseer) = make_subsystem_context(pool);

	let sub = DummySubsystem;

	let sub_intercepted = InterceptedSubsystem::new(sub, BlackHoleInterceptor);

	// Try to send a message we know is going to be filtered.
	let test_fut = async move {
		let (tx, rx) = futures::channel::oneshot::channel();
		overseer_send(
			&mut overseer,
			AvailabilityStoreMessage::QueryChunk(Default::default(), 0.into(), tx),
		)
		.await;
		let _ = rx.timeout(std::time::Duration::from_millis(100)).await.unwrap();
		overseer
	};
	let subsystem = async move {
		sub_intercepted.start(context).future.await.unwrap();
	};

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);

	futures::executor::block_on(futures::future::join(
		async move {
			let mut overseer = test_fut.await;
			overseer.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		},
		subsystem,
	))
	.1;
}
