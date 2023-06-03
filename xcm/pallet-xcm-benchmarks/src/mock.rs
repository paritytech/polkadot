// Copyright (C) Parity Technologies (UK) Ltd.
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
use frame_support::{parameter_types, traits::ContainsPair};
use xcm::latest::Weight;

// An xcm sender/receiver akin to > /dev/null
pub struct DevNull;
impl xcm::opaque::latest::SendXcm for DevNull {
	type Ticket = ();
	fn validate(_: &mut Option<MultiLocation>, _: &mut Option<Xcm<()>>) -> SendResult<()> {
		Ok(((), MultiAssets::new()))
	}
	fn deliver(_: ()) -> Result<XcmHash, SendError> {
		Ok([0; 32])
	}
}

impl xcm_executor::traits::OnResponse for DevNull {
	fn expecting_response(_: &MultiLocation, _: u64, _: Option<&MultiLocation>) -> bool {
		false
	}
	fn on_response(
		_: &MultiLocation,
		_: u64,
		_: Option<&MultiLocation>,
		_: Response,
		_: Weight,
		_: &XcmContext,
	) -> Weight {
		Weight::zero()
	}
}

pub struct AccountIdConverter;
impl xcm_executor::traits::ConvertLocation<u64> for AccountIdConverter {
	fn convert_location(ml: &MultiLocation) -> Option<u64> {
		match ml {
			MultiLocation { parents: 0, interior: X1(Junction::AccountId32 { id, .. }) } =>
				Some(<u64 as codec::Decode>::decode(&mut &*id.to_vec()).unwrap()),
			_ => None,
		}
	}
}

parameter_types! {
	pub UniversalLocation: InteriorMultiLocation = Junction::Parachain(101).into();
	pub UnitWeightCost: Weight = Weight::from_parts(10, 10);
	pub WeightPrice: (AssetId, u128, u128) = (Concrete(Here.into()), 1_000_000, 1024);
}

pub struct AllAssetLocationsPass;
impl ContainsPair<MultiAsset, MultiLocation> for AllAssetLocationsPass {
	fn contains(_: &MultiAsset, _: &MultiLocation) -> bool {
		true
	}
}
