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

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use codec::{Decode, Encode};
use frame_support::traits::{Contains, EnsureOrigin, Get, OriginTrait};
use sp_runtime::{traits::BadOrigin, RuntimeDebug};
use sp_std::{
	boxed::Box,
	convert::{TryFrom, TryInto},
	marker::PhantomData,
	prelude::*,
	vec,
};
use xcm::{
	latest::prelude::*, Version as XcmVersion, VersionedMultiAssets, VersionedMultiLocation,
	VersionedXcm,
};
use xcm_executor::traits::{ConvertOrigin, VersionChangeNotifier};

use frame_support::PalletId;
pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;
	use sp_runtime::traits::AccountIdConversion;
	use xcm_executor::traits::{InvertLocation, WeightBounds};

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
		type SendXcmOrigin: EnsureOrigin<Self::Origin, Success = MultiLocation>;

		/// The type used to actually dispatch an XCM to its destination.
		type XcmRouter: SendXcm;

		/// Required origin for executing XCM messages, including the teleport functionality. If successful,
		/// then it resolves to `MultiLocation` which exists as an interior location within this chain's XCM
		/// context.
		type ExecuteXcmOrigin: EnsureOrigin<Self::Origin, Success = MultiLocation>;

		/// Our XCM filter which messages to be executed using `XcmExecutor` must pass.
		type XcmExecuteFilter: Contains<(MultiLocation, Xcm<Self::Call>)>;

		/// Something to execute an XCM message.
		type XcmExecutor: ExecuteXcm<Self::Call>;

		/// Our XCM filter which messages to be teleported using the dedicated extrinsic must pass.
		type XcmTeleportFilter: Contains<(MultiLocation, Vec<MultiAsset>)>;

		/// Our XCM filter which messages to be reserve-transferred using the dedicated extrinsic must pass.
		type XcmReserveTransferFilter: Contains<(MultiLocation, Vec<MultiAsset>)>;

		/// Means of measuring the weight consumed by an XCM message locally.
		type Weigher: WeightBounds<Self::Call>;

		/// Means of inverting a location.
		type LocationInverter: InvertLocation;
	}

	/// The maximum number of distinct assets allowed to be transferred in a single helper extrinsic.
	const MAX_ASSETS_FOR_TRANSFER: usize = 2;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		Attempted(xcm::latest::Outcome),
		Sent(MultiLocation, MultiLocation, Xcm<()>),
	}

	#[pallet::error]
	pub enum Error<T> {
		Unreachable,
		SendFailure,
		/// The message execution fails the filter.
		Filtered,
		/// The message's weight could not be determined.
		UnweighableMessage,
		/// The assets to be sent are empty.
		Empty,
		/// Could not re-anchor the assets to declare the fees for the destination chain.
		CannotReanchor,
		/// Too many assets have been attempted for transfer.
		TooManyAssets,
		/// Origin is invalid for sending.
		InvalidOrigin,
		/// The version of the `Versioned` value used is not able to be interpreted.
		BadVersion,
	}

	/// The target locations that are subscribed to our version changes, as well as the most recent
	/// of our versions we informed them of.
	#[pallet::storage]
	pub(super) type VersionNotifyTargets<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		XcmVersion,
		Blake2_128Concat,
		VersionedMultiLocation,
		(u64, u64, XcmVersion),
		OptionQuery,
	>;

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(100_000_000)]
		pub fn send(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			message: Box<VersionedXcm<()>>,
		) -> DispatchResult {
			let origin_location = T::SendXcmOrigin::ensure_origin(origin)?;
			let interior =
				origin_location.clone().try_into().map_err(|_| Error::<T>::InvalidOrigin)?;
			let dest = MultiLocation::try_from(*dest).map_err(|()| Error::<T>::BadVersion)?;
			let message: Xcm<()> = (*message).try_into().map_err(|()| Error::<T>::BadVersion)?;

			Self::send_xcm(interior, dest.clone(), message.clone()).map_err(|e| match e {
				XcmError::CannotReachDestination(..) => Error::<T>::Unreachable,
				_ => Error::<T>::SendFailure,
			})?;
			Self::deposit_event(Event::Sent(origin_location, dest, message));
			Ok(())
		}

		/// Teleport some assets from the local chain to some destination chain.
		///
		/// Fee payment on the destination side is made from the first asset listed in the `assets` vector.
		///
		/// - `origin`: Must be capable of withdrawing the `assets` and executing XCM.
		/// - `dest`: Destination context for the assets. Will typically be `X2(Parent, Parachain(..))` to send
		///   from parachain to parachain, or `X1(Parachain(..))` to send from relay to parachain.
		/// - `beneficiary`: A beneficiary location for the assets in the context of `dest`. Will generally be
		///   an `AccountId32` value.
		/// - `assets`: The assets to be withdrawn. The first item should be the currency used to to pay the fee on the
		///   `dest` side. May not be empty.
		/// - `dest_weight`: Equal to the total weight on `dest` of the XCM message
		///   `Teleport { assets, effects: [ BuyExecution{..}, DepositAsset{..} ] }`.
		#[pallet::weight({
			let maybe_assets: Result<MultiAssets, ()> = (*assets.clone()).try_into();
			let maybe_dest: Result<MultiLocation, ()> = (*dest.clone()).try_into();
			match (maybe_assets, maybe_dest) {
				(Ok(assets), Ok(dest)) => {
					let mut message = Xcm::WithdrawAsset {
						assets,
						effects: sp_std::vec![ InitiateTeleport {
							assets: Wild(All),
							dest,
							effects: sp_std::vec![],
						} ]
					};
					T::Weigher::weight(&mut message).map_or(Weight::max_value(), |w| 100_000_000 + w)
				},
				_ => Weight::max_value(),
			}
		})]
		pub fn teleport_assets(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			beneficiary: Box<VersionedMultiLocation>,
			assets: Box<VersionedMultiAssets>,
			fee_asset_item: u32,
			dest_weight: Weight,
		) -> DispatchResult {
			let origin_location = T::ExecuteXcmOrigin::ensure_origin(origin)?;
			let dest = MultiLocation::try_from(*dest).map_err(|()| Error::<T>::BadVersion)?;
			let beneficiary =
				MultiLocation::try_from(*beneficiary).map_err(|()| Error::<T>::BadVersion)?;
			let assets = MultiAssets::try_from(*assets).map_err(|()| Error::<T>::BadVersion)?;

			ensure!(assets.len() <= MAX_ASSETS_FOR_TRANSFER, Error::<T>::TooManyAssets);
			let value = (origin_location, assets.drain());
			ensure!(T::XcmTeleportFilter::contains(&value), Error::<T>::Filtered);
			let (origin_location, assets) = value;
			let inv_dest = T::LocationInverter::invert_location(&dest);
			let fees = assets
				.get(fee_asset_item as usize)
				.ok_or(Error::<T>::Empty)?
				.clone()
				.reanchored(&inv_dest)
				.map_err(|_| Error::<T>::CannotReanchor)?;
			let max_assets = assets.len() as u32;
			let assets = assets.into();
			let mut message = Xcm::WithdrawAsset {
				assets,
				effects: vec![InitiateTeleport {
					assets: Wild(All),
					dest,
					effects: vec![
						BuyExecution {
							fees,
							// Zero weight for additional XCM (since there are none to execute)
							weight: 0,
							debt: dest_weight,
							halt_on_error: false,
							instructions: vec![],
						},
						DepositAsset { assets: Wild(All), max_assets, beneficiary },
					],
				}],
			};
			let weight =
				T::Weigher::weight(&mut message).map_err(|()| Error::<T>::UnweighableMessage)?;
			let outcome =
				T::XcmExecutor::execute_xcm_in_credit(origin_location, message, weight, weight);
			Self::deposit_event(Event::Attempted(outcome));
			Ok(())
		}

		/// Transfer some assets from the local chain to the sovereign account of a destination chain and forward
		/// a notification XCM.
		///
		/// Fee payment on the destination side is made from the first asset listed in the `assets` vector.
		///
		/// - `origin`: Must be capable of withdrawing the `assets` and executing XCM.
		/// - `dest`: Destination context for the assets. Will typically be `X2(Parent, Parachain(..))` to send
		///   from parachain to parachain, or `X1(Parachain(..))` to send from relay to parachain.
		/// - `beneficiary`: A beneficiary location for the assets in the context of `dest`. Will generally be
		///   an `AccountId32` value.
		/// - `assets`: The assets to be withdrawn. This should include the assets used to pay the fee on the
		///   `dest` side.
		/// - `dest_weight`: Equal to the total weight on `dest` of the XCM message
		///   `ReserveAssetDeposited { assets, effects: [ BuyExecution{..}, DepositAsset{..} ] }`.
		#[pallet::weight({
			match ((*assets.clone()).try_into(), (*dest.clone()).try_into()) {
				(Ok(assets), Ok(dest)) => {
					let effects = sp_std::vec![];
					let mut message = Xcm::TransferReserveAsset { assets, dest, effects };
					T::Weigher::weight(&mut message).map_or(Weight::max_value(), |w| 100_000_000 + w)
				},
				_ => Weight::max_value(),
			}
		})]
		pub fn reserve_transfer_assets(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			beneficiary: Box<VersionedMultiLocation>,
			assets: Box<VersionedMultiAssets>,
			fee_asset_item: u32,
			dest_weight: Weight,
		) -> DispatchResult {
			let origin_location = T::ExecuteXcmOrigin::ensure_origin(origin)?;
			let dest = (*dest).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let beneficiary = (*beneficiary).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let assets: MultiAssets = (*assets).try_into().map_err(|()| Error::<T>::BadVersion)?;

			ensure!(assets.len() <= MAX_ASSETS_FOR_TRANSFER, Error::<T>::TooManyAssets);
			let value = (origin_location, assets.drain());
			ensure!(T::XcmReserveTransferFilter::contains(&value), Error::<T>::Filtered);
			let (origin_location, assets) = value;
			let inv_dest = T::LocationInverter::invert_location(&dest);
			let fees = assets
				.get(fee_asset_item as usize)
				.ok_or(Error::<T>::Empty)?
				.clone()
				.reanchored(&inv_dest)
				.map_err(|_| Error::<T>::CannotReanchor)?;
			let max_assets = assets.len() as u32;
			let assets = assets.into();
			let mut message = Xcm::TransferReserveAsset {
				assets,
				dest,
				effects: vec![
					BuyExecution {
						fees,
						// Zero weight for additional instructions/orders (since there are none to execute)
						weight: 0,
						debt: dest_weight, // covers this, `TransferReserveAsset` xcm, and `DepositAsset` order.
						halt_on_error: false,
						instructions: vec![],
					},
					DepositAsset { assets: Wild(All), max_assets, beneficiary },
				],
			};
			let weight =
				T::Weigher::weight(&mut message).map_err(|()| Error::<T>::UnweighableMessage)?;
			let outcome =
				T::XcmExecutor::execute_xcm_in_credit(origin_location, message, weight, weight);
			Self::deposit_event(Event::Attempted(outcome));
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
		#[pallet::weight(max_weight.saturating_add(100_000_000u64))]
		pub fn execute(
			origin: OriginFor<T>,
			message: Box<VersionedXcm<T::Call>>,
			max_weight: Weight,
		) -> DispatchResult {
			let origin_location = T::ExecuteXcmOrigin::ensure_origin(origin)?;
			let message = (*message).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let value = (origin_location, message);
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
		pub fn send_xcm(
			interior: Junctions,
			dest: MultiLocation,
			message: Xcm<()>,
		) -> Result<(), XcmError> {
			let message = if let Junctions::Here = interior {
				message
			} else {
				Xcm::<()>::RelayedFrom { who: interior, message: Box::new(message) }
			};
			log::trace!(target: "xcm::send_xcm", "dest: {:?}, message: {:?}", &dest, &message);
			T::XcmRouter::send_xcm(dest, message)
		}

		pub fn check_account() -> T::AccountId {
			const ID: PalletId = PalletId(*b"py/xcmch");
			AccountIdConversion::<T::AccountId>::into_account(&ID)
		}
	}

	impl<T: Config> VersionChangeNotifier for Pallet<T> {
		/// Start notifying `location` should the XCM version of this chain change.
		///
		/// When it does, this type should ensure an `QueryResponse` message is sent with the given
		/// `query_id` & `max_weight` and with a `response` of `Repsonse::Version`. This should happen
		/// until/unless `stop` is called with the correct `query_id`.
		///
		/// If the `location` has an ongoing notification and when this function is called, then an
		/// error should be returned.
		fn start(dest: &MultiLocation, query_id: u64, max_weight: u64) -> XcmResult {
			let versioned_dest: VersionedMultiLocation = dest.clone().into();
			let already = VersionNotifyTargets::<T>::contains_key(XCM_VERSION, &versioned_dest);
			ensure!(!already, XcmError::InvalidLocation);

			let xcm_version = XCM_VERSION;
			let response = Response::Version(xcm_version);
			let message = QueryResponse { query_id, response };
			T::XcmRouter::send_xcm(dest.clone(), message)?;

			let value = (query_id, max_weight, xcm_version);
			VersionNotifyTargets::<T>::insert(XCM_VERSION, versioned_dest, value);
			Ok(())
		}

		/// Stop notifying `location` should the XCM change. This is a no-op if there was never a
		/// subscription.
		fn stop(dest: &MultiLocation) -> XcmResult {
			VersionNotifyTargets::<T>::remove(
				XCM_VERSION,
				VersionedMultiLocation::from(dest.clone()),
			);
			Ok(())
		}
	}

	/// Origin for the parachains module.
	#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
	#[pallet::origin]
	pub enum Origin {
		/// It comes from somewhere in the XCM space.
		Xcm(MultiLocation),
	}

	impl From<MultiLocation> for Origin {
		fn from(location: MultiLocation) -> Origin {
			Origin::Xcm(location)
		}
	}
}

/// Ensure that the origin `o` represents a sibling parachain.
/// Returns `Ok` with the parachain ID of the sibling or an `Err` otherwise.
pub fn ensure_xcm<OuterOrigin>(o: OuterOrigin) -> Result<MultiLocation, BadOrigin>
where
	OuterOrigin: Into<Result<Origin, OuterOrigin>>,
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
impl<Prefix: Get<MultiLocation>, Body: Get<BodyId>> Contains<MultiLocation>
	for IsMajorityOfBody<Prefix, Body>
{
	fn contains(l: &MultiLocation) -> bool {
		let maybe_suffix = l.match_and_split(&Prefix::get());
		matches!(maybe_suffix, Some(Plurality { id, part }) if id == &Body::get() && part.is_majority())
	}
}

/// `EnsureOrigin` implementation succeeding with a `MultiLocation` value to recognize and filter the
/// `Origin::Xcm` item.
pub struct EnsureXcm<F>(PhantomData<F>);
impl<O: OriginTrait + From<Origin>, F: Contains<MultiLocation>> EnsureOrigin<O> for EnsureXcm<F>
where
	O::PalletsOrigin: From<Origin> + TryInto<Origin, Error = O::PalletsOrigin>,
{
	type Success = MultiLocation;

	fn try_origin(outer: O) -> Result<Self::Success, O> {
		outer.try_with_caller(|caller| {
			caller.try_into().and_then(|Origin::Xcm(location)| {
				if F::contains(&location) {
					Ok(location)
				} else {
					Err(Origin::Xcm(location).into())
				}
			})
		})
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> O {
		O::from(Origin::Xcm(Here.into()))
	}
}

/// A simple passthrough where we reuse the `MultiLocation`-typed XCM origin as the inner value of
/// this crate's `Origin::Xcm` value.
pub struct XcmPassthrough<Origin>(PhantomData<Origin>);
impl<Origin: From<crate::Origin>> ConvertOrigin<Origin> for XcmPassthrough<Origin> {
	fn convert_origin(origin: MultiLocation, kind: OriginKind) -> Result<Origin, MultiLocation> {
		match kind {
			OriginKind::Xcm => Ok(crate::Origin::Xcm(origin).into()),
			_ => Err(origin),
		}
	}
}
