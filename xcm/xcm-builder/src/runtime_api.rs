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

//! Runtime API definition for fungibles.
//!
//! (Initial version: https://github.com/paritytech/cumulus/pull/2180#issuecomment-1442952274)

use frame_support::RuntimeDebug;
use parity_scale_codec::{Codec, Decode, Encode};
use sp_std::vec::Vec;
use xcm::latest::MultiAsset;

/// The possible errors that can happen querying the storage of assets.
#[derive(Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum FungiblesAccessError {
	/// `MultiLocation` to `AssetId`/`ClassId` conversion failed.
	AssetIdConversionFailed,
	/// `u128` amount to currency `Balance` conversion failed.
	AmountToBalanceConversionFailed,
}

sp_api::decl_runtime_apis! {
	/// The API for querying account's balances from runtime.
	pub trait FungiblesApi<AccountId>
	where
		AccountId: Codec,
	{
		/// Returns the list of all [`MultiAsset`] that an `AccountId` has.
		fn query_account_balances(account: AccountId) -> Result<Vec<MultiAsset>, FungiblesAccessError>;
	}
}
