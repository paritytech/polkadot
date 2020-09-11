// Copyright 2020 Parity Technologies (UK) Ltd.
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

#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::convert::TryInto;
use frame_support::{decl_event, decl_error, decl_module};
use frame_system::ensure_signed;
use xcm::{VersionedXcm, v0::{Error as XcmError, ExecuteXcm}};
use xcm_executor::traits::LocationConversion;

/// Configuration trait of this pallet.
pub trait Trait: frame_system::Trait {
	/// Event type used by the runtime.
	type Event: From<Event> + Into<<Self as frame_system::Trait>::Event>;

	/// Utility for converting from the signed origin (of type `Self::AccountId`) into a sensible
	/// `MultiLocation` ready for passing to the XCM interpreter.
	type AccountIdConverter: LocationConversion<Self::AccountId>;

	/// The interpreter.
	type XcmExecutor: ExecuteXcm;
}

decl_event! {
	pub enum Event {
		/// Xcm execution completed OK.
		XcmSuccess,
		/// Xcm execution had some issues.
		XcmFail(XcmError),
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Location given was invalid or unsupported.
		BadLocation,
		/// The XCM message version is not supported.
		BadVersion,
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: <T as frame_system::Trait>::Origin {
		fn deposit_event() = default;

		#[weight = 10]
		fn execute(origin, xcm: VersionedXcm) {
			let who = ensure_signed(origin)?;
			let xcm_origin = T::AccountIdConverter::into_location(who)
				.ok_or(Error::<T>::BadLocation)?;

			let xcm = xcm.try_into().map_err(|_| Error::<T>::BadVersion)?;
			let xcm_result = T::XcmExecutor::execute_xcm(xcm_origin, xcm);
			let event = match xcm_result {
				Ok(_) => Event::XcmSuccess,
				Err(e) => Event::XcmFail(e),
			};
			Self::deposit_event(event);
		}
	}
}
