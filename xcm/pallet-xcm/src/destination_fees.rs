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

//! Utilities for handling fees and `BuyExecution` on destination.

use xcm::prelude::*;

/// Describes how to handle `BuyExecution` on destination chains,
/// useful for handling foreign asset transfers.
pub enum DestinationFeesSetup {
	/// Fees are directly paid using Origin's indicated asset.
	ByOrigin,
	/// `UniversalLocation` does some conversion from origin's indicated asset.
	/// E.g. swap origin's fee and/or `BuyExecution` on destination using different asset.
	ByUniversalLocation {
		/// Local account where we want to place additional withdrawn assets from origin
		/// (e.g. treasury, staking pot, BH...).
		local_account: MultiLocation,
	},
}

/// Pair of local and remote assets (equivalent in economic value).
pub struct DestinationFees {
	/// Assets withdrawn on **source** chain.
	pub proportional_amount_to_withdraw: MultiAsset,

	/// Assets used for `BuyExecution` on **destination** chain.
	pub proportional_amount_to_buy_execution: MultiAsset,
}

/// Manages fees handling.
pub trait DestinationFeesManager {
	/// Decide how to handle `BuyExecution` based on `destination` and `requested_fee_asset_id`.
	fn decide_for(
		destination: &MultiLocation,
		requested_fee_asset_id: &AssetId,
	) -> DestinationFeesSetup;

	/// Estimate destination fees.
	fn estimate_fee_for(
		destination: &MultiLocation,
		requested_fee_asset_id: &AssetId,
		weight: &WeightLimit,
	) -> Option<DestinationFees>;
}

/// Implementation handles setup as `DestinationFeesSetup::Origin`
impl DestinationFeesManager for () {
	fn decide_for(
		_destination: &MultiLocation,
		_desired_fee_asset_id: &AssetId,
	) -> DestinationFeesSetup {
		DestinationFeesSetup::ByOrigin
	}

	fn estimate_fee_for(
		_destination: &MultiLocation,
		_desired_fee_asset_id: &AssetId,
		_weight: &WeightLimit,
	) -> Option<DestinationFees> {
		// dont do any conversion or whatever, handle what origin wants on input
		None
	}
}
