// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! Collator for the `Undying` test parachain.
//! Relay chain interface module

use async_trait::async_trait;
use polkadot_client::{ClientHandle, ExecuteWithClient};
use polkadot_primitives::{
	runtime_api::ParachainHost,
	v2::{Block, BlockId, Hash, InboundDownwardMessage, InboundHrmpMessage},
};
use polkadot_service::{AuxStore, ParaId};
use sp_api::{ApiError, ProvideRuntimeApi};

use std::{collections::BTreeMap, sync::Arc};

#[async_trait]
pub trait RelayChainInterface: Send + Sync {
	/// Returns the whole contents of the downward message queue for the parachain we are collating
	/// for.
	///
	/// Returns `None` in case of an error.
	async fn retrieve_dmq_contents(
		&self,
		para_id: ParaId,
		relay_parent: Hash,
	) -> Result<Vec<InboundDownwardMessage>, ApiError>;

	/// Returns channels contents for each inbound HRMP channel addressed to the parachain we are
	/// collating for.
	///
	/// Empty channels are also included.
	async fn retrieve_all_inbound_hrmp_channel_contents(
		&self,
		para_id: ParaId,
		relay_parent: Hash,
	) -> Result<BTreeMap<ParaId, Vec<InboundHrmpMessage>>, ApiError>;
}

pub struct RelayChainInProcessInterface<Client> {
	full_client: Arc<Client>,
}

impl<Client: Send + Sync> RelayChainInProcessInterface<Client> {
	/// Create a new instance of [`RelayChainInProcessInterface`]
	pub fn new(full_client: Arc<Client>) -> Self {
		Self { full_client }
	}
}

#[async_trait]
impl<Client> RelayChainInterface for RelayChainInProcessInterface<Client>
where
	Client: ProvideRuntimeApi<Block> + Sync + Send + 'static,
	Client::Api: ParachainHost<Block>,
{
	async fn retrieve_dmq_contents(
		&self,
		para_id: ParaId,
		relay_parent: Hash,
	) -> Result<Vec<InboundDownwardMessage>, ApiError> {
		self.full_client
			.runtime_api()
			.dmq_contents(&BlockId::Hash(relay_parent), para_id)
	}

	async fn retrieve_all_inbound_hrmp_channel_contents(
		&self,
		para_id: ParaId,
		relay_parent: Hash,
	) -> Result<BTreeMap<ParaId, Vec<InboundHrmpMessage>>, ApiError> {
		self.full_client
			.runtime_api()
			.inbound_hrmp_channels_contents(&BlockId::Hash(relay_parent), para_id)
	}
}

pub struct RelayChainInterfaceBuilder {
	pub polkadot_client: polkadot_client::Client,
}

impl RelayChainInterfaceBuilder {
	pub fn build(self) -> Arc<dyn RelayChainInterface> {
		self.polkadot_client.clone().execute_with(self)
	}
}

impl ExecuteWithClient for RelayChainInterfaceBuilder {
	type Output = Arc<dyn RelayChainInterface>;

	fn execute_with_client<Client, Api, Backend>(self, client: Arc<Client>) -> Self::Output
	where
		Client: ProvideRuntimeApi<Block> + AuxStore + 'static + Sync + Send,
		Client::Api: ParachainHost<Block>,
	{
		Arc::new(RelayChainInProcessInterface::new(client))
	}
}
