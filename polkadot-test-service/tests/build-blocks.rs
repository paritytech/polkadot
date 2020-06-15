use polkadot_test_service::*;
use polkadot_service::{PolkadotChainSpec, Configuration, Role, chain_spec::polkadot_local_testnet_config};
use service::{TaskType, BasePath, DatabaseConfig, config::{KeystoreConfig, WasmExecutionMethod}};
use sc_network::{config::{NetworkConfiguration, TransportConfig}, multiaddr};
use tempfile::{Builder, TempDir};
use std::pin::Pin;
use std::sync::Arc;
use futures::{FutureExt as _, select, pin_mut};
use std::iter;
use std::net::Ipv4Addr;
use std::time::Duration;

fn node_config(
	index: usize,
	spec: &PolkadotChainSpec,
	role: Role,
	task_executor: Arc<dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync>,
	key_seed: Option<String>,
	base_port: u16,
	root: &TempDir,
) -> Configuration
{
	let root = root.path().join(format!("node-{}", index));

	let mut network_config = NetworkConfiguration::new(
		format!("Node {}", index),
		"network/test/0.1",
		Default::default(),
		None,
	);

	network_config.allow_non_globals_in_dht = true;

	network_config.listen_addresses.push(
		iter::once(multiaddr::Protocol::Ip4(Ipv4Addr::new(127, 0, 0, 1)))
			.chain(iter::once(multiaddr::Protocol::Tcp(base_port + index as u16)))
			.collect()
	);

	network_config.transport = TransportConfig::Normal {
		enable_mdns: false,
		allow_private_ipv4: true,
		wasm_external_transport: None,
		use_yamux_flow_control: true,
	};

	Configuration {
		impl_name: "network-test-impl",
		impl_version: "0.1",
		role,
		task_executor,
		transaction_pool: Default::default(),
		network: network_config,
		keystore: KeystoreConfig::Path {
			path: root.join("key"),
			password: None
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
		base_path: Some(BasePath::new(root)),
	}
}

#[async_std::test]
async fn ensure_test_service_build_blocks() {
	let spec = polkadot_local_testnet_config();
	let temp = Builder::new().prefix("polkadot-test-service").tempdir().unwrap();
	let task_executor = {
		Arc::new(move |fut: Pin<Box<dyn futures::Future<Output = ()> + Send>>, _| { async_std::task::spawn(fut.unit_error()); })
	};
	let key = String::new();
	let base_port = 27015;
	let config = node_config(
		0,
		&spec,
		Role::Authority { sentry_nodes: Vec::new() },
		task_executor,
		Some(key),
		base_port,
		&temp,
	);
	let authority_discovery_enabled = false;
	let (service, _client, _handles) = polkadot_test_new_full(
		config,
		None,
		None,
		authority_discovery_enabled,
		6000,
	).unwrap();

	let t1 = service.fuse();
	let t2 = async_std::task::sleep(Duration::from_secs(10)).fuse();

	pin_mut!(t1, t2);

	select! {
		_ = t1 => {},
		_ = t2 => {},
	}

	assert!(false);
}
