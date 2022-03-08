// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! This is almost 1:1 copy of `node/service/parachains_db/mod.rs` file from Polkadot repository.
//! The only exception is that we don't support db upgrades => no `upgrade.rs` module.

use kvdb::KeyValueDB;
use std::{io, path::PathBuf, sync::Arc};

mod columns {
	pub const NUM_COLUMNS: u32 = 5;

	pub const COL_AVAILABILITY_DATA: u32 = 0;
	pub const COL_AVAILABILITY_META: u32 = 1;
	pub const COL_APPROVAL_DATA: u32 = 2;
	pub const COL_CHAIN_SELECTION_DATA: u32 = 3;
	pub const COL_DISPUTE_COORDINATOR_DATA: u32 = 4;
}

/// Columns used by different subsystems.
#[derive(Debug, Clone)]
pub struct ColumnsConfig {
	/// The column used by the av-store for data.
	pub col_availability_data: u32,
	/// The column used by the av-store for meta information.
	pub col_availability_meta: u32,
	/// The column used by approval voting for data.
	pub col_approval_data: u32,
	/// The column used by chain selection for data.
	pub col_chain_selection_data: u32,
	/// The column used by dispute coordinator for data.
	pub col_dispute_coordinator_data: u32,
}

/// The real columns used by the parachains DB.
pub const REAL_COLUMNS: ColumnsConfig = ColumnsConfig {
	col_availability_data: columns::COL_AVAILABILITY_DATA,
	col_availability_meta: columns::COL_AVAILABILITY_META,
	col_approval_data: columns::COL_APPROVAL_DATA,
	col_chain_selection_data: columns::COL_CHAIN_SELECTION_DATA,
	col_dispute_coordinator_data: columns::COL_DISPUTE_COORDINATOR_DATA,
};

/// The cache size for each column, in megabytes.
#[derive(Debug, Clone)]
pub struct CacheSizes {
	/// Cache used by availability data.
	pub availability_data: usize,
	/// Cache used by availability meta.
	pub availability_meta: usize,
	/// Cache used by approval data.
	pub approval_data: usize,
}

impl Default for CacheSizes {
	fn default() -> Self {
		CacheSizes { availability_data: 25, availability_meta: 1, approval_data: 5 }
	}
}

fn other_io_error(err: String) -> io::Error {
	io::Error::new(io::ErrorKind::Other, err)
}

/// Open the database on disk, creating it if it doesn't exist.
pub fn open_creating(root: PathBuf, cache_sizes: CacheSizes) -> io::Result<Arc<dyn KeyValueDB>> {
	use kvdb_rocksdb::{Database, DatabaseConfig};

	let path = root.join("parachains").join("db");

	let mut db_config = DatabaseConfig::with_columns(columns::NUM_COLUMNS);

	let _ = db_config
		.memory_budget
		.insert(columns::COL_AVAILABILITY_DATA, cache_sizes.availability_data);
	let _ = db_config
		.memory_budget
		.insert(columns::COL_AVAILABILITY_META, cache_sizes.availability_meta);
	let _ = db_config
		.memory_budget
		.insert(columns::COL_APPROVAL_DATA, cache_sizes.approval_data);

	let path_str = path
		.to_str()
		.ok_or_else(|| other_io_error(format!("Bad database path: {:?}", path)))?;

	std::fs::create_dir_all(&path_str)?;
	let db = Database::open(&db_config, path_str)?;

	Ok(Arc::new(db))
}
