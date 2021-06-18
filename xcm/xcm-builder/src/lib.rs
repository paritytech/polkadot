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

//! # XCM-Builder
//!
//! Types and helpers for *building* XCM configuration.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

mod location_conversion;
pub use location_conversion::{
	Account32Hash, ParentIsDefault, ChildParachainConvertsVia, SiblingParachainConvertsVia, AccountId32Aliases,
	AccountKey20Aliases, LocationInverter,
};

mod origin_conversion;
pub use origin_conversion::{
	SovereignSignedViaLocation, ParentAsSuperuser, ChildSystemParachainAsSuperuser, SiblingSystemParachainAsSuperuser,
	ChildParachainAsNative, SiblingParachainAsNative, RelayChainAsNative, SignedAccountId32AsNative,
	SignedAccountKey20AsNative, EnsureXcmOrigin, SignedToAccountId32, BackingToPlurality,
};

mod barriers;
pub use barriers::{
	TakeWeightCredit, AllowUnpaidExecutionFrom, AllowTopLevelPaidExecutionFrom, AllowKnownQueryResponses,
	IsChildSystemParachain,
};

mod currency_adapter;
pub use currency_adapter::CurrencyAdapter;

mod fungibles_adapter;
pub use fungibles_adapter::FungiblesAdapter;

mod weight;
pub use weight::{FixedRateOfConcreteFungible, FixedWeightBounds, UsingComponents, TakeRevenue};

mod matches_fungible;
pub use matches_fungible::{IsAbstract, IsConcrete};

mod filter_asset_location;
pub use filter_asset_location::{Case, NativeAsset};
