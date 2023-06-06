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

// TODO: set this based on the number of validators
const MAX_DISABLED_VALIDATORS_COUNT: usize = 10;

pub struct ValidatorDisabling<C> {
	_phantom: sp_std::marker::PhantomData<C>,
}

impl<T> ValidatorDisablingHandler<T::BlockNumber> for ValidatorDisabling<Pallet<T>>
where
	T: pallet::Config,
{
	fn report_offender(
		validators: impl IntoIterator<Item = ValidatorIndex>,
		offence_kind: SlashingOffenceKind,
	) {
		// Add all new offenders to `Offenders`
		for validator_idx in validators.into_iter() {
			<Offenders<T>>::mutate(validator_idx, |current_offence| {
				let current_offence = current_offence.get_or_insert(offence_kind);
				if *current_offence == SlashingOffenceKind::ForInvalid &&
					offence_kind == SlashingOffenceKind::ForInvalid
				{
					*current_offence = offence_kind;
				}
			});
		}

		// Add to disabled validators, if there is enough space
		let disabled = if <Offenders<T>>::iter().count() <= MAX_DISABLED_VALIDATORS_COUNT {
			<Offenders<T>>::iter().map(|offender| offender.0).collect()
		} else {
			<Pallet<T>>::reshuffle_disabled_validators()
		};

		<DisabledValidators<T>>::set(disabled);
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
	pub type DisabledValidators<T> = StorageValue<_, Vec<ValidatorIndex>, ValueQuery>;

	// All offenders reported during the current session
	#[pallet::storage]
	pub type Offenders<T> = StorageMap<_, Twox64Concat, ValidatorIndex, SlashingOffenceKind>;

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		/// Called when a new block is being created
		fn on_initialize(_n: T::BlockNumber) -> Weight {
			if <Offenders<T>>::iter().count() > MAX_DISABLED_VALIDATORS_COUNT {
				let disabled = <Pallet<T>>::reshuffle_disabled_validators();
				<DisabledValidators<T>>::set(disabled);
			}

			// TODO: set proper weight
			Weight::zero()
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
		let _ = <Offenders<T>>::clear(u32::max_value(), None);
		<DisabledValidators<T>>::set(Vec::new());
	}

	fn reshuffle_disabled_validators() -> Vec<ValidatorIndex> {
		let mut disabled_capacity = MAX_DISABLED_VALIDATORS_COUNT;
		let mut disabled = Vec::new();

		// First partition the validators by their offence. At the moment there are only two offence
		// types so a simple partition can do the job.
		let (big_offenders, small_offenders): (
			Vec<(ValidatorIndex, SlashingOffenceKind)>,
			Vec<(ValidatorIndex, SlashingOffenceKind)>,
		) = <Offenders<T>>::iter().partition(|offender| offender.1 == SlashingOffenceKind::ForInvalid);

		// Strip `SlashingOffenceKind`
		let mut big_offenders =
			big_offenders.into_iter().map(|offender| offender.0).collect::<Vec<_>>();

		// If there are too many `big_offenders` pick offenders randomly until `disabled is out of
		// capacity.
		if big_offenders.len() > disabled_capacity {
			return Self::random_selection(&mut big_offenders, disabled_capacity)
		}

		// If the `big_offenders` fits - add all validators from the disabled set
		disabled_capacity -= big_offenders.len();
		disabled.extend(big_offenders.into_iter());

		// Strip `SlashingOffenceKind`
		let mut small_offenders =
			small_offenders.into_iter().map(|offender| offender.0).collect::<Vec<_>>();

		// Next try to extend the disabled list with small offenders
		if small_offenders.len() > disabled_capacity {
			return Self::random_selection(&mut small_offenders, disabled_capacity)
		}

		// All small offenders can be inserted.
		disabled.extend(small_offenders.into_iter());

		disabled
	}

	fn random_selection(input: &mut Vec<ValidatorIndex>, capacity: usize) -> Vec<ValidatorIndex> {
		debug_assert!(input.len() > capacity);
		// TODO: source of randomness?!
		input.split_off(capacity)
	}
}
