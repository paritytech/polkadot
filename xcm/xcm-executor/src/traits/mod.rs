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

//! Various traits used in configuring the executor.

mod conversion;
pub use conversion::{CallDispatcher, ConvertLocation, ConvertOrigin, WithOriginFilter};
mod drop_assets;
pub use drop_assets::{ClaimAssets, DropAssets};
mod asset_lock;
pub use asset_lock::{AssetLock, Enact, LockError};
mod asset_exchange;
pub use asset_exchange::AssetExchange;
mod export;
pub use export::{export_xcm, validate_export, ExportXcm};
mod fee_manager;
pub use fee_manager::{FeeManager, FeeReason};
mod filter_asset_location;
#[allow(deprecated)]
pub use filter_asset_location::FilterAssetLocation;
mod token_matching;
pub use token_matching::{
	Error, MatchesFungible, MatchesFungibles, MatchesNonFungible, MatchesNonFungibles,
};
mod on_response;
pub use on_response::{OnResponse, QueryHandler, QueryResponseStatus, VersionChangeNotifier};
mod should_execute;
pub use should_execute::{CheckSuspension, Properties, ShouldExecute};
mod transact_asset;
pub use transact_asset::TransactAsset;
mod weight;
#[deprecated = "Use `sp_runtime::traits::` instead"]
pub use sp_runtime::traits::{Identity, TryConvertInto as JustTry};
pub use weight::{WeightBounds, WeightTrader};

pub mod prelude {
	pub use super::{
		export_xcm, validate_export, AssetExchange, AssetLock, ClaimAssets, ConvertOrigin,
		DropAssets, Enact, Error, ExportXcm, FeeManager, FeeReason, LockError, MatchesFungible,
		MatchesFungibles, MatchesNonFungible, MatchesNonFungibles, OnResponse, ShouldExecute,
		TransactAsset, VersionChangeNotifier, WeightBounds, WeightTrader, WithOriginFilter,
	};
	#[allow(deprecated)]
	pub use super::{Identity, JustTry};
}
