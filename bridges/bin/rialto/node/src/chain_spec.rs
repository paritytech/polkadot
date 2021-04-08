// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

use bp_rialto::derive_account_from_millau_id;
use rialto_runtime::{
	AccountId, AuraConfig, BalancesConfig, BridgeKovanConfig, BridgeMillauConfig, BridgeRialtoPoAConfig, GenesisConfig,
	GrandpaConfig, SessionConfig, SessionKeys, Signature, SudoConfig, SystemConfig, WASM_BINARY,
};
use sp_consensus_aura::sr25519::AuthorityId as AuraId;
use sp_core::{sr25519, Pair, Public};
use sp_finality_grandpa::AuthorityId as GrandpaId;
use sp_runtime::traits::{IdentifyAccount, Verify};

/// Specialized `ChainSpec`. This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec<GenesisConfig>;

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

/// Helper function to generate an authority key for Aura
pub fn get_authority_keys_from_seed(s: &str) -> (AccountId, AuraId, GrandpaId) {
	(
		get_account_id_from_seed::<sr25519::Public>(s),
		get_from_seed::<AuraId>(s),
		get_from_seed::<GrandpaId>(s),
	)
}

impl Alternative {
	/// Get an actual chain config from one of the alternatives.
	pub(crate) fn load(self) -> ChainSpec {
		match self {
			Alternative::Development => ChainSpec::from_genesis(
				"Development",
				"dev",
				sc_service::ChainType::Development,
				|| {
					testnet_genesis(
						vec![get_authority_keys_from_seed("Alice")],
						get_account_id_from_seed::<sr25519::Public>("Alice"),
						vec![
							get_account_id_from_seed::<sr25519::Public>("Alice"),
							get_account_id_from_seed::<sr25519::Public>("Bob"),
							get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
							get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
						],
						true,
					)
				},
				vec![],
				None,
				None,
				None,
				None,
			),
			Alternative::LocalTestnet => ChainSpec::from_genesis(
				"Local Testnet",
				"local_testnet",
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
							pallet_message_lane::Module::<rialto_runtime::Runtime, pallet_message_lane::DefaultInstance>::relayer_fund_account_id(),
							derive_account_from_millau_id(bp_runtime::SourceAccount::Account(
								get_account_id_from_seed::<sr25519::Public>("Dave"),
							)),
						],
						true,
					)
				},
				vec![],
				None,
				None,
				None,
				None,
			),
		}
	}
}

fn session_keys(aura: AuraId, grandpa: GrandpaId) -> SessionKeys {
	SessionKeys { aura, grandpa }
}

fn testnet_genesis(
	initial_authorities: Vec<(AccountId, AuraId, GrandpaId)>,
	root_key: AccountId,
	endowed_accounts: Vec<AccountId>,
	_enable_println: bool,
) -> GenesisConfig {
	GenesisConfig {
		frame_system: Some(SystemConfig {
			code: WASM_BINARY.to_vec(),
			changes_trie_config: Default::default(),
		}),
		pallet_balances: Some(BalancesConfig {
			balances: endowed_accounts.iter().cloned().map(|k| (k, 1 << 50)).collect(),
		}),
		pallet_aura: Some(AuraConfig {
			authorities: Vec::new(),
		}),
		pallet_bridge_eth_poa_Instance1: Some(load_rialto_poa_bridge_config()),
		pallet_bridge_eth_poa_Instance2: Some(load_kovan_bridge_config()),
		pallet_grandpa: Some(GrandpaConfig {
			authorities: Vec::new(),
		}),
		pallet_substrate_bridge: Some(BridgeMillauConfig {
			// We'll initialize the pallet with a dispatchable instead.
			init_data: None,
			owner: Some(root_key.clone()),
		}),
		pallet_sudo: Some(SudoConfig { key: root_key }),
		pallet_session: Some(SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.0.clone(), session_keys(x.1.clone(), x.2.clone())))
				.collect::<Vec<_>>(),
		}),
	}
}

fn load_rialto_poa_bridge_config() -> BridgeRialtoPoAConfig {
	BridgeRialtoPoAConfig {
		initial_header: rialto_runtime::rialto_poa::genesis_header(),
		initial_difficulty: 0.into(),
		initial_validators: rialto_runtime::rialto_poa::genesis_validators(),
	}
}

fn load_kovan_bridge_config() -> BridgeKovanConfig {
	BridgeKovanConfig {
		initial_header: rialto_runtime::kovan::genesis_header(),
		initial_difficulty: 0.into(),
		initial_validators: rialto_runtime::kovan::genesis_validators(),
	}
}

#[test]
fn derived_dave_account_is_as_expected() {
	let dave = get_account_id_from_seed::<sr25519::Public>("Dave");
	let derived: AccountId = derive_account_from_millau_id(bp_runtime::SourceAccount::Account(dave));
	assert_eq!(
		derived.to_string(),
		"5HZhdv53gSJmWWtD8XR5Ypu4PgbT5JNWwGw2mkE75cN61w9t".to_string()
	);
}
