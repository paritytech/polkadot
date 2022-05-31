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

//! Chain specifications for the test runtime.

use babe_primitives::AuthorityId as BabeId;
use grandpa::AuthorityId as GrandpaId;
use pallet_staking::Forcing;
use polkadot_primitives::v2::{AccountId, AssignmentId, ValidatorId, MAX_CODE_SIZE, MAX_POV_SIZE};
use polkadot_service::chain_spec::{
	get_account_id_from_seed, get_from_seed, polkadot_chain_spec_properties, Extensions,
};
use polkadot_test_runtime::BABE_GENESIS_EPOCH_CONFIG;
use sc_chain_spec::{ChainSpec, ChainType};
use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;
use sp_core::sr25519;
use sp_runtime::Perbill;
use test_runtime_constants::currency::DOTS;

const DEFAULT_PROTOCOL_ID: &str = "dot";

/// The `ChainSpec` parameterized for polkadot test runtime.
pub type PolkadotChainSpec =
	sc_service::GenericChainSpec<polkadot_test_runtime::GenesisConfig, Extensions>;

/// Local testnet config (multivalidator Alice + Bob)
pub fn polkadot_local_testnet_config() -> PolkadotChainSpec {
	PolkadotChainSpec::from_genesis(
		"Local Testnet",
		"local_testnet",
		ChainType::Local,
		|| polkadot_local_testnet_genesis(),
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		Some(polkadot_chain_spec_properties()),
		Default::default(),
	)
}

/// Local testnet genesis config (multivalidator Alice + Bob)
pub fn polkadot_local_testnet_genesis() -> polkadot_test_runtime::GenesisConfig {
	polkadot_testnet_genesis(
		vec![get_authority_keys_from_seed("Alice"), get_authority_keys_from_seed("Bob")],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Helper function to generate stash, controller and session key from seed
fn get_authority_keys_from_seed(
	seed: &str,
) -> (AccountId, AccountId, BabeId, GrandpaId, ValidatorId, AssignmentId, AuthorityDiscoveryId) {
	(
		get_account_id_from_seed::<sr25519::Public>(&format!("{}//stash", seed)),
		get_account_id_from_seed::<sr25519::Public>(seed),
		get_from_seed::<BabeId>(seed),
		get_from_seed::<GrandpaId>(seed),
		get_from_seed::<ValidatorId>(seed),
		get_from_seed::<AssignmentId>(seed),
		get_from_seed::<AuthorityDiscoveryId>(seed),
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

/// Helper function to create polkadot `GenesisConfig` for testing
fn polkadot_testnet_genesis(
	initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
	)>,
	root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> polkadot_test_runtime::GenesisConfig {
	use polkadot_test_runtime as runtime;

	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

	const ENDOWMENT: u128 = 1_000_000 * DOTS;
	const STASH: u128 = 100 * DOTS;

	runtime::GenesisConfig {
		system: runtime::SystemConfig {
			code: runtime::WASM_BINARY.expect("Wasm binary must be built for testing").to_vec(),
			..Default::default()
		},
		indices: runtime::IndicesConfig { indices: vec![] },
		balances: runtime::BalancesConfig {
			balances: endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect(),
		},
		session: runtime::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						runtime::SessionKeys {
							babe: x.2.clone(),
							grandpa: x.3.clone(),
							para_validator: x.4.clone(),
							para_assignment: x.5.clone(),
							authority_discovery: x.6.clone(),
						},
					)
				})
				.collect::<Vec<_>>(),
		},
		staking: runtime::StakingConfig {
			minimum_validator_count: 1,
			validator_count: 2,
			stakers: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, runtime::StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
			force_era: Forcing::NotForcing,
			slash_reward_fraction: Perbill::from_percent(10),
			..Default::default()
		},
		babe: runtime::BabeConfig {
			authorities: vec![],
			epoch_config: Some(BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		authority_discovery: runtime::AuthorityDiscoveryConfig { keys: vec![] },
		claims: runtime::ClaimsConfig { claims: vec![], vesting: vec![] },
		vesting: runtime::VestingConfig { vesting: vec![] },
		sudo: runtime::SudoConfig { key: Some(root_key) },
		configuration: runtime::ConfigurationConfig {
			config: polkadot_runtime_parachains::configuration::HostConfiguration {
				validation_upgrade_cooldown: 10u32,
				validation_upgrade_delay: 5,
				code_retention_period: 1200,
				max_code_size: MAX_CODE_SIZE,
				max_pov_size: MAX_POV_SIZE,
				max_head_data_size: 32 * 1024,
				group_rotation_frequency: 20,
				chain_availability_period: 4,
				thread_availability_period: 4,
				no_show_slots: 10,
				minimum_validation_upgrade_delay: 5,
				..Default::default()
			},
		},
	}
}

/// Can be called for a `Configuration` to check if it is a configuration for the `Test` network.
pub trait IdentifyVariant {
	/// Returns if this is a configuration for the `Test` network.
	fn is_test(&self) -> bool;
}

impl IdentifyVariant for Box<dyn ChainSpec> {
	fn is_test(&self) -> bool {
		self.id().starts_with("test")
	}
}
