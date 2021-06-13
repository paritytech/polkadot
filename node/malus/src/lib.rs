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


/// Filter incoming and outgoing messages.
trait MsgFilter: Send + Clone + 'static {

	type Message: Send + 'static;

	/// Filter messages that are to be received by
	/// the subsystem.
	fn filter_in(&self, msg: Self::Message) -> Option<Self::Message> {
		Some(msg)
	}

	/// Modify outgoing messages.
	fn filter_out(&self, msg: Self::Message) -> Option<Self::Message> {
		Some(msg)
	}
}

/// A sender with the outgoing messages filtered.
#[derive(Clone)]
struct FilteredSender<Sender, Fil> {
	inner: Sender,
	message_filter: Fil,
}

#[async_trait::async_trait]
impl SubsystemSender for FilteredSender<Sender> where Sender: SubsystemSender {
	async fn send_message(&mut self, msg: AllMessages) {
		if let Some(msg) = self.message_filter.filter_out(msg) {
			self.inner.send_message(msg);
		}
	}

	async fn send_messages<T>(&mut self, msgs: T)
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send {
		for msg in msgs {
			self.send_message(msg)
		}
	}

	fn send_unbounded_message(&mut self, msg: AllMessages) {
		if let Some(msg) = self.message_filter.filter_out(msg) {
			self.inner.send_unbounded_message(msg);
		}
	}

}

/// A subsystem context, that filters the outgoing messages.
struct FilteredContext<X, Fil>{
	inner: X,
	message_filter: Fil,
}

impl FilteredContext<X: SubsystemContext, Fil: MsgFilter> {
	pub fn new(inner: X, message_filter: Fil) {
		Self {
			inner,
			message_filter,
		}
	}
}

impl<X, Fil> SubsystemContext for FilteredContext<X, Fil>
where
X: SubsystemContext,
Fil: MsgFilter,
{
	type Message = <X as SubsystemContext>::Message;
	type Sender = FilteredSender<
		<X as SubsystemContext>::Sender,
		Fil,
	>;

	async fn try_recv(&mut self) -> Result<Option<FromOverseer<Self::Message>>, ()> {
		loop {
			match self.inner.try_recv().await? {
				None => return None,
				Some(msg) => if let Some(msg) = self.message_filter.filter_in(msg) {
					return Ok(Some(msg))
				}
			}
		}
	}

	async fn recv(&mut self) -> SubsystemResult<FromOverseer<Self::Message>> {
		loop {
			if let Some(msg) = self.message_filter.filter_in(self.inner.recv().await?) {
				return Ok(msg)
			}
		}
	}

	async fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>) -> SubsystemResult<()> {
		self.inner.spawn(name, s)
	}

	async fn spawn_blocking(
		&mut self,
		name: &'static str,
		s: Pin<Box<dyn Future<Output = ()> + Send>>,
	) -> SubsystemResult<()> {
		self.inner.spawn_blocking(name, s)
	}

	fn sender(&mut self) -> &mut Self::Sender {
		self.inner.sender()
	}
}

/// A subsystem to which incoming and outgoing filters are applied.
struct FilteredSubsystem<Sub, Fil> {
	pub subsystem: Sub,
	pub message_filter: Fil,
}

impl<Context, Sub, Fil> FilteredSubsystem<Sub, Fil>
where
	Sub: Subsystem<Context>,
	Context: SubsystemContext,
	Fil: MsgFilter<Message = <Context as SubsystemContext>::Message>,
{
	fn new(subsystem: Sub, message_filter: Fil) -> Self {
		Self {
			subsystem,
			message_filter,
		}
	}
}

impl<Context, Sub> Subsystem<Context> for FilteredSubsystem<Sub>
where
	Sub: Subsystem<Context>,
	Context: SubsystemContext + Sync + Send,
{
	fn start(self, ctx: Context) -> SpawendSubsystem {
		self.subsystem.start(ctx)
	}
}
