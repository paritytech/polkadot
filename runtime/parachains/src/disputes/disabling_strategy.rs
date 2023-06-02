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

//! A pallet implementing a strategy for validator disabling
//!
//! We can't disable more than f validators otherwise we will break the security model. For this
//! reason the pallet contains two storage items:
//! * `DisabledValidators` which holds the indexes of all disabled validators at the moment
//! * `Offenders` which holds all slashed validators with their corresponding offence
//!
//! TODO: add more details

use super::slashing::ValidatorDisablingHandler;
use frame_support::weights::Weight;
use primitives::{vstaging::slashing::SlashingOffenceKind, SessionIndex, ValidatorIndex};
use sp_std::prelude::*;
pub struct ValidatorDisabling<C> {
	_phantom: sp_std::marker::PhantomData<C>,
}

impl<T> ValidatorDisablingHandler<T::BlockNumber> for ValidatorDisabling<Pallet<T>>
where
	T: pallet::Config,
{
	fn report_offender(
		_validators: impl IntoIterator<Item = ValidatorIndex>,
		_offence_kind: SlashingOffenceKind,
	) {
		// Add to offenders

		// Add to disabled validators, if there is enough space

		// Else - reshuffle

		todo!()
	}

	fn initializer_initialize(now: T::BlockNumber) -> Weight {
		Pallet::<T>::initializer_initialize(now)
	}

	fn initializer_finalize() {
		Pallet::<T>::initializer_finalize()
	}

	fn initializer_on_new_session(session_index: SessionIndex) {
		Pallet::<T>::initializer_on_new_session(session_index)
	}
}

struct Offender {
	index: ValidatorIndex,
	offence: SlashingOffenceKind,
}

impl Offender {
	fn cmp(&self, other: &Self) -> core::cmp::Ordering {
		use SlashingOffenceKind::{AgainstValid, ForInvalid};

		let Offender { index, offence } = self;
		match (offence, other.offence) {
			(AgainstValid, AgainstValid) | (ForInvalid, ForInvalid) => index.cmp(&other.index),
			(AgainstValid, ForInvalid) => core::cmp::Ordering::Less,
			(ForInvalid, AgainstValid) => core::cmp::Ordering::Greater,
		}
	}
}
impl PartialEq for Offender {
	fn eq(&self, other: &Self) -> bool {
		let Offender { index, offence } = self;
		*index == other.index && *offence == other.offence
	}
}

impl Eq for Offender {}

impl PartialOrd for Offender {
	fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for Offender {
	fn cmp(&self, other: &Self) -> core::cmp::Ordering {
		use SlashingOffenceKind::{AgainstValid, ForInvalid};

		let Offender { index, offence } = self;
		match (offence, other.offence) {
			(AgainstValid, AgainstValid) | (ForInvalid, ForInvalid) => index.cmp(&other.index),
			(AgainstValid, ForInvalid) => core::cmp::Ordering::Less,
			(ForInvalid, AgainstValid) => core::cmp::Ordering::Greater,
		}
	}
}

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{pallet_prelude::*, traits::Hooks};
	use frame_system::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config {}

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	// Disabled validators at the moment
	#[pallet::storage]
	#[pallet::getter(fn disabled_validators)]
	type DisabledValidators<T> = StorageValue<_, Vec<ValidatorIndex>, ValueQuery>;

	// All offenders reported during the current session
	#[pallet::storage]
	type Offenders<T> = StorageMap<_, Twox64Concat, ValidatorIndex, SlashingOffenceKind>;

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		/// Called when a new block is being created
		fn on_initialize(_n: T::BlockNumber) -> Weight {
			todo!()
			// TODO: block is being initialized, reshuffle disabled validators if necessary
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the module.
	fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		Weight::zero()
	}

	/// Called by the initializer to finalize the pallet.
	fn initializer_finalize() {}

	/// Called by the initializer to note a new session in the pallet.
	fn initializer_on_new_session(_session_index: SessionIndex) {
		todo!()
		// TODO: clear validators list
	}
}
