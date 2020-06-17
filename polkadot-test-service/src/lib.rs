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
use sc_network::{
	config::{NetworkConfiguration, TransportConfig},
	multiaddr,
};
use service::{
	config::{DatabaseConfig, KeystoreConfig, WasmExecutionMethod},
	error::Error as ServiceError,
	TaskType,
};
use service::{AbstractService, Configuration, Role, TFullBackend};
use sp_state_machine::BasicExternalities;
use std::net::Ipv4Addr;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use sc_executor::native_executor_instance;

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

fn node_config<P: AsRef<Path>>(
	spec: &PolkadotChainSpec,
	role: Role,
	task_executor: Arc<
		dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync,
	>,
	key_seed: Option<String>,
	port: u16,
	root: P,
) -> Configuration {
	let root = root.as_ref().to_path_buf();
	let mut network_config = NetworkConfiguration::new(
		format!("Polkadot Test Node on {}", port),
		"network/test/0.1",
		Default::default(),
		None,
	);

	network_config.allow_non_globals_in_dht = true;

	network_config.listen_addresses.push(
		std::iter::once(multiaddr::Protocol::Ip4(Ipv4Addr::new(127, 0, 0, 1)))
			.chain(std::iter::once(multiaddr::Protocol::Tcp(port)))
			.collect(),
	);

	network_config.transport = TransportConfig::Normal {
		enable_mdns: true, // TODO: keep it that way or disable it?
		allow_private_ipv4: true,
		wasm_external_transport: None,
		use_yamux_flow_control: true,
	};

	Configuration {
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
		chain_spec: Box::new((*spec).clone()),
		wasm_method: WasmExecutionMethod::Interpreted,
		execution_strategies: Default::default(),
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
		dev_key_seed: key_seed,
		tracing_targets: None,
		tracing_receiver: Default::default(),
		max_runtime_instances: 8,
		announce_block: true,
		base_path: Some(root.into()),
	}
}

pub fn run_test_node(
	task_executor: Arc<
		dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync,
	>,
	port: u16,
	key: &str,
	storage_update_func: impl Fn(),
) -> Result<
	(
		impl AbstractService,
		Arc<impl PolkadotClient<Block, TFullBackend<Block>, polkadot_test_runtime::RuntimeApi>>,
		FullNodeHandles,
		tempfile::TempDir,
	),
	(),
> {
	let base_path = tempfile::Builder::new()
		.prefix("polkadot-test-service")
		.tempdir()
		.unwrap();
	let mut spec = polkadot_local_testnet_config();

	let mut storage = spec.as_storage_builder().build_storage().unwrap();
	BasicExternalities::execute_with_storage(&mut storage, storage_update_func);

	spec.set_storage(storage);

	let config = node_config(
		&spec,
		Role::Authority {
			sentry_nodes: Vec::new(),
		},
		task_executor,
		Some(format!("//{}", key)),
		port,
		&base_path,
	);
	let authority_discovery_enabled = false;
	let (service, client, handles) =
		polkadot_test_new_full(config, None, None, authority_discovery_enabled, 6000).unwrap();

	Ok((service, client, handles, base_path))
}
