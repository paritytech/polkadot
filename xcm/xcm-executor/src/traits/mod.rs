// Copyright 2020 Parity Technologies (UK) Ltd.
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
pub use conversion::{Convert, ConvertOrigin, Decoded, Encoded, Identity, InvertLocation, JustTry};
mod drop_assets;
pub use drop_assets::{ClaimAssets, DropAssets};
mod filter_asset_location;
pub use filter_asset_location::FilterAssetLocation;
mod matches_fungible;
pub use matches_fungible::MatchesFungible;
mod matches_fungibles;
pub use matches_fungibles::{Error, MatchesFungibles};
mod on_response;
pub use on_response::{OnResponse, VersionChangeNotifier};
mod should_execute;
pub use should_execute::ShouldExecute;
mod transact_asset;
pub use transact_asset::TransactAsset;
mod weight;
pub use weight::{WeightBounds, WeightTrader};
