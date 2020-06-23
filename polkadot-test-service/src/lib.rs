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

//! Polkadot test service only.

mod chain_spec;

pub use chain_spec::*;
use consensus_common::{block_validation::Chain, SelectChain};
use futures::{future::Future, StreamExt};
use grandpa::FinalityProofProvider as GrandpaFinalityProofProvider;
use log::info;
use polkadot_network::{legacy::gossip::Known, protocol as network_protocol};
use polkadot_primitives::parachain::CollatorId;
use polkadot_primitives::Block;
use polkadot_primitives::{
	parachain::{self},
	BlockId, Hash,
};
use polkadot_service::PolkadotClient;
use polkadot_service::{new_full, new_full_start, FullNodeHandles};
use sc_chain_spec::ChainSpec;
use sc_client_api::BlockchainEvents;
use sc_executor::native_executor_instance;
use sc_network::{
	config::{NetworkConfiguration, TransportConfig},
	multiaddr,
};
use service::{
	config::{DatabaseConfig, KeystoreConfig, MultiaddrWithPeerId, WasmExecutionMethod},
	error::Error as ServiceError,
	TaskType,
};
use service::{AbstractService, Configuration, Role, TFullBackend};
use sp_keyring::Sr25519Keyring;
use sp_state_machine::BasicExternalities;
use std::collections::HashSet;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use sc_client_api::execution_extensions::ExecutionStrategies;

native_executor_instance!(
	pub PolkadotTestExecutor,
	polkadot_test_runtime::api::dispatch,
	polkadot_test_runtime::native_version,
	frame_benchmarking::benchmarking::HostFunctions,
);

/// Create a new Polkadot test service for a full node.
pub fn polkadot_test_new_full(
	config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
) -> Result<
	(
		impl AbstractService,
		Arc<impl PolkadotClient<Block, TFullBackend<Block>, polkadot_test_runtime::RuntimeApi>>,
		FullNodeHandles,
	),
	ServiceError,
> {
	let (service, client, handles) = new_full!(test
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		polkadot_test_runtime::RuntimeApi,
		PolkadotTestExecutor,
	);

	Ok((service, client, handles))
}

pub fn node_config<P: AsRef<Path>>(
	storage_update_func: impl Fn(),
	task_executor: Arc<
		dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync,
	>,
	key: Sr25519Keyring,
	root: P,
	boot_nodes: Vec<MultiaddrWithPeerId>,
) -> Result<Configuration, ServiceError> {
	let root = root.as_ref().to_path_buf();
	let role = Role::Authority {
		sentry_nodes: Vec::new(),
	};
	let key_seed = key.to_seed();
	let mut spec = polkadot_local_testnet_config();
	let mut storage = spec.as_storage_builder().build_storage()?;

	BasicExternalities::execute_with_storage(&mut storage, storage_update_func);
	spec.set_storage(storage);

	let mut network_config = NetworkConfiguration::new(
		format!("Polkadot Test Node for: {}", key_seed),
		"network/test/0.1",
		Default::default(),
		None,
	);

	network_config.boot_nodes = boot_nodes;

	network_config.allow_non_globals_in_dht = true;

	network_config
		.listen_addresses
		.push(multiaddr::Protocol::Memory(rand::random()).into());

	network_config.transport = TransportConfig::MemoryOnly;

	Ok(Configuration {
		impl_name: "polkadot-test-node",
		impl_version: "0.1",
		role,
		task_executor,
		transaction_pool: Default::default(),
		network: network_config,
		keystore: KeystoreConfig::Path {
			path: root.join("key"),
			password: None,
		},
		database: DatabaseConfig::RocksDb {
			path: root.join("db"),
			cache_size: 128,
		},
		state_cache_size: 16777216,
		state_cache_child_ratio: None,
		pruning: Default::default(),
		chain_spec: Box::new(spec),
		wasm_method: WasmExecutionMethod::Interpreted,
		execution_strategies: ExecutionStrategies {
			syncing: sc_client_api::ExecutionStrategy::NativeWhenPossible,
			importing: sc_client_api::ExecutionStrategy::NativeWhenPossible,
			block_construction: sc_client_api::ExecutionStrategy::NativeWhenPossible,
			offchain_worker: sc_client_api::ExecutionStrategy::NativeWhenPossible,
			other: sc_client_api::ExecutionStrategy::NativeWhenPossible,
		},
		rpc_http: None,
		rpc_ws: None,
		rpc_ws_max_connections: None,
		rpc_cors: None,
		rpc_methods: Default::default(),
		prometheus_config: None,
		telemetry_endpoints: None,
		telemetry_external_transport: None,
		default_heap_pages: None,
		offchain_worker: Default::default(),
		force_authoring: false,
		disable_grandpa: false,
		dev_key_seed: Some(key_seed),
		tracing_targets: None,
		tracing_receiver: Default::default(),
		max_runtime_instances: 8,
		announce_block: true,
		base_path: Some(root.into()),
	})
}

pub fn run_test_node(
	task_executor: Arc<
		dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync,
	>,
	key: Sr25519Keyring,
	storage_update_func: impl Fn(),
	boot_nodes: Vec<MultiaddrWithPeerId>,
) -> Result<
	PolkadotTestNode<
		impl AbstractService,
		impl PolkadotClient<Block, TFullBackend<Block>, polkadot_test_runtime::RuntimeApi>,
	>,
	ServiceError,
> {
	let base_path = tempfile::Builder::new()
		.prefix("polkadot-test-service")
		.tempdir()?;

	let config = node_config(
		storage_update_func,
		task_executor,
		key,
		&base_path,
		boot_nodes,
	)?;
	let multiaddr = config.network.listen_addresses[0].clone();
	let authority_discovery_enabled = false;
	let (service, client, handles) =
		polkadot_test_new_full(config, None, None, authority_discovery_enabled, 6000)?;

	let peer_id = service.network().local_peer_id().clone();
	let multiaddr_with_peer_id = MultiaddrWithPeerId { multiaddr, peer_id };

	let node = PolkadotTestNode {
		service,
		client,
		handles,
		multiaddr_with_peer_id,
		base_path,
	};

	Ok(node)
}

pub struct PolkadotTestNode<S, C> {
	pub service: S,
	pub client: Arc<C>,
	pub handles: FullNodeHandles,
	pub multiaddr_with_peer_id: MultiaddrWithPeerId,
	pub base_path: tempfile::TempDir,
}

impl<S, C> PolkadotTestNode<S, C>
where
	C: BlockchainEvents<Block>,
{
	pub fn wait_for_blocks(&self, count: usize) -> impl Future<Output = ()> {
		assert_ne!(count, 0, "'count' argument must be greater than 1");
		let client = self.client.clone();

		async move {
			let mut import_notification_stream = client.import_notification_stream();
			let mut blocks = HashSet::new();

			while let Some(notification) = import_notification_stream.next().await {
				blocks.insert(notification.hash);
				if blocks.len() == 3 {
					break;
				}
			}
		}
	}
}

/// Can be called for a `Configuration` to check if it is a configuration for the `Test` network.
pub trait IdentifyVariant {
	/// Returns if this is a configuration for the `Test` network.
	fn is_test(&self) -> bool;
}

impl IdentifyVariant for Box<dyn ChainSpec> {
	fn is_test(&self) -> bool {
		self.id().starts_with("test") || self.id().starts_with("tst") // TODO not sure why I need a 3 chars variant
	}
}
