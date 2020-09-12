// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Version 0 of the Cross-Consensus Message format data structures.

use sp_std::vec::Vec;
use sp_runtime::RuntimeDebug;
use codec::{self, Encode, Decode};
use super::{MultiAsset, MultiLocation};

/// An instruction to be executed on some or all of the assets in holding, used by asset-related XCM messages.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum Order {
	/// Do nothing. Not generally used.
	Null,
	/// Remove the asset(s) (`assets`) from holding and deposit them in this consensus system under the ownership of
	/// `dest`.
	///
	/// - `assets`:
	/// - `dest`:
	DepositAsset { assets: Vec<MultiAsset>, dest: MultiLocation },
	/// Remove the asset(s) (`assets`) from holding and deposit them in this consensus system under the ownership of
	/// `dest`.
	///
	/// Send an onward XCM message to `dest` of `ReserveAssetDeposit` with the
	///
	/// - `assets`:
	/// - `dest`:
	/// - `effects`:
	DepositReserveAsset { assets: Vec<MultiAsset>, dest: MultiLocation, effects: Vec<Order> },


	ExchangeAsset { give: Vec<MultiAsset>, receive: Vec<MultiAsset> },
	InitiateReserveWithdraw { assets: Vec<MultiAsset>, reserve: MultiLocation, effects: Vec<Order> },
	InitiateTeleport { assets: Vec<MultiAsset>, dest: MultiLocation, effects: Vec<Order> },
	QueryHolding { #[codec(compact)] query_id: u64, dest: MultiLocation, assets: Vec<MultiAsset> },
}
