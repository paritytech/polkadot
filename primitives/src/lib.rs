// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot types shared between the runtime and the Node-side code.

#![warn(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]

/// The minimum supported version of the primitives by this implementation.
pub const MIN_SUPPORTED_VERSION: u32 = 1;
/// The maximum supported version of the primitives by this implementation.
pub const MAX_SUPPORTED_VERSION: u32 = 2;

/// The STAGING version.
pub const STAGING_VERSION: u32 = u32::MAX;

pub mod v2;

// The 'staging' version is special - while other versions are set in stone,
// the staging version is malleable. Once it's released, it gets the next
// version number.
pub mod vstaging;
