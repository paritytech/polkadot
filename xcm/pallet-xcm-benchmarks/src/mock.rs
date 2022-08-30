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

use crate::*;
use frame_support::parameter_types;
use xcm::latest::Weight as XCMWeight;
use xcm_executor::traits::FilterAssetLocation;

// An xcm sender/receiver akin to > /dev/null
pub struct DevNull;
impl xcm::opaque::latest::SendXcm for DevNull {
	fn send_xcm(_: impl Into<MultiLocation>, _: Xcm<()>) -> SendResult {
		Ok(())
	}
}

impl xcm_executor::traits::OnResponse for DevNull {
	fn expecting_response(_: &MultiLocation, _: u64) -> bool {
		false
	}
	fn on_response(_: &MultiLocation, _: u64, _: Response, _: XCMWeight) -> XCMWeight {
		0
	}
}

pub struct AccountIdConverter;
impl xcm_executor::traits::Convert<MultiLocation, u64> for AccountIdConverter {
	fn convert(ml: MultiLocation) -> Result<u64, MultiLocation> {
		match ml {
			MultiLocation { parents: 0, interior: X1(Junction::AccountId32 { id, .. }) } =>
				Ok(<u64 as codec::Decode>::decode(&mut &*id.to_vec()).unwrap()),
			_ => Err(ml),
		}
	}

	fn reverse(acc: u64) -> Result<MultiLocation, u64> {
		Err(acc)
	}
}

parameter_types! {
	pub Ancestry: MultiLocation = Junction::Parachain(101).into();
	pub UnitWeightCost: u64 = 10;
	pub WeightPrice: (AssetId, u128) = (Concrete(Here.into()), 1_000_000);
}

pub struct AllAssetLocationsPass;
impl FilterAssetLocation for AllAssetLocationsPass {
	fn filter_asset_location(_: &MultiAsset, _: &MultiLocation) -> bool {
		true
	}
}
