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
	ClaimAssets, ConvertOrigin, DropAssets, FilterAssetLocation, InvertLocation, OnResponse,
	ShouldExecute, TransactAsset, VersionChangeNotifier, WeightBounds, WeightTrader,
};
use frame_support::dispatch::{Dispatchable, GetDispatchInfo, Parameter, PostDispatchInfo};
use xcm::latest::SendXcm;

/// The trait to parameterize the `XcmExecutor`.
pub trait Config {
	/// The outer call dispatch type.
	type RuntimeCall: Parameter + Dispatchable<PostInfo = PostDispatchInfo> + GetDispatchInfo;

	/// How to send an onward XCM message.
	type XcmSender: SendXcm;

	/// How to withdraw and deposit an asset.
	type AssetTransactor: TransactAsset;

	/// How to get a call origin from a `OriginKind` value.
	type OriginConverter: ConvertOrigin<<Self::RuntimeCall as Dispatchable>::RuntimeOrigin>;

	/// Combinations of (Location, Asset) pairs which we trust as reserves.
	type IsReserve: FilterAssetLocation;

	/// Combinations of (Location, Asset) pairs which we trust as teleporters.
	type IsTeleporter: FilterAssetLocation;

	/// Means of inverting a location.
	type LocationInverter: InvertLocation;

	/// Whether we should execute the given XCM at all.
	type Barrier: ShouldExecute;

	/// The means of determining an XCM message's weight.
	type Weigher: WeightBounds<Self::RuntimeCall>;

	/// The means of purchasing weight credit for XCM execution.
	type Trader: WeightTrader;

	/// What to do when a response of a query is found.
	type ResponseHandler: OnResponse;

	/// The general asset trap - handler for when assets are left in the Holding Register at the
	/// end of execution.
	type AssetTrap: DropAssets;

	/// The handler for when there is an instruction to claim assets.
	type AssetClaims: ClaimAssets;

	/// How we handle version subscription requests.
	type SubscriptionService: VersionChangeNotifier;
}
