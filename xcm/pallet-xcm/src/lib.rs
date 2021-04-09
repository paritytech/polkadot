// Copyright 2020-2021 Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

// Cumulus is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Cumulus is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Pallet to handle XCM messages.

#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use xcm::v0::{Xcm, MultiLocation, Error as XcmError, SendXcm};

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	/// The module configuration trait.
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// Required origin for sending XCM messages. If successful, the it resolves to `MultiLocation`
		/// which exists as an interior location within this chain's XCM context.
		type SendXcmOrigin: EnsureOrigin<Self::Origin, Success=MultiLocation>;

		/// The type used to actually dispatch an XCM to its destination.
		type XcmRouter: SendXcm;
	}

	#[pallet::event]
	pub enum Event<T: Config> {}

	#[pallet::error]
	pub enum Error<T> {
		Unreachable,
		SendFailure,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(1_000)]
		fn send(origin: OriginFor<T>, dest: MultiLocation, message: Xcm<()>) -> DispatchResult {
			let origin_location = T::SendXcmOrigin::ensure_origin(origin)?;
			Self::send_xcm(origin_location, dest, message)
				.map_err(|e| match e {
					XcmError::CannotReachDestination(..) => Error::<T>::Unreachable,
					_ => Error::<T>::SendFailure,
				})?;
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Relay an XCM `message` from a given `interior` location in this context to a given `dest`
		/// location. A null `dest` is not handled.
		pub fn send_xcm(interior: MultiLocation, dest: MultiLocation, message: Xcm<()>) -> Result<(), XcmError> {
			let message = match interior {
				MultiLocation::Null => message,
				who => Xcm::<()>::RelayedFrom { who, message: sp_std::boxed::Box::new(message) },
			};
			T::XcmRouter::send_xcm(dest, message)
		}
	}
}
