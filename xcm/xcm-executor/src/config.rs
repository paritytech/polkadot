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

use crate::traits::{
	AssetExchange, AssetLock, ClaimAssets, ConvertOrigin, DropAssets, ExportXcm, FeeManager,
	FilterAssetLocation, OnResponse, ShouldExecute, TransactAsset, UniversalLocation,
	VersionChangeNotifier, WeightBounds, WeightTrader,
};
use frame_support::{
	dispatch::{Dispatchable, Parameter},
	traits::{Contains, Get, PalletsInfoAccess},
	weights::{GetDispatchInfo, PostDispatchInfo},
};
use xcm::latest::{Junction, MultiLocation, SendXcm};

/// The trait to parameterize the `XcmExecutor`.
pub trait Config {
	/// The outer call dispatch type.
	type Call: Parameter + Dispatchable<PostInfo = PostDispatchInfo> + GetDispatchInfo;

	/// How to send an onward XCM message.
	type XcmSender: SendXcm;

	/// How to withdraw and deposit an asset.
	type AssetTransactor: TransactAsset;

	/// How to get a call origin from a `OriginKind` value.
	type OriginConverter: ConvertOrigin<<Self::Call as Dispatchable>::Origin>;

	/// Combinations of (Location, Asset) pairs which we trust as reserves.
	type IsReserve: FilterAssetLocation;

	/// Combinations of (Location, Asset) pairs which we trust as teleporters.
	type IsTeleporter: FilterAssetLocation;

	/// Means of inverting a location.
	type LocationInverter: UniversalLocation;

	/// Whether we should execute the given XCM at all.
	type Barrier: ShouldExecute;

	/// The means of determining an XCM message's weight.
	type Weigher: WeightBounds<Self::Call>;

	/// The means of purchasing weight credit for XCM execution.
	type Trader: WeightTrader;

	/// What to do when a response of a query is found.
	type ResponseHandler: OnResponse;

	/// The general asset trap - handler for when assets are left in the Holding Register at the
	/// end of execution.
	type AssetTrap: DropAssets;

	/// Handler for asset locking.
	type AssetLocker: AssetLock;

	/// Handler for exchanging assets.
	type AssetExchanger: AssetExchange;

	/// The handler for when there is an instruction to claim assets.
	type AssetClaims: ClaimAssets;

	/// How we handle version subscription requests.
	type SubscriptionService: VersionChangeNotifier;

	/// Information on all pallets.
	type PalletInstancesInfo: PalletsInfoAccess;

	/// The maximum number of assets we target to have in the Holding Register at any one time.
	///
	/// NOTE: In the worse case, the Holding Register may contain up to twice as many assets as this
	/// and any benchmarks should take that into account.
	type MaxAssetsIntoHolding: Get<u32>;

	/// Configure the fees.
	type FeeManager: FeeManager;

	/// The method of exporting a message.
	type MessageExporter: ExportXcm;

	/// The origin locations and specific universal junctions to which they are allowed to elevate
	/// themselves.
	type UniversalAliases: Contains<(MultiLocation, Junction)>;
}
