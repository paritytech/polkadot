// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

//! A small set of wrapping types to cover most of our adversary test cases.
//!
//! This allows types with internal mutability to synchronize across
//! multiple subsystems and intercept or replace incoming and outgoing
//! messages on the overseer level.

use polkadot_node_subsystem::*;
pub use polkadot_node_subsystem::{messages::AllMessages, overseer, FromOverseer};
use std::{future::Future, pin::Pin};

/// Filter incoming and outgoing messages.
pub trait MessageInterceptor<Sender>: Send + Sync + Clone + 'static
where
	Sender: overseer::SubsystemSender<Self::Message> + Clone + 'static,
{
	/// The message type the original subsystem handles incoming.
	type Message: Send + 'static;

	/// Filter messages that are to be received by
	/// the subsystem.
	///
	/// For non-trivial cases, the `sender` can be used to send
	/// multiple messages after doing some additional processing.
	fn intercept_incoming(
		&self,
		_sender: &mut Sender,
		msg: FromOverseer<Self::Message>,
	) -> Option<FromOverseer<Self::Message>> {
		Some(msg)
	}

	/// Modify outgoing messages.
	fn intercept_outgoing(&self, msg: AllMessages) -> Option<AllMessages> {
		Some(msg)
	}
}

/// A sender with the outgoing messages filtered.
#[derive(Clone)]
pub struct InterceptedSender<Sender, Fil> {
	inner: Sender,
	message_filter: Fil,
}

#[async_trait::async_trait]
impl<Sender, Fil> overseer::SubsystemSender<AllMessages> for InterceptedSender<Sender, Fil>
where
	Sender: overseer::SubsystemSender<AllMessages>
		+ overseer::SubsystemSender<<Fil as MessageInterceptor<Sender>>::Message>,
	Fil: MessageInterceptor<Sender>,
{
	async fn send_message(&mut self, msg: AllMessages) {
		if let Some(msg) = self.message_filter.intercept_outgoing(msg) {
			self.inner.send_message(msg).await;
		}
	}

	async fn send_messages<T>(&mut self, msgs: T)
	where
		T: IntoIterator<Item = AllMessages> + Send,
		T::IntoIter: Send,
	{
		for msg in msgs {
			self.send_message(msg).await;
		}
	}

	fn send_unbounded_message(&mut self, msg: AllMessages) {
		if let Some(msg) = self.message_filter.intercept_outgoing(msg) {
			self.inner.send_unbounded_message(msg);
		}
	}
}

/// A subsystem context, that filters the outgoing messages.
pub struct InterceptedContext<Context, Fil>
where
	Context: overseer::SubsystemContext + SubsystemContext,
	Fil: MessageInterceptor<<Context as overseer::SubsystemContext>::Sender>,
	<Context as overseer::SubsystemContext>::Sender: overseer::SubsystemSender<
		<Fil as MessageInterceptor<<Context as overseer::SubsystemContext>::Sender>>::Message,
	>,
{
	inner: Context,
	message_filter: Fil,
	sender: InterceptedSender<<Context as overseer::SubsystemContext>::Sender, Fil>,
}

impl<Context, Fil> InterceptedContext<Context, Fil>
where
	Context: overseer::SubsystemContext + SubsystemContext,
	Fil: MessageInterceptor<
		<Context as overseer::SubsystemContext>::Sender,
		Message = <Context as overseer::SubsystemContext>::Message,
	>,
	<Context as overseer::SubsystemContext>::Sender: overseer::SubsystemSender<
		<Fil as MessageInterceptor<<Context as overseer::SubsystemContext>::Sender>>::Message,
	>,
{
	pub fn new(mut inner: Context, message_filter: Fil) -> Self {
		let sender = InterceptedSender::<<Context as overseer::SubsystemContext>::Sender, Fil> {
			inner: inner.sender().clone(),
			message_filter: message_filter.clone(),
		};
		Self { inner, message_filter, sender }
	}
}

#[async_trait::async_trait]
impl<Context, Fil> overseer::SubsystemContext for InterceptedContext<Context, Fil>
where
	Context: overseer::SubsystemContext + SubsystemContext,
	Fil: MessageInterceptor<
		<Context as overseer::SubsystemContext>::Sender,
		Message = <Context as overseer::SubsystemContext>::Message,
	>,
	<Context as overseer::SubsystemContext>::AllMessages:
		From<<Context as overseer::SubsystemContext>::Message>,
	<Context as overseer::SubsystemContext>::Sender: overseer::SubsystemSender<
		<Fil as MessageInterceptor<<Context as overseer::SubsystemContext>::Sender>>::Message,
	>,
{
	type Message = <Context as overseer::SubsystemContext>::Message;
	type Sender = InterceptedSender<<Context as overseer::SubsystemContext>::Sender, Fil>;
	type Error = <Context as overseer::SubsystemContext>::Error;
	type AllMessages = <Context as overseer::SubsystemContext>::AllMessages;
	type Signal = <Context as overseer::SubsystemContext>::Signal;

	async fn try_recv(&mut self) -> Result<Option<FromOverseer<Self::Message>>, ()> {
		loop {
			match self.inner.try_recv().await? {
				None => return Ok(None),
				Some(msg) =>
					if let Some(msg) =
						self.message_filter.intercept_incoming(self.inner.sender(), msg)
					{
						return Ok(Some(msg))
					},
			}
		}
	}

	async fn recv(&mut self) -> SubsystemResult<FromOverseer<Self::Message>> {
		loop {
			let msg = self.inner.recv().await?;
			if let Some(msg) = self.message_filter.intercept_incoming(self.inner.sender(), msg) {
				return Ok(msg)
			}
		}
	}

	fn spawn(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> SubsystemResult<()> {
		self.inner.spawn(name, s)
	}

	fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> SubsystemResult<()> {
		self.inner.spawn_blocking(name, s)
	}

	fn sender(&mut self) -> &mut Self::Sender {
		&mut self.sender
	}
}

/// A subsystem to which incoming and outgoing filters are applied.
pub struct InterceptedSubsystem<Sub, Interceptor> {
	pub subsystem: Sub,
	pub message_interceptor: Interceptor,
}

impl<Sub, Interceptor> InterceptedSubsystem<Sub, Interceptor> {
	pub fn new(subsystem: Sub, message_interceptor: Interceptor) -> Self {
		Self { subsystem, message_interceptor }
	}
}

impl<Context, Sub, Interceptor> overseer::Subsystem<Context, SubsystemError> for InterceptedSubsystem<Sub, Interceptor>
where
	Context: overseer::SubsystemContext + SubsystemContext + Sync + Send,
	Sub: overseer::Subsystem<InterceptedContext<Context, Interceptor>, SubsystemError>,
	InterceptedContext<Context, Interceptor>: overseer::SubsystemContext + SubsystemContext,
	Interceptor: MessageInterceptor<
		<Context as overseer::SubsystemContext>::Sender,
		Message = <Context as overseer::SubsystemContext>::Message,
	>,
	<Context as overseer::SubsystemContext>::Sender: overseer::SubsystemSender<
		<Interceptor as MessageInterceptor<<Context as overseer::SubsystemContext>::Sender>>::Message,
	>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let ctx = InterceptedContext::new(ctx, self.message_interceptor);
		overseer::Subsystem::<InterceptedContext<Context, Interceptor>, SubsystemError>::start(
			self.subsystem,
			ctx,
		)
	}
}
