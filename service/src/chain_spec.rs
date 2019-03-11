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

use primitives::{Ed25519AuthorityId as AuthorityId, ed25519};
use polkadot_primitives::AccountId;
use polkadot_runtime::{
	GenesisConfig, ConsensusConfig, CouncilSeatsConfig, DemocracyConfig, TreasuryConfig,
	SessionConfig, StakingConfig, TimestampConfig, BalancesConfig, Perbill,
	CouncilVotingConfig, GrandpaConfig, SudoConfig, IndicesConfig, FeesConfig, Permill,
	CuratedGrandpaConfig, UpgradeKeyConfig,
};
use telemetry::TelemetryEndpoints;
use keystore::pad_seed;

const STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
const DEFAULT_PROTOCOL_ID: &str = "dot";

/// Specialised `ChainSpec`.
pub type ChainSpec = ::service::ChainSpec<GenesisConfig>;

pub fn poc_3_testnet_config() -> Result<ChainSpec, String> {
	ChainSpec::from_embedded(include_bytes!("../res/alexander.json"))
}

fn staging_testnet_config_genesis() -> GenesisConfig {
	let initial_authorities: Vec<(AccountId, AccountId, AuthorityId)> = vec![(
		hex!["863ab9379a9b78fe28368d794094bd576ce0c18536012605da7fc76e4f331faf"].into(), // 5F6hicQ1rnmQKy9q6yX9BUaBtQsfBLxAhrozQ8LrKxnPuBP7
		hex!["6d090ef6cde4dc1df057e8ce928a156b5793b461dc35eabbb8a17ee4bd41a576"].into(), // 5EXfmUBE1Vs13jz4tbApSRJqT3sEQsMWNQmHWeknc4Gy9CuU
		hex!["fba60a57436218519abf2dde5b5cadf02771c33833a27527ac4808c0690cfe72"].into(), // 5HkfCaoiFt41WSqEby4fkgRrXtxwfB9G8q8pPuoxDrsqGmFm
	),(
		hex!["46cf99718ddcf7868a1c7073862f6b821a58ac1ee57655b6a78b29559cf6fa9b"].into(), // 5DfYswB6NLVyvft5ggt6NJuX13X4VQjXqxuhha8C79ntPra9
		hex!["c4d4dae0d95c3c012322709e12f20ce154b9e04c23c75dce3fe63cd907c2f4a0"].into(), // 5GWnURnpMXqk5Yzfr8AJBE6fNM3AKmWikFMFvia8A7hGBPUT
		hex!["337c9a3f05221973d94995c9e9448c582e2ff3382c64f624318acc5164525244"].into(), // 5DEDKARKMZsecqU2s2onmg7ZxQDsZSYKTvxxHKMBMCU2UiCB
	),(
		hex!["6d14eced242492088e6ab054e6da3300b435de1fbab57e79103071cdc1dfff94"].into(), // 5EXjHw7GK6GEJjT7b9kGAPzQdCbrmjt27TYwVB1igN5uai3Y
		hex!["971da4fe7d20cd83c05186d699e3a0756b1772bc7c16309c71bab36dac0909e8"].into(), // 5FUqtohZwTyhY12GNxSb2ExhGtrEGB7B8nLv29mevbPep5zk
		hex!["79e9fd0469f6563bea8e2019c27c5c53a685675221ea4c7715c3f9fca75c6aa8"].into(), // 5EpZAFPsyuMEVKfdB1VtG6b2E11XS6T5ch646zq13xfmbsgS
	),(
		hex!["e2f736077b2a1522339f7a0dc90468ba2223c5f98fe82a0283138a3e48463f02"].into(), // 5HCJ7gu6kH5mYyp4DVDPGDmJf4Zf5zJgW5Swi3dizi8qq8vm
		hex!["91627d4b51c11c1b8b6d54dbe727219e4eccbea6a86599ebe25a316e8ba3bf15"].into(), // 5FML4K5Vz45nKvndwmnAGwv6CTKvacEK2Nr6UbGf2zJYxuwd
		hex!["08d9e438d2ccc88b66115e2e2f07b0cbdcf912e0f147124b39024735e6b58057"].into(), // 5CGJy52caRtJUjEnCK7gj7jYvBTfpffPqP443g4aM2GZt5ns
	)];
	let endowed_accounts = vec![
		hex!["f295940fa750df68a686fcf4abd4111c8a9c5a5a5a83c4c8639c451a94a7adfd"].into(),
	];
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
		consensus: Some(ConsensusConfig {
			// TODO: Change after Substrate 1252 is fixed (https://github.com/paritytech/substrate/issues/1252)
			code: include_bytes!("../../runtime/wasm/target/wasm32-unknown-unknown/release/polkadot_runtime.compact.wasm").to_vec(),
			authorities: initial_authorities.iter().map(|x| x.2.clone()).collect(),
		}),
		system: None,
		balances: Some(BalancesConfig {
			balances: endowed_accounts.iter()
				.map(|&k| (k, ENDOWMENT))
				.chain(initial_authorities.iter().map(|x| (x.0.clone(), STASH)))
				.collect(),
			existential_deposit: 1 * DOLLARS,
			transfer_fee: 1 * CENTS,
			creation_fee: 1 * CENTS,
			vesting: vec![],
		}),
		indices: Some(IndicesConfig {
			ids: endowed_accounts.iter().cloned()
				.chain(initial_authorities.iter().map(|x| x.0.clone()))
				.collect::<Vec<_>>(),
		}),
		session: Some(SessionConfig {
			validators: initial_authorities.iter().map(|x| x.1.into()).collect(),
			session_length: 5 * MINUTES,
			keys: initial_authorities.iter().map(|x| (x.1.clone(), x.2.clone())).collect::<Vec<_>>(),
		}),
		staking: Some(StakingConfig {
			current_era: 0,
			offline_slash: Perbill::from_billionths(1_000_000),
			session_reward: Perbill::from_billionths(2_065),
			current_offline_slash: 0,
			current_session_reward: 0,
			validator_count: 7,
			sessions_per_era: 12,
			bonding_duration: 60 * MINUTES,
			offline_slash_grace: 4,
			minimum_validator_count: 4,
			stakers: initial_authorities.iter().map(|x| (x.0.into(), x.1.into(), STASH)).collect(),
			invulnerables: initial_authorities.iter().map(|x| x.1.into()).collect(),
		}),
		democracy: Some(DemocracyConfig {
			launch_period: 10 * MINUTES,    // 1 day per public referendum
			voting_period: 10 * MINUTES,    // 3 days to discuss & vote on an active referendum
			minimum_deposit: 50 * DOLLARS,    // 12000 as the minimum deposit for a referendum
			public_delay: 10 * MINUTES,
			max_lock_periods: 6,
		}),
		council_seats: Some(CouncilSeatsConfig {
			active_council: vec![],
			candidacy_bond: 10 * DOLLARS,
			voter_bond: 1 * DOLLARS,
			present_slash_per_voter: 1 * CENTS,
			carry_count: 6,
			presentation_duration: 1 * DAYS,
			approval_voting_period: 2 * DAYS,
			term_duration: 28 * DAYS,
			desired_seats: 0,
			inactive_grace_period: 1,    // one additional vote should go by before an inactive voter can be reaped.
		}),
		council_voting: Some(CouncilVotingConfig {
			cooloff_period: 4 * DAYS,
			voting_period: 1 * DAYS,
			enact_delay_period: 0,
		}),
		timestamp: Some(TimestampConfig {
			period: SECS_PER_BLOCK / 2, // due to the nature of aura the slots are 2*period
		}),
		treasury: Some(TreasuryConfig {
			proposal_bond: Permill::from_percent(5),
			proposal_bond_minimum: 1 * DOLLARS,
			spend_period: 1 * DAYS,
			burn: Permill::from_percent(50),
		}),
		sudo: Some(SudoConfig {
			key: endowed_accounts[0].clone(),
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities.iter().map(|x| (x.2.clone(), 1)).collect(),
		}),
		fees: Some(FeesConfig {
			transaction_base_fee: 1 * CENTS,
			transaction_byte_fee: 10 * MILLICENTS,
		}),
		parachains: Some(Default::default()),
		upgrade_key: Some(UpgradeKeyConfig {
			key: endowed_accounts[0],
		}),
		curated_grandpa: Some(CuratedGrandpaConfig {
			shuffle_period: 1024,
		})
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

/// Helper function to generate AuthorityID from seed
pub fn get_account_id_from_seed(seed: &str) -> AccountId {
	let padded_seed = pad_seed(seed);
	// NOTE from ed25519 impl:
	// prefer pkcs#8 unless security doesn't matter -- this is used primarily for tests.
	ed25519::Pair::from_seed(&padded_seed).public().0.into()
}

/// Helper function to generate stash, controller and session key from seed
pub fn get_authority_keys_from_seed(seed: &str) -> (AccountId, AccountId, AuthorityId) {
	let padded_seed = pad_seed(seed);
	// NOTE from ed25519 impl:
	// prefer pkcs#8 unless security doesn't matter -- this is used primarily for tests.
	(
		get_account_id_from_seed(&format!("{}-stash", seed)),
		get_account_id_from_seed(seed),
		ed25519::Pair::from_seed(&padded_seed).public().0.into()
	)
}

/// Helper function to create GenesisConfig for testing
pub fn testnet_genesis(
	initial_authorities: Vec<(AccountId, AccountId, AuthorityId)>,
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
		]
	});

	const STASH: u128 = 1 << 20;
	const ENDOWMENT: u128 = 1 << 20;

	GenesisConfig {
		consensus: Some(ConsensusConfig {
			code: include_bytes!("../../runtime/wasm/target/wasm32-unknown-unknown/release/polkadot_runtime.compact.wasm").to_vec(),
			authorities: initial_authorities.iter().map(|x| x.2.clone()).collect(),
		}),
		system: None,
		indices: Some(IndicesConfig {
			ids: endowed_accounts.clone(),
		}),
		balances: Some(BalancesConfig {
			existential_deposit: 500,
			transfer_fee: 0,
			creation_fee: 0,
			balances: endowed_accounts.iter().map(|&k| (k.into(), ENDOWMENT)).collect(),
			vesting: vec![],
		}),
		session: Some(SessionConfig {
			validators: initial_authorities.iter().map(|x| x.1.into()).collect(),
			session_length: 10,
			keys: initial_authorities.iter().map(|x| (x.1.clone(), x.2.clone())).collect::<Vec<_>>(),
		}),
		staking: Some(StakingConfig {
			current_era: 0,
			minimum_validator_count: 1,
			validator_count: 2,
			sessions_per_era: 5,
			bonding_duration: 2 * 60 * 12,
			offline_slash: Perbill::zero(),
			session_reward: Perbill::zero(),
			current_offline_slash: 0,
			current_session_reward: 0,
			offline_slash_grace: 0,
			stakers: initial_authorities.iter().map(|x| (x.0.into(), x.1.into(), STASH)).collect(),
			invulnerables: initial_authorities.iter().map(|x| x.1.into()).collect(),
		}),
		democracy: Some(DemocracyConfig {
			launch_period: 9,
			voting_period: 18,
			minimum_deposit: 10,
			public_delay: 0,
			max_lock_periods: 6,
		}),
		council_seats: Some(CouncilSeatsConfig {
			active_council: endowed_accounts.iter()
				.filter(|&endowed| initial_authorities.iter().find(|&(_, controller, _)| controller == endowed).is_none())
				.map(|a| (a.clone().into(), 1000000)).collect(),
			candidacy_bond: 10,
			voter_bond: 2,
			present_slash_per_voter: 1,
			carry_count: 4,
			presentation_duration: 10,
			approval_voting_period: 20,
			term_duration: 1000000,
			desired_seats: (endowed_accounts.len() - initial_authorities.len()) as u32,
			inactive_grace_period: 1,
		}),
		council_voting: Some(CouncilVotingConfig {
			cooloff_period: 75,
			voting_period: 20,
			enact_delay_period: 0,
		}),
		parachains: Some(Default::default()),
		timestamp: Some(TimestampConfig {
			period: 2,                    // 2*2=4 second block time.
		}),
		treasury: Some(TreasuryConfig {
			proposal_bond: Permill::from_percent(5),
			proposal_bond_minimum: 1_000_000,
			spend_period: 12 * 60 * 24,
			burn: Permill::from_percent(50),
		}),
		sudo: Some(SudoConfig {
			key: root_key,
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities.iter().map(|x| (x.2.clone(), 1)).collect(),
		}),
		fees: Some(FeesConfig {
			transaction_base_fee: 1,
			transaction_byte_fee: 0,
		}),
		curated_grandpa: Some(CuratedGrandpaConfig {
			shuffle_period: 1024,
		}),
		upgrade_key: Some(UpgradeKeyConfig {
			key: root_key,
		}),
	}
}


fn development_config_genesis() -> GenesisConfig {
	testnet_genesis(
		vec![
			get_authority_keys_from_seed("Alice"),
		],
		get_account_id_from_seed("Alice").into(),
		None,
	)
}

/// Development config (single validator Alice)
pub fn development_config() -> ChainSpec {
	ChainSpec::from_genesis("Development", "dev", development_config_genesis, vec![], None, Some(DEFAULT_PROTOCOL_ID), None, None)
}

fn local_testnet_genesis() -> GenesisConfig {
	testnet_genesis(
		vec![
			get_authority_keys_from_seed("Alice"),
			get_authority_keys_from_seed("Bob"),
		],
		get_account_id_from_seed("Alice").into(),
		None,
	)
}

/// Local testnet config (multivalidator Alice + Bob)
pub fn local_testnet_config() -> ChainSpec {
	ChainSpec::from_genesis("Local Testnet", "local_testnet", local_testnet_genesis, vec![], None, Some(DEFAULT_PROTOCOL_ID), None, None)
}
