// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Tool for creating the genesis block.

use std::collections::BTreeMap;
use super::{AccountId, WASM_BINARY, constants::currency};
use sp_core::ChangesTrieConfiguration;
use sp_core::storage::Storage;
use sp_runtime::BuildStorage;

/// Configuration of a general Substrate test genesis block.
pub struct GenesisConfig {
	changes_trie_config: Option<ChangesTrieConfiguration>,
	balances: Vec<(AccountId, u128)>,
	/// Additional storage key pairs that will be added to the genesis map.
	extra_storage: Storage,
}

impl GenesisConfig {
	pub fn new(
		changes_trie_config: Option<ChangesTrieConfiguration>,
		endowed_accounts: Vec<AccountId>,
		balance: u128,
		extra_storage: Storage,
	) -> Self {
		GenesisConfig {
			changes_trie_config,
			balances: endowed_accounts.into_iter().map(|a| (a, balance * currency::DOLLARS)).collect(),
			extra_storage,
		}
	}

	pub fn genesis_map(&self) -> Storage {
		// Assimilate the system genesis config.
		let mut storage = Storage {
			top: BTreeMap::new(),
			children_default: self.extra_storage.children_default.clone(),
		};
		let config = crate::GenesisConfig {
			system: Some(system::GenesisConfig {
				changes_trie_config: self.changes_trie_config.clone(),
				code: WASM_BINARY.to_vec(),
			}),
			babe: None,
			indices: None,
			balances: Some(balances::GenesisConfig {
				balances: self.balances.clone()
			}),
			staking: None,
			session: None,
			grandpa: None,
			claims: None,
			parachains: None,
			registrar: None,
			vesting: None,
		};
		config.assimilate_storage(&mut storage).expect("Adding `system::GensisConfig` to the genesis");

		storage
	}
}
