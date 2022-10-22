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

use codec::{Decode, Encode, EncodeLike, MaxEncodedLen};
use frame_support::traits::{Contains, EnsureOrigin, Get, OriginTrait};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{BadOrigin, Saturating},
	RuntimeDebug,
};
use sp_std::{boxed::Box, marker::PhantomData, prelude::*, result::Result, vec};
use xcm::{latest::Weight as XcmWeight, prelude::*};
use xcm_executor::traits::ConvertOrigin;

use frame_support::PalletId;
pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::{
		dispatch::{Dispatchable, GetDispatchInfo, PostDispatchInfo},
		pallet_prelude::*,
		parameter_types,
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

	parameter_types! {
		/// An implementation of `Get<u32>` which just returns the latest XCM version which we can
		/// support.
		pub const CurrentXcmVersion: u32 = XCM_VERSION;
	}

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	/// The module configuration trait.
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Required origin for sending XCM messages. If successful, it resolves to `MultiLocation`
		/// which exists as an interior location within this chain's XCM context.
		type SendXcmOrigin: EnsureOrigin<
			<Self as SysConfig>::RuntimeOrigin,
			Success = MultiLocation,
		>;

		/// The type used to actually dispatch an XCM to its destination.
		type XcmRouter: SendXcm;

		/// Required origin for executing XCM messages, including the teleport functionality. If successful,
		/// then it resolves to `MultiLocation` which exists as an interior location within this chain's XCM
		/// context.
		type ExecuteXcmOrigin: EnsureOrigin<
			<Self as SysConfig>::RuntimeOrigin,
			Success = MultiLocation,
		>;

		/// Our XCM filter which messages to be executed using `XcmExecutor` must pass.
		type XcmExecuteFilter: Contains<(MultiLocation, Xcm<<Self as SysConfig>::RuntimeCall>)>;

		/// Something to execute an XCM message.
		type XcmExecutor: ExecuteXcm<<Self as SysConfig>::RuntimeCall>;

		/// Our XCM filter which messages to be teleported using the dedicated extrinsic must pass.
		type XcmTeleportFilter: Contains<(MultiLocation, Vec<MultiAsset>)>;

		/// Our XCM filter which messages to be reserve-transferred using the dedicated extrinsic must pass.
		type XcmReserveTransferFilter: Contains<(MultiLocation, Vec<MultiAsset>)>;

		/// Means of measuring the weight consumed by an XCM message locally.
		type Weigher: WeightBounds<<Self as SysConfig>::RuntimeCall>;

		/// Means of inverting a location.
		type LocationInverter: InvertLocation;

		/// The outer `Origin` type.
		type RuntimeOrigin: From<Origin> + From<<Self as SysConfig>::RuntimeOrigin>;

		/// The outer `Call` type.
		type RuntimeCall: Parameter
			+ GetDispatchInfo
			+ IsType<<Self as frame_system::Config>::RuntimeCall>
			+ Dispatchable<
				RuntimeOrigin = <Self as Config>::RuntimeOrigin,
				PostInfo = PostDispatchInfo,
			>;

		const VERSION_DISCOVERY_QUEUE_SIZE: u32;

		/// The latest supported version that we advertise. Generally just set it to
		/// `pallet_xcm::CurrentXcmVersion`.
		type AdvertisedXcmVersion: Get<XcmVersion>;
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
		/// storage by this runtime previously cannot be decoded. The query remains registered.
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
		VersionChangeNotified(MultiLocation, XcmVersion),
		/// The supported version of a location has been changed. This might be through an
		/// automatic notification or a manual intervention.
		///
		/// \[ location, XCM version \]
		SupportedVersionChanged(MultiLocation, XcmVersion),
		/// A given location which had a version change subscription was dropped owing to an error
		/// sending the notification to it.
		///
		/// \[ location, query ID, error \]
		NotifyTargetSendFail(MultiLocation, QueryId, XcmError),
		/// A given location which had a version change subscription was dropped owing to an error
		/// migrating the location to our new XCM format.
		///
		/// \[ location, query ID \]
		NotifyTargetMigrationFail(VersionedMultiLocation, QueryId),
		/// Some assets have been claimed from an asset trap
		///
		/// \[ hash, origin, assets \]
		AssetsClaimed(H256, MultiLocation, VersionedMultiAssets),
	}

	#[pallet::origin]
	#[derive(PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, TypeInfo, MaxEncodedLen)]
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
		/// The location is invalid since it already has a subscription from us.
		AlreadySubscribed,
	}

	/// The status of a query.
	#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug, TypeInfo)]
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
	pub(crate) struct LatestVersionedMultiLocation<'a>(pub(crate) &'a MultiLocation);
	impl<'a> EncodeLike<VersionedMultiLocation> for LatestVersionedMultiLocation<'a> {}
	impl<'a> Encode for LatestVersionedMultiLocation<'a> {
		fn encode(&self) -> Vec<u8> {
			let mut r = VersionedMultiLocation::from(MultiLocation::default()).encode();
			r.truncate(1);
			self.0.using_encoded(|d| r.extend_from_slice(d));
			r
		}
	}

	#[derive(Clone, Encode, Decode, Eq, PartialEq, Ord, PartialOrd, TypeInfo)]
	pub enum VersionMigrationStage {
		MigrateSupportedVersion,
		MigrateVersionNotifiers,
		NotifyCurrentTargets(Option<Vec<u8>>),
		MigrateAndNotifyOldTargets,
	}

	impl Default for VersionMigrationStage {
		fn default() -> Self {
			Self::MigrateSupportedVersion
		}
	}

	/// The latest available query index.
	#[pallet::storage]
	pub(super) type QueryCounter<T: Config> = StorageValue<_, QueryId, ValueQuery>;

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

	/// The Latest versions that we know various locations support.
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

	/// The target locations that are subscribed to our version changes, as well as the most recent
	/// of our versions we informed them of.
	#[pallet::storage]
	pub(super) type VersionNotifyTargets<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		XcmVersion,
		Blake2_128Concat,
		VersionedMultiLocation,
		(QueryId, u64, XcmVersion),
		OptionQuery,
	>;

	pub struct VersionDiscoveryQueueSize<T>(PhantomData<T>);
	impl<T: Config> Get<u32> for VersionDiscoveryQueueSize<T> {
		fn get() -> u32 {
			T::VERSION_DISCOVERY_QUEUE_SIZE
		}
	}

	/// Destinations whose latest XCM version we would like to know. Duplicates not allowed, and
	/// the `u32` counter is the number of times that a send to the destination has been attempted,
	/// which is used as a prioritization.
	#[pallet::storage]
	pub(super) type VersionDiscoveryQueue<T: Config> = StorageValue<
		_,
		BoundedVec<(VersionedMultiLocation, u32), VersionDiscoveryQueueSize<T>>,
		ValueQuery,
	>;

	/// The current migration's stage, if any.
	#[pallet::storage]
	pub(super) type CurrentMigration<T: Config> =
		StorageValue<_, VersionMigrationStage, OptionQuery>;

	#[pallet::genesis_config]
	pub struct GenesisConfig {
		/// The default version to encode outgoing XCM messages with.
		pub safe_xcm_version: Option<XcmVersion>,
	}

	#[cfg(feature = "std")]
	impl Default for GenesisConfig {
		fn default() -> Self {
			Self { safe_xcm_version: Some(XCM_VERSION) }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig {
		fn build(&self) {
			SafeXcmVersion::<T>::set(self.safe_xcm_version);
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_n: BlockNumberFor<T>) -> Weight {
			let mut weight_used = Weight::zero();
			if let Some(migration) = CurrentMigration::<T>::get() {
				// Consume 10% of block at most
				let max_weight = T::BlockWeights::get().max_block / 10;
				let (w, maybe_migration) = Self::check_xcm_version_change(migration, max_weight);
				CurrentMigration::<T>::set(maybe_migration);
				weight_used.saturating_accrue(w);
			}

			// Here we aim to get one successful version negotiation request sent per block, ordered
			// by the destinations being most sent to.
			let mut q = VersionDiscoveryQueue::<T>::take().into_inner();
			// TODO: correct weights.
			weight_used.saturating_accrue(T::DbWeight::get().reads_writes(1, 1));
			q.sort_by_key(|i| i.1);
			while let Some((versioned_dest, _)) = q.pop() {
				if let Ok(dest) = MultiLocation::try_from(versioned_dest) {
					if Self::request_version_notify(dest).is_ok() {
						// TODO: correct weights.
						weight_used.saturating_accrue(T::DbWeight::get().reads_writes(1, 1));
						break
					}
				}
			}
			// Should never fail since we only removed items. But better safe than panicking as it's
			// way better to drop the queue than panic on initialize.
			if let Ok(q) = BoundedVec::try_from(q) {
				VersionDiscoveryQueue::<T>::put(q);
			}
			weight_used
		}
		fn on_runtime_upgrade() -> Weight {
			// Start a migration (this happens before on_initialize so it'll happen later in this
			// block, which should be good enough)...
			CurrentMigration::<T>::put(VersionMigrationStage::default());
			T::DbWeight::get().writes(1)
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
			let interior: Junctions =
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
		/// Fee payment on the destination side is made from the asset in the `assets` vector of
		/// index `fee_asset_item`. The weight limit for fees is not provided and thus is unlimited,
		/// with all fees taken as needed from the asset.
		///
		/// - `origin`: Must be capable of withdrawing the `assets` and executing XCM.
		/// - `dest`: Destination context for the assets. Will typically be `X2(Parent, Parachain(..))` to send
		///   from parachain to parachain, or `X1(Parachain(..))` to send from relay to parachain.
		/// - `beneficiary`: A beneficiary location for the assets in the context of `dest`. Will generally be
		///   an `AccountId32` value.
		/// - `assets`: The assets to be withdrawn. The first item should be the currency used to to pay the fee on the
		///   `dest` side. May not be empty.
		/// - `fee_asset_item`: The index into `assets` of the item which should be used to pay
		///   fees.
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
					T::Weigher::weight(&mut message).map_or(Weight::MAX, |w| Weight::from_ref_time(100_000_000.saturating_add(w)))
				},
				_ => Weight::MAX,
			}
		})]
		pub fn teleport_assets(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			beneficiary: Box<VersionedMultiLocation>,
			assets: Box<VersionedMultiAssets>,
			fee_asset_item: u32,
		) -> DispatchResult {
			Self::do_teleport_assets(origin, dest, beneficiary, assets, fee_asset_item, None)
		}

		/// Transfer some assets from the local chain to the sovereign account of a destination
		/// chain and forward a notification XCM.
		///
		/// Fee payment on the destination side is made from the asset in the `assets` vector of
		/// index `fee_asset_item`. The weight limit for fees is not provided and thus is unlimited,
		/// with all fees taken as needed from the asset.
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
					T::Weigher::weight(&mut message).map_or(Weight::MAX, |w| Weight::from_ref_time(100_000_000.saturating_add(w)))
				},
				_ => Weight::MAX,
			}
		})]
		pub fn reserve_transfer_assets(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			beneficiary: Box<VersionedMultiLocation>,
			assets: Box<VersionedMultiAssets>,
			fee_asset_item: u32,
		) -> DispatchResult {
			Self::do_reserve_transfer_assets(
				origin,
				dest,
				beneficiary,
				assets,
				fee_asset_item,
				None,
			)
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
		#[pallet::weight(Weight::from_ref_time(max_weight.saturating_add(100_000_000u64)))]
		pub fn execute(
			origin: OriginFor<T>,
			message: Box<VersionedXcm<<T as SysConfig>::RuntimeCall>>,
			max_weight: XcmWeight,
		) -> DispatchResultWithPostInfo {
			let origin_location = T::ExecuteXcmOrigin::ensure_origin(origin)?;
			let message = (*message).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let value = (origin_location, message);
			ensure!(T::XcmExecuteFilter::contains(&value), Error::<T>::Filtered);
			let (origin_location, message) = value;
			let outcome = T::XcmExecutor::execute_xcm_in_credit(
				origin_location,
				message,
				max_weight,
				max_weight,
			);
			let result = Ok(Some(outcome.weight_used().saturating_add(100_000_000)).into());
			Self::deposit_event(Event::Attempted(outcome));
			result
		}

		/// Extoll that a particular destination can be communicated with through a particular
		/// version of XCM.
		///
		/// - `origin`: Must be Root.
		/// - `location`: The destination that is being described.
		/// - `xcm_version`: The latest version of XCM that `location` supports.
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

		/// Ask a location to notify us regarding their XCM version and any changes to it.
		///
		/// - `origin`: Must be Root.
		/// - `location`: The location to which we should subscribe for XCM version notifications.
		#[pallet::weight(100_000_000u64)]
		pub fn force_subscribe_version_notify(
			origin: OriginFor<T>,
			location: Box<VersionedMultiLocation>,
		) -> DispatchResult {
			ensure_root(origin)?;
			let location: MultiLocation =
				(*location).try_into().map_err(|()| Error::<T>::BadLocation)?;
			Self::request_version_notify(location).map_err(|e| {
				match e {
					XcmError::InvalidLocation => Error::<T>::AlreadySubscribed,
					_ => Error::<T>::InvalidOrigin,
				}
				.into()
			})
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
			location: Box<VersionedMultiLocation>,
		) -> DispatchResult {
			ensure_root(origin)?;
			let location: MultiLocation =
				(*location).try_into().map_err(|()| Error::<T>::BadLocation)?;
			Self::unrequest_version_notify(location).map_err(|e| {
				match e {
					XcmError::InvalidLocation => Error::<T>::NoSubscription,
					_ => Error::<T>::InvalidOrigin,
				}
				.into()
			})
		}

		/// Transfer some assets from the local chain to the sovereign account of a destination
		/// chain and forward a notification XCM.
		///
		/// Fee payment on the destination side is made from the asset in the `assets` vector of
		/// index `fee_asset_item`, up to enough to pay for `weight_limit` of weight. If more weight
		/// is needed than `weight_limit`, then the operation will fail and the assets send may be
		/// at risk.
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
		/// - `weight_limit`: The remote-side weight limit, if any, for the XCM fee purchase.
		#[pallet::weight({
			match ((*assets.clone()).try_into(), (*dest.clone()).try_into()) {
				(Ok(assets), Ok(dest)) => {
					use sp_std::vec;
					let mut message = Xcm(vec![
						TransferReserveAsset { assets, dest, xcm: Xcm(vec![]) }
					]);
					T::Weigher::weight(&mut message).map_or(Weight::MAX, |w| Weight::from_ref_time(100_000_000.saturating_add(w)))
				},
				_ => Weight::MAX,
			}
		})]
		pub fn limited_reserve_transfer_assets(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			beneficiary: Box<VersionedMultiLocation>,
			assets: Box<VersionedMultiAssets>,
			fee_asset_item: u32,
			weight_limit: WeightLimit,
		) -> DispatchResult {
			Self::do_reserve_transfer_assets(
				origin,
				dest,
				beneficiary,
				assets,
				fee_asset_item,
				Some(weight_limit),
			)
		}

		/// Teleport some assets from the local chain to some destination chain.
		///
		/// Fee payment on the destination side is made from the asset in the `assets` vector of
		/// index `fee_asset_item`, up to enough to pay for `weight_limit` of weight. If more weight
		/// is needed than `weight_limit`, then the operation will fail and the assets send may be
		/// at risk.
		///
		/// - `origin`: Must be capable of withdrawing the `assets` and executing XCM.
		/// - `dest`: Destination context for the assets. Will typically be `X2(Parent, Parachain(..))` to send
		///   from parachain to parachain, or `X1(Parachain(..))` to send from relay to parachain.
		/// - `beneficiary`: A beneficiary location for the assets in the context of `dest`. Will generally be
		///   an `AccountId32` value.
		/// - `assets`: The assets to be withdrawn. The first item should be the currency used to to pay the fee on the
		///   `dest` side. May not be empty.
		/// - `fee_asset_item`: The index into `assets` of the item which should be used to pay
		///   fees.
		/// - `weight_limit`: The remote-side weight limit, if any, for the XCM fee purchase.
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
					T::Weigher::weight(&mut message).map_or(Weight::MAX, |w| Weight::from_ref_time(100_000_000.saturating_add(w)))
				},
				_ => Weight::MAX,
			}
		})]
		pub fn limited_teleport_assets(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			beneficiary: Box<VersionedMultiLocation>,
			assets: Box<VersionedMultiAssets>,
			fee_asset_item: u32,
			weight_limit: WeightLimit,
		) -> DispatchResult {
			Self::do_teleport_assets(
				origin,
				dest,
				beneficiary,
				assets,
				fee_asset_item,
				Some(weight_limit),
			)
		}
	}

	impl<T: Config> Pallet<T> {
		fn do_reserve_transfer_assets(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			beneficiary: Box<VersionedMultiLocation>,
			assets: Box<VersionedMultiAssets>,
			fee_asset_item: u32,
			maybe_weight_limit: Option<WeightLimit>,
		) -> DispatchResult {
			let origin_location = T::ExecuteXcmOrigin::ensure_origin(origin)?;
			let dest = (*dest).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let beneficiary: MultiLocation =
				(*beneficiary).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let assets: MultiAssets = (*assets).try_into().map_err(|()| Error::<T>::BadVersion)?;

			ensure!(assets.len() <= MAX_ASSETS_FOR_TRANSFER, Error::<T>::TooManyAssets);
			let value = (origin_location, assets.drain());
			ensure!(T::XcmReserveTransferFilter::contains(&value), Error::<T>::Filtered);
			let (origin_location, assets) = value;
			let ancestry = T::LocationInverter::ancestry();
			let fees = assets
				.get(fee_asset_item as usize)
				.ok_or(Error::<T>::Empty)?
				.clone()
				.reanchored(&dest, &ancestry)
				.map_err(|_| Error::<T>::CannotReanchor)?;
			let max_assets = assets.len() as u32;
			let assets: MultiAssets = assets.into();
			let weight_limit = match maybe_weight_limit {
				Some(weight_limit) => weight_limit,
				None => {
					let beneficiary = beneficiary.clone();
					let fees = fees.clone();
					let mut remote_message = Xcm(vec![
						ReserveAssetDeposited(assets.clone()),
						ClearOrigin,
						BuyExecution { fees, weight_limit: Limited(0) },
						DepositAsset { assets: Wild(All), max_assets, beneficiary },
					]);
					// use local weight for remote message and hope for the best.
					let remote_weight = T::Weigher::weight(&mut remote_message)
						.map_err(|()| Error::<T>::UnweighableMessage)?;
					Limited(remote_weight)
				},
			};
			let xcm = Xcm(vec![
				BuyExecution { fees, weight_limit },
				DepositAsset { assets: Wild(All), max_assets, beneficiary },
			]);
			let mut message = Xcm(vec![TransferReserveAsset { assets, dest, xcm }]);
			let weight =
				T::Weigher::weight(&mut message).map_err(|()| Error::<T>::UnweighableMessage)?;
			let outcome =
				T::XcmExecutor::execute_xcm_in_credit(origin_location, message, weight, weight);
			Self::deposit_event(Event::Attempted(outcome));
			Ok(())
		}

		fn do_teleport_assets(
			origin: OriginFor<T>,
			dest: Box<VersionedMultiLocation>,
			beneficiary: Box<VersionedMultiLocation>,
			assets: Box<VersionedMultiAssets>,
			fee_asset_item: u32,
			maybe_weight_limit: Option<WeightLimit>,
		) -> DispatchResult {
			let origin_location = T::ExecuteXcmOrigin::ensure_origin(origin)?;
			let dest = (*dest).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let beneficiary: MultiLocation =
				(*beneficiary).try_into().map_err(|()| Error::<T>::BadVersion)?;
			let assets: MultiAssets = (*assets).try_into().map_err(|()| Error::<T>::BadVersion)?;

			ensure!(assets.len() <= MAX_ASSETS_FOR_TRANSFER, Error::<T>::TooManyAssets);
			let value = (origin_location, assets.drain());
			ensure!(T::XcmTeleportFilter::contains(&value), Error::<T>::Filtered);
			let (origin_location, assets) = value;
			let ancestry = T::LocationInverter::ancestry();
			let fees = assets
				.get(fee_asset_item as usize)
				.ok_or(Error::<T>::Empty)?
				.clone()
				.reanchored(&dest, &ancestry)
				.map_err(|_| Error::<T>::CannotReanchor)?;
			let max_assets = assets.len() as u32;
			let assets: MultiAssets = assets.into();
			let weight_limit = match maybe_weight_limit {
				Some(weight_limit) => weight_limit,
				None => {
					let beneficiary = beneficiary.clone();
					let fees = fees.clone();
					let mut remote_message = Xcm(vec![
						ReceiveTeleportedAsset(assets.clone()),
						ClearOrigin,
						BuyExecution { fees, weight_limit: Limited(0) },
						DepositAsset { assets: Wild(All), max_assets, beneficiary },
					]);
					// use local weight for remote message and hope for the best.
					let remote_weight = T::Weigher::weight(&mut remote_message)
						.map_err(|()| Error::<T>::UnweighableMessage)?;
					Limited(remote_weight)
				},
			};
			let xcm = Xcm(vec![
				BuyExecution { fees, weight_limit },
				DepositAsset { assets: Wild(All), max_assets, beneficiary },
			]);
			let mut message =
				Xcm(vec![WithdrawAsset(assets), InitiateTeleport { assets: Wild(All), dest, xcm }]);
			let weight =
				T::Weigher::weight(&mut message).map_err(|()| Error::<T>::UnweighableMessage)?;
			let outcome =
				T::XcmExecutor::execute_xcm_in_credit(origin_location, message, weight, weight);
			Self::deposit_event(Event::Attempted(outcome));
			Ok(())
		}

		/// Will always make progress, and will do its best not to use much more than `weight_cutoff`
		/// in doing so.
		pub(crate) fn check_xcm_version_change(
			mut stage: VersionMigrationStage,
			weight_cutoff: Weight,
		) -> (Weight, Option<VersionMigrationStage>) {
			let mut weight_used = Weight::zero();

			// TODO: Correct weights for the components of this:
			let todo_sv_migrate_weight: Weight = T::DbWeight::get().reads_writes(1, 1);
			let todo_vn_migrate_weight: Weight = T::DbWeight::get().reads_writes(1, 1);
			let todo_vnt_already_notified_weight: Weight = T::DbWeight::get().reads(1);
			let todo_vnt_notify_weight: Weight = T::DbWeight::get().reads_writes(1, 3);
			let todo_vnt_migrate_weight: Weight = T::DbWeight::get().reads_writes(1, 1);
			let todo_vnt_migrate_fail_weight: Weight = T::DbWeight::get().reads_writes(1, 1);
			let todo_vnt_notify_migrate_weight: Weight = T::DbWeight::get().reads_writes(1, 3);

			use VersionMigrationStage::*;

			if stage == MigrateSupportedVersion {
				// We assume that supported XCM version only ever increases, so just cycle through lower
				// XCM versioned from the current.
				for v in 0..XCM_VERSION {
					for (old_key, value) in SupportedVersion::<T>::drain_prefix(v) {
						if let Ok(new_key) = old_key.into_latest() {
							SupportedVersion::<T>::insert(XCM_VERSION, new_key, value);
						}
						weight_used.saturating_accrue(todo_sv_migrate_weight);
						if weight_used.any_gte(weight_cutoff) {
							return (weight_used, Some(stage))
						}
					}
				}
				stage = MigrateVersionNotifiers;
			}
			if stage == MigrateVersionNotifiers {
				for v in 0..XCM_VERSION {
					for (old_key, value) in VersionNotifiers::<T>::drain_prefix(v) {
						if let Ok(new_key) = old_key.into_latest() {
							VersionNotifiers::<T>::insert(XCM_VERSION, new_key, value);
						}
						weight_used.saturating_accrue(todo_vn_migrate_weight);
						if weight_used.any_gte(weight_cutoff) {
							return (weight_used, Some(stage))
						}
					}
				}
				stage = NotifyCurrentTargets(None);
			}

			let xcm_version = T::AdvertisedXcmVersion::get();

			if let NotifyCurrentTargets(maybe_last_raw_key) = stage {
				let mut iter = match maybe_last_raw_key {
					Some(k) => VersionNotifyTargets::<T>::iter_prefix_from(XCM_VERSION, k),
					None => VersionNotifyTargets::<T>::iter_prefix(XCM_VERSION),
				};
				while let Some((key, value)) = iter.next() {
					let (query_id, max_weight, target_xcm_version) = value;
					let new_key: MultiLocation = match key.clone().try_into() {
						Ok(k) if target_xcm_version != xcm_version => k,
						_ => {
							// We don't early return here since we need to be certain that we
							// make some progress.
							weight_used.saturating_accrue(todo_vnt_already_notified_weight);
							continue
						},
					};
					let response = Response::Version(xcm_version);
					let message = Xcm(vec![QueryResponse { query_id, response, max_weight }]);
					let event = match T::XcmRouter::send_xcm(new_key.clone(), message) {
						Ok(()) => {
							let value = (query_id, max_weight, xcm_version);
							VersionNotifyTargets::<T>::insert(XCM_VERSION, key, value);
							Event::VersionChangeNotified(new_key, xcm_version)
						},
						Err(e) => {
							VersionNotifyTargets::<T>::remove(XCM_VERSION, key);
							Event::NotifyTargetSendFail(new_key, query_id, e.into())
						},
					};
					Self::deposit_event(event);
					weight_used.saturating_accrue(todo_vnt_notify_weight);
					if weight_used.any_gte(weight_cutoff) {
						let last = Some(iter.last_raw_key().into());
						return (weight_used, Some(NotifyCurrentTargets(last)))
					}
				}
				stage = MigrateAndNotifyOldTargets;
			}
			if stage == MigrateAndNotifyOldTargets {
				for v in 0..XCM_VERSION {
					for (old_key, value) in VersionNotifyTargets::<T>::drain_prefix(v) {
						let (query_id, max_weight, target_xcm_version) = value;
						let new_key = match MultiLocation::try_from(old_key.clone()) {
							Ok(k) => k,
							Err(()) => {
								Self::deposit_event(Event::NotifyTargetMigrationFail(
									old_key, value.0,
								));
								weight_used.saturating_accrue(todo_vnt_migrate_fail_weight);
								if weight_used.any_gte(weight_cutoff) {
									return (weight_used, Some(stage))
								}
								continue
							},
						};

						let versioned_key = LatestVersionedMultiLocation(&new_key);
						if target_xcm_version == xcm_version {
							VersionNotifyTargets::<T>::insert(XCM_VERSION, versioned_key, value);
							weight_used.saturating_accrue(todo_vnt_migrate_weight);
						} else {
							// Need to notify target.
							let response = Response::Version(xcm_version);
							let message =
								Xcm(vec![QueryResponse { query_id, response, max_weight }]);
							let event = match T::XcmRouter::send_xcm(new_key.clone(), message) {
								Ok(()) => {
									VersionNotifyTargets::<T>::insert(
										XCM_VERSION,
										versioned_key,
										(query_id, max_weight, xcm_version),
									);
									Event::VersionChangeNotified(new_key, xcm_version)
								},
								Err(e) => Event::NotifyTargetSendFail(new_key, query_id, e.into()),
							};
							Self::deposit_event(event);
							weight_used.saturating_accrue(todo_vnt_notify_migrate_weight);
						}
						if weight_used.any_gte(weight_cutoff) {
							return (weight_used, Some(stage))
						}
					}
				}
			}
			(weight_used, None)
		}

		/// Request that `dest` informs us of its version.
		pub fn request_version_notify(dest: impl Into<MultiLocation>) -> XcmResult {
			let dest = dest.into();
			let versioned_dest = VersionedMultiLocation::from(dest.clone());
			let already = VersionNotifiers::<T>::contains_key(XCM_VERSION, &versioned_dest);
			ensure!(!already, XcmError::InvalidLocation);
			let query_id = QueryCounter::<T>::mutate(|q| {
				let r = *q;
				q.saturating_inc();
				r
			});
			// TODO #3735: Correct weight.
			let instruction = SubscribeVersion { query_id, max_response_weight: 0 };
			T::XcmRouter::send_xcm(dest, Xcm(vec![instruction]))?;
			VersionNotifiers::<T>::insert(XCM_VERSION, &versioned_dest, query_id);
			let query_status =
				QueryStatus::VersionNotifier { origin: versioned_dest, is_active: false };
			Queries::<T>::insert(query_id, query_status);
			Ok(())
		}

		/// Request that `dest` ceases informing us of its version.
		pub fn unrequest_version_notify(dest: impl Into<MultiLocation>) -> XcmResult {
			let dest = dest.into();
			let versioned_dest = LatestVersionedMultiLocation(&dest);
			let query_id = VersionNotifiers::<T>::take(XCM_VERSION, versioned_dest)
				.ok_or(XcmError::InvalidLocation)?;
			T::XcmRouter::send_xcm(dest.clone(), Xcm(vec![UnsubscribeVersion]))?;
			Queries::<T>::remove(query_id);
			Ok(())
		}

		/// Relay an XCM `message` from a given `interior` location in this context to a given `dest`
		/// location. A null `dest` is not handled.
		pub fn send_xcm(
			interior: impl Into<Junctions>,
			dest: impl Into<MultiLocation>,
			mut message: Xcm<()>,
		) -> Result<(), SendError> {
			let interior = interior.into();
			let dest = dest.into();
			if interior != Junctions::Here {
				message.0.insert(0, DescendOrigin(interior))
			};
			log::trace!(target: "xcm::send_xcm", "dest: {:?}, message: {:?}", &dest, &message);
			T::XcmRouter::send_xcm(dest, message)
		}

		pub fn check_account() -> T::AccountId {
			const ID: PalletId = PalletId(*b"py/xcmch");
			AccountIdConversion::<T::AccountId>::into_account_truncating(&ID)
		}

		fn do_new_query(
			responder: impl Into<MultiLocation>,
			maybe_notify: Option<(u8, u8)>,
			timeout: T::BlockNumber,
		) -> u64 {
			QueryCounter::<T>::mutate(|q| {
				let r = *q;
				q.saturating_inc();
				Queries::<T>::insert(
					r,
					QueryStatus::Pending {
						responder: responder.into().into(),
						maybe_notify,
						timeout,
					},
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
			responder: impl Into<MultiLocation>,
			timeout: T::BlockNumber,
		) -> Result<QueryId, XcmError> {
			let responder = responder.into();
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
			responder: impl Into<MultiLocation>,
			notify: impl Into<<T as Config>::RuntimeCall>,
			timeout: T::BlockNumber,
		) -> Result<(), XcmError> {
			let responder = responder.into();
			let dest = T::LocationInverter::invert_location(&responder)
				.map_err(|()| XcmError::MultiLocationNotInvertible)?;
			let notify: <T as Config>::RuntimeCall = notify.into();
			let max_response_weight = notify.get_dispatch_info().weight;
			let query_id = Self::new_notify_query(responder, notify, timeout);
			let report_error = Xcm(vec![ReportError {
				dest,
				query_id,
				max_response_weight: max_response_weight.ref_time(),
			}]);
			message.0.insert(0, SetAppendix(report_error));
			Ok(())
		}

		/// Attempt to create a new query ID and register it as a query that is yet to respond.
		pub fn new_query(responder: impl Into<MultiLocation>, timeout: T::BlockNumber) -> u64 {
			Self::do_new_query(responder, None, timeout)
		}

		/// Attempt to create a new query ID and register it as a query that is yet to respond, and
		/// which will call a dispatchable when a response happens.
		pub fn new_notify_query(
			responder: impl Into<MultiLocation>,
			notify: impl Into<<T as Config>::RuntimeCall>,
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
			log::trace!(
				target: "xcm::pallet_xcm::note_unknown_version",
				"XCM version is unknown for destination: {:?}",
				dest,
			);
			let versioned_dest = VersionedMultiLocation::from(dest.clone());
			VersionDiscoveryQueue::<T>::mutate(|q| {
				if let Some(index) = q.iter().position(|i| &i.0 == &versioned_dest) {
					// exists - just bump the count.
					q[index].1.saturating_inc();
				} else {
					let _ = q.try_push((versioned_dest, 1));
				}
			});
		}
	}

	impl<T: Config> WrapVersion for Pallet<T> {
		fn wrap_version<RuntimeCall>(
			dest: &MultiLocation,
			xcm: impl Into<VersionedXcm<RuntimeCall>>,
		) -> Result<VersionedXcm<RuntimeCall>, ()> {
			SupportedVersion::<T>::get(XCM_VERSION, LatestVersionedMultiLocation(dest))
				.or_else(|| {
					Self::note_unknown_version(dest);
					SafeXcmVersion::<T>::get()
				})
				.ok_or_else(|| {
					log::trace!(
						target: "xcm::pallet_xcm::wrap_version",
						"Could not determine a version to wrap XCM for destination: {:?}",
						dest,
					);
					()
				})
				.and_then(|v| xcm.into().into_version(v.min(XCM_VERSION)))
		}
	}

	impl<T: Config> VersionChangeNotifier for Pallet<T> {
		/// Start notifying `location` should the XCM version of this chain change.
		///
		/// When it does, this type should ensure a `QueryResponse` message is sent with the given
		/// `query_id` & `max_weight` and with a `response` of `Repsonse::Version`. This should happen
		/// until/unless `stop` is called with the correct `query_id`.
		///
		/// If the `location` has an ongoing notification and when this function is called, then an
		/// error should be returned.
		fn start(dest: &MultiLocation, query_id: QueryId, max_weight: u64) -> XcmResult {
			let versioned_dest = LatestVersionedMultiLocation(dest);
			let already = VersionNotifyTargets::<T>::contains_key(XCM_VERSION, versioned_dest);
			ensure!(!already, XcmError::InvalidLocation);

			let xcm_version = T::AdvertisedXcmVersion::get();
			let response = Response::Version(xcm_version);
			let instruction = QueryResponse { query_id, response, max_weight };
			T::XcmRouter::send_xcm(dest.clone(), Xcm(vec![instruction]))?;

			let value = (query_id, max_weight, xcm_version);
			VersionNotifyTargets::<T>::insert(XCM_VERSION, versioned_dest, value);
			Ok(())
		}

		/// Stop notifying `location` should the XCM change. This is a no-op if there was never a
		/// subscription.
		fn stop(dest: &MultiLocation) -> XcmResult {
			VersionNotifyTargets::<T>::remove(XCM_VERSION, LatestVersionedMultiLocation(dest));
			Ok(())
		}

		/// Return true if a location is subscribed to XCM version changes.
		fn is_subscribed(dest: &MultiLocation) -> bool {
			let versioned_dest = LatestVersionedMultiLocation(dest);
			VersionNotifyTargets::<T>::contains_key(XCM_VERSION, versioned_dest)
		}
	}

	impl<T: Config> DropAssets for Pallet<T> {
		fn drop_assets(origin: &MultiLocation, assets: Assets) -> u64 {
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
			let hash = BlakeTwo256::hash_of(&(origin, versioned.clone()));
			match AssetTraps::<T>::get(hash) {
				0 => return false,
				1 => AssetTraps::<T>::remove(hash),
				n => AssetTraps::<T>::insert(hash, n - 1),
			}
			Self::deposit_event(Event::AssetsClaimed(hash, origin.clone(), versioned));
			return true
		}
	}

	impl<T: Config> OnResponse for Pallet<T> {
		fn expecting_response(origin: &MultiLocation, query_id: QueryId) -> bool {
			match Queries::<T>::get(query_id) {
				Some(QueryStatus::Pending { responder, .. }) =>
					MultiLocation::try_from(responder).map_or(false, |r| origin == &r),
				Some(QueryStatus::VersionNotifier { origin: r, .. }) =>
					MultiLocation::try_from(r).map_or(false, |r| origin == &r),
				_ => false,
			}
		}

		fn on_response(
			origin: &MultiLocation,
			query_id: QueryId,
			response: Response,
			max_weight: u64,
		) -> u64 {
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
							if let Ok(call) = bare.using_encoded(|mut bytes| {
								<T as Config>::RuntimeCall::decode(&mut bytes)
							}) {
								Queries::<T>::remove(query_id);
								let weight = call.get_dispatch_info().weight;
								let max_weight = Weight::from_ref_time(max_weight);
								if weight.any_gt(max_weight) {
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
								.ref_time()
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
	fn try_successful_origin() -> Result<O, ()> {
		Ok(O::from(Origin::Xcm(Here.into())))
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
	fn try_successful_origin() -> Result<O, ()> {
		Ok(O::from(Origin::Response(Here.into())))
	}
}

/// A simple passthrough where we reuse the `MultiLocation`-typed XCM origin as the inner value of
/// this crate's `Origin::Xcm` value.
pub struct XcmPassthrough<RuntimeOrigin>(PhantomData<RuntimeOrigin>);
impl<RuntimeOrigin: From<crate::Origin>> ConvertOrigin<RuntimeOrigin>
	for XcmPassthrough<RuntimeOrigin>
{
	fn convert_origin(
		origin: impl Into<MultiLocation>,
		kind: OriginKind,
	) -> Result<RuntimeOrigin, MultiLocation> {
		let origin = origin.into();
		match kind {
			OriginKind::Xcm => Ok(crate::Origin::Xcm(origin).into()),
			_ => Err(origin),
		}
	}
}
