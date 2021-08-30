// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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

//! Pallet to handle XCM messages.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use codec::{Decode, Encode, EncodeLike};
use frame_support::traits::{Contains, EnsureOrigin, Get, OriginTrait};
use sp_runtime::{traits::{BadOrigin, Saturating}, RuntimeDebug};
use sp_std::{
	boxed::Box,
	convert::{TryFrom, TryInto},
	marker::PhantomData,
	prelude::*,
	result::Result,
	vec,
};
use xcm::prelude::*;
use xcm_executor::traits::ConvertOrigin;

use frame_support::PalletId;
pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{
		dispatch::{Dispatchable, GetDispatchInfo, PostDispatchInfo},
		pallet_prelude::*,
	};
	use frame_system::{pallet_prelude::*, Config as SysConfig};
	use sp_core::H256;
	use sp_runtime::traits::{AccountIdConversion, BlakeTwo256, BlockNumberProvider, Hash};
	use xcm_executor::{
		traits::{
			ClaimAssets, DropAssets, InvertLocation, OnResponse, VersionChangeNotifier,
			WeightBounds,
		},
		Assets,
	};

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
		type SendXcmOrigin: EnsureOrigin<<Self as SysConfig>::Origin, Success = MultiLocation>;

		/// The type used to actually dispatch an XCM to its destination.
		type XcmRouter: SendXcm;

		/// Required origin for executing XCM messages, including the teleport functionality. If successful,
		/// then it resolves to `MultiLocation` which exists as an interior location within this chain's XCM
		/// context.
		type ExecuteXcmOrigin: EnsureOrigin<<Self as SysConfig>::Origin, Success = MultiLocation>;

		/// Our XCM filter which messages to be executed using `XcmExecutor` must pass.
		type XcmExecuteFilter: Contains<(MultiLocation, Xcm<<Self as SysConfig>::Call>)>;

		/// Something to execute an XCM message.
		type XcmExecutor: ExecuteXcm<<Self as SysConfig>::Call>;

		/// Our XCM filter which messages to be teleported using the dedicated extrinsic must pass.
		type XcmTeleportFilter: Contains<(MultiLocation, Vec<MultiAsset>)>;

		/// Our XCM filter which messages to be reserve-transferred using the dedicated extrinsic must pass.
		type XcmReserveTransferFilter: Contains<(MultiLocation, Vec<MultiAsset>)>;

		/// Means of measuring the weight consumed by an XCM message locally.
		type Weigher: WeightBounds<<Self as SysConfig>::Call>;

		/// Means of inverting a location.
		type LocationInverter: InvertLocation;

		/// The outer `Origin` type.
		type Origin: From<Origin> + From<<Self as SysConfig>::Origin>;

		/// The outer `Call` type.
		type Call: Parameter
			+ GetDispatchInfo
			+ IsType<<Self as frame_system::Config>::Call>
			+ Dispatchable<Origin = <Self as Config>::Origin, PostInfo = PostDispatchInfo>;

		/// The maximum number of items that we can store in the version discovery queue.
		type VersionDiscoveryQueueSize: Get<u32>;
	}

	/// The maximum number of distinct assets allowed to be transferred in a single helper extrinsic.
	const MAX_ASSETS_FOR_TRANSFER: usize = 2;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Execution of an XCM message was attempted.
		///
		/// \[ outcome \]
		Attempted(xcm::latest::Outcome),
		/// A XCM message was sent.
		///
		/// \[ origin, destination, message \]
		Sent(MultiLocation, MultiLocation, Xcm<()>),
		/// Query response received which does not match a registered query. This may be because a
		/// matching query was never registered, it may be because it is a duplicate response, or
		/// because the query timed out.
		///
		/// \[ origin location, id \]
		UnexpectedResponse(MultiLocation, QueryId),
		/// Query response has been received and is ready for taking with `take_response`. There is
		/// no registered notification call.
		///
		/// \[ id, response \]
		ResponseReady(QueryId, Response),
		/// Query response has been received and query is removed. The registered notification has
		/// been dispatched and executed successfully.
		///
		/// \[ id, pallet index, call index \]
		Notified(QueryId, u8, u8),
		/// Query response has been received and query is removed. The registered notification could
		/// not be dispatched because the dispatch weight is greater than the maximum weight
		/// originally budgeted by this runtime for the query result.
		///
		/// \[ id, pallet index, call index, actual weight, max budgeted weight \]
		NotifyOverweight(QueryId, u8, u8, Weight, Weight),
		/// Query response has been received and query is removed. There was a general error with
		/// dispatching the notification call.
		///
		/// \[ id, pallet index, call index \]
		NotifyDispatchError(QueryId, u8, u8),
		/// Query response has been received and query is removed. The dispatch was unable to be
		/// decoded into a `Call`; this might be due to dispatch function having a signature which
		/// is not `(origin, QueryId, Response)`.
		///
		/// \[ id, pallet index, call index \]
		NotifyDecodeFailed(QueryId, u8, u8),
		/// Expected query response has been received but the origin location of the response does
		/// not match that expected. The query remains registered for a later, valid, response to
		/// be received and acted upon.
		///
		/// \[ origin location, id, expected location \]
		InvalidResponder(MultiLocation, QueryId, Option<MultiLocation>),
		/// Expected query response has been received but the expected origin location placed in
		/// storate by this runtime previously cannot be decoded. The query remains registered.
		///
		/// This is unexpected (since a location placed in storage in a previously executing
		/// runtime should be readable prior to query timeout) and dangerous since the possibly
		/// valid response will be dropped. Manual governance intervention is probably going to be
		/// needed.
		///
		/// \[ origin location, id \]
		InvalidResponderVersion(MultiLocation, QueryId),
		/// Received query response has been read and removed.
		///
		/// \[ id \]
		ResponseTaken(QueryId),
		/// Some assets have been placed in an asset trap.
		///
		/// \[ hash, origin, assets \]
		AssetsTrapped(H256, MultiLocation, VersionedMultiAssets),
		/// An XCM version change notification message has been attempted to be sent.
		///
		/// \[ destination, result \]
		VersionChangeNotified(MultiLocation),
		/// The supported version of a location has been changed. This might be through an
		/// automatic notification or a manual intervention.
		///
		/// \[ location, XCM version \]
		SupportedVersionChanged(MultiLocation, XcmVersion),
		/// A given location which had a version change subscription was dropped owing to an error
		/// either migrating the location to our new XCM format or sending the notification to it.
		///
		/// \[ location, query ID, error type \]
		NotifyTargetDropped(VersionedMultiLocation, QueryId, XcmError),
	}

	#[pallet::origin]
	#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug)]
	pub enum Origin {
		/// It comes from somewhere in the XCM space wanting to transact.
		Xcm(MultiLocation),
		/// It comes as an expected response from an XCM location.
		Response(MultiLocation),
	}
	impl From<MultiLocation> for Origin {
		fn from(location: MultiLocation) -> Origin {
			Origin::Xcm(location)
		}
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The desired destination was unreachable, generally because there is a no way of routing
		/// to it.
		Unreachable,
		/// There was some other issue (i.e. not to do with routing) in sending the message. Perhaps
		/// a lack of space for buffering the message.
		SendFailure,
		/// The message execution fails the filter.
		Filtered,
		/// The message's weight could not be determined.
		UnweighableMessage,
		/// The destination `MultiLocation` provided cannot be inverted.
		DestinationNotInvertible,
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
		/// The given location could not be used (e.g. because it cannot be expressed in the
		/// desired version of XCM).
		BadLocation,
		/// The referenced subscription could not be found.
		NoSubscription,
	}

	/// The status of a query.
	#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
	pub enum QueryStatus<BlockNumber> {
		/// The query was sent but no response has yet been received.
		Pending {
			responder: VersionedMultiLocation,
			maybe_notify: Option<(u8, u8)>,
			timeout: BlockNumber,
		},
		/// The query is for an ongoing version notification subscription.
		VersionNotifier { origin: VersionedMultiLocation, is_active: bool },
		/// A response has been received.
		Ready { response: VersionedResponse, at: BlockNumber },
	}

	#[derive(Copy, Clone)]
	struct LatestVersionedMultiLocation<'a>(&'a MultiLocation);
	impl<'a> EncodeLike<VersionedMultiLocation> for LatestVersionedMultiLocation<'a> {}
	impl<'a> Encode for LatestVersionedMultiLocation<'a> {
		fn encode(&self) -> Vec<u8> {
			let mut r = vec![XCM_VERSION as u8];
			self.0.using_encoded(|d| r.extend_from_slice(d));
			r
		}
	}

	/// The latest available query index.
	#[pallet::storage]
	pub(super) type QueryCount<T: Config> = StorageValue<_, QueryId, ValueQuery>;

	/// The ongoing queries.
	#[pallet::storage]
	#[pallet::getter(fn query)]
	pub(super) type Queries<T: Config> =
		StorageMap<_, Blake2_128Concat, QueryId, QueryStatus<T::BlockNumber>, OptionQuery>;

	/// The existing asset traps.
	///
	/// Key is the blake2 256 hash of (origin, versioned `MultiAssets`) pair. Value is the number of
	/// times this pair has been trapped (usually just 1 if it exists at all).
	#[pallet::storage]
	#[pallet::getter(fn asset_trap)]
	pub(super) type AssetTraps<T: Config> = StorageMap<_, Identity, H256, u32, ValueQuery>;

	/// Default version to encode XCM when latest version of destination is unknown. If `None`,
	/// then the destinations whose XCM version is unknown are considered unreachable.
	#[pallet::storage]
	pub(super) type SafeXcmVersion<T: Config> = StorageValue<_, XcmVersion, OptionQuery>;

	/// Latest versions that we know various locations support.
	#[pallet::storage]
	pub(super) type SupportedVersion<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		XcmVersion,
		Blake2_128Concat,
		VersionedMultiLocation,
		XcmVersion,
		OptionQuery,
	>;

	/// All locations that we have requested version notifications from.
	#[pallet::storage]
	pub(super) type VersionNotifiers<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		XcmVersion,
		Blake2_128Concat,
		VersionedMultiLocation,
		QueryId,
		OptionQuery,
	>;

	/// Latest versions that we know various locations support.
	#[pallet::storage]
	pub(super) type VersionNotifyTargets<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		XcmVersion,
		Blake2_128Concat,
		VersionedMultiLocation,
		(QueryId, u64),
		OptionQuery,
	>;

	/// Destinations whose latest XCM version we would like to know. Duplicates allowed. The block
	/// number is the most recent block which pushed to it.
	#[pallet::storage]
	pub(super) type VersionDiscoveryQueue<T: Config> = StorageValue<
		_,
		BoundedVec<(VersionedMultiLocation, u32), T::VersionDiscoveryQueueSize>,
		ValueQuery,
	>;

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
			// Here we aim to get one successful version negotiation request sent per block, ordered
			// by the destinations being most sent to.
			let mut q = VersionDiscoveryQueue::<T>::get().into_inner();
			q.sort_by_key(|i| i.1);
			while let Some((versioned_dest, _)) = q.pop() {
				if let Ok(dest) = versioned_dest.try_into() {
					if Self::request_version_notify(dest).is_ok() {
						break;
					}
				}
			}
			// Should never fail since we only removed items. But better safe than panicking as it's
			// way better to drop the queue than panic on initialize.
			if let Ok(q) = BoundedVec::try_from(q) {
				VersionDiscoveryQueue::<T>::put(q);
			}
			// TODO: correct weight.
			0
		}
		fn on_runtime_upgrade() -> Weight {
			for v in 0..XCM_VERSION {
				for (old_key, value) in SupportedVersion::<T>::drain_prefix(v) {
					if let Ok(new_key) = old_key.into_latest() {
						SupportedVersion::<T>::insert(XCM_VERSION, new_key, value);
					}
				}
				for (old_key, value) in VersionNotifiers::<T>::drain_prefix(v) {
					if let Ok(new_key) = old_key.into_latest() {
						VersionNotifiers::<T>::insert(XCM_VERSION, new_key, value);
					}
				}
				let response = Response::Version(XCM_VERSION);
				for (old_key, value) in VersionNotifyTargets::<T>::drain_prefix(v) {
					let new_key = match MultiLocation::try_from(old_key.clone()) {
						Ok(k) => k,
						Err(()) => {
							Self::deposit_event(Event::NotifyTargetDropped(
								old_key,
								value.0,
								XcmError::InvalidLocation,
							));
							return 0
						},
					};
					let instruction = QueryResponse {
						query_id: value.0,
						response: response.clone(),
						max_weight: value.1,
					};
					match T::XcmRouter::send_xcm(new_key.clone(), Xcm(vec![instruction])) {
						Ok(()) => {
							VersionNotifyTargets::<T>::insert(
								XCM_VERSION,
								LatestVersionedMultiLocation(&new_key),
								value,
							);
							Self::deposit_event(Event::VersionChangeNotified(new_key));
						},
						Err(e) => {
							let new_key = new_key.into();
							Self::deposit_event(Event::NotifyTargetDropped(
								new_key,
								value.0,
								e.into(),
							));
						},
					}
				}
			}
			0
		}
	}

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
				SendError::CannotReachDestination(..) => Error::<T>::Unreachable,
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
					use sp_std::vec;
					let mut message = Xcm(vec![
						WithdrawAsset(assets),
						InitiateTeleport { assets: Wild(All), dest, xcm: Xcm(vec![]) },
					]);
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
			let inv_dest = T::LocationInverter::invert_location(&dest)
				.map_err(|()| Error::<T>::DestinationNotInvertible)?;
			let fees = assets
				.get(fee_asset_item as usize)
				.ok_or(Error::<T>::Empty)?
				.clone()
				.reanchored(&inv_dest)
				.map_err(|_| Error::<T>::CannotReanchor)?;
			let max_assets = assets.len() as u32;
			let assets = assets.into();
			let mut message = Xcm(vec![
				WithdrawAsset(assets),
				InitiateTeleport {
					assets: Wild(All),
					dest,
					xcm: Xcm(vec![
						BuyExecution { fees, weight_limit: Unlimited },
						DepositAsset { assets: Wild(All), max_assets, beneficiary },
					]),
				},
			]);
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
		/// - `fee_asset_item`: The index into `assets` of the item which should be used to pay
		///   fees.
		#[pallet::weight({
			match ((*assets.clone()).try_into(), (*dest.clone()).try_into()) {
				(Ok(assets), Ok(dest)) => {
					use sp_std::vec;
					let mut message = Xcm(vec![
						TransferReserveAsset { assets, dest, xcm: Xcm(vec![]) }
					]);
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
		) -> DispatchResult {
			let origin_location = T::ExecuteXcmOrigin::ensure_origin(origin)?;
			let dest = (*dest).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let beneficiary = (*beneficiary).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let assets: MultiAssets = (*assets).try_into().map_err(|()| Error::<T>::BadVersion)?;

			ensure!(assets.len() <= MAX_ASSETS_FOR_TRANSFER, Error::<T>::TooManyAssets);
			let value = (origin_location, assets.drain());
			ensure!(T::XcmReserveTransferFilter::contains(&value), Error::<T>::Filtered);
			let (origin_location, assets) = value;
			let inv_dest = T::LocationInverter::invert_location(&dest)
				.map_err(|()| Error::<T>::DestinationNotInvertible)?;
			let fees = assets
				.get(fee_asset_item as usize)
				.ok_or(Error::<T>::Empty)?
				.clone()
				.reanchored(&inv_dest)
				.map_err(|_| Error::<T>::CannotReanchor)?;
			let max_assets = assets.len() as u32;
			let assets = assets.into();
			let mut message = Xcm(vec![TransferReserveAsset {
				assets,
				dest,
				xcm: Xcm(vec![
					BuyExecution { fees, weight_limit: Unlimited },
					DepositAsset { assets: Wild(All), max_assets, beneficiary },
				]),
			}]);
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
			message: Box<VersionedXcm<<T as SysConfig>::Call>>,
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

		/// Extoll that a particular destination can be communicated with through a particular
		/// version of XCM.
		///
		/// - `origin`: Must be Root.
		/// - `location`: The destination that is being described.
		/// - `xcm_version`: The latest version of XCM that `location` supports.
		///
		/// Errors:
		/// - `BadLocation`: If the `location` cannot be expressed with the XCM version used by this
		///   chain.
		#[pallet::weight(100_000_000u64)]
		pub fn force_xcm_version(
			origin: OriginFor<T>,
			location: Box<MultiLocation>,
			xcm_version: XcmVersion,
		) -> DispatchResult {
			ensure_root(origin)?;
			let location = *location;
			SupportedVersion::<T>::insert(
				XCM_VERSION,
				LatestVersionedMultiLocation(&location),
				xcm_version,
			);
			Self::deposit_event(Event::SupportedVersionChanged(location, xcm_version));
			Ok(())
		}

		/// Set a safe XCM version (the version that XCM should be encoded with if the most recent
		/// version a destination can accept is unknown).
		///
		/// - `origin`: Must be Root.
		/// - `maybe_xcm_version`: The default XCM encoding version, or `None` to disable.
		#[pallet::weight(100_000_000u64)]
		pub fn force_default_xcm_version(
			origin: OriginFor<T>,
			maybe_xcm_version: Option<XcmVersion>,
		) -> DispatchResult {
			ensure_root(origin)?;
			SafeXcmVersion::<T>::set(maybe_xcm_version);
			Ok(())
		}

		/// Require that a particular destination should no longer notify us regarding any XCM
		/// version changes.
		///
		/// - `origin`: Must be Root.
		/// - `location`: The location to which we are currently subscribed for XCM version
		///   notifications which we no longer desire.
		#[pallet::weight(100_000_000u64)]
		pub fn force_unsubscribe_version_notify(
			origin: OriginFor<T>,
			location: Box<MultiLocation>,
		) -> DispatchResult {
			ensure_root(origin)?;
			let location = *location;
			Self::unrequest_version_notify(location).map_err(|e| {
				match e {
					XcmError::InvalidLocation => Error::<T>::NoSubscription,
					_ => Error::<T>::InvalidOrigin,
				}
				.into()
			})
		}
	}

	impl<T: Config> Pallet<T> {
		/// Request that `dest` informs us of its version.
		pub fn request_version_notify(dest: MultiLocation) -> XcmResult {
			let query_id = QueryCount::<T>::mutate(|q| {
				let r = *q;
				*q += 1;
				r
			});
			// TODO #3735: Correct weight.
			let instruction = SubscribeVersion { query_id, max_response_weight: 0 };
			T::XcmRouter::send_xcm(dest.clone(), Xcm(vec![instruction]))?;
			let versioned_dest = VersionedMultiLocation::from(dest);
			VersionNotifiers::<T>::insert(XCM_VERSION, &versioned_dest, query_id);
			let query_status =
				QueryStatus::VersionNotifier { origin: versioned_dest, is_active: false };
			Queries::<T>::insert(query_id, query_status);
			Ok(())
		}

		/// Request that `dest` ceases informing us of its version.
		pub fn unrequest_version_notify(dest: MultiLocation) -> XcmResult {
			let versioned_dest = LatestVersionedMultiLocation(&dest);
			let query_id = VersionNotifiers::<T>::take(XCM_VERSION, versioned_dest)
				.ok_or(XcmError::InvalidLocation)?;
			T::XcmRouter::send_xcm(dest.clone(), Xcm(vec![UnsubscribeVersion]))?;
			Queries::<T>::remove(query_id);
			Ok(())
		}

		// TODO: Barriers to let through Subscribe/UnsubscribeVersion single instruction messages.
		// TODO: Weight for Subscribe/UnsubscribeVersion should be high to ensure non-first-class
		//   locations can use them.

		/// Relay an XCM `message` from a given `interior` location in this context to a given `dest`
		/// location. A null `dest` is not handled.
		pub fn send_xcm(
			interior: Junctions,
			dest: MultiLocation,
			mut message: Xcm<()>,
		) -> Result<(), SendError> {
			if interior != Junctions::Here {
				message.0.insert(0, DescendOrigin(interior))
			};
			log::trace!(target: "xcm::send_xcm", "dest: {:?}, message: {:?}", &dest, &message);
			T::XcmRouter::send_xcm(dest, message)
		}

		pub fn check_account() -> T::AccountId {
			const ID: PalletId = PalletId(*b"py/xcmch");
			AccountIdConversion::<T::AccountId>::into_account(&ID)
		}

		fn do_new_query(
			responder: MultiLocation,
			maybe_notify: Option<(u8, u8)>,
			timeout: T::BlockNumber,
		) -> u64 {
			QueryCount::<T>::mutate(|q| {
				let r = *q;
				*q += 1;
				Queries::<T>::insert(
					r,
					QueryStatus::Pending { responder: responder.into(), maybe_notify, timeout },
				);
				r
			})
		}

		/// Consume `message` and return another which is equivalent to it except that it reports
		/// back the outcome.
		///
		/// - `message`: The message whose outcome should be reported.
		/// - `responder`: The origin from which a response should be expected.
		/// - `timeout`: The block number after which it is permissible for `notify` not to be
		///   called even if a response is received.
		///
		/// `report_outcome` may return an error if the `responder` is not invertible.
		///
		/// To check the status of the query, use `fn query()` passing the resultant `QueryId`
		/// value.
		pub fn report_outcome(
			message: &mut Xcm<()>,
			responder: MultiLocation,
			timeout: T::BlockNumber,
		) -> Result<QueryId, XcmError> {
			let dest = T::LocationInverter::invert_location(&responder)
				.map_err(|()| XcmError::MultiLocationNotInvertible)?;
			let query_id = Self::new_query(responder, timeout);
			let report_error = Xcm(vec![ReportError { dest, query_id, max_response_weight: 0 }]);
			message.0.insert(0, SetAppendix(report_error));
			Ok(query_id)
		}

		/// Consume `message` and return another which is equivalent to it except that it reports
		/// back the outcome and dispatches `notify` on this chain.
		///
		/// - `message`: The message whose outcome should be reported.
		/// - `responder`: The origin from which a response should be expected.
		/// - `notify`: A dispatchable function which will be called once the outcome of `message`
		///   is known. It may be a dispatchable in any pallet of the local chain, but other than
		///   the usual origin, it must accept exactly two arguments: `query_id: QueryId` and
		///   `outcome: Response`, and in that order. It should expect that the origin is
		///   `Origin::Response` and will contain the responder's location.
		/// - `timeout`: The block number after which it is permissible for `notify` not to be
		///   called even if a response is received.
		///
		/// `report_outcome_notify` may return an error if the `responder` is not invertible.
		///
		/// NOTE: `notify` gets called as part of handling an incoming message, so it should be
		/// lightweight. Its weight is estimated during this function and stored ready for
		/// weighing `ReportOutcome` on the way back. If it turns out to be heavier once it returns
		/// then reporting the outcome will fail. Futhermore if the estimate is too high, then it
		/// may be put in the overweight queue and need to be manually executed.
		pub fn report_outcome_notify(
			message: &mut Xcm<()>,
			responder: MultiLocation,
			notify: impl Into<<T as Config>::Call>,
			timeout: T::BlockNumber,
		) -> Result<(), XcmError> {
			let dest = T::LocationInverter::invert_location(&responder)
				.map_err(|()| XcmError::MultiLocationNotInvertible)?;
			let notify: <T as Config>::Call = notify.into();
			let max_response_weight = notify.get_dispatch_info().weight;
			let query_id = Self::new_notify_query(responder, notify, timeout);
			let report_error = Xcm(vec![ReportError { dest, query_id, max_response_weight }]);
			message.0.insert(0, SetAppendix(report_error));
			Ok(())
		}

		/// Attempt to create a new query ID and register it as a query that is yet to respond.
		pub fn new_query(responder: MultiLocation, timeout: T::BlockNumber) -> u64 {
			Self::do_new_query(responder, None, timeout)
		}

		/// Attempt to create a new query ID and register it as a query that is yet to respond, and
		/// which will call a dispatchable when a response happens.
		pub fn new_notify_query(
			responder: MultiLocation,
			notify: impl Into<<T as Config>::Call>,
			timeout: T::BlockNumber,
		) -> u64 {
			let notify =
				notify.into().using_encoded(|mut bytes| Decode::decode(&mut bytes)).expect(
					"decode input is output of Call encode; Call guaranteed to have two enums; qed",
				);
			Self::do_new_query(responder, Some(notify), timeout)
		}

		/// Attempt to remove and return the response of query with ID `query_id`.
		///
		/// Returns `None` if the response is not (yet) available.
		pub fn take_response(query_id: QueryId) -> Option<(Response, T::BlockNumber)> {
			if let Some(QueryStatus::Ready { response, at }) = Queries::<T>::get(query_id) {
				let response = response.try_into().ok()?;
				Queries::<T>::remove(query_id);
				Self::deposit_event(Event::ResponseTaken(query_id));
				Some((response, at))
			} else {
				None
			}
		}

		/// Note that a particular destination to whom we would like to send a message is unknown
		/// and queue it for version discovery.
		fn note_unknown_version(dest: &MultiLocation) {
			let versioned_dest = VersionedMultiLocation::from(dest.clone());
			VersionDiscoveryQueue::<T>::mutate(|q| {
				if let Some(index) = q.iter().position(|i| &i.0 == &versioned_dest) {
					// exists - just bump the count.
					q[index].1.saturating_inc();
				} else {
					let _ = q.try_push((versioned_dest, 0));
				}
			});
		}
	}

	impl<T: Config> WrapVersion for Pallet<T> {
		fn wrap_version<Call>(
			dest: &MultiLocation,
			xcm: impl Into<VersionedXcm<Call>>,
		) -> Result<VersionedXcm<Call>, ()> {
			SupportedVersion::<T>::get(XCM_VERSION, LatestVersionedMultiLocation(dest))
				.or_else(|| {
					Self::note_unknown_version(dest);
					SafeXcmVersion::<T>::get()
				})
				.ok_or(())
				.and_then(|v| xcm.into().into_version(v))
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
		fn start(dest: &MultiLocation, query_id: QueryId, max_weight: u64) -> XcmResult {
			let versioned_dest = LatestVersionedMultiLocation(dest);
			let already = VersionNotifyTargets::<T>::contains_key(XCM_VERSION, versioned_dest);
			ensure!(!already, XcmError::InvalidLocation);
			VersionNotifyTargets::<T>::insert(XCM_VERSION, versioned_dest, (query_id, max_weight));
			Ok(())
		}

		/// Stop notifying `location` should the XCM change. Returns an error if there is no existing
		/// notification set up.
		fn stop(dest: &MultiLocation) -> XcmResult {
			let versioned_dest = LatestVersionedMultiLocation(dest);
			let already = VersionNotifyTargets::<T>::contains_key(XCM_VERSION, versioned_dest);
			ensure!(already, XcmError::InvalidLocation);
			VersionNotifyTargets::<T>::remove(XCM_VERSION, versioned_dest);
			Ok(())
		}
	}
	impl<T: Config> DropAssets for Pallet<T> {
		fn drop_assets(origin: &MultiLocation, assets: Assets) -> Weight {
			if assets.is_empty() {
				return 0
			}
			let versioned = VersionedMultiAssets::from(MultiAssets::from(assets));
			let hash = BlakeTwo256::hash_of(&(&origin, &versioned));
			AssetTraps::<T>::mutate(hash, |n| *n += 1);
			Self::deposit_event(Event::AssetsTrapped(hash, origin.clone(), versioned));
			// TODO #3735: Put the real weight in there.
			0
		}
	}

	impl<T: Config> ClaimAssets for Pallet<T> {
		fn claim_assets(
			origin: &MultiLocation,
			ticket: &MultiLocation,
			assets: &MultiAssets,
		) -> bool {
			let mut versioned = VersionedMultiAssets::from(assets.clone());
			match (ticket.parents, &ticket.interior) {
				(0, X1(GeneralIndex(i))) =>
					versioned = match versioned.into_version(*i as u32) {
						Ok(v) => v,
						Err(()) => return false,
					},
				(0, Here) => (),
				_ => return false,
			};
			let hash = BlakeTwo256::hash_of(&(origin, versioned));
			match AssetTraps::<T>::get(hash) {
				0 => return false,
				1 => AssetTraps::<T>::remove(hash),
				n => AssetTraps::<T>::insert(hash, n - 1),
			}
			return true
		}
	}

	impl<T: Config> OnResponse for Pallet<T> {
		fn expecting_response(origin: &MultiLocation, query_id: QueryId) -> bool {
			if let Some(QueryStatus::Pending { responder, .. }) = Queries::<T>::get(query_id) {
				return MultiLocation::try_from(responder).map_or(false, |r| origin == &r)
			}
			false
		}

		fn on_response(
			origin: &MultiLocation,
			query_id: QueryId,
			response: Response,
			max_weight: Weight,
		) -> Weight {
			match (response, Queries::<T>::get(query_id)) {
				(
					Response::Version(v),
					Some(QueryStatus::VersionNotifier { origin: expected_origin, is_active }),
				) => {
					let origin: MultiLocation = match expected_origin.try_into() {
						Ok(o) if &o == origin => o,
						Ok(o) => {
							Self::deposit_event(Event::InvalidResponder(
								origin.clone(),
								query_id,
								Some(o),
							));
							return 0
						},
						_ => {
							Self::deposit_event(Event::InvalidResponder(
								origin.clone(),
								query_id,
								None,
							));
							// TODO #3735: Correct weight for this.
							return 0
						},
					};
					// TODO #3735: Check max_weight is correct.
					if !is_active {
						Queries::<T>::insert(
							query_id,
							QueryStatus::VersionNotifier {
								origin: origin.clone().into(),
								is_active: true,
							},
						);
					}
					// We're being notified of a version change.
					SupportedVersion::<T>::insert(
						XCM_VERSION,
						LatestVersionedMultiLocation(&origin),
						v,
					);
					Self::deposit_event(Event::SupportedVersionChanged(origin, v));
					0
				},
				(response, Some(QueryStatus::Pending { responder, maybe_notify, .. })) => {
					let responder = match MultiLocation::try_from(responder) {
						Ok(r) => r,
						Err(_) => {
							Self::deposit_event(Event::InvalidResponderVersion(
								origin.clone(),
								query_id,
							));
							return 0
						},
					};
					if origin != &responder {
						Self::deposit_event(Event::InvalidResponder(
							origin.clone(),
							query_id,
							Some(responder),
						));
						return 0
					}
					return match maybe_notify {
						Some((pallet_index, call_index)) => {
							// This is a bit horrible, but we happen to know that the `Call` will
							// be built by `(pallet_index: u8, call_index: u8, QueryId, Response)`.
							// So we just encode that and then re-encode to a real Call.
							let bare = (pallet_index, call_index, query_id, response);
							if let Ok(call) = bare
								.using_encoded(|mut bytes| <T as Config>::Call::decode(&mut bytes))
							{
								Queries::<T>::remove(query_id);
								let weight = call.get_dispatch_info().weight;
								if weight > max_weight {
									let e = Event::NotifyOverweight(
										query_id,
										pallet_index,
										call_index,
										weight,
										max_weight,
									);
									Self::deposit_event(e);
									return 0
								}
								let dispatch_origin = Origin::Response(origin.clone()).into();
								match call.dispatch(dispatch_origin) {
									Ok(post_info) => {
										let e = Event::Notified(query_id, pallet_index, call_index);
										Self::deposit_event(e);
										post_info.actual_weight
									},
									Err(error_and_info) => {
										let e = Event::NotifyDispatchError(
											query_id,
											pallet_index,
											call_index,
										);
										Self::deposit_event(e);
										// Not much to do with the result as it is. It's up to the parachain to ensure that the
										// message makes sense.
										error_and_info.post_info.actual_weight
									},
								}
								.unwrap_or(weight)
							} else {
								let e =
									Event::NotifyDecodeFailed(query_id, pallet_index, call_index);
								Self::deposit_event(e);
								0
							}
						},
						None => {
							let e = Event::ResponseReady(query_id, response.clone());
							Self::deposit_event(e);
							let at = frame_system::Pallet::<T>::current_block_number();
							let response = response.into();
							Queries::<T>::insert(query_id, QueryStatus::Ready { response, at });
							0
						},
					}
				},
				_ => {
					Self::deposit_event(Event::UnexpectedResponse(origin.clone(), query_id));
					return 0
				},
			}
		}
	}
}

/// Ensure that the origin `o` represents an XCM (`Transact`) origin.
///
/// Returns `Ok` with the location of the XCM sender or an `Err` otherwise.
pub fn ensure_xcm<OuterOrigin>(o: OuterOrigin) -> Result<MultiLocation, BadOrigin>
where
	OuterOrigin: Into<Result<Origin, OuterOrigin>>,
{
	match o.into() {
		Ok(Origin::Xcm(location)) => Ok(location),
		_ => Err(BadOrigin),
	}
}

/// Ensure that the origin `o` represents an XCM response origin.
///
/// Returns `Ok` with the location of the responder or an `Err` otherwise.
pub fn ensure_response<OuterOrigin>(o: OuterOrigin) -> Result<MultiLocation, BadOrigin>
where
	OuterOrigin: Into<Result<Origin, OuterOrigin>>,
{
	match o.into() {
		Ok(Origin::Response(location)) => Ok(location),
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
			caller.try_into().and_then(|o| match o {
				Origin::Xcm(location) if F::contains(&location) => Ok(location),
				Origin::Xcm(location) => Err(Origin::Xcm(location).into()),
				o => Err(o.into()),
			})
		})
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> O {
		O::from(Origin::Xcm(Here.into()))
	}
}

/// `EnsureOrigin` implementation succeeding with a `MultiLocation` value to recognize and filter
/// the `Origin::Response` item.
pub struct EnsureResponse<F>(PhantomData<F>);
impl<O: OriginTrait + From<Origin>, F: Contains<MultiLocation>> EnsureOrigin<O>
	for EnsureResponse<F>
where
	O::PalletsOrigin: From<Origin> + TryInto<Origin, Error = O::PalletsOrigin>,
{
	type Success = MultiLocation;

	fn try_origin(outer: O) -> Result<Self::Success, O> {
		outer.try_with_caller(|caller| {
			caller.try_into().and_then(|o| match o {
				Origin::Response(responder) => Ok(responder),
				o => Err(o.into()),
			})
		})
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn successful_origin() -> O {
		O::from(Origin::Response(Here.into()))
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
