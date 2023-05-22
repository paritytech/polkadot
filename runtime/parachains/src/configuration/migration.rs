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

//! A module that is responsible for migration of storage.

use frame_support::traits::StorageVersion;
use primitives::ExecutorParams;

/// The current storage version.
///
/// v0-v1: <https://github.com/paritytech/polkadot/pull/3575>
/// v1-v2: <https://github.com/paritytech/polkadot/pull/4420>
/// v2-v3: <https://github.com/paritytech/polkadot/pull/6091>
/// v3-v4: <https://github.com/paritytech/polkadot/pull/6345>
/// v4-v5: <https://github.com/paritytech/polkadot/pull/6937>
///      + <https://github.com/paritytech/polkadot/pull/6961>
///      + <https://github.com/paritytech/polkadot/pull/6934>
/// v5-v6: <https://github.com/paritytech/polkadot/pull/6271> (remove UMP dispatch queue)
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(6);

pub mod v5;
pub mod v6;
