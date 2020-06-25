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

use polkadot_subsystem::{SubsystemContext, FromOverseer, SubsystemResult, SubsystemError};
use polkadot_subsystem::messages::AllMessages;

use futures::prelude::*;
use futures::channel::mpsc;
use futures::task::{Spawn, SpawnExt};
use futures::poll;

use std::pin::Pin;
use std::task::Poll;

/// A test subsystem context.
pub struct TestSubsystemContext<M, S> {
	tx: mpsc::UnboundedSender<AllMessages>,
	rx: mpsc::UnboundedReceiver<FromOverseer<M>>,
	spawn: S,
}

#[async_trait::async_trait]
impl<M: Send + 'static, S: Spawn + Send + 'static> SubsystemContext for TestSubsystemContext<M, S> {
	type Message = M;

	async fn try_recv(&mut self) -> Result<Option<FromOverseer<M>>, ()> {
		match poll!(self.rx.next()) {
			Poll::Ready(Some(msg)) => Ok(Some(msg)),
			Poll::Ready(None) => Err(()),
			Poll::Pending => Ok(None),
		}
	}

	async fn recv(&mut self) -> SubsystemResult<FromOverseer<M>> {
		self.rx.next().await.ok_or(SubsystemError)
	}

	async fn spawn(&mut self, s: Pin<Box<dyn Future<Output = ()> + Send>>) -> SubsystemResult<()> {
		self.spawn.spawn(s).map_err(Into::into)
	}

	async fn send_message(&mut self, msg: AllMessages) -> SubsystemResult<()> {
		self.tx.unbounded_send(msg).expect("test overseer no longer live");
		Ok(())
	}

	async fn send_messages<T>(&mut self, msgs: T) -> SubsystemResult<()>
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send
	{
		for msg in msgs {
			self.tx.unbounded_send(msg).expect("test overseer no longer live");
		}

		Ok(())
	}
}

/// A handle for interacting with the subsystem context.
pub struct TestSubsystemContextHandle<M> {
	tx: mpsc::UnboundedSender<FromOverseer<M>>,
	rx: mpsc::UnboundedReceiver<AllMessages>,
}

impl<M> TestSubsystemContextHandle<M> {
	/// Send a message or signal to the subsystem.
	pub fn send(&self, from_overseer: FromOverseer<M>) {
		self.tx.unbounded_send(from_overseer).expect("Test subsystem no longer live");
	}

	/// Receive the next message from the subsystem.
	pub async fn recv(&mut self) -> AllMessages {
		self.rx.next().await.expect("Test subsystem no longer live")
	}
}

/// Make a test subsystem context.
pub fn make_subsystem_context<M, S>(spawn: S)
	-> (TestSubsystemContext<M, S>, TestSubsystemContextHandle<M>)
{
	let (overseer_tx, overseer_rx) = mpsc::unbounded();
	let (all_messages_tx, all_messages_rx) = mpsc::unbounded();

	(
		TestSubsystemContext {
			tx: all_messages_tx,
			rx: overseer_rx,
			spawn,
		},
		TestSubsystemContextHandle {
			tx: overseer_tx,
			rx: all_messages_rx
		},
	)
}
