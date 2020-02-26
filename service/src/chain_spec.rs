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

//! Polkadot chain configurations.

use sp_core::{Pair, Public, crypto::UncheckedInto, sr25519};
use polkadot_primitives::{AccountId, AccountPublic, parachain::ValidatorId};
use polkadot_runtime as polkadot;
use polkadot_runtime::constants::currency::DOTS;
use sc_chain_spec::ChainSpecExtension;
use sp_runtime::{traits::IdentifyAccount, Perbill};
use serde::{Serialize, Deserialize};
use telemetry::TelemetryEndpoints;
use hex_literal::hex;
use babe_primitives::AuthorityId as BabeId;
use grandpa::AuthorityId as GrandpaId;
use im_online::sr25519::{AuthorityId as ImOnlineId};
use authority_discovery_primitives::AuthorityId as AuthorityDiscoveryId;
use pallet_staking::Forcing;

const STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
const DEFAULT_PROTOCOL_ID: &str = "dot";

/// Node `ChainSpec` extensions.
///
/// Additional parameters for some Substrate core modules,
/// customizable from the chain spec.
#[derive(Default, Clone, Serialize, Deserialize, ChainSpecExtension)]
#[serde(rename_all = "camelCase")]
pub struct Extensions {
	/// Block numbers with known hashes.
	pub fork_blocks: sc_client::ForkBlocks<polkadot_primitives::Block>,
	/// Known bad block hashes.
	pub bad_blocks: sc_client::BadBlocks<polkadot_primitives::Block>,
}

/// The `ChainSpec`.
///
/// We use the same `ChainSpec` type for Polkadot and Kusama. As Kusama
/// is only loaded from a file, the `GenesisConfig` type is not used.
pub type ChainSpec = service::ChainSpec<
	polkadot::GenesisConfig,
	Extensions,
>;

pub fn kusama_config() -> Result<ChainSpec, String> {
	ChainSpec::from_json_bytes(&include_bytes!("../res/kusama.json")[..])
}

pub fn westend_config() -> Result<ChainSpec, String> {
	ChainSpec::from_json_bytes(&include_bytes!("../res/westend.json")[..])
}

fn session_keys(
	babe: BabeId,
	grandpa: GrandpaId,
	im_online: ImOnlineId,
	parachain_validator: ValidatorId,
	authority_discovery: AuthorityDiscoveryId
) -> polkadot::SessionKeys {
	polkadot::SessionKeys { babe, grandpa, im_online, parachain_validator, authority_discovery }
}

fn staging_testnet_config_genesis() -> polkadot::GenesisConfig {
	// subkey inspect "$SECRET"
	let endowed_accounts = vec![
		// 5CVFESwfkk7NmhQ6FwHCM9roBvr9BGa4vJHFYU8DnGQxrXvz
		hex!["12b782529c22032ed4694e0f6e7d486be7daa6d12088f6bc74d593b3900b8438"].into(),
	];

	// for i in 1 2 3 4; do for j in stash controller; do subkey inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in babe; do subkey --sr25519 inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in grandpa; do subkey --ed25519 inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in im_online; do subkey --sr25519 inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in parachains; do subkey --sr25519 inspect "$SECRET//$i//$j"; done; done
	let initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AuthorityDiscoveryId
	)> = vec![(
		// 5DD7Q4VEfPTLEdn11CnThoHT5f9xKCrnofWJL5SsvpTghaAT
		hex!["32a5718e87d16071756d4b1370c411bbbb947eb62f0e6e0b937d5cbfc0ea633b"].into(),
		// 5GNzaEqhrZAtUQhbMe2gn9jBuNWfamWFZHULryFwBUXyd1cG
		hex!["bee39fe862c85c91aaf343e130d30b643c6ea0b4406a980206f1df8331f7093b"].into(),
		// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
		hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"].unchecked_into(),
		// 5EjvdwATjyFFikdZibVvx1q5uBHhphS2Mnsq5c7yfaYK25vm
		hex!["76620f7c98bce8619979c2b58cf2b0aff71824126d2b039358729dad993223db"].unchecked_into(),
		// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
		hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"].unchecked_into(),
		// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
		hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"].unchecked_into(),
		// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
		hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"].unchecked_into(),
	),(
		// 5G9VGb8ESBeS8Ca4or43RfhShzk9y7T5iTmxHk5RJsjZwsRx
		hex!["b496c98a405ceab59b9e970e59ef61acd7765a19b704e02ab06c1cdfe171e40f"].into(),
		// 5F7V9Y5FcxKXe1aroqvPeRiUmmeQwTFcL3u9rrPXcMuMiCNx
		hex!["86d3a7571dd60139d297e55d8238d0c977b2e208c5af088f7f0136b565b0c103"].into(),
		// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
		hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"].unchecked_into(),
		// 5HBDAaybNqjmY7ww8ZcZZY1L5LHxvpnyfqJwoB7HhR6raTmG
		hex!["e2234d661bee4a04c38392c75d1566200aa9e6ae44dd98ee8765e4cc9af63cb7"].unchecked_into(),
		// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
		hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"].unchecked_into(),
		// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
		hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"].unchecked_into(),
		// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
		hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"].unchecked_into(),
	),(
		// 5FzwpgGvk2kk9agow6KsywLYcPzjYc8suKej2bne5G5b9YU3
		hex!["ae12f70078a22882bf5135d134468f77301927aa67c376e8c55b7ff127ace115"].into(),
		// 5EqoZhVC2BcsM4WjvZNidu2muKAbu5THQTBKe3EjvxXkdP7A
		hex!["7addb914ec8486bbc60643d2647685dcc06373401fa80e09813b630c5831d54b"].into(),
		// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
		hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"].unchecked_into(),
		// 5E8ULLQrDAtWhfnVfZmX41Yux86zNAwVJYguWJZVWrJvdhBe
		hex!["5b57ed1443c8967f461db1f6eb2ada24794d163a668f1cf9d9ce3235dfad8799"].unchecked_into(),
		// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
		hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"].unchecked_into(),
		// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
		hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"].unchecked_into(),
		// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
		hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"].unchecked_into(),
	),(
		// 5CFj6Kg9rmVn1vrqpyjau2ztyBzKeVdRKwNPiA3tqhB5HPqq
		hex!["0867dbb49721126df589db100dda728dc3b475cbf414dad8f72a1d5e84897252"].into(),
		// 5CwQXP6nvWzigFqNhh2jvCaW9zWVzkdveCJY3tz2MhXMjTon
		hex!["26ab2b4b2eba2263b1e55ceb48f687bb0018130a88df0712fbdaf6a347d50e2a"].into(),
		// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
		hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"].unchecked_into(),
		// 5HGLmrZsiTFTPp3QoS1W8w9NxByt8PVq79reqvdxNcQkByqK
		hex!["e60d23f49e93c1c1f2d7c115957df5bbd7faf5ebf138d1e9d02e8b39a1f63df0"].unchecked_into(),
		// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
		hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"].unchecked_into(),
		// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
		hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"].unchecked_into(),
		// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
		hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"].unchecked_into(),
	)];

	const ENDOWMENT: u128 = 1_000_000 * DOTS;
	const STASH: u128 = 100 * DOTS;

	polkadot::GenesisConfig {
		system: Some(polkadot::SystemConfig {
			code: polkadot::WASM_BINARY.to_vec(),
			changes_trie_config: Default::default(),
		}),
		balances: Some(polkadot::BalancesConfig {
			balances: endowed_accounts.iter()
				.map(|k: &AccountId| (k.clone(), ENDOWMENT))
				.chain(initial_authorities.iter().map(|x| (x.0.clone(), STASH)))
				.collect(),
		}),
		indices: Some(polkadot::IndicesConfig {
			indices: vec![],
		}),
		session: Some(polkadot::SessionConfig {
			keys: initial_authorities.iter().map(|x| (
				x.0.clone(),
				session_keys(x.2.clone(), x.3.clone(), x.4.clone(), x.5.clone(), x.6.clone()),
			)).collect::<Vec<_>>(),
		}),
		staking: Some(polkadot::StakingConfig {
			current_era: 0,
			validator_count: 50,
			minimum_validator_count: 4,
			stakers: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, polkadot::StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
			force_era: Forcing::ForceNone,
			slash_reward_fraction: Perbill::from_percent(10),
			.. Default::default()
		}),
		democracy: Some(Default::default()),
		collective_Instance1: Some(polkadot::CouncilConfig {
			members: vec![],
			phantom: Default::default(),
		}),
		collective_Instance2: Some(polkadot::TechnicalCommitteeConfig {
			members: vec![],
			phantom: Default::default(),
		}),
		membership_Instance1: Some(Default::default()),
		babe: Some(Default::default()),
		grandpa: Some(Default::default()),
		im_online: Some(Default::default()),
		authority_discovery: Some(polkadot::AuthorityDiscoveryConfig {
			keys: vec![],
		}),
		parachains: Some(polkadot::ParachainsConfig {
			authorities: vec![],
		}),
		registrar: Some(polkadot::RegistrarConfig {
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
		sudo: Some(polkadot::SudoConfig {
			key: endowed_accounts[0].clone(),
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
		Default::default(),
	)
}

/// Helper function to generate a crypto pair from seed
pub fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}


/// Helper function to generate an account ID from seed
pub fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId where
	AccountPublic: From<<TPublic::Pair as Pair>::Public>
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}

/// Helper function to generate stash, controller and session key from seed
pub fn get_authority_keys_from_seed(seed: &str) -> (
	AccountId,
	AccountId,
	BabeId,
	GrandpaId,
	ImOnlineId,
	ValidatorId,
	AuthorityDiscoveryId
) {
	(
		get_account_id_from_seed::<sr25519::Public>(&format!("{}//stash", seed)),
		get_account_id_from_seed::<sr25519::Public>(seed),
		get_from_seed::<BabeId>(seed),
		get_from_seed::<GrandpaId>(seed),
		get_from_seed::<ImOnlineId>(seed),
		get_from_seed::<ValidatorId>(seed),
		get_from_seed::<AuthorityDiscoveryId>(seed),
	)
}

/// Helper function to create GenesisConfig for testing
pub fn testnet_genesis(
	initial_authorities: Vec<(AccountId, AccountId, BabeId, GrandpaId, ImOnlineId, ValidatorId, AuthorityDiscoveryId)>,
	root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> polkadot::GenesisConfig {
	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(|| {
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
	});

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
				session_keys(x.2.clone(), x.3.clone(), x.4.clone(), x.5.clone(), x.6.clone()),
			)).collect::<Vec<_>>(),
		}),
		staking: Some(polkadot::StakingConfig {
			current_era: 0,
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
		democracy: Some(polkadot::DemocracyConfig::default()),
		collective_Instance1: Some(polkadot::CouncilConfig {
			members: vec![],
			phantom: Default::default(),
		}),
		collective_Instance2: Some(polkadot::TechnicalCommitteeConfig {
			members: vec![],
			phantom: Default::default(),
		}),
		membership_Instance1: Some(Default::default()),
		babe: Some(Default::default()),
		grandpa: Some(Default::default()),
		im_online: Some(Default::default()),
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
		sudo: Some(polkadot::SudoConfig {
			key: root_key,
		}),
	}
}

fn development_config_genesis() -> polkadot::GenesisConfig {
	testnet_genesis(
		vec![
			get_authority_keys_from_seed("Alice"),
		],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
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
		Default::default(),
	)
}

fn local_testnet_genesis() -> polkadot::GenesisConfig {
	testnet_genesis(
		vec![
			get_authority_keys_from_seed("Alice"),
			get_authority_keys_from_seed("Bob"),
		],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
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
		Default::default(),
	)
}
