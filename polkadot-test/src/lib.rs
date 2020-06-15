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

//! Polkadot service. Specialized wrapper over substrate service.

//pub mod chain_spec;
//mod grandpa_support;
//mod client;

use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::{parachain, Hash, BlockId, AccountId, Nonce, Balance};
#[cfg(feature = "full-node")]
use polkadot_network::{legacy::gossip::Known, protocol as network_protocol};
use service::{error::Error as ServiceError, ServiceBuilder};
use grandpa::{self, FinalityProofProvider as GrandpaFinalityProofProvider};
use sc_executor::native_executor_instance;
use log::info;
pub use service::{
	AbstractService, Role, PruningMode, TransactionPoolOptions, Error, RuntimeGenesis,
	TFullClient, TLightClient, TFullBackend, TLightBackend, TFullCallExecutor, TLightCallExecutor,
	Configuration, ChainSpec, ServiceBuilderCommand,
};
pub use service::config::{DatabaseConfig, PrometheusConfig};
pub use sc_executor::NativeExecutionDispatch;
pub use sc_client_api::{Backend, ExecutionStrategy, CallExecutor};
pub use sc_consensus::LongestChain;
pub use sp_api::{Core as CoreApi, ConstructRuntimeApi, ProvideRuntimeApi, StateBackend};
pub use sp_runtime::traits::{HashFor, NumberFor};
pub use consensus_common::{SelectChain, BlockImport, block_validation::Chain};
pub use polkadot_primitives::parachain::{CollatorId, ParachainHost};
pub use polkadot_primitives::Block;
pub use sp_runtime::traits::{Block as BlockT, self as runtime_traits, BlakeTwo256};
pub use polkadot_service::{PolkadotChainSpec, KusamaChainSpec, WestendChainSpec};
pub use consensus::run_validation_worker;
pub use codec::Codec;
pub use polkadot_runtime;
pub use kusama_runtime;
pub use westend_runtime;
pub use polkadot_test_runtime;
use prometheus_endpoint::Registry;
pub use polkadot_service::PolkadotClient;
pub use polkadot_service::{grandpa_support, new_full, new_full_start, FullNodeHandles, IdentifyVariant, PolkadotExecutor};

/// Create a new Polkadot test service for a full node.
pub fn polkadot_test_new_full(
	mut config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
	grandpa_pause: Option<(u32, u32)>,
	informant_prefix: Option<String>,
)
	-> Result<(
		impl AbstractService,
		Arc<impl PolkadotClient<
			Block,
			TFullBackend<Block>,
			polkadot_test_runtime::RuntimeApi
		>>,
		FullNodeHandles,
	), ServiceError>
{
	let (service, client, handles) = new_full!(
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		grandpa_pause,
		polkadot_test_runtime::RuntimeApi,
		PolkadotExecutor,
		informant_prefix,
	);

	Ok((service, client, handles))
}
