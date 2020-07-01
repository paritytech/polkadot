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

#![warn(missing_docs)]

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
use polkadot_service::{
	new_full, new_full_start, BlockT, FullNodeHandles, PolkadotClient, ServiceComponents,
};
use sc_chain_spec::ChainSpec;
use sc_client_api::{execution_extensions::ExecutionStrategies, BlockchainEvents};
use sc_executor::native_executor_instance;
use sc_informant::OutputFormat;
use sc_network::{
	config::{NetworkConfiguration, TransportConfig},
	multiaddr, NetworkService,
};
use service::{
	config::{DatabaseConfig, KeystoreConfig, MultiaddrWithPeerId, WasmExecutionMethod},
	error::Error as ServiceError,
	RpcHandlers, RpcSession, TaskExecutor, TaskManager,
};
use service::{BasePath, Configuration, Role, TFullBackend};
use sp_keyring::Sr25519Keyring;
use sp_runtime::{codec::Encode, OpaqueExtrinsic};
use sp_state_machine::BasicExternalities;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

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
		TaskManager,
		Arc<impl PolkadotClient<Block, TFullBackend<Block>, polkadot_test_runtime::RuntimeApi>>,
		FullNodeHandles,
		Arc<NetworkService<Block, Hash>>,
		Arc<RpcHandlers>,
	),
	ServiceError,
> {
	let (task_manager, client, handles, network, rpc_handlers) = new_full!(test
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		polkadot_test_runtime::RuntimeApi,
		PolkadotTestExecutor,
	);

	Ok((task_manager, client, handles, network, rpc_handlers))
}

/// Create a Polkadot `Configuration`. By default an in-memory socket will be used, therefore you need to provide boot
/// nodes if you want the future node to be connected to other nodes. The `storage_update_func` can be used to make
/// adjustements to the runtime before the node starts.
pub fn node_config(
	storage_update_func: impl Fn(),
	task_executor: TaskExecutor,
	key: Sr25519Keyring,
	boot_nodes: Vec<MultiaddrWithPeerId>,
) -> Result<Configuration, ServiceError> {
	let base_path = BasePath::new_temp_dir()?;
	let root = base_path.path();
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
	let informant_output_format = OutputFormat {
		enable_color: false,
		prefix: format!("[{}] ", key_seed),
	};

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
		// NOTE: we enforce the use of the native runtime to make the errors more debuggable
		execution_strategies: ExecutionStrategies {
			syncing: sc_client_api::ExecutionStrategy::NativeWhenPossible,
			importing: sc_client_api::ExecutionStrategy::NativeWhenPossible,
			block_construction: sc_client_api::ExecutionStrategy::NativeWhenPossible,
			offchain_worker: sc_client_api::ExecutionStrategy::NativeWhenPossible,
			other: sc_client_api::ExecutionStrategy::NativeWhenPossible,
		},
		rpc_http: None,
		rpc_ws: None,
		rpc_ipc: None,
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
		base_path: Some(base_path),
		informant_output_format,
	})
}

/// Run a Polkadot test node using the Polkadot test runtime. The node will be using an in-memory socket, therefore you
/// need to provide boot nodes if you want it to be connected to other nodes. The `storage_update_func` can be used to
/// make adjustements to the runtime before the node starts.
pub fn run_test_node(
	task_executor: TaskExecutor,
	key: Sr25519Keyring,
	storage_update_func: impl Fn(),
	boot_nodes: Vec<MultiaddrWithPeerId>,
) -> Result<
	PolkadotTestNode<
		TaskManager,
		impl PolkadotClient<Block, TFullBackend<Block>, polkadot_test_runtime::RuntimeApi>,
	>,
	ServiceError,
> {
	let config = node_config(storage_update_func, task_executor, key, boot_nodes)?;
	let multiaddr = config.network.listen_addresses[0].clone();
	let authority_discovery_enabled = false;
	let (task_manager, client, handles, network, rpc_handlers) =
		polkadot_test_new_full(config, None, None, authority_discovery_enabled, 6000)?;

	let peer_id = network.local_peer_id().clone();
	let multiaddr_with_peer_id = MultiaddrWithPeerId { multiaddr, peer_id };

	let node = PolkadotTestNode {
		task_manager,
		client,
		handles,
		multiaddr_with_peer_id,
		rpc_handlers,
	};

	Ok(node)
}

/// A Polkadot test node instance used for testing.
pub struct PolkadotTestNode<S, C> {
	/// TaskManager's instance.
	pub task_manager: S,
	/// Client's instance.
	pub client: Arc<C>,
	/// Node's handles.
	pub handles: FullNodeHandles,
	/// The `MultiaddrWithPeerId` to this node. This is useful if you want to pass it as "boot node" to other nodes.
	pub multiaddr_with_peer_id: MultiaddrWithPeerId,
	/// RPCHandlers to make RPC queries.
	pub rpc_handlers: Arc<RpcHandlers>,
}

impl<S, C> PolkadotTestNode<S, C>
where
	C: BlockchainEvents<Block>,
{
	/// Wait for `count` blocks to be imported in the node and then exit. This function will not return if no blocks
	/// are ever created, thus you should restrict the maximum amount of time of the test execution.
	pub fn wait_for_blocks(&self, count: usize) -> impl Future<Output = ()> {
		assert_ne!(count, 0, "'count' argument must be greater than 1");
		let client = self.client.clone();

		wait_for_blocks(client.clone(), count)
	}

	/// Send a transaction through the RPCHandlers.
	pub fn send_transaction(
		&self,
		extrinsic: OpaqueExtrinsic,
	) -> impl Future<
		Output = (
			Option<String>,
			RpcSession,
			futures01::sync::mpsc::Receiver<String>,
		),
	> {
		let rpc_handlers = self.rpc_handlers.clone();

		send_transaction(rpc_handlers, extrinsic)
	}
}

/// Wait for `count` blocks to be imported in the node and then exit. This function will not return if no blocks
/// are ever created, thus you should restrict the maximum amount of time of the test execution.
pub async fn wait_for_blocks<C, B>(client: Arc<C>, count: usize)
where
	C: BlockchainEvents<B>,
	B: BlockT,
{
	assert_ne!(count, 0, "'count' argument must be greater than 1");

	let mut import_notification_stream = client.import_notification_stream();
	let mut blocks = HashSet::new();

	while let Some(notification) = import_notification_stream.next().await {
		blocks.insert(notification.hash);
		if blocks.len() == count {
			break;
		}
	}
}

/// Send a transaction through the RPCHandlers.
pub async fn send_transaction(
	rpc_handlers: Arc<RpcHandlers>,
	extrinsic: OpaqueExtrinsic,
) -> (
	Option<String>,
	RpcSession,
	futures01::sync::mpsc::Receiver<String>,
) {
	let (tx, rx) = futures01::sync::mpsc::channel(0);
	let mem = RpcSession::new(tx.into());
	let res = rpc_handlers
		.rpc_query(
			&mem,
			format!(
				r#"{{
					"jsonrpc": "2.0",
					"method": "author_submitExtrinsic",
					"params": ["0x{}"],
					"id": 0
				}}"#,
				hex::encode(extrinsic.encode())
			)
			.as_str(),
		)
		.await;

	(res, mem, rx)
}
