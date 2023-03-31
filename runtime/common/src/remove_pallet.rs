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

use frame_support::{storage::unhashed::contains_prefixed_key, weights::constants::RocksDbWeight};
use sp_core::Get;
use sp_io::{hashing::twox_128, storage::clear_prefix, KillStorageResult};
use sp_std::{marker::PhantomData, vec::Vec};

/// Implements a definition of `OnRuntimeUpgrade` which when executed removes all storage items
/// of a specified pallet.
///
/// This can be useful for cleaning up the storage of a pallet which has been removed from the
/// runtime.
///
/// It takes one generic parameter `P`, which is the name of the pallet to remove.
/// `P` can be defined using the `parameter_types!` macro.
///
/// Example usage:
/// ```
/// construct_runtime! {
/// 	pub enum Runtime where
/// 		Block = Block,
///		NodeBlock = primitives::Block,
/// 		UncheckedExtrinsic = UncheckedExtrinsic
/// 	{
/// 		System: frame_system::{Pallet, Call, Storage, Config, Event<T>} = 0,
///
/// 		SomePalletToRemove: pallet_something::{Pallet, Call, Storage, Event<T>} = 1,
/// 		AnotherPalletToRemove: pallet_something_else::{Pallet, Call, Storage, Event<T>} = 2,
///
/// 		YourOtherPallets...
/// 	}
/// };
///
/// parameter_types! {
///		pub const SomePalletToRemoveStr: &'static str = "SomePalletToRemove";
///		pub const AnotherPalletToRemoveStr: &'static str = "AnotherPalletToRemove";
/// }
///
/// pub type Migrations = (
/// 	RemovePallet<SomePalletToRemoveStr>,
/// 	RemovePallet<AnotherPalletToRemoveStr>,
/// 	AnyOtherMigrations...
/// );
///
/// pub type Executive = frame_executive::Executive<
/// 	Runtime,
/// 	Block,
/// 	frame_system::ChainContext<Runtime>,
/// 	Runtime,
/// 	Migrations
/// >;
/// ```
///
/// WARNING: `RemovePallet` has no guard rails preventing it from bricking the chain if the
/// operation of removing storage for the given pallet would exceed the block weight limit.
///
/// If your pallet has too many keys to be removed in a single block, it is advised to wait for
/// a multi-block scheduler currently under development which will allow for removal of storage
/// items (and performing other heavy migrations) over multiple blocks.
/// (https://github.com/paritytech/substrate/issues/13690)
pub struct RemovePallet<P: Get<&'static str>>(PhantomData<P>);
impl<P: Get<&'static str>> frame_support::traits::OnRuntimeUpgrade for RemovePallet<P> {
	fn on_runtime_upgrade() -> frame_support::weights::Weight {
		let hashed_prefix = twox_128(P::get().as_bytes());
		let keys_removed = match clear_prefix(&hashed_prefix, None) {
			KillStorageResult::AllRemoved(value) => value,
			KillStorageResult::SomeRemaining(value) => value,
		} as u64;

		log::info!("Removed {} {} keys 🧹", keys_removed, P::get());

		RocksDbWeight::get().reads_writes(keys_removed + 1, keys_removed)
	}

	#[cfg(feature = "try-runtime")]
	fn pre_upgrade() -> Result<Vec<u8>, &'static str> {
		let hashed_prefix = twox_128(P::get().as_bytes());
		match contains_prefixed_key(&hashed_prefix) {
			true => log::info!("Found {} keys pre-removal 👀", P::get()),
			false => log::warn!("No {} keys found pre-removal ⚠️", P::get()),
		};
		Ok(Vec::new())
	}

	#[cfg(feature = "try-runtime")]
	fn post_upgrade(_state: Vec<u8>) -> Result<(), &'static str> {
		let hashed_prefix = twox_128(P::get().as_bytes());
		match contains_prefixed_key(&hashed_prefix) {
			true => log::error!("{} has keys remaining post-removal ❗", P::get()),
			false => log::info!("No {} keys found post-removal 🎉", P::get()),
		};
		Ok(())
	}
}
