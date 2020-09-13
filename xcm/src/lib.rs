// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Cross-Consensus Message format data structures.

#![cfg_attr(not(feature = "std"), no_std)]

use sp_runtime::RuntimeDebug;
use codec::{Encode, Decode};

pub mod v0;

/// A single XCM message, together with its version code.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum VersionedXcm {
	V0(v0::Xcm),
}

/// A versioned multi-location, a relative location of a cross-consensus system identifier.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum VersionedMultiLocation {
	V0(v0::MultiLocation),
}

/// A versioned multi-asset, an identifier for an asset within a consensus system.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum VersionedMultiAsset {
	V0(v0::MultiAsset),
}
