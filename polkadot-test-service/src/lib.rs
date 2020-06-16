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

use std::sync::Arc;
use std::time::Duration;
use polkadot_primitives::{parachain::{self, ValidatorId}, Hash, BlockId, AccountId};
use polkadot_network::{legacy::gossip::Known, protocol as network_protocol};
use service::{error::Error as ServiceError, TaskType, BasePath, config::{KeystoreConfig, DatabaseConfig,
WasmExecutionMethod}};
use grandpa::{FinalityProofProvider as GrandpaFinalityProofProvider};
use log::info;
use service::{AbstractService, Role, TFullBackend, Configuration};
use sc_network::{config::{NetworkConfiguration, TransportConfig}, multiaddr};
use consensus_common::{SelectChain, block_validation::Chain};
use polkadot_primitives::parachain::{CollatorId};
use polkadot_primitives::Block;
use polkadot_service::PolkadotClient;
use polkadot_service::{new_full, new_full_start, FullNodeHandles, PolkadotExecutor,
chain_spec::{get_account_id_from_seed, get_from_seed, Extensions}};
use std::path::PathBuf;
use std::pin::Pin;
use std::net::Ipv4Addr;
use babe_primitives::AuthorityId as BabeId;
use grandpa::AuthorityId as GrandpaId;
use polkadot_test_runtime::constants::currency::DOTS;
use sp_core::sr25519;
use sc_chain_spec::{ChainType, ChainSpec};
use sp_runtime::{Perbill};
use pallet_staking::Forcing;
use sp_state_machine::BasicExternalities;

const DEFAULT_PROTOCOL_ID: &str = "dot";

/// The `ChainSpec parametrised for polkadot runtime`.
pub type PolkadotChainSpec = service::GenericChainSpec<
	polkadot_test_runtime::GenesisConfig,
	Extensions,
>;

/// Create a new Polkadot test service for a full node.
pub fn polkadot_test_new_full(
	config: Configuration,
	collating_for: Option<(CollatorId, parachain::Id)>,
	max_block_data_size: Option<u64>,
	authority_discovery_enabled: bool,
	slot_duration: u64,
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
	let (service, client, handles) = new_full!(test
		config,
		collating_for,
		max_block_data_size,
		authority_discovery_enabled,
		slot_duration,
		polkadot_test_runtime::RuntimeApi,
		PolkadotExecutor,
	);

	Ok((service, client, handles))
}

fn node_config(
	index: usize,
	spec: &PolkadotChainSpec,
	role: Role,
	task_executor: Arc<dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync>,
	key_seed: Option<String>,
	base_port: u16,
	root: &PathBuf,
) -> Configuration
{
	let root = root.join(format!("node-{}", index));

	let mut network_config = NetworkConfiguration::new(
		format!("Node {}", index),
		"network/test/0.1",
		Default::default(),
		None,
	);

	network_config.allow_non_globals_in_dht = true;

	network_config.listen_addresses.push(
		std::iter::once(multiaddr::Protocol::Ip4(Ipv4Addr::new(127, 0, 0, 1)))
			.chain(std::iter::once(multiaddr::Protocol::Tcp(base_port + index as u16)))
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

pub fn run_test_node(
	task_executor: Arc<dyn Fn(Pin<Box<dyn futures::Future<Output = ()> + Send>>, TaskType) + Send + Sync>,
	root: &PathBuf,
	storage_update_func: impl Fn(),
) -> Result<impl AbstractService, ()> {
	let mut spec = polkadot_local_testnet_config();

	let mut storage = spec.as_storage_builder().build_storage().unwrap();
	BasicExternalities::execute_with_storage(
		&mut storage,
		storage_update_func,
	);

	spec.set_storage(storage);

	let mut storage = spec.as_storage_builder().build_storage().unwrap();
	BasicExternalities::execute_with_storage(
		&mut storage,
		|| {
			use polkadot_test_runtime::*;
			panic!("{:?}", EpochDuration::get());
		},
	);

	let key = String::new();
	let base_port = 27015;
	let config = node_config(
		0,
		&spec,
		Role::Authority { sentry_nodes: Vec::new() },
		task_executor,
		Some(key),
		base_port,
		&root,
	);
	let authority_discovery_enabled = false;
	let (service, _client, _handles) = polkadot_test_new_full(
		config,
		None,
		None,
		authority_discovery_enabled,
		6000,
	).unwrap();

	Ok(service)
}

/// Polkadot local testnet config (multivalidator Alice + Bob)
fn polkadot_local_testnet_config() -> PolkadotChainSpec {
	PolkadotChainSpec::from_genesis(
		"Local Testnet",
		"local_testnet",
		ChainType::Local,
		polkadot_local_testnet_genesis,
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		Default::default(),
	)
}

fn polkadot_local_testnet_genesis() -> polkadot_test_runtime::GenesisConfig {
	polkadot_testnet_genesis(
		vec![
			get_authority_keys_from_seed("Alice"),
			get_authority_keys_from_seed("Bob"),
		],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Helper function to generate stash, controller and session key from seed
fn get_authority_keys_from_seed(seed: &str) -> (
	AccountId,
	AccountId,
	BabeId,
	GrandpaId,
	ValidatorId,
) {
	(
		get_account_id_from_seed::<sr25519::Public>(&format!("{}//stash", seed)),
		get_account_id_from_seed::<sr25519::Public>(seed),
		get_from_seed::<BabeId>(seed),
		get_from_seed::<GrandpaId>(seed),
		get_from_seed::<ValidatorId>(seed),
	)
}

fn testnet_accounts() -> Vec<AccountId> {
	vec![
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		get_account_id_from_seed::<sr25519::Public>("Bob"),
		get_account_id_from_seed::<sr25519::Public>("Charlie"),
		get_account_id_from_seed::<sr25519::Public>("Dave"),
		get_account_id_from_seed::<sr25519::Public>("Eve"),
		get_account_id_from_seed::<sr25519::Public>("Ferdie"),
		get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
		get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
		get_account_id_from_seed::<sr25519::Public>("Charlie//stash"),
		get_account_id_from_seed::<sr25519::Public>("Dave//stash"),
		get_account_id_from_seed::<sr25519::Public>("Eve//stash"),
		get_account_id_from_seed::<sr25519::Public>("Ferdie//stash"),
	]
}

/// Helper function to create polkadot GenesisConfig for testing
fn polkadot_testnet_genesis(
	initial_authorities: Vec<(AccountId, AccountId, BabeId, GrandpaId, ValidatorId)>,
	root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> polkadot_test_runtime::GenesisConfig {
	use polkadot_test_runtime as polkadot;

	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

	const ENDOWMENT: u128 = 1_000_000 * DOTS;
	const STASH: u128 = 100 * DOTS;

	polkadot::GenesisConfig {
		system: Some(polkadot::SystemConfig {
			code: polkadot::WASM_BINARY.to_vec(),
			changes_trie_config: Default::default(),
		}),
		indices: Some(polkadot::IndicesConfig {
			indices: vec![],
		}),
		balances: Some(polkadot::BalancesConfig {
			balances: endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect(),
		}),
		session: Some(polkadot::SessionConfig {
			keys: initial_authorities.iter().map(|x| (
						  x.0.clone(),
						  x.0.clone(),
						polkadot_test_runtime::SessionKeys {
							babe: x.2.clone(),
							grandpa: x.3.clone(),
							parachain_validator: x.4.clone(),
						},
				  )).collect::<Vec<_>>(),
		}),
		staking: Some(polkadot::StakingConfig {
			minimum_validator_count: 1,
			validator_count: 2,
			stakers: initial_authorities.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, polkadot::StakerStatus::Validator))
				.collect(),
				invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
				force_era: Forcing::NotForcing,
				slash_reward_fraction: Perbill::from_percent(10),
				.. Default::default()
		}),
		babe: Some(Default::default()),
		grandpa: Some(Default::default()),
		authority_discovery: Some(polkadot::AuthorityDiscoveryConfig {
			keys: vec![],
		}),
		parachains: Some(polkadot::ParachainsConfig {
			authorities: vec![],
		}),
		registrar: Some(polkadot::RegistrarConfig{
			parachains: vec![],
			_phdata: Default::default(),
		}),
		claims: Some(polkadot::ClaimsConfig {
			claims: vec![],
			vesting: vec![],
		}),
		vesting: Some(polkadot::VestingConfig {
			vesting: vec![],
		}),
		// TODO: it should have one?
		/*
		sudo: Some(polkadot::SudoConfig {
			key: root_key,
		}),
		*/
	}
}
