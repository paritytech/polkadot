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

use primitives::{H256, Ed25519AuthorityId as AuthorityId, ed25519};
use polkadot_runtime::{
	GenesisConfig, ConsensusConfig, CouncilSeatsConfig, DemocracyConfig, TreasuryConfig,
	SessionConfig, StakingConfig, TimestampConfig, BalancesConfig, Perbill,
	CouncilVotingConfig, GrandpaConfig, UpgradeKeyConfig, SudoConfig, IndicesConfig,
	Permill
};

const STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
const DEFAULT_PROTOCOL_ID: &str = "dot";

/// Specialised `ChainSpec`.
pub type ChainSpec = ::service::ChainSpec<GenesisConfig>;

pub fn poc_3_testnet_config() -> Result<ChainSpec, String> {
	ChainSpec::from_embedded(include_bytes!("../res/alexander.json"))
}

fn staging_testnet_config_genesis() -> GenesisConfig {
	let initial_authorities = vec![
		hex!["4bd3620064cda1f4cf405bf9ab565c9bad69446034c48884ffc5363a5286b145"].into(),
		hex!["3a92077b16fbb87972be7ebaf1b7e70f5b4fac9636c136936a28d0fb494d1ed4"].into(),
		hex!["ca8feb6f870330cdaea24e49c2f850b66729340cab164aea86c0a782ddecf57a"].into(),
		hex!["dcb83e46917c3c0ca35b9a18a32ba6d3912b6d50ab2bd382341d2e4fd2e6946f"].into(),
	];
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
	GenesisConfig {
		consensus: Some(ConsensusConfig {
			code: include_bytes!("../../runtime/wasm/target/wasm32-unknown-unknown/release/polkadot_runtime.compact.wasm").to_vec(),	// TODO change
			authorities: initial_authorities.clone(),
		}),
		system: None,
		indices: Some(IndicesConfig {
			ids: endowed_accounts.clone(),
		}),
		balances: Some(BalancesConfig {
			balances: endowed_accounts.iter().map(|&k| (k, 10_000_000 * DOLLARS)).collect(),
			transaction_base_fee: 1 * CENTS,
			transaction_byte_fee: 10 * MILLICENTS,
			existential_deposit: 1 * DOLLARS,
			transfer_fee: 1 * CENTS,
			creation_fee: 1 * CENTS,
		}),
		session: Some(SessionConfig {
			validators: initial_authorities.iter().cloned().map(Into::into).collect(),
			session_length: 5 * MINUTES,
		}),
		staking: Some(StakingConfig {
			current_era: 0,
			intentions: initial_authorities.iter().cloned().map(Into::into).collect(),
			offline_slash: Perbill::from_billionths(1_000_000),
			session_reward: Perbill::from_billionths(2_065),
			current_offline_slash: 0,
			current_session_reward: 0,
			validator_count: 7,
			sessions_per_era: 12,
			bonding_duration: 60 * MINUTES,
			offline_slash_grace: 4,
			minimum_validator_count: 4,
			invulnerables: initial_authorities.iter().cloned().map(Into::into).collect(),
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
		parachains: Some(Default::default()),
		upgrade_key: Some(UpgradeKeyConfig {
			key: endowed_accounts[0],
		}),
		sudo: Some(SudoConfig {
			key: endowed_accounts[0],
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities.clone().into_iter().map(|k| (k, 1)).collect(),
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
		Some(STAGING_TELEMETRY_URL.into()),
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
	)
}

fn testnet_genesis(initial_authorities: Vec<AuthorityId>, upgrade_key: H256) -> GenesisConfig {
	let endowed_accounts = vec![
		ed25519::Pair::from_seed(b"Alice                           ").public().0.into(),
		ed25519::Pair::from_seed(b"Bob                             ").public().0.into(),
		ed25519::Pair::from_seed(b"Charlie                         ").public().0.into(),
		ed25519::Pair::from_seed(b"Dave                            ").public().0.into(),
		ed25519::Pair::from_seed(b"Eve                             ").public().0.into(),
		ed25519::Pair::from_seed(b"Ferdie                          ").public().0.into(),
	];
	GenesisConfig {
		consensus: Some(ConsensusConfig {
			code: include_bytes!("../../runtime/wasm/target/wasm32-unknown-unknown/release/polkadot_runtime.compact.wasm").to_vec(),
			authorities: initial_authorities.clone(),
		}),
		system: None,
		indices: Some(IndicesConfig {
			ids: endowed_accounts.clone(),
		}),
		balances: Some(BalancesConfig {
			transaction_base_fee: 1,
			transaction_byte_fee: 0,
			existential_deposit: 500,
			transfer_fee: 0,
			creation_fee: 0,
			balances: endowed_accounts.iter().map(|&k|(k, (1u128 << 60))).collect(),
		}),
		session: Some(SessionConfig {
			validators: initial_authorities.iter().cloned().map(Into::into).collect(),
			session_length: 10,
		}),
		staking: Some(StakingConfig {
			current_era: 0,
			intentions: initial_authorities.iter().cloned().map(Into::into).collect(),
			minimum_validator_count: 1,
			validator_count: 2,
			sessions_per_era: 5,
			bonding_duration: 2 * 60 * 12,
			offline_slash: Perbill::zero(),
			session_reward: Perbill::zero(),
			current_offline_slash: 0,
			current_session_reward: 0,
			offline_slash_grace: 0,
			invulnerables: initial_authorities.iter().cloned().map(Into::into).collect(),
		}),
		democracy: Some(DemocracyConfig {
			launch_period: 9,
			voting_period: 18,
			minimum_deposit: 10,
			public_delay: 10 * 60,
			max_lock_periods: 6,
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities.clone().into_iter().map(|k| (k, 1)).collect(),
		}),
		council_seats: Some(CouncilSeatsConfig {
			active_council: endowed_accounts.iter().filter(|a| initial_authorities.iter().find(|&b| a[..] == b.0).is_none()).map(|a| (a.clone(), 1000000)).collect(),
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
			period: 2,					// 2*2=4 second block time.
		}),
		treasury: Some(Default::default()),
		upgrade_key: Some(UpgradeKeyConfig {
			key: upgrade_key,
		}),
		sudo: Some(SudoConfig {
			key: upgrade_key,
		}),
	}
}

fn development_config_genesis() -> GenesisConfig {
	testnet_genesis(
		vec![
			ed25519::Pair::from_seed(b"Alice                           ").public().into(),
		],
		ed25519::Pair::from_seed(b"Alice                           ").public().0.into()
	)
}

/// Development config (single validator Alice)
pub fn development_config() -> ChainSpec {
	ChainSpec::from_genesis(
		"Development",
		"development",
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
			ed25519::Pair::from_seed(b"Alice                           ").public().into(),
			ed25519::Pair::from_seed(b"Bob                             ").public().into(),
		],
		ed25519::Pair::from_seed(b"Alice                           ").public().0.into()
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
