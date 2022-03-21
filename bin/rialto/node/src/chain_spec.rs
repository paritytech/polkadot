// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use beefy_primitives::crypto::AuthorityId as BeefyId;
use bp_rialto::derive_account_from_millau_id;
use polkadot_primitives::v2::{AssignmentId, ValidatorId};
use rialto_runtime::{
	AccountId, BabeConfig, BalancesConfig, BeefyConfig, BridgeMillauMessagesConfig,
	ConfigurationConfig, GenesisConfig, GrandpaConfig, SessionConfig, SessionKeys, Signature,
	SudoConfig, SystemConfig, WASM_BINARY,
};
use serde_json::json;
use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;
use sp_consensus_babe::AuthorityId as BabeId;
use sp_core::{sr25519, Pair, Public};
use sp_finality_grandpa::AuthorityId as GrandpaId;
use sp_runtime::traits::{IdentifyAccount, Verify};

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec =
	sc_service::GenericChainSpec<GenesisConfig, polkadot_service::chain_spec::Extensions>;

/// The chain specification option. This is expected to come in from the CLI and
/// is little more than one of a number of alternatives which can easily be converted
/// from a string (`--chain=...`) into a `ChainSpec`.
#[derive(Clone, Debug)]
pub enum Alternative {
	/// Whatever the current runtime is, with just Alice as an auth.
	Development,
	/// Whatever the current runtime is, with simple Alice/Bob/Charlie/Dave/Eve auths.
	LocalTestnet,
}

/// Helper function to generate a crypto pair from seed
pub fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

type AccountPublic = <Signature as Verify>::Signer;

/// Helper function to generate an account ID from seed
pub fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
where
	AccountPublic: From<<TPublic::Pair as Pair>::Public>,
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}

/// Helper function to generate authority keys.
pub fn get_authority_keys_from_seed(
	s: &str,
) -> (AccountId, BabeId, BeefyId, GrandpaId, ValidatorId, AssignmentId, AuthorityDiscoveryId) {
	(
		get_account_id_from_seed::<sr25519::Public>(s),
		get_from_seed::<BabeId>(s),
		get_from_seed::<BeefyId>(s),
		get_from_seed::<GrandpaId>(s),
		get_from_seed::<ValidatorId>(s),
		get_from_seed::<AssignmentId>(s),
		get_from_seed::<AuthorityDiscoveryId>(s),
	)
}

impl Alternative {
	/// Get an actual chain config from one of the alternatives.
	pub(crate) fn load(self) -> ChainSpec {
		let properties = Some(
			json!({
				"tokenDecimals": 9,
				"tokenSymbol": "RLT"
			})
			.as_object()
			.expect("Map given; qed")
			.clone(),
		);
		match self {
			Alternative::Development => ChainSpec::from_genesis(
				"Rialto Development",
				"rialto_dev",
				sc_service::ChainType::Development,
				|| {
					testnet_genesis(
						vec![get_authority_keys_from_seed("Alice")],
						get_account_id_from_seed::<sr25519::Public>("Alice"),
						endowed_accounts(),
						true,
					)
				},
				vec![],
				None,
				None,
				None,
				properties,
				Default::default(),
			),
			Alternative::LocalTestnet => ChainSpec::from_genesis(
				"Rialto Local",
				"rialto_local",
				sc_service::ChainType::Local,
				|| {
					testnet_genesis(
						vec![
							get_authority_keys_from_seed("Alice"),
							get_authority_keys_from_seed("Bob"),
							get_authority_keys_from_seed("Charlie"),
							get_authority_keys_from_seed("Dave"),
							get_authority_keys_from_seed("Eve"),
						],
						get_account_id_from_seed::<sr25519::Public>("Alice"),
						endowed_accounts(),
						true,
					)
				},
				vec![],
				None,
				None,
				None,
				properties,
				Default::default(),
			),
		}
	}
}

/// We're using the same set of endowed accounts on all Millau chains (dev/local) to make
/// sure that all accounts, required for bridge to be functional (e.g. relayers fund account,
/// accounts used by relayers in our test deployments, accounts used for demonstration
/// purposes), are all available on these chains.
fn endowed_accounts() -> Vec<AccountId> {
	vec![
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		get_account_id_from_seed::<sr25519::Public>("Bob"),
		get_account_id_from_seed::<sr25519::Public>("Charlie"),
		get_account_id_from_seed::<sr25519::Public>("Dave"),
		get_account_id_from_seed::<sr25519::Public>("Eve"),
		get_account_id_from_seed::<sr25519::Public>("Ferdie"),
		get_account_id_from_seed::<sr25519::Public>("George"),
		get_account_id_from_seed::<sr25519::Public>("Harry"),
		get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
		get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
		get_account_id_from_seed::<sr25519::Public>("Charlie//stash"),
		get_account_id_from_seed::<sr25519::Public>("Dave//stash"),
		get_account_id_from_seed::<sr25519::Public>("Eve//stash"),
		get_account_id_from_seed::<sr25519::Public>("Ferdie//stash"),
		get_account_id_from_seed::<sr25519::Public>("George//stash"),
		get_account_id_from_seed::<sr25519::Public>("Harry//stash"),
		get_account_id_from_seed::<sr25519::Public>("MillauMessagesOwner"),
		get_account_id_from_seed::<sr25519::Public>("WithMillauTokenSwap"),
		pallet_bridge_messages::relayer_fund_account_id::<
			bp_rialto::AccountId,
			bp_rialto::AccountIdConverter,
		>(),
		derive_account_from_millau_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Alice"),
		)),
		derive_account_from_millau_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Bob"),
		)),
		derive_account_from_millau_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Charlie"),
		)),
		derive_account_from_millau_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Dave"),
		)),
		derive_account_from_millau_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Eve"),
		)),
		derive_account_from_millau_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Ferdie"),
		)),
	]
}

fn session_keys(
	babe: BabeId,
	beefy: BeefyId,
	grandpa: GrandpaId,
	para_validator: ValidatorId,
	para_assignment: AssignmentId,
	authority_discovery: AuthorityDiscoveryId,
) -> SessionKeys {
	SessionKeys { babe, beefy, grandpa, para_validator, para_assignment, authority_discovery }
}

fn testnet_genesis(
	initial_authorities: Vec<(
		AccountId,
		BabeId,
		BeefyId,
		GrandpaId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
	)>,
	root_key: AccountId,
	endowed_accounts: Vec<AccountId>,
	_enable_println: bool,
) -> GenesisConfig {
	GenesisConfig {
		system: SystemConfig {
			code: WASM_BINARY.expect("Rialto development WASM not available").to_vec(),
		},
		balances: BalancesConfig {
			balances: endowed_accounts.iter().cloned().map(|k| (k, 1 << 50)).collect(),
		},
		babe: BabeConfig {
			authorities: Vec::new(),
			epoch_config: Some(rialto_runtime::BABE_GENESIS_EPOCH_CONFIG),
		},
		beefy: BeefyConfig { authorities: Vec::new() },
		grandpa: GrandpaConfig { authorities: Vec::new() },
		sudo: SudoConfig { key: Some(root_key) },
		session: SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						session_keys(
							x.1.clone(),
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		authority_discovery: Default::default(),
		hrmp: Default::default(),
		// this configuration is exact copy of configuration from Polkadot repo
		// (see /node/service/src/chain_spec.rs:default_parachains_host_configuration)
		configuration: ConfigurationConfig {
			config: polkadot_runtime_parachains::configuration::HostConfiguration {
				validation_upgrade_cooldown: 2u32,
				validation_upgrade_delay: 2,
				code_retention_period: 1200,
				max_code_size: polkadot_primitives::v2::MAX_CODE_SIZE,
				max_pov_size: polkadot_primitives::v2::MAX_POV_SIZE,
				max_head_data_size: 32 * 1024,
				group_rotation_frequency: 20,
				chain_availability_period: 4,
				thread_availability_period: 4,
				max_upward_queue_count: 8,
				max_upward_queue_size: 1024 * 1024,
				max_downward_message_size: 1024 * 1024,
				ump_service_total_weight: 100_000_000_000,
				max_upward_message_size: 50 * 1024,
				max_upward_message_num_per_candidate: 5,
				hrmp_sender_deposit: 0,
				hrmp_recipient_deposit: 0,
				hrmp_channel_max_capacity: 8,
				hrmp_channel_max_total_size: 8 * 1024,
				hrmp_max_parachain_inbound_channels: 4,
				hrmp_max_parathread_inbound_channels: 4,
				hrmp_channel_max_message_size: 1024 * 1024,
				hrmp_max_parachain_outbound_channels: 4,
				hrmp_max_parathread_outbound_channels: 4,
				hrmp_max_message_num_per_candidate: 5,
				dispute_period: 6,
				no_show_slots: 2,
				n_delay_tranches: 25,
				needed_approvals: 2,
				relay_vrf_modulo_samples: 2,
				zeroth_delay_tranche_width: 0,
				minimum_validation_upgrade_delay: 5,
				..Default::default()
			},
		},
		paras: Default::default(),
		bridge_millau_messages: BridgeMillauMessagesConfig {
			owner: Some(get_account_id_from_seed::<sr25519::Public>("MillauMessagesOwner")),
			..Default::default()
		},
	}
}

#[test]
fn derived_dave_account_is_as_expected() {
	let dave = get_account_id_from_seed::<sr25519::Public>("Dave");
	let derived: AccountId =
		derive_account_from_millau_id(bp_runtime::SourceAccount::Account(dave));
	assert_eq!(derived.to_string(), "5HZhdv53gSJmWWtD8XR5Ypu4PgbT5JNWwGw2mkE75cN61w9t".to_string());
}
