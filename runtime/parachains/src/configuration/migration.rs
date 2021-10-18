// Copyright 2021 Parity Technologies (UK) Ltd.
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

use crate::configuration::Config;
use frame_support::{traits::StorageVersion, weights::Weight};

/// The current storage version.
pub const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

/// Migrates the pallet storage to the most recent version, checking and setting the `StorageVersion`.
pub fn migrate_to_latest<T: Config>() -> Weight {
	0
}
