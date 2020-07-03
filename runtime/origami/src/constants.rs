//! Helper module to build a genesis configuration for the super-runtime

use super::{
	AccountId,
	BabeConfig,
	BalancesConfig,
	GenesisConfig,
	GrandpaConfig,
	Signature,
	SudoConfig,
	SystemConfig,
	WASM_BINARY,
};
use sp_consensus_babe::AuthorityId as BabeId;
use sp_core::{sr25519, Pair, Public};
use sp_finality_grandpa::AuthorityId as GrandpaId;
use sp_runtime::traits::{IdentifyAccount, Verify};

/// Helper function to generate a crypto pair from seed
fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

type AccountPublic = <Signature as Verify>::Signer;

/// Helper function to generate an account ID from seed
pub fn account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
where
	AccountPublic: From<<TPublic::Pair as Pair>::Public>,
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}

/// Helper function to generate session key from seed
pub fn authority_keys_from_seed(seed: &str) -> (BabeId, GrandpaId) {
	(
		get_from_seed::<BabeId>(seed),
		get_from_seed::<GrandpaId>(seed),
	)
}

pub fn dev_genesis() -> GenesisConfig {
	testnet_genesis(
		// Initial Authorities
		vec![authority_keys_from_seed("Alice")],
		// Root Key
		account_id_from_seed::<sr25519::Public>("Alice"),
		// Endowed Accounts
		vec![
			account_id_from_seed::<sr25519::Public>("Alice"),
			account_id_from_seed::<sr25519::Public>("Bob"),
			account_id_from_seed::<sr25519::Public>("Alice//stash"),
			account_id_from_seed::<sr25519::Public>("Bob//stash"),
		],
	)
}

/// Helper function to build a genesis configuration
pub fn testnet_genesis(
	initial_authorities: Vec<(BabeId, GrandpaId)>,
	root_key: AccountId,
	endowed_accounts: Vec<AccountId>,
) -> GenesisConfig {
	GenesisConfig {
		system: Some(SystemConfig {
			code: WASM_BINARY.to_vec(),
			changes_trie_config: Default::default(),
		}),
		balances: Some(BalancesConfig {
			balances: endowed_accounts
				.iter()
				.cloned()
				.map(|k| (k, 1 << 60))
				.collect(),
		}),
		sudo: Some(SudoConfig { key: root_key }),
		babe: Some(BabeConfig {
			authorities: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), 1))
				.collect(),
		}),
		grandpa: Some(GrandpaConfig {
			authorities: initial_authorities
				.iter()
				.map(|x| (x.1.clone(), 1))
				.collect(),
		}),
	}
}
