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
use bp_millau::derive_account_from_rialto_id;
use millau_runtime::{
	AccountId, AuraConfig, BalancesConfig, BeefyConfig, BridgeRialtoMessagesConfig,
	BridgeWestendGrandpaConfig, GenesisConfig, GrandpaConfig, SessionConfig, SessionKeys,
	Signature, SudoConfig, SystemConfig, WASM_BINARY,
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
pub fn get_authority_keys_from_seed(s: &str) -> (AccountId, AuraId, BeefyId, GrandpaId) {
	(
		get_account_id_from_seed::<sr25519::Public>(s),
		get_from_seed::<AuraId>(s),
		get_from_seed::<BeefyId>(s),
		get_from_seed::<GrandpaId>(s),
	)
}

impl Alternative {
	/// Get an actual chain config from one of the alternatives.
	pub(crate) fn load(self) -> ChainSpec {
		let properties = Some(
			serde_json::json!({
				"tokenDecimals": 9,
				"tokenSymbol": "MLAU"
			})
			.as_object()
			.expect("Map given; qed")
			.clone(),
		);
		match self {
			Alternative::Development => ChainSpec::from_genesis(
				"Millau Development",
				"millau_dev",
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
				None,
			),
			Alternative::LocalTestnet => ChainSpec::from_genesis(
				"Millau Local",
				"millau_local",
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
				None,
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
		get_account_id_from_seed::<sr25519::Public>("RialtoMessagesOwner"),
		get_account_id_from_seed::<sr25519::Public>("WithRialtoTokenSwap"),
		pallet_bridge_messages::relayer_fund_account_id::<
			bp_millau::AccountId,
			bp_millau::AccountIdConverter,
		>(),
		derive_account_from_rialto_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Alice"),
		)),
		derive_account_from_rialto_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Bob"),
		)),
		derive_account_from_rialto_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Charlie"),
		)),
		derive_account_from_rialto_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Dave"),
		)),
		derive_account_from_rialto_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Eve"),
		)),
		derive_account_from_rialto_id(bp_runtime::SourceAccount::Account(
			get_account_id_from_seed::<sr25519::Public>("Ferdie"),
		)),
	]
}

fn session_keys(aura: AuraId, beefy: BeefyId, grandpa: GrandpaId) -> SessionKeys {
	SessionKeys { aura, beefy, grandpa }
}

fn testnet_genesis(
	initial_authorities: Vec<(AccountId, AuraId, BeefyId, GrandpaId)>,
	root_key: AccountId,
	endowed_accounts: Vec<AccountId>,
	_enable_println: bool,
) -> GenesisConfig {
	GenesisConfig {
		system: SystemConfig {
			code: WASM_BINARY.expect("Millau development WASM not available").to_vec(),
		},
		balances: BalancesConfig {
			balances: endowed_accounts.iter().cloned().map(|k| (k, 1 << 50)).collect(),
		},
		aura: AuraConfig { authorities: Vec::new() },
		beefy: BeefyConfig { authorities: Vec::new() },
		grandpa: GrandpaConfig { authorities: Vec::new() },
		sudo: SudoConfig { key: Some(root_key) },
		session: SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(x.0.clone(), x.0.clone(), session_keys(x.1.clone(), x.2.clone(), x.3.clone()))
				})
				.collect::<Vec<_>>(),
		},
		bridge_westend_grandpa: BridgeWestendGrandpaConfig {
			// for our deployments to avoid multiple same-nonces transactions:
			// //Alice is already used to initialize Rialto<->Millau bridge
			// => let's use //George to initialize Westend->Millau bridge
			owner: Some(get_account_id_from_seed::<sr25519::Public>("George")),
			..Default::default()
		},
		bridge_rialto_messages: BridgeRialtoMessagesConfig {
			owner: Some(get_account_id_from_seed::<sr25519::Public>("RialtoMessagesOwner")),
			..Default::default()
		},
	}
}

#[test]
fn derived_dave_account_is_as_expected() {
	let dave = get_account_id_from_seed::<sr25519::Public>("Dave");
	let derived: AccountId =
		derive_account_from_rialto_id(bp_runtime::SourceAccount::Account(dave));
	assert_eq!(derived.to_string(), "5DNW6UVnb7TN6wX5KwXtDYR3Eccecbdzuw89HqjyNfkzce6J".to_string());
}
