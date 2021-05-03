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

use sp_std::{marker::PhantomData, convert::TryInto, boxed::Box};
use codec::{Encode, Decode};
use xcm::v0::{BodyId, OriginKind, MultiLocation, Junction::Plurality};
use xcm_executor::traits::ConvertOrigin;
use sp_runtime::{RuntimeDebug, traits::BadOrigin};
use frame_support::traits::{EnsureOrigin, OriginTrait, Filter, Get, Contains};

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use xcm::v0::{Xcm, MultiLocation, Error as XcmError, SendXcm, ExecuteXcm};

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

		/// Required origin for executing XCM messages. If successful, the it resolves to `MultiLocation`
		/// which exists as an interior location within this chain's XCM context.
		type ExecuteXcmOrigin: EnsureOrigin<Self::Origin, Success=MultiLocation>;

		/// Our XCM filter which messages to be executed using `XcmExecutor` must pass.
		type XcmExecuteFilter: Contains<(MultiLocation, Xcm<Self::Call>)>;

		/// Something to execute an XCM message.
		type XcmExecutor: ExecuteXcm<Self::Call>;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		Attempted(xcm::v0::Outcome),
		Sent(MultiLocation, MultiLocation, Xcm<()>),
	}

	#[pallet::error]
	pub enum Error<T> {
		Unreachable,
		SendFailure,
		/// The message execution fails the filter.
		Filtered,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(1_000)]
		fn send(origin: OriginFor<T>, dest: MultiLocation, message: Xcm<()>) -> DispatchResult {
			let origin_location = T::SendXcmOrigin::ensure_origin(origin)?;
			Self::send_xcm(origin_location.clone(), dest.clone(), message.clone())
				.map_err(|e| match e {
					XcmError::CannotReachDestination(..) => Error::<T>::Unreachable,
					_ => Error::<T>::SendFailure,
				})?;
			Self::deposit_event(Event::Sent(origin_location, dest, message));
			Ok(())
		}

		/// Execute an XCM message from a local, signed, origin.
		///
		/// An event is deposited indicating whether `msg` could be executed completely or only
		/// partially.
		///
		/// No more than `max_weight` will be used in its attempted execution. If this is less than the
		/// maximum amount of weight that the message could take to be executed, then no execution
		/// attempt will be made.
		///
		/// NOTE: A successful return to this does *not* imply that the `msg` was executed successfully
		/// to completion; only that *some* of it was executed.
		#[pallet::weight(max_weight.saturating_add(1_000u64))]
		fn execute(origin: OriginFor<T>, message: Box<Xcm<T::Call>>, max_weight: Weight)
			-> DispatchResult
		{
			let origin_location = T::ExecuteXcmOrigin::ensure_origin(origin)?;
			let value = (origin_location, *message);
			ensure!(T::XcmExecuteFilter::contains(&value), Error::<T>::Filtered);
			let (origin_location, message) = value;
			let outcome = T::XcmExecutor::execute_xcm(origin_location, message, max_weight);
			Self::deposit_event(Event::Attempted(outcome));
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Relay an XCM `message` from a given `interior` location in this context to a given `dest`
		/// location. A null `dest` is not handled.
		pub fn send_xcm(interior: MultiLocation, dest: MultiLocation, message: Xcm<()>) -> Result<(), XcmError> {
			let message = match interior {
				MultiLocation::Null => message,
				who => Xcm::<()>::RelayedFrom { who, message: Box::new(message) },
			};
			T::XcmRouter::send_xcm(dest, message)
		}
	}
}

/// Origin for the parachains module.
#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
pub enum Origin {
	/// It comes from somewhere in the XCM space.
	Xcm(MultiLocation),
}

impl From<MultiLocation> for Origin {
	fn from(location: MultiLocation) -> Origin {
		Origin::Xcm(location)
	}
}

/// Ensure that the origin `o` represents a sibling parachain.
/// Returns `Ok` with the parachain ID of the sibling or an `Err` otherwise.
pub fn ensure_xcm<OuterOrigin>(o: OuterOrigin) -> Result<MultiLocation, BadOrigin>
	where OuterOrigin: Into<Result<Origin, OuterOrigin>>
{
	match o.into() {
		Ok(Origin::Xcm(location)) => Ok(location),
		_ => Err(BadOrigin),
	}
}

/// Filter for `MultiLocation` to find those which represent a strict majority approval of an identified
/// plurality.
///
/// May reasonably be used with `EnsureXcm`.
pub struct IsMajorityOfBody<Prefix, Body>(PhantomData<(Prefix, Body)>);
impl<Prefix: Get<MultiLocation>, Body: Get<BodyId>> Filter<MultiLocation> for IsMajorityOfBody<Prefix, Body> {
	fn filter(l: &MultiLocation) -> bool {
		let maybe_suffix = l.match_and_split(&Prefix::get());
		matches!(maybe_suffix, Some(Plurality { id, part }) if id == &Body::get() && part.is_majority())
	}
}

/// `EnsureOrigin` implementation succeeding with a `MultiLocation` value to recognise and filter the
/// `Origin::Xcm` item.
pub struct EnsureXcm<F>(PhantomData<F>);
impl<O: OriginTrait + From<Origin>, F: Filter<MultiLocation>> EnsureOrigin<O> for EnsureXcm<F>
	where O::PalletsOrigin: From<Origin> + TryInto<Origin, Error=O::PalletsOrigin>
{
	type Success = MultiLocation;

	fn try_origin(outer: O) -> Result<Self::Success, O> {
		outer.try_with_caller(|caller| caller.try_into()
			.and_then(|Origin::Xcm(location)|
				if F::filter(&location) {
					Ok(location)
				} else {
					Err(Origin::Xcm(location).into())
				}
			))
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> O {
		O::from(Origin::Xcm(MultiLocation::Null))
	}
}

/// A simple passthrough where we reuse the `MultiLocation`-typed XCM origin as the inner value of
/// this crate's `Origin::Xcm` value.
pub struct XcmPassthrough<Origin>(PhantomData<Origin>);
impl<
	Origin: From<crate::Origin>,
> ConvertOrigin<Origin> for XcmPassthrough<Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match (kind, origin) {
			(OriginKind::Xcm, l) => Ok(crate::Origin::Xcm(l).into()),
			(_, origin) => Err(origin),
		}
	}
}
