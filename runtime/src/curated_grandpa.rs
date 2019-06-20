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

//! A module for manually curated GRANDPA set.

use {grandpa, system};
use parity_codec::Decode;
use sr_primitives::traits::{Hash as HashT, BlakeTwo256, Zero};
use rstd::prelude::*;
use srml_support::{decl_storage, decl_module};

pub trait Trait: grandpa::Trait {}

decl_storage! {
	trait Store for Module<T: Trait> as CuratedGrandpa {
		/// How often to shuffle the GRANDPA sets.
		///
		/// 0 means never.
		pub ShufflePeriod get(shuffle_period) config(shuffle_period): T::BlockNumber;
	}
}

decl_module! {
	/// curated GRANDPA set.
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		/// Changes the GRANDPA voter set.
		fn set_voters(origin, voters: Vec<(grandpa::AuthorityId, u64)>) {
			system::ensure_root(origin)?;
			grandpa::Module::<T>::schedule_change(voters, T::BlockNumber::zero(), None)?;
		}

		fn on_finalize(block_number: T::BlockNumber) {
			let shuffle_period = Self::shuffle_period();

			// every so often shuffle the voters and issue a change.
			if shuffle_period.is_zero() { return }

			if (block_number % shuffle_period).is_zero() {
				let mut voters = grandpa::Module::<T>::grandpa_authorities();
				let voter_count = voters.len();

				if voter_count == 0 { return }

				let mut seed = {
					let phrase = b"grandpa_shuffling";
					let seed = system::Module::<T>::random(&phrase[..]);
					let seed_len = seed.as_ref().len();
					let needed_bytes = voter_count * 4;

					// hash only the needed bits of the random seed.
					// if earlier bits are influencable, they will not factor into
					// the seed used here.
					let seed_off = if needed_bytes >= seed_len {
						0
					} else {
						seed_len - needed_bytes
					};

					BlakeTwo256::hash(&seed.as_ref()[seed_off..])
				};

				for i in 0..(voter_count - 1) {
					// 4 bytes of entropy used per cycle, 32 bytes entropy per hash
					let offset = (i * 4 % 32) as usize;

					// number of roles remaining to select from.
					let remaining = (voter_count - i) as usize;

					// 8 32-bit ints per 256-bit seed.
					let voter_index = u32::decode(&mut &seed[offset..offset + 4]).expect("using 4 bytes for a 32-bit quantity") as usize % remaining;

					if offset == 28 {
						// into the last 4 bytes - rehash to gather new entropy
						seed = BlakeTwo256::hash(seed.as_ref());
					}

					// exchange last item with randomly chosen first.
					voters.swap(remaining - 1, voter_index);
				}

				// finalization order is undefined, so grandpa's on_finalize might
				// have already been called. calling it again is OK though.
				let _ = grandpa::Module::<T>::schedule_change(voters, T::BlockNumber::zero(), None);
				grandpa::Module::<T>::on_finalize(block_number);
			}
		}
	}
}
