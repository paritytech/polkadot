// Copyright (C) Parity Technologies (UK) Ltd.
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

//! A pallet for any shared state that other pallets may want access to.
//!
//! To avoid cyclic dependencies, it is important that this pallet is not
//! dependent on any of the other pallets.

use frame_support::pallet_prelude::*;
use frame_system::pallet_prelude::BlockNumberFor;
use primitives::{SessionIndex, ValidatorId, ValidatorIndex};
use sp_runtime::traits::AtLeast32BitUnsigned;
use sp_std::{collections::vec_deque::VecDeque, vec::Vec};

use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha20Rng;

use crate::configuration::HostConfiguration;

pub use pallet::*;

// `SESSION_DELAY` is used to delay any changes to Paras registration or configurations.
// Wait until the session index is 2 larger then the current index to apply any changes,
// which guarantees that at least one full session has passed before any changes are applied.
pub(crate) const SESSION_DELAY: SessionIndex = 2;

#[cfg(test)]
mod tests;

/// Information about past relay-parents.
#[derive(Encode, Decode, Default, TypeInfo)]
pub struct AllowedRelayParentsTracker<Hash, BlockNumber> {
	// The past relay parents, paired with state roots, that are viable to build upon.
	//
	// They are in ascending chronologic order, so the newest relay parents are at
	// the back of the deque.
	//
	// (relay_parent, state_root)
	buffer: VecDeque<(Hash, Hash)>,

	// The number of the most recent relay-parent, if any.
	// If the buffer is empty, this value has no meaning and may
	// be nonsensical.
	latest_number: BlockNumber,
}

impl<Hash: PartialEq + Copy, BlockNumber: AtLeast32BitUnsigned + Copy>
	AllowedRelayParentsTracker<Hash, BlockNumber>
{
	/// Add a new relay-parent to the allowed relay parents, along with info about the header.
	/// Provide a maximum ancestry length for the buffer, which will cause old relay-parents to be
	/// pruned.
	pub(crate) fn update(
		&mut self,
		relay_parent: Hash,
		state_root: Hash,
		number: BlockNumber,
		max_ancestry_len: u32,
	) {
		// + 1 for the most recent block, which is always allowed.
		let buffer_size_limit = max_ancestry_len as usize + 1;

		self.buffer.push_back((relay_parent, state_root));
		self.latest_number = number;
		while self.buffer.len() > buffer_size_limit {
			let _ = self.buffer.pop_front();
		}

		// We only allow relay parents within the same sessions, the buffer
		// gets cleared on session changes.
	}

	/// Attempt to acquire the state root and block number to be used when building
	/// upon the given relay-parent.
	///
	/// This only succeeds if the relay-parent is one of the allowed relay-parents.
	/// If a previous relay-parent number is passed, then this only passes if the new relay-parent
	/// is more recent than the previous.
	pub(crate) fn acquire_info(
		&self,
		relay_parent: Hash,
		prev: Option<BlockNumber>,
	) -> Option<(Hash, BlockNumber)> {
		let pos = self.buffer.iter().position(|(rp, _)| rp == &relay_parent)?;
		let age = (self.buffer.len() - 1) - pos;
		let number = self.latest_number - BlockNumber::from(age as u32);

		if let Some(prev) = prev {
			if prev > number {
				return None
			}
		}

		Some((self.buffer[pos].1, number))
	}

	/// Returns block number of the earliest block the buffer would contain if
	/// `now` is pushed into it.
	pub(crate) fn hypothetical_earliest_block_number(
		&self,
		now: BlockNumber,
		max_ancestry_len: u32,
	) -> BlockNumber {
		let allowed_ancestry_len = max_ancestry_len.min(self.buffer.len() as u32);

		now - allowed_ancestry_len.into()
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {}

	/// The current session index.
	#[pallet::storage]
	#[pallet::getter(fn session_index)]
	pub(super) type CurrentSessionIndex<T: Config> = StorageValue<_, SessionIndex, ValueQuery>;

	/// All the validators actively participating in parachain consensus.
	/// Indices are into the broader validator set.
	#[pallet::storage]
	#[pallet::getter(fn active_validator_indices)]
	pub(super) type ActiveValidatorIndices<T: Config> =
		StorageValue<_, Vec<ValidatorIndex>, ValueQuery>;

	/// The parachain attestation keys of the validators actively participating in parachain
	/// consensus. This should be the same length as `ActiveValidatorIndices`.
	#[pallet::storage]
	#[pallet::getter(fn active_validator_keys)]
	pub(super) type ActiveValidatorKeys<T: Config> = StorageValue<_, Vec<ValidatorId>, ValueQuery>;

	/// All allowed relay-parents.
	#[pallet::storage]
	#[pallet::getter(fn allowed_relay_parents)]
	pub(crate) type AllowedRelayParents<T: Config> =
		StorageValue<_, AllowedRelayParentsTracker<T::Hash, BlockNumberFor<T>>, ValueQuery>;

	#[pallet::call]
	impl<T: Config> Pallet<T> {}
}

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the configuration pallet.
	pub(crate) fn initializer_initialize(_now: BlockNumberFor<T>) -> Weight {
		Weight::zero()
	}

	/// Called by the initializer to finalize the configuration pallet.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note that a new session has started.
	///
	/// Returns the list of outgoing paras from the actions queue.
	pub(crate) fn initializer_on_new_session(
		session_index: SessionIndex,
		random_seed: [u8; 32],
		new_config: &HostConfiguration<BlockNumberFor<T>>,
		all_validators: Vec<ValidatorId>,
	) -> Vec<ValidatorId> {
		// Drop allowed relay parents buffer on a session change.
		//
		// During the initialization of the next block we always add its parent
		// to the tracker.
		//
		// With asynchronous backing candidates built on top of relay
		// parent `R` are still restricted by the runtime to be backed
		// by the group assigned at `number(R) + 1`, which is guaranteed
		// to be in the current session.
		AllowedRelayParents::<T>::mutate(|tracker| tracker.buffer.clear());

		CurrentSessionIndex::<T>::set(session_index);
		let mut rng: ChaCha20Rng = SeedableRng::from_seed(random_seed);

		let mut shuffled_indices: Vec<_> = (0..all_validators.len())
			.enumerate()
			.map(|(i, _)| ValidatorIndex(i as _))
			.collect();

		shuffled_indices.shuffle(&mut rng);

		if let Some(max) = new_config.max_validators {
			shuffled_indices.truncate(max as usize);
		}

		let active_validator_keys =
			crate::util::take_active_subset(&shuffled_indices, &all_validators);

		ActiveValidatorIndices::<T>::set(shuffled_indices);
		ActiveValidatorKeys::<T>::set(active_validator_keys.clone());

		active_validator_keys
	}

	/// Return the session index that should be used for any future scheduled changes.
	pub fn scheduled_session() -> SessionIndex {
		Self::session_index().saturating_add(SESSION_DELAY)
	}

	/// Test function for setting the current session index.
	#[cfg(any(feature = "std", feature = "runtime-benchmarks", test))]
	pub fn set_session_index(index: SessionIndex) {
		CurrentSessionIndex::<T>::set(index);
	}

	#[cfg(any(feature = "std", feature = "runtime-benchmarks", test))]
	pub fn set_active_validators_ascending(active: Vec<ValidatorId>) {
		ActiveValidatorIndices::<T>::set(
			(0..active.len()).map(|i| ValidatorIndex(i as _)).collect(),
		);
		ActiveValidatorKeys::<T>::set(active);
	}

	#[cfg(test)]
	pub(crate) fn set_active_validators_with_indices(
		indices: Vec<ValidatorIndex>,
		keys: Vec<ValidatorId>,
	) {
		assert_eq!(indices.len(), keys.len());
		ActiveValidatorIndices::<T>::set(indices);
		ActiveValidatorKeys::<T>::set(keys);
	}
}
