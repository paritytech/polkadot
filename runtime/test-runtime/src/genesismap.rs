// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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
use sp_io::hashing::blake2_256;
use super::{AccountId, WASM_BINARY};
use codec::{Encode, KeyedVec, Joiner};
use sp_core::ChangesTrieConfiguration;
use sp_core::storage::{well_known_keys, Storage};
use sp_runtime::BuildStorage;

/// Configuration of a general Substrate test genesis block.
pub struct GenesisConfig {
	changes_trie_config: Option<ChangesTrieConfiguration>,
	balances: Vec<(AccountId, u64)>,
	heap_pages_override: Option<u64>,
	/// Additional storage key pairs that will be added to the genesis map.
	extra_storage: Storage,
}

impl GenesisConfig {
	pub fn new(
		changes_trie_config: Option<ChangesTrieConfiguration>,
		endowed_accounts: Vec<AccountId>,
		balance: u64,
		heap_pages_override: Option<u64>,
		extra_storage: Storage,
	) -> Self {
		GenesisConfig {
			changes_trie_config,
			balances: endowed_accounts.into_iter().map(|a| (a, balance)).collect(),
			heap_pages_override,
			extra_storage,
		}
	}

	pub fn genesis_map(&self) -> Storage {
		let wasm_runtime = WASM_BINARY.to_vec();
		let mut map: BTreeMap<Vec<u8>, Vec<u8>> = self.balances.iter()
			.map(|&(ref account, balance)| (account.to_keyed_vec(b"balance:"), vec![].and(&balance)))
			.map(|(k, v)| (blake2_256(&k[..])[..].to_vec(), v.to_vec()))
			.chain(vec![
				(well_known_keys::CODE.into(), wasm_runtime),
				(
					well_known_keys::HEAP_PAGES.into(),
					vec![].and(&(self.heap_pages_override.unwrap_or(16 as u64))),
				),
			].into_iter())
			.collect();
		if let Some(ref changes_trie_config) = self.changes_trie_config {
			map.insert(well_known_keys::CHANGES_TRIE_CONFIG.to_vec(), changes_trie_config.encode());
		}
		// Add the extra storage entries.
		map.extend(self.extra_storage.top.clone().into_iter());

		// Assimilate the system genesis config.
		let mut storage = Storage { top: map, children: self.extra_storage.children.clone()};
		let config = crate::GenesisConfig {
            system: None,
            babe: None,
            indices: None,
            balances: None,
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
