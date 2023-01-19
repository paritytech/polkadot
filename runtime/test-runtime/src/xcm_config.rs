// Copyright 2021 Parity Technologies (UK) Ltd.
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

use frame_support::{
	parameter_types,
	traits::{Everything, Nothing},
	weights::Weight,
};
use xcm::latest::prelude::*;
use xcm_builder::{
	AllowUnpaidExecutionFrom, EnsureXcmOrigin, FixedWeightBounds, SignedAccountId32AsNative,
	SignedToAccountId32,
};
use xcm_executor::{
	traits::{TransactAsset, WeightTrader},
	Assets,
};

parameter_types! {
	pub const BaseXcmWeight: xcm::latest::Weight = Weight::from_parts(1_000, 1_000);
	pub const AnyNetwork: Option<NetworkId> = None;
	pub const MaxInstructions: u32 = 100;
	pub const MaxAssetsIntoHolding: u32 = 16;
	pub const UniversalLocation: xcm::latest::InteriorMultiLocation = xcm::latest::Junctions::Here;
}

/// Type to convert an `Origin` type value into a `MultiLocation` value which represents an interior location
/// of this chain.
pub type LocalOriginToLocation = (
	// And a usual Signed origin to be used in XCM as a corresponding AccountId32
	SignedToAccountId32<crate::RuntimeOrigin, crate::AccountId, AnyNetwork>,
);

pub struct DoNothingRouter;
impl SendXcm for DoNothingRouter {
	type Ticket = ();
	fn validate(_dest: &mut Option<MultiLocation>, _msg: &mut Option<Xcm<()>>) -> SendResult<()> {
		Ok(((), MultiAssets::new()))
	}
	fn deliver(_: ()) -> Result<XcmHash, SendError> {
		Ok([0; 32])
	}
}

pub type Barrier = AllowUnpaidExecutionFrom<Everything>;

pub struct DummyAssetTransactor;
impl TransactAsset for DummyAssetTransactor {
	fn deposit_asset(_what: &MultiAsset, _who: &MultiLocation, _context: &XcmContext) -> XcmResult {
		Ok(())
	}

	fn withdraw_asset(
		_what: &MultiAsset,
		_who: &MultiLocation,
		_maybe_context: Option<&XcmContext>,
	) -> Result<Assets, XcmError> {
		let asset: MultiAsset = (Parent, 100_000).into();
		Ok(asset.into())
	}
}

pub struct DummyWeightTrader;
impl WeightTrader for DummyWeightTrader {
	fn new() -> Self {
		DummyWeightTrader
	}

	fn buy_weight(&mut self, _weight: Weight, _payment: Assets) -> Result<Assets, XcmError> {
		Ok(Assets::default())
	}
}

type OriginConverter = (
	pallet_xcm::XcmPassthrough<super::RuntimeOrigin>,
	SignedAccountId32AsNative<AnyNetwork, super::RuntimeOrigin>,
);

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type RuntimeCall = super::RuntimeCall;
	type XcmSender = DoNothingRouter;
	type AssetTransactor = DummyAssetTransactor;
	type OriginConverter = OriginConverter;
	type IsReserve = ();
	type IsTeleporter = ();
	type UniversalLocation = UniversalLocation;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<BaseXcmWeight, super::RuntimeCall, MaxInstructions>;
	type Trader = DummyWeightTrader;
	type ResponseHandler = super::Xcm;
	type AssetTrap = super::Xcm;
	type AssetLocker = ();
	type AssetExchanger = ();
	type AssetClaims = super::Xcm;
	type SubscriptionService = super::Xcm;
	type PalletInstancesInfo = ();
	type MaxAssetsIntoHolding = MaxAssetsIntoHolding;
	type FeeManager = ();
	type MessageExporter = ();
	type UniversalAliases = Nothing;
	type CallDispatcher = super::RuntimeCall;
	type SafeCallFilter = Everything;
}

#[cfg(feature = "runtime-benchmarks")]
parameter_types! {
	pub ReachableDest: Option<MultiLocation> = Some(xcm::latest::Junctions::Here.into());
}

impl pallet_xcm::Config for crate::Runtime {
	// The config types here are entirely configurable, since the only one that is sorely needed
	// is `XcmExecutor`, which will be used in unit tests located in xcm-executor.
	type RuntimeEvent = crate::RuntimeEvent;
	type ExecuteXcmOrigin = EnsureXcmOrigin<crate::RuntimeOrigin, LocalOriginToLocation>;
	type UniversalLocation = UniversalLocation;
	type SendXcmOrigin = EnsureXcmOrigin<crate::RuntimeOrigin, LocalOriginToLocation>;
	type Weigher = FixedWeightBounds<BaseXcmWeight, crate::RuntimeCall, MaxInstructions>;
	type XcmRouter = DoNothingRouter;
	type XcmExecuteFilter = Everything;
	type XcmExecutor = xcm_executor::XcmExecutor<XcmConfig>;
	type XcmTeleportFilter = Everything;
	type XcmReserveTransferFilter = Everything;
	type RuntimeOrigin = crate::RuntimeOrigin;
	type RuntimeCall = crate::RuntimeCall;
	const VERSION_DISCOVERY_QUEUE_SIZE: u32 = 100;
	type AdvertisedXcmVersion = pallet_xcm::CurrentXcmVersion;
	type Currency = crate::Balances;
	type CurrencyMatcher = ();
	type TrustedLockers = ();
	type SovereignAccountOf = ();
	type MaxLockers = frame_support::traits::ConstU32<8>;
	type WeightInfo = pallet_xcm::TestWeightInfo;
	#[cfg(feature = "runtime-benchmarks")]
	type ReachableDest = ReachableDest;
}
