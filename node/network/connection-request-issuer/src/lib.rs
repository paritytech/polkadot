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

//! This subsystem is responsible for keeping track of session changes
//! and issuing a connection request to the validators relevant to
//! the gossiping subsystems on every new session.

use futures::FutureExt as _;
use polkadot_node_subsystem::{
	messages::{
		ConnectionRequestIssuerMessage,
	},
	ActiveLeavesUpdate, FromOverseer, OverseerSignal,
	Subsystem, SpawnedSubsystem, SubsystemContext,
};
use polkadot_node_subsystem_util::{
	validator_discovery::{ConnectionRequest, self},
	self as util,
};
use polkadot_primitives::v1::{
	Hash, ValidatorId, SessionIndex,
};
use polkadot_node_network_protocol::peer_set::PeerSet;

const LOG_TARGET: &str = "connection_request_issuer";

/// The Connection Request Issuer subsystem.
pub struct ConnectionRequestIssuer {}

#[derive(Default)]
struct State {
	last_session_index: Option<SessionIndex>,
	/// when we overwrite this, it automatically drops the previous request
	last_connection_request: Option<ConnectionRequest>,
}

impl ConnectionRequestIssuer {
	/// Create a new instance of the [`ConnectionRequestIssuer`] subsystem.
	pub fn new() -> Self {
		Self {}
	}

	#[tracing::instrument(skip(self, ctx), fields(subsystem = LOG_TARGET))]
	async fn run<Context>(self, ctx: Context)
	where
		Context: SubsystemContext<Message = ConnectionRequestIssuerMessage>,
	{
		let mut state = State::default();
		self.run_inner(ctx, &mut state).await
	}

	#[tracing::instrument(skip(self, ctx, state), fields(subsystem = LOG_TARGET))]
	async fn run_inner<Context>(self, mut ctx: Context, state: &mut State)
	where
		Context: SubsystemContext<Message = ConnectionRequestIssuerMessage>,
	{
		loop {
			let message = match ctx.recv().await {
				Ok(message) => message,
				Err(e) => {
					tracing::debug!(
						target: LOG_TARGET,
						err = ?e,
						"Failed to receive a message from Overseer, exiting"
					);
					return;
				},
			};
			match message {
				FromOverseer::Communication {
					msg: _,
				} => {}
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated,
					..
				})) => {
					tracing::trace!(target: LOG_TARGET, "active leaves signal");

					let leaves = activated.into_iter().map(|(h, _)| h);
					if let Err(e) = state.handle_active_leaves(&mut ctx, leaves).await {
						tracing::debug!(target: LOG_TARGET, "Error {}", e);
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, _number)) => {},
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					return;
				}
			}
		}
	}
}

async fn determine_relevant_validators(
	ctx: &mut impl SubsystemContext,
	relay_parent: Hash,
	_session: SessionIndex,
) -> Result<Vec<ValidatorId>, util::Error> {
	let validators = util::request_validators_ctx(relay_parent, ctx).await?.await??;
	Ok(validators)
}

impl State {
	/// 1. Determine if the current session index has changed.
	/// 2. If it has, determine relevant validators
	///    and issue a connection request.
	async fn handle_active_leaves(
		&mut self,
		ctx: &mut impl SubsystemContext,
		leaves: impl Iterator<Item = Hash>,
	) -> Result<(), util::Error> {
		for leaf in leaves {
			let current_index = util::request_session_index_for_child_ctx(leaf, ctx).await?.await??;
			let maybe_new_session = match self.last_session_index {
				Some(i) if i <= current_index => None,
				_ => Some((current_index, leaf)),
			};

			if let Some((new_session, relay_parent)) = maybe_new_session {
				tracing::debug!(target: LOG_TARGET, "New session detected {}", new_session);
				let validators = determine_relevant_validators(ctx, relay_parent, new_session).await?;
				tracing::debug!(target: LOG_TARGET, "Issuing a connection request to {:?}", validators);

				let request = validator_discovery::connect_to_validators_in_session(
					ctx,
					relay_parent,
					validators,
					PeerSet::Validation,
					new_session,
				).await?;

				self.last_session_index = Some(new_session);
				self.last_connection_request = Some(request);
			}
		}

		Ok(())
	}
}

impl<C> Subsystem<C> for ConnectionRequestIssuer
where
	C: SubsystemContext<Message = ConnectionRequestIssuerMessage> + Sync + Send,
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = self.run(ctx)
			.map(|_| Ok(()))
			.boxed();

		SpawnedSubsystem {
			name: "connection-request-issuer-subsystem",
			future,
		}
	}
}
