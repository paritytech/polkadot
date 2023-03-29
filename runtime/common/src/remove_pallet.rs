// Copyright 2017-2023 Parity Technologies (UK) Ltd.
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
// along with Polkadot. If not, see <http://www.gnu.org/licenses/>.

//! `RemovePallet` implements `OnRuntimeUpgrade` and deletes all storage items of a given pallet.
//!
//! This can be useful for cleaning up the storage of a pallet which has been removed from the
//! runtime.
//!
//! WARNING: `RemovePallet` has no guard rails preventing it from bricking the chain if the
//! operation of removing storage for the given pallet would exceed the block weight limit.
//!
//! If your pallet has too many keys to be removed in a single block, it is advised to wait for
//! a multi-block scheduler currently under development which will allow for removal of storage
//! items (and performing other heavy migrations) over multiple blocks.
//! (https://github.com/paritytech/substrate/issues/13690)

use frame_support::weights::constants::RocksDbWeight;
use sp_core::Get;
use sp_io::hashing::twox_128;
use sp_std::{marker::PhantomData, vec::Vec};

pub struct RemovePallet<P: Get<&'static str>>(PhantomData<P>);
impl<P: Get<&'static str>> RemovePallet<P> {
	fn clear_keys(dry_run: bool) -> (u64, frame_support::weights::Weight) {
		let prefix = twox_128(P::get().as_bytes());
		let mut current = prefix.clone().to_vec();
		let mut counter = 0;
		while let Some(next) = sp_io::storage::next_key(&current[..]) {
			if !next.starts_with(&prefix) {
				break
			}
			if !dry_run {
				sp_io::storage::clear(&next);
			}
			counter += 1;
			current = next;
		}
		// Extra read for initial prefix read.
		(counter, RocksDbWeight::get().reads_writes(counter + 1, counter))
	}
}
impl<P: Get<&'static str>> frame_support::traits::OnRuntimeUpgrade for RemovePallet<P> {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		Self::clear_keys(false).1
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
		let (key_count, _) = Self::clear_keys(true);
		if key_count > 0 {
			log::info!("Found {} keys for pallet {} pre-removal 👀", key_count, P::get());
		} else {
			log::warn!("No keys found for pallet {} pre-removal ⚠️", P::get());
		}
		Ok(Vec::new())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(_state: Vec<u8>) -> Result<(), &'static str> {
		let (key_count, _) = Self::clear_keys(true);
		if key_count > 0 {
			log::error!("{} pallet {} keys remaining post-removal ❗", key_count, P::get());
		} else {
			log::info!("No {} keys remaining post-removal 🎉", P::get())
		}
		Ok(())
	}
}
