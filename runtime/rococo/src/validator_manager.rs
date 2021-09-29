// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! A pallet for managing validators on Rococo.

use frame_support::{decl_error, decl_event, decl_module, decl_storage, traits::EnsureOrigin};
use sp_staking::SessionIndex;
use sp_std::vec::Vec;

type Session<T> = pallet_session::Pallet<T>;

/// Configuration for the parachain proposer.
pub trait Config: pallet_session::Config {
	/// The overreaching event type.
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;

	/// Privileged origin that can add or remove validators.
	type PrivilegedOrigin: EnsureOrigin<<Self as frame_system::Config>::Origin>;
}

decl_event! {
	pub enum Event<T> where ValidatorId = <T as pallet_session::Config>::ValidatorId {
		/// New validators were added to the set.
		ValidatorsRegistered(Vec<ValidatorId>),
		/// Validators were removed from the set.
		ValidatorsDeregistered(Vec<ValidatorId>),
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {}
}

decl_storage! {
	trait Store for Module<T: Config> as ParachainProposer {
		/// Validators that should be retired, because their Parachain was deregistered.
		ValidatorsToRetire: Vec<T::ValidatorId>;
		/// Validators that should be added.
		ValidatorsToAdd: Vec<T::ValidatorId>;
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;

		fn deposit_event() = default;

		/// Add new validators to the set.
		///
		/// The new validators will be active from current session + 2.
		#[weight = 100_000]
		fn register_validators(
			origin,
			validators: Vec<T::ValidatorId>,
		) {
			T::PrivilegedOrigin::ensure_origin(origin)?;

			validators.clone().into_iter().for_each(|v| ValidatorsToAdd::<T>::append(v));

			Self::deposit_event(RawEvent::ValidatorsRegistered(validators));
		}

		/// Remove validators from the set.
		///
		/// The removed validators will be deactivated from current session + 2.
		#[weight = 100_000]
		fn deregister_validators(
			origin,
			validators: Vec<T::ValidatorId>,
		) {
			T::PrivilegedOrigin::ensure_origin(origin)?;

			validators.clone().into_iter().for_each(|v| ValidatorsToRetire::<T>::append(v));

			Self::deposit_event(RawEvent::ValidatorsDeregistered(validators));
		}
	}
}

impl<T: Config> Module<T> {}

impl<T: Config> pallet_session::SessionManager<T::ValidatorId> for Module<T> {
	fn new_session(new_index: SessionIndex) -> Option<Vec<T::ValidatorId>> {
		if new_index <= 1 {
			return None
		}

		let mut validators = Session::<T>::validators();

		ValidatorsToRetire::<T>::take().iter().for_each(|v| {
			if let Some(pos) = validators.iter().position(|r| r == v) {
				validators.swap_remove(pos);
			}
		});

		ValidatorsToAdd::<T>::take().into_iter().for_each(|v| {
			if !validators.contains(&v) {
				validators.push(v);
			}
		});

		Some(validators)
	}

	fn end_session(_: SessionIndex) {}

	fn start_session(_start_index: SessionIndex) {}
}

impl<T: Config> pallet_session::historical::SessionManager<T::ValidatorId, ()> for Module<T> {
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
