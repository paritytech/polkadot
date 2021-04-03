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

//! A RocksDB instance for storing parachain data; availability data, and approvals.

#[cfg(feature = "real-overseer")]
use {
	std::io,
	std::path::PathBuf,
	std::sync::Arc,

	kvdb::KeyValueDB,
};


mod columns {
	#[cfg(feature = "real-overseer")]
	pub const NUM_COLUMNS: u32 = 3;


	pub const COL_AVAILABILITY_DATA: u32 = 0;
	pub const COL_AVAILABILITY_META: u32 = 1;
	pub const COL_APPROVAL_DATA: u32 = 2;
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
}

/// The real columns used by the parachains DB.
pub const REAL_COLUMNS: ColumnsConfig = ColumnsConfig {
	col_availability_data: columns::COL_AVAILABILITY_DATA,
	col_availability_meta: columns::COL_AVAILABILITY_META,
	col_approval_data: columns::COL_APPROVAL_DATA,
};

/// The cache size for each column, in bytes.
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
		CacheSizes {
			availability_data: 25 * 1024 * 1024,
			availability_meta: 512 * 1024,
			approval_data: 5 * 1024 * 1024,
		}
	}
}

/// Open the database on disk, creating it if it doesn't exist.
#[cfg(feature = "real-overseer")]
pub fn open_creating(
	root: PathBuf,
	cache_sizes: CacheSizes,
) -> io::Result<Arc<dyn KeyValueDB>> {
	use kvdb_rocksdb::{DatabaseConfig, Database};

	let path = root.join("parachains").join("db");

	let mut db_config = DatabaseConfig::with_columns(columns::NUM_COLUMNS);

	let _ = db_config.memory_budget
		.insert(columns::COL_AVAILABILITY_DATA, cache_sizes.availability_data);
	let _ = db_config.memory_budget
		.insert(columns::COL_AVAILABILITY_META, cache_sizes.availability_meta);
	let _ = db_config.memory_budget
		.insert(columns::COL_APPROVAL_DATA, cache_sizes.approval_data);

	let path = path.to_str().ok_or_else(|| io::Error::new(
		io::ErrorKind::Other,
		format!("Bad database path: {:?}", path),
	))?;

	std::fs::create_dir_all(&path)?;
	let db = Database::open(&db_config, &path)?;

	Ok(Arc::new(db))
}
