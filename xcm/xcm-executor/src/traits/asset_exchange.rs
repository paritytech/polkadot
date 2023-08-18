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

use crate::Assets;
use xcm::prelude::*;

/// A service for exchanging assets.
pub trait AssetExchange {
	/// Handler for exchanging an asset.
	///
	/// - `origin`: The location attempting the exchange; this should generally not matter.
	/// - `give`: The assets which have been removed from the caller.
	/// - `want`: The minimum amount of assets which should be given to the caller in case any
	///   exchange happens. If more assets are provided, then they should generally be of the same
	///   asset class if at all possible.
	/// - `maximal`: If `true`, then as much as possible should be exchanged.
	///
	/// `Ok` is returned along with the new set of assets which have been exchanged for `give`. At
	/// least want must be in the set. Some assets originally in `give` may also be in this set. In
	/// the case of returning an `Err`, then `give` is returned.
	fn exchange_asset(
		origin: Option<&MultiLocation>,
		give: Assets,
		want: &MultiAssets,
		maximal: bool,
	) -> Result<Assets, Assets>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl AssetExchange for Tuple {
	fn exchange_asset(
		origin: Option<&MultiLocation>,
		give: Assets,
		want: &MultiAssets,
		maximal: bool,
	) -> Result<Assets, Assets> {
		for_tuples!( #(
			let give = match Tuple::exchange_asset(origin, give, want, maximal) {
				Ok(r) => return Ok(r),
				Err(a) => a,
			};
		)* );
		Err(give)
	}
}
