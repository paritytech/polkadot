// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! Polkadot chain configurations.

use primitives::{ed25519, sr25519, Pair, crypto::UncheckedInto};
use polkadot_primitives::{AccountId, SessionKey};
use polkadot_runtime::{
	GenesisConfig, CouncilSeatsConfig, DemocracyConfig, SystemConfig, AuraConfig,
	SessionConfig, StakingConfig, TimestampConfig, BalancesConfig, Perbill, SessionKeys,
	GrandpaConfig, SudoConfig, IndicesConfig, CuratedGrandpaConfig, StakerStatus, WASM_BINARY,
};
use telemetry::TelemetryEndpoints;
use hex_literal::hex;

const STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
const DEFAULT_PROTOCOL_ID: &str = "dot";

/// Specialised `ChainSpec`.
pub type ChainSpec = ::service::ChainSpec<GenesisConfig>;

pub fn poc_3_testnet_config() -> Result<ChainSpec, String> {
	ChainSpec::from_embedded(include_bytes!("../res/alexander.json"))
}

fn staging_testnet_config_genesis() -> GenesisConfig {
	// subkey inspect "$SECRET"
	let endowed_accounts = vec![
		hex!["42d69e4222c08885a4d6ff65f01852ba4a1599b683ad66286e4603d825e26b49"].unchecked_into(), // 5DaLmkrGTFvSTHBpShqqqRYaydw1sT4u3NCogYiE8Q1LqUUp
	];

	// for i in 1 2 3 4; do for j in stash controller; do subkey inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in session; do subkey -e inspect "$SECRET//$i//$j"; done; done
	let initial_authorities: Vec<(AccountId, AccountId, SessionKey)> = vec![(
		hex!["543cf15f6a0289e48eb4f30d451d1731c5fb0e1b2c5a4b99439c11808af3432d"].unchecked_into(), // 5Dy9yz2mjwDmTgjkDFxjPBpovKmKAgTndiRiTp4DfrTEdUvi
		hex!["8a6ea654337e4a28ce7be124f73ad84702619942722d01cc271e5b421653c56d"].unchecked_into(), // 5FCDLPUMZpZPRfouRfQDZp74typV9SjSxPgG6ymwe5Z3Sbko
		hex!["03644a181bc4e4197914aa109f3c97b6fe8c4787a82a1ddfab54e4ebedd8ab20"].unchecked_into(), // 5C99nwu8Ucq1yUJfajviwbqMAejpmaERHpmkPVWiFdxiF6yg
	),(
		hex!["c4957aa922910004f3b006d638b034070407dcb21e0905cb5cca9b58aec7fa3e"].unchecked_into(), // 5GWTeVF49JR9dAMVe4rRAAMXuhEjRAhSiYqQV4LbwpHTDLei
		hex!["1adea46f5c3d272cd6426b338dd77d5bca3aff615338c82a0f02f4c62d89280f"].unchecked_into(), // 5CfwEv8TQKnszHNhYPuij6EtLZHCcaN3DgzfPCozcS9oxZzB
		hex!["d739e1bb4c2b13ea1fff9be72e72d3bb1b364eb3b26176ab9a9512d386b7510b"].unchecked_into(), // 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
	),(
		hex!["6c3d14686e97d393814a09bea4246b9f273dcdbdef6731dcab3430b36820f135"].unchecked_into(), // 5EWdAzp9aJseLKVNeWJwE2K8PD47qzMbKVysCXr67xnEohYL
		hex!["7a73b9fcb97cee5c2240d88ca9baea06758fac450efe6f90a72014c939d41857"].unchecked_into(), // 5EqG5RSujgrtSBB5DQvR1Z2EMxAe92sAdWNTNHtX4nL2MkPi
		hex!["145791f7187d91398d8b445598f62be39b766d6e33e9d57b69c4d23fca218d7f"].unchecked_into(), // 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
	),(
		hex!["be6726c17ad7b5844c9e0ab6a1698d00d88bf183f0f82d8ec9627531c9ddc934"].unchecked_into(), // 5GNMbce1P2FfjvcPcoUxzjj6bYSRdQ2RpsbCyF9ozwMxx3NS
		hex!["123b9048ba61265547ad3f336dfa48c16851ba1a96691e5d1ab3be1725db0614"].unchecked_into(), // 5CUcQvAgMzXMpQSz8mgzeiswDFHED88NiEK4byfS5TLaTJow
		hex!["8abecfa66704176be23df099bf441ea65444992d63b3ced3e76a17a4d38b0b0e"].unchecked_into(), // 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
	)];

	const MILLICENTS: u128 = 1_000_000_000;
	const CENTS: u128 = 1_000 * MILLICENTS;    // assume this is worth about a cent.
	const DOLLARS: u128 = 100 * CENTS;

	const SECS_PER_BLOCK: u64 = 6;
	const MINUTES: u64 = 60 / SECS_PER_BLOCK;
	const HOURS: u64 = MINUTES * 60;
	const DAYS: u64 = HOURS * 24;

	const ENDOWMENT: u128 = 10_000_000 * DOLLARS;
	const STASH: u128 = 100 * DOLLARS;

	GenesisConfig {
		system: Some(SystemConfig {
			code: WASM_BINARY.to_vec(),
			changes_trie_config: Default::default(),
		}),
		balances: Some(BalancesConfig {
			balances: endowed_accounts.iter()
				.map(|k: &AccountId| (k.clone(), ENDOWMENT))
				.chain(initial_authorities.iter().map(|x| (x.0.clone(), STASH)))
				.collect(),
			vesting: vec![],
		}),
		indices: Some(IndicesConfig {
			ids: endowed_accounts.iter().cloned()
				.chain(initial_authorities.iter().map(|x| x.0.clone()))
				.collect::<Vec<_>>(),
		}),
		session: Some(SessionConfig {
			keys: initial_authorities.iter().map(|x| (
				x.0.clone(),
				SessionKeys { ed25519: x.2.clone() },
			)).collect::<Vec<_>>(),
		}),
		staking: Some(StakingConfig {
			current_era: 0,
			offline_slash: Perbill::from_parts(1_000_000),
			session_reward: Perbill::from_parts(2_065),
			current_session_reward: 0,
			validator_count: 7,
			offline_slash_grace: 4,
			minimum_validator_count: 4,
			stakers: initial_authorities.iter().map(|x| (x.0.clone(), x.1.clone(), STASH, StakerStatus::Validator)).collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
		}),
		democracy: Some(Default::default()),
		council_seats: Some(CouncilSeatsConfig {
			active_council: vec![],
			presentation_duration: 1 * DAYS,
			term_duration: 28 * DAYS,
			desired_seats: 0,
		}),
		timestamp: Some(TimestampConfig {
			minimum_period: SECS_PER_BLOCK / 2, // due to the nature of aura the slots are 2*period
		}),
		sudo: Some(SudoConfig {
			key: endowed_accounts[0].clone(),
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities.iter().map(|x| (x.2.clone(), 1)).collect(),
		}),
		aura: Some(AuraConfig {
			authorities: initial_authorities.iter().map(|x| x.2.clone()).collect(),
		}),
		parachains: Some(Default::default()),
		curated_grandpa: Some(CuratedGrandpaConfig {
			shuffle_period: 1024,
		}),
	}
}

/// Staging testnet config.
pub fn staging_testnet_config() -> ChainSpec {
	let boot_nodes = vec![];
	ChainSpec::from_genesis(
		"Staging Testnet",
		"staging_testnet",
		staging_testnet_config_genesis,
		boot_nodes,
		Some(TelemetryEndpoints::new(vec![(STAGING_TELEMETRY_URL.to_string(), 0)])),
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
	)
}

/// Helper function to generate AccountId from seed
pub fn get_account_id_from_seed(seed: &str) -> AccountId {
	sr25519::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

/// Helper function to generate SessionKey from seed
pub fn get_session_key_from_seed(seed: &str) -> SessionKey {
	ed25519::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

/// Helper function to generate stash, controller and session key from seed
pub fn get_authority_keys_from_seed(seed: &str) -> (AccountId, AccountId, SessionKey) {
	(
		get_account_id_from_seed(&format!("{}//stash", seed)),
		get_account_id_from_seed(seed),
		get_session_key_from_seed(seed),
	)
}

/// Helper function to create GenesisConfig for testing
pub fn testnet_genesis(
	initial_authorities: Vec<(AccountId, AccountId, SessionKey)>,
	root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> GenesisConfig {
	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(|| {
		vec![
			get_account_id_from_seed("Alice"),
			get_account_id_from_seed("Bob"),
			get_account_id_from_seed("Charlie"),
			get_account_id_from_seed("Dave"),
			get_account_id_from_seed("Eve"),
			get_account_id_from_seed("Ferdie"),
			get_account_id_from_seed("Alice//stash"),
			get_account_id_from_seed("Bob//stash"),
			get_account_id_from_seed("Charlie//stash"),
			get_account_id_from_seed("Dave//stash"),
			get_account_id_from_seed("Eve//stash"),
			get_account_id_from_seed("Ferdie//stash"),
		]
	});

	const STASH: u128 = 1 << 20;
	const ENDOWMENT: u128 = 1 << 20;
	let council_desired_seats = (endowed_accounts.len() / 2 - initial_authorities.len()) as u32;

	GenesisConfig {
		system: Some(SystemConfig {
			code: WASM_BINARY.to_vec(),
			changes_trie_config: Default::default(),
		}),
		indices: Some(IndicesConfig {
			ids: endowed_accounts.clone(),
		}),
		balances: Some(BalancesConfig {
			balances: endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect(),
			vesting: vec![],
		}),
		session: Some(SessionConfig {
			keys: initial_authorities.iter().map(|x| (
				x.0.clone(),
				SessionKeys { ed25519: x.2.clone() },
			)).collect::<Vec<_>>(),
		}),
		staking: Some(StakingConfig {
			current_era: 0,
			minimum_validator_count: 1,
			validator_count: 2,
			offline_slash: Perbill::zero(),
			session_reward: Perbill::zero(),
			current_session_reward: 0,
			offline_slash_grace: 0,
			stakers: initial_authorities.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
		}),
		democracy: Some(DemocracyConfig::default()),
		council_seats: Some(CouncilSeatsConfig {
			active_council: endowed_accounts.iter()
				.filter(|&endowed| initial_authorities.iter()
					.find(|&(_, controller, _)| controller == endowed)
					.is_none()
				).map(|a| (a.clone(), 1000000)).collect(),
			presentation_duration: 10,
			term_duration: 1000000,
			desired_seats: council_desired_seats,
		}),
		parachains: Some(Default::default()),
		timestamp: Some(TimestampConfig {
			minimum_period: 2,                    // 2*2=4 second block time.
		}),
		sudo: Some(SudoConfig {
			key: root_key,
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities.iter().map(|x| (x.2.clone(), 1)).collect(),
		}),
		aura: Some(AuraConfig {
			authorities: initial_authorities.iter().map(|x| x.2.clone()).collect(),
		}),
		curated_grandpa: Some(CuratedGrandpaConfig {
			shuffle_period: 1024,
		}),
	}
}


fn development_config_genesis() -> GenesisConfig {
	testnet_genesis(
		vec![
			get_authority_keys_from_seed("Alice"),
		],
		get_account_id_from_seed("Alice"),
		None,
	)
}

/// Development config (single validator Alice)
pub fn development_config() -> ChainSpec {
	ChainSpec::from_genesis(
		"Development",
		"dev",
		development_config_genesis,
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
	)
}

fn local_testnet_genesis() -> GenesisConfig {
	testnet_genesis(
		vec![
			get_authority_keys_from_seed("Alice"),
			get_authority_keys_from_seed("Bob"),
		],
		get_account_id_from_seed("Alice"),
		None,
	)
}

/// Local testnet config (multivalidator Alice + Bob)
pub fn local_testnet_config() -> ChainSpec {
	ChainSpec::from_genesis(
		"Local Testnet",
		"local_testnet",
		local_testnet_genesis,
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
	)
}
