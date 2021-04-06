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

#![cfg_attr(not(feature = "std"), no_std)]

mod location_conversion;
pub use location_conversion::{
	Account32Hash, ParentIsDefault, ChildParachainConvertsVia, SiblingParachainConvertsVia, AccountId32Aliases, AccountKey20Aliases,
};

mod origin_conversion;
pub use origin_conversion::{
	SovereignSignedViaLocation, ParentAsSuperuser, ChildSystemParachainAsSuperuser, SiblingSystemParachainAsSuperuser,
	ChildParachainAsNative, SiblingParachainAsNative, RelayChainAsNative, SignedAccountId32AsNative, SignedAccountKey20AsNative,
};

mod currency_adapter;
mod fungibles_adapter;
pub use currency_adapter::CurrencyAdapter;
pub use fungibles_adapter::FungiblesAdapter;

#[deprecated="use `xcm-executor::traits::LocationInverter` instead"]
pub use xcm_executor::traits::LocationInverter;
