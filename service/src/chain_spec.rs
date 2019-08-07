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
	GenesisConfig, CouncilConfig, ElectionsConfig, DemocracyConfig, SystemConfig, BabeConfig,
	SessionConfig, StakingConfig, BalancesConfig, Perbill, SessionKeys, TechnicalCommitteeConfig,
	GrandpaConfig, SudoConfig, IndicesConfig, CuratedGrandpaConfig, StakerStatus, WASM_BINARY,
	ParachainsConfig,
};
use polkadot_runtime::constants::{currency::DOTS, time::*};
use telemetry::TelemetryEndpoints;
use hex_literal::hex;
use babe_primitives::AuthorityId as BabeId;
use grandpa::AuthorityId as GrandpaId;

const STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
const DEFAULT_PROTOCOL_ID: &str = "dot";

/// Specialised `ChainSpec`.
pub type ChainSpec = ::service::ChainSpec<GenesisConfig>;

pub fn poc_3_testnet_config() -> Result<ChainSpec, String> {
	ChainSpec::from_json_bytes(&include_bytes!("../res/alexander.json")[..])
}

fn session_keys(ed_key: ed25519::Public, sr_key: sr25519::Public) -> SessionKeys {
	SessionKeys {
		ed25519: ed_key,
		sr25519: sr_key,
	}
}

fn staging_testnet_config_genesis() -> GenesisConfig {
	// subkey inspect "$SECRET"
	let endowed_accounts = vec![
		hex!["12b782529c22032ed4694e0f6e7d486be7daa6d12088f6bc74d593b3900b8438"].unchecked_into(), // 5CVFESwfkk7NmhQ6FwHCM9roBvr9BGa4vJHFYU8DnGQxrXvz
	];

	// for i in 1 2 3 4; do for j in stash controller; do subkey inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in session_babe; do subkey --sr25519 inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in session_fggp; do subkey --ed25519 inspect "$SECRET//$i//$j"; done; done
	let initial_authorities: Vec<(AccountId, AccountId, BabeId, GrandpaId)> = vec![(
		hex!["32a5718e87d16071756d4b1370c411bbbb947eb62f0e6e0b937d5cbfc0ea633b"].unchecked_into(), // 5DD7Q4VEfPTLEdn11CnThoHT5f9xKCrnofWJL5SsvpTghaAT
		hex!["bee39fe862c85c91aaf343e130d30b643c6ea0b4406a980206f1df8331f7093b"].unchecked_into(), // 5GNzaEqhrZAtUQhbMe2gn9jBuNWfamWFZHULryFwBUXyd1cG
		hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"].unchecked_into(), // 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
		hex!["76620f7c98bce8619979c2b58cf2b0aff71824126d2b039358729dad993223db"].unchecked_into(), // 5EjvdwATjyFFikdZibVvx1q5uBHhphS2Mnsq5c7yfaYK25vm
	),(
		hex!["b496c98a405ceab59b9e970e59ef61acd7765a19b704e02ab06c1cdfe171e40f"].unchecked_into(), // 5G9VGb8ESBeS8Ca4or43RfhShzk9y7T5iTmxHk5RJsjZwsRx
		hex!["86d3a7571dd60139d297e55d8238d0c977b2e208c5af088f7f0136b565b0c103"].unchecked_into(), // 5F7V9Y5FcxKXe1aroqvPeRiUmmeQwTFcL3u9rrPXcMuMiCNx
		hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"].unchecked_into(), // 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
		hex!["e2234d661bee4a04c38392c75d1566200aa9e6ae44dd98ee8765e4cc9af63cb7"].unchecked_into(), // 5HBDAaybNqjmY7ww8ZcZZY1L5LHxvpnyfqJwoB7HhR6raTmG
	),(
		hex!["ae12f70078a22882bf5135d134468f77301927aa67c376e8c55b7ff127ace115"].unchecked_into(), // 5FzwpgGvk2kk9agow6KsywLYcPzjYc8suKej2bne5G5b9YU3
		hex!["7addb914ec8486bbc60643d2647685dcc06373401fa80e09813b630c5831d54b"].unchecked_into(), // 5EqoZhVC2BcsM4WjvZNidu2muKAbu5THQTBKe3EjvxXkdP7A
		hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"].unchecked_into(), // 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
		hex!["5b57ed1443c8967f461db1f6eb2ada24794d163a668f1cf9d9ce3235dfad8799"].unchecked_into(), // 5E8ULLQrDAtWhfnVfZmX41Yux86zNAwVJYguWJZVWrJvdhBe
	),(
		hex!["0867dbb49721126df589db100dda728dc3b475cbf414dad8f72a1d5e84897252"].unchecked_into(), // 5CFj6Kg9rmVn1vrqpyjau2ztyBzKeVdRKwNPiA3tqhB5HPqq
		hex!["26ab2b4b2eba2263b1e55ceb48f687bb0018130a88df0712fbdaf6a347d50e2a"].unchecked_into(), // 5CwQXP6nvWzigFqNhh2jvCaW9zWVzkdveCJY3tz2MhXMjTon
		hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"].unchecked_into(), // 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
		hex!["e60d23f49e93c1c1f2d7c115957df5bbd7faf5ebf138d1e9d02e8b39a1f63df0"].unchecked_into(), // 5HGLmrZsiTFTPp3QoS1W8w9NxByt8PVq79reqvdxNcQkByqK
	)];

	const ENDOWMENT: u128 = 1_000_000 * DOTS;
	const STASH: u128 = 100 * DOTS;

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
				session_keys(x.3.clone(), x.2.clone()),
			)).collect::<Vec<_>>(),
		}),
		staking: Some(StakingConfig {
			current_era: 0,
			offline_slash: Perbill::from_parts(1_000_000),
			validator_count: 7,
			offline_slash_grace: 4,
			minimum_validator_count: 4,
			stakers: initial_authorities.iter().map(|x| (x.0.clone(), x.1.clone(), STASH, StakerStatus::Validator)).collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
		}),
		democracy: Some(Default::default()),
		collective_Instance1: Some(CouncilConfig {
			members: vec![],
			phantom: Default::default(),
		}),
		collective_Instance2: Some(TechnicalCommitteeConfig {
			members: vec![],
			phantom: Default::default(),
		}),
		elections: Some(ElectionsConfig {
			members: vec![],
			presentation_duration: 1 * DAYS,
			term_duration: 28 * DAYS,
			desired_seats: 0,
		}),
		sudo: Some(SudoConfig {
			key: endowed_accounts[0].clone(),
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities.iter().map(|x| (x.3.clone(), 1)).collect(),
		}),
		babe: Some(BabeConfig {
			authorities: initial_authorities.iter().map(|x| (x.2.clone(), 1)).collect(),
		}),
		parachains: Some(ParachainsConfig {
			// TODO: double check this. x3 is cool?
			authorities: initial_authorities.iter().map(|x| x.3.clone()).collect(),
			parachains: vec![],
			_phdata: Default::default(),
		}),
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

/// Helper function to generate BabeId from seed
pub fn get_babe_id_from_seed(seed: &str) -> BabeId {
	sr25519::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

/// Helper function to generate SessionKey from seed
pub fn get_grandpa_id_from_seed(seed: &str) -> SessionKey {
	ed25519::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

/// Helper function to generate stash, controller and session key from seed
pub fn get_authority_keys_from_seed(seed: &str) -> (AccountId, AccountId, BabeId, GrandpaId) {
	(
		get_account_id_from_seed(&format!("{}//stash", seed)),
		get_account_id_from_seed(seed),
		get_babe_id_from_seed(seed),
		get_grandpa_id_from_seed(seed)
	)
}

/// Helper function to create GenesisConfig for testing
pub fn testnet_genesis(
	initial_authorities: Vec<(AccountId, AccountId, BabeId, GrandpaId)>,
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

	const ENDOWMENT: u128 = 1_000_000 * DOTS;
	const STASH: u128 = 100 * DOTS;

	let desired_seats = (endowed_accounts.len() / 2 - initial_authorities.len()) as u32;

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
				session_keys(x.3.clone(), x.2.clone()),
			)).collect::<Vec<_>>(),
		}),
		staking: Some(StakingConfig {
			current_era: 0,
			minimum_validator_count: 1,
			validator_count: 2,
			offline_slash: Perbill::zero(),
			offline_slash_grace: 0,
			stakers: initial_authorities.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
		}),
		democracy: Some(DemocracyConfig::default()),
		collective_Instance1: Some(CouncilConfig {
			members: vec![],
			phantom: Default::default(),
		}),
		collective_Instance2: Some(TechnicalCommitteeConfig {
			members: vec![],
			phantom: Default::default(),
		}),
		elections: Some(ElectionsConfig {
			members: endowed_accounts.iter()
				.filter(|&endowed| initial_authorities.iter()
					.find(|&(_, controller, _, _)| controller == endowed)
					.is_none()
				).map(|a| (a.clone(), 1000000)).collect(),
			presentation_duration: 10,
			term_duration: 1000000,
			desired_seats,
		}),
		parachains: Some(ParachainsConfig {
			// TODO: double check this. x3 is cool?
			authorities: initial_authorities.iter().map(|x| x.3.clone()).collect(),
			parachains: vec![],
			_phdata: Default::default(),
		}),
		sudo: Some(SudoConfig {
			key: root_key,
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities.iter().map(|x| (x.3.clone(), 1)).collect(),
		}),
		babe: Some(BabeConfig {
			authorities: initial_authorities.iter().map(|x| (x.2.clone(), 1)).collect(),
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
