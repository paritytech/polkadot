// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use polkadot_test_service::new_full;
use polkadot_primitives::v1::{
	Id as ParaId, HeadData, ValidationCode, Balance, CollatorPair, CollatorId,
};
use polkadot_runtime_common::BlockHashCount;
use polkadot_service::{
	Error, NewFull, FullClient, ClientHandle, ExecuteWithClient, IsCollator,
};
use polkadot_node_subsystem::messages::{CollatorProtocolMessage, CollationGenerationMessage};
use polkadot_test_runtime::{
	Runtime, SignedExtra, SignedPayload, VERSION, ParasSudoWrapperCall, SudoCall, UncheckedExtrinsic,
};
use polkadot_node_primitives::{CollatorFn, CollationGenerationConfig};
use polkadot_runtime_parachains::paras::ParaGenesisArgs;
use sc_chain_spec::ChainSpec;
use sc_client_api::execution_extensions::ExecutionStrategies;
use sc_executor::native_executor_instance;
use sc_network::{
	config::{NetworkConfiguration, TransportConfig},
	multiaddr,
};
use service::{
	config::{DatabaseConfig, KeystoreConfig, MultiaddrWithPeerId, WasmExecutionMethod},
	RpcHandlers, TaskExecutor, TaskManager, KeepBlocks, TransactionStorageMode,
};
use service::{BasePath, Configuration, Role};
use sp_arithmetic::traits::SaturatedConversion;
use sp_blockchain::HeaderBackend;
use sp_keyring::Sr25519Keyring;
use sp_runtime::{codec::Encode, generic, traits::IdentifyAccount, MultiSigner};
use sp_state_machine::BasicExternalities;
use std::sync::Arc;
use substrate_test_client::{BlockchainEventsExt, RpcHandlersExt, RpcTransactionOutput, RpcTransactionError};
use polkadot_service::chain_spec::mousetrap_local_testnet_config;
use futures::{pin_mut, future::{self, Future}};
use polkadot_test_service::construct_extrinsic;
use polkadot_test_service::PolkadotTestExecutor;
use polkadot_overseer::OverseerHandler;
use futures::FutureExt;
// native_executor_instance!(
// 	pub MoustrapTestExecutor,
// 	polkadot_test_runtime::api::dispatch,
// 	polkadot_test_runtime::native_version,
// 	frame_benchmarking::benchmarking::HostFunctions,
// );

/// The client type being used by the test service.
pub type Client = FullClient<polkadot_test_runtime::RuntimeApi, PolkadotTestExecutor>;

pub use polkadot_service::FullBackend;


pub fn node_config(
	storage_update_func: impl Fn(),
	task_executor: TaskExecutor,
	key: Sr25519Keyring,
	boot_nodes: Vec<MultiaddrWithPeerId>,
	is_validator: bool,
) -> Configuration {
	let base_path = BasePath::new_temp_dir().expect("could not create temporary directory");
	let root = base_path.path();
	let role = if is_validator {
		Role::Authority
	} else {
		Role::Full
	};
	let key_seed = key.to_seed();
	let mut spec = mousetrap_local_testnet_config().unwrap();
	let mut storage = spec
		.as_storage_builder()
		.build_storage()
		.expect("could not build storage");

	BasicExternalities::execute_with_storage(&mut storage, storage_update_func);
	spec.set_storage(storage);

	let mut network_config = NetworkConfiguration::new(
		key_seed.to_string(),
		"network/test/0.1",
		Default::default(),
		None,
	);

	network_config.boot_nodes = boot_nodes;

	network_config.allow_non_globals_in_dht = true;

	let addr: multiaddr::Multiaddr = multiaddr::Protocol::Memory(rand::random()).into();
	network_config
		.listen_addresses
		.push(addr.clone());

	network_config
		.public_addresses
		.push(addr);

	network_config.transport = TransportConfig::MemoryOnly;

	Configuration {
		impl_name: "mousetrap-test-node".to_string(),
		impl_version: "0.1".to_string(),
		role,
		task_executor,
		transaction_pool: Default::default(),
		network: network_config,
		keystore: KeystoreConfig::InMemory,
		keystore_remote: Default::default(),
		database: DatabaseConfig::RocksDb {
			path: root.join("db"),
			cache_size: 128,
		},
		state_cache_size: 16777216,
		state_cache_child_ratio: None,
		state_pruning: Default::default(),
		keep_blocks: KeepBlocks::All,
		transaction_storage: TransactionStorageMode::BlockBody,
		chain_spec: Box::new(spec),
		wasm_method: WasmExecutionMethod::Interpreted,
		wasm_runtime_overrides: Default::default(),
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
		informant_output_format: Default::default(),
		disable_log_reloading: false,
	}
}

fn run_validator_node(
	task_executor: TaskExecutor,
	key: Sr25519Keyring,
	storage_update_func: impl Fn(),
	boot_nodes: Vec<MultiaddrWithPeerId>,
) -> MousetrapTestNode {
	let config = node_config(storage_update_func, task_executor, key, boot_nodes, true);
	let multiaddr = config.network.listen_addresses[0].clone();
	let NewFull { task_manager, client, network, rpc_handlers, overseer_handler, .. } =
		polkadot_service::new_full::<polkadot_test_runtime::RuntimeApi, PolkadotTestExecutor>(
			config,
			IsCollator::No,
			None,
			None,
			polkadot_parachain::wasm_executor::IsolationStrategy::InProcess,
			None,
		).expect("could not create Polkadot test service");

	let overseer_handler = overseer_handler.expect("test node must have an overseer handler");
	let peer_id = network.local_peer_id().clone();
	let addr = MultiaddrWithPeerId { multiaddr, peer_id };

	MousetrapTestNode {
		task_manager,
		client,
		overseer_handler,
		addr,
		rpc_handlers,
	}
}





/// Iterate over a hardcoded set of test keys.
///
/// Test helper.
pub(crate) struct KeysIter(Vec<Sr25519Keyring>);

impl KeysIter {
	pub(crate) fn new() -> Self {
		KeysIter(vec![
			// grp 1
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			// grp 2
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			// grp 3
			Sr25519Keyring::Eve,
			Sr25519Keyring::Ferdie,
		])
	}
}

impl Iterator for KeysIter {
	type Item = Sr25519Keyring;
	fn next(&mut self) -> Option<Self::Item> {
		self.0.pop()
	}
}

use test_parachain_adder_collator::Collator;






/// A Polkadot test node instance used for testing.
pub struct MousetrapTestNode {
	/// TaskManager's instance.
	pub task_manager: TaskManager,
	/// Client's instance.
	pub client: Arc<Client>,
	/// The overseer handler.
	pub overseer_handler: OverseerHandler,
	/// The `MultiaddrWithPeerId` to this node. This is useful if you want to pass it as "boot node" to other nodes.
	pub addr: MultiaddrWithPeerId,
	/// RPCHandlers to make RPC queries.
	pub rpc_handlers: RpcHandlers,
}

impl MousetrapTestNode {
	/// Send an extrinsic to this node.
	pub async fn send_extrinsic(
		&self,
		function: impl Into<polkadot_test_runtime::Call>,
		caller: Sr25519Keyring,
	) -> Result<RpcTransactionOutput, RpcTransactionError> {
		let extrinsic = construct_extrinsic(&*self.client, function, caller);

		self.rpc_handlers.send_transaction(extrinsic.into()).await
	}

	/// Register a parachain at this relay chain.
	pub async fn register_parachain(
		&self,
		id: ParaId,
		validation_code: impl Into<ValidationCode>,
		genesis_head: impl Into<HeadData>,
	) -> Result<(), RpcTransactionError> {
		let call = ParasSudoWrapperCall::sudo_schedule_para_initialize(
			id,
			ParaGenesisArgs {
				genesis_head: genesis_head.into(),
				validation_code: validation_code.into(),
				parachain: true,
			},
		);

		self.send_extrinsic(SudoCall::sudo(Box::new(call.into())), Sr25519Keyring::Alice).await.map(drop)
	}

	/// Wait for `count` blocks to be imported in the node and then exit. This function will not return if no blocks
	/// are ever created, thus you should restrict the maximum amount of time of the test execution.
	pub fn wait_for_blocks(&self, count: usize) -> impl Future<Output = ()> {
		self.client.wait_for_blocks(count)
	}

	/// Register the collator functionality in the overseer of this node.
	pub async fn register_collator(
		&mut self,
		collator_key: CollatorPair,
		para_id: ParaId,
		collator: CollatorFn,
	) {
		let config = CollationGenerationConfig {
			key: collator_key,
			collator,
			para_id,
		};

		self.overseer_handler
			.send_msg(CollationGenerationMessage::Initialize(config))
			.await;

		self.overseer_handler
			.send_msg(CollatorProtocolMessage::CollateOn(para_id))
			.await;
	}
}



#[substrate_test_utils::test(core_threads = 8)]
async fn multiple_validator_groups_work(task_executor: TaskExecutor) {
	let mut builder = sc_cli::LoggerBuilder::new("");
	builder.with_colors(false);
	builder.init().expect("Setting up logger always works. qed");


	// 1 collator
	let collator = Collator::new();

	// create nodes with all previous nodes as boot nodes
	let nodes = KeysIter::new()
		.scan(Vec::<MultiaddrWithPeerId>::new(), |boot_nodes, key: Sr25519Keyring| {
			let mut node = run_validator_node(
				task_executor.clone(),
				key,
				|| {},
				boot_nodes.clone(),
			);

			boot_nodes.push(node.addr.clone());

			Some(node)
		})
		.collect::<Vec<MousetrapTestNode>>();

	let genesis_head = collator.genesis_head();
	let validation_code = collator.validation_code().to_vec();
	{
		for node in nodes.iter() {
			node.register_parachain(0x7777_u32.into(),
			validation_code.clone(),
			genesis_head.clone()).await.unwrap();
		}

		let fut = future::join_all(nodes.iter().map(|node| {
			node.wait_for_blocks(40).fuse()
		}) ).fuse();

		pin_mut!(fut);

		fut.await;
	}

	for node in nodes {
		node.task_manager.clean_shutdown().await;
	}
	// collator.task_manager.clean_shutdown().await;
}
