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

//! Implements the Runtime API Subsystem
//!
//! This provides a clean, ownerless wrapper around the parachain-related runtime APIs. This crate
//! can also be used to cache responses from heavy runtime APIs.

use polkadot_subsystem::{
	Subsystem, SpawnedSubsystem, SubsystemResult, SubsystemContext,
	FromOverseer, OverseerSignal,
};
use polkadot_subsystem::messages::{
	RuntimeApiMessage, RuntimeApiRequest as Request, RuntimeApiError,
};
use polkadot_primitives::v1::{Block, BlockId, Hash, ParachainHost};

use sp_api::{ApiExt, ProvideRuntimeApi};

/// The `RuntimeApiSubsystem`. See module docs for more details.
pub struct RuntimeApiSubsystem<Client>(Client);

impl<Client> RuntimeApiSubsystem<Client> {
	/// Create a new Runtime API subsystem wrapping the given client.
	pub fn new(client: Client) -> Self {
		RuntimeApiSubsystem(client)
	}
}

async fn run<Client>(
	ctx: &mut impl SubsystemContext<Message = RuntimeApiMessage>,
	client: Client,
) -> SubsystemResult<()> where
	Client: ProvideRuntimeApi<Block>,
	Client::Api: ParachainHost<Block>,
{
	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(_)) => {},
			FromOverseer::Signal(OverseerSignal::BlockFinalized(_)) => {},
			FromOverseer::Communication { msg } => match msg {
				RuntimeApiMessage::Request(relay_parent, request) => make_runtime_api_request(
					&client,
					relay_parent,
					request,
				),
			}
		}
	}
}

fn make_runtime_api_request<Client>(
	client: &Client,
	relay_parent: Hash,
	request: Request,
) where
	Client: ProvideRuntimeApi<Block>,
	Client::Api: ParachainHost<Block>,
{
	macro_rules! query {
		($api_name:ident ($($param:expr),*), $sender:expr) => {{
			let sender = $sender;
			let api = client.runtime_api();
			let res = api.$api_name(&BlockId::Hash(relay_parent), $($param),*)
				.map_err(|e| RuntimeApiError(format!("{:?}", e)));

			let _ = sender.send(res);
		}}
	}

	match request {
		Request::Validators(sender) => query!(validators(), sender),
		Request::ValidatorGroups(sender) => query!(validator_groups(), sender),
		Request::AvailabilityCores(sender) => query!(availability_cores(), sender),
		Request::GlobalValidationData(sender) => query!(global_validation_data(), sender),
		Request::LocalValidationData(para, assumption, sender) =>
			query!(local_validation_data(para, assumption), sender),
		Request::SessionIndexForChild(sender) => query!(session_index_for_child(), sender),
		Request::ValidationCode(para, assumption, sender) =>
			query!(validation_code(para, assumption), sender),
		Request::CandidatePendingAvailability(para, sender) =>
			query!(candidate_pending_availability(para), sender),
		Request::CandidateEvents(sender) => query!(candidate_events(), sender),
	}
}
