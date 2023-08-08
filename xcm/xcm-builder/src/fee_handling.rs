// Copyright Parity Technologies (UK) Ltd.
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

use core::marker::PhantomData;
use frame_support::traits::{Contains, Get};
use xcm::prelude::*;
use xcm_executor::traits::{FeeManager, FeeReason, TransactAsset};

/// A `FeeManager` implementation that simply deposits the fees handled into a specific on-chain
/// `ReceiverAccount`.
///
/// It reuses the `AssetTransactor` configured on the XCM executor to deposit fee assets, and also
/// permits specifying `WaivedLocations` for locations that are privileged to not pay for fees. If
/// the `AssetTransactor` returns an error while calling `deposit_asset`, then a warning will be
/// logged.
pub struct XcmFeesToAccount<XcmConfig, WaivedLocations, AccountId, ReceiverAccount>(
	PhantomData<(XcmConfig, WaivedLocations, AccountId, ReceiverAccount)>,
);
impl<
		XcmConfig: xcm_executor::Config,
		WaivedLocations: Contains<MultiLocation>,
		AccountId: Clone + Into<[u8; 32]>,
		ReceiverAccount: Get<Option<AccountId>>,
	> FeeManager for XcmFeesToAccount<XcmConfig, WaivedLocations, AccountId, ReceiverAccount>
{
	fn is_waived(origin: Option<&MultiLocation>, _: FeeReason) -> bool {
		let Some(loc) = origin else { return false };
		WaivedLocations::contains(loc)
	}

	fn handle_fee(fees: MultiAssets, context: Option<&XcmContext>) {
		if let Some(receiver) = ReceiverAccount::get() {
			let dest = AccountId32 { network: None, id: receiver.into() }.into();
			for asset in fees.into_inner() {
				if let Err(e) = XcmConfig::AssetTransactor::deposit_asset(&asset, &dest, context) {
					log::warn!(
						target: "xcm::fees",
						"`AssetTransactor::deposit_asset` returned error: {:?}, burning fees: {:?}",
						e, asset,
					);
				}
			}
		}
	}
}
