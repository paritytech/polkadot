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

use frame_support::{parameter_types, traits::Everything};
use xcm::latest::{prelude::*, Weight as XCMWeight};
use xcm_builder::{AllowUnpaidExecutionFrom, FixedWeightBounds, SignedToAccountId32};
use xcm_executor::{
	traits::{InvertLocation, TransactAsset, WeightTrader},
	Assets,
};

parameter_types! {
	pub const OurNetwork: NetworkId = NetworkId::Polkadot;
	pub const MaxInstructions: u32 = 100;
}

/// Type to convert an `Origin` type value into a `MultiLocation` value which represents an interior location
/// of this chain.
pub type LocalOriginToLocation = (
	// And a usual Signed origin to be used in XCM as a corresponding AccountId32
	SignedToAccountId32<crate::Origin, crate::AccountId, OurNetwork>,
);

pub struct DoNothingRouter;
impl SendXcm for DoNothingRouter {
	fn send_xcm(_dest: impl Into<MultiLocation>, _msg: Xcm<()>) -> SendResult {
		Ok(())
	}
}

pub type Barrier = AllowUnpaidExecutionFrom<Everything>;

pub struct DummyAssetTransactor;
impl TransactAsset for DummyAssetTransactor {
	fn deposit_asset(_what: &MultiAsset, _who: &MultiLocation) -> XcmResult {
		Ok(())
	}

	fn withdraw_asset(_what: &MultiAsset, _who: &MultiLocation) -> Result<Assets, XcmError> {
		let asset: MultiAsset = (Parent, 100_000).into();
		Ok(asset.into())
	}
}

pub struct DummyWeightTrader;
impl WeightTrader for DummyWeightTrader {
	fn new() -> Self {
		DummyWeightTrader
	}

	fn buy_weight(&mut self, _weight: XCMWeight, _payment: Assets) -> Result<Assets, XcmError> {
		Ok(Assets::default())
	}
}

pub struct InvertNothing;
impl InvertLocation for InvertNothing {
	fn invert_location(_: &MultiLocation) -> sp_std::result::Result<MultiLocation, ()> {
		Ok(Here.into())
	}
	fn ancestry() -> MultiLocation {
		Here.into()
	}
}

pub struct XcmConfig;
impl xcm_executor::Config for XcmConfig {
	type Call = super::Call;
	type XcmSender = DoNothingRouter;
	type AssetTransactor = DummyAssetTransactor;
	type OriginConverter = pallet_xcm::XcmPassthrough<super::Origin>;
	type IsReserve = ();
	type IsTeleporter = ();
	type LocationInverter = InvertNothing;
	type Barrier = Barrier;
	type Weigher = FixedWeightBounds<super::BaseXcmWeight, super::Call, MaxInstructions>;
	type Trader = DummyWeightTrader;
	type ResponseHandler = super::Xcm;
	type AssetTrap = super::Xcm;
	type AssetClaims = super::Xcm;
	type SubscriptionService = super::Xcm;
}
