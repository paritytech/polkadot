// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! A pallet for managing a validator set that is bridged, i.e.
//! managed remotely from another chain.

use parity_scale_codec::{Decode, Encode};
use sp_staking::SessionIndex;
use sp_std::vec::Vec;

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use sp_std::collections::vec_deque::VecDeque;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + pallet_session::Config {}

	/// The validator sets that should be used in the upcoming sessions.
	#[pallet::storage]
	#[pallet::getter(fn next_validators)]
	pub type NextValidators<T: Config> =
		StorageValue<_, VecDeque<(SessionIndex, Vec<T::ValidatorId>)>, ValueQuery>;
}

impl<T: Config> Pallet<T> {
	pub fn push_next_validator_set(session_index: SessionIndex, validators: Vec<T::ValidatorId>) {
		// HACK: we shouldn't blindly push without checking session index
		NextValidators::<T>::mutate(|next_validators| {
			next_validators.push_back((session_index, validators))
		})
	}
}

// NOTE: this assumes session keys for these validators have been previously registered
impl<T: Config> pallet_session::SessionManager<T::ValidatorId> for Pallet<T> {
	fn new_session(new_index: sp_staking::SessionIndex) -> Option<Vec<T::ValidatorId>> {
		// HACK: session index matching should be handled more gracefully
		if let Some((session_index, _)) = NextValidators::<T>::get().front() {
			if *session_index > new_index {
				log::warn!(
					target: "runtime::bridged-session-manager",
					"Skipping bridged session change, current: {:?}, pending: {:?}",
					new_index,
					session_index,
				);
				return None
			}
		}

		let mut validators = None;
		NextValidators::<T>::mutate(|next_validators| {
			if let Some((session_index, next_validators)) = next_validators.pop_front() {
				if session_index != new_index {
					log::warn!(
						target: "runtime::bridged-session-manager",
						"Mismatched bridged session index, expected: {:?}, got: {:?}",
						new_index,
						session_index,
					);
				}

				validators = Some(next_validators);
			}
		});

		validators
	}

	fn start_session(_start_index: sp_staking::SessionIndex) {}

	fn end_session(_end_index: sp_staking::SessionIndex) {}
}

impl<T: Config> pallet_session::historical::SessionManager<T::ValidatorId, ()> for Pallet<T> {
	fn new_session(new_index: SessionIndex) -> Option<Vec<(T::ValidatorId, ())>> {
		<Self as pallet_session::SessionManager<_>>::new_session(new_index)
			.map(|r| r.into_iter().map(|v| (v, Default::default())).collect())
	}

	fn start_session(start_index: SessionIndex) {
		<Self as pallet_session::SessionManager<_>>::start_session(start_index)
	}

	fn end_session(end_index: SessionIndex) {
		<Self as pallet_session::SessionManager<_>>::end_session(end_index)
	}
}

/// Messages sent over the bridge to other relay-chains to manage the validator set.
#[derive(Debug, Decode, Encode)]
pub enum BridgedSessionManagerMessage<ValidatorId> {
	NewValidatorSet(SessionIndex, Vec<ValidatorId>),
}
