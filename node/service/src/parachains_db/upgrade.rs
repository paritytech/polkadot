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

//! Migration code for the parachain's DB.

#![cfg(feature = "full-node")]

use super::{columns, other_io_error, DatabaseKind, LOG_TARGET};
use std::{
	fs, io,
	path::{Path, PathBuf},
	str::FromStr,
};

use polkadot_node_core_approval_voting::approval_db::v2::{
	migration_helpers::v1_to_v2, Config as ApprovalDbConfig,
};
type Version = u32;

/// Version file name.
const VERSION_FILE_NAME: &'static str = "parachain_db_version";

/// Current db version.
/// Version 4 changes approval db format for `OurAssignment`.
pub(crate) const CURRENT_VERSION: Version = 4;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("I/O error when reading/writing the version")]
	Io(#[from] io::Error),
	#[error("The version file format is incorrect")]
	CorruptedVersionFile,
	#[error("Parachains DB has a future version (expected {current:?}, found {got:?})")]
	FutureVersion { current: Version, got: Version },
	#[error("Parachain DB migration failed")]
	MigrationFailed,
	#[error("Parachain DB migration would take forever")]
	MigrationLoop,
}

impl From<Error> for io::Error {
	fn from(me: Error) -> io::Error {
		match me {
			Error::Io(e) => e,
			_ => super::other_io_error(me.to_string()),
		}
	}
}

/// Try upgrading parachain's database to a target version.
pub(crate) fn try_upgrade_db(
	db_path: &Path,
	db_kind: DatabaseKind,
	target_version: Version,
) -> Result<(), Error> {
	// Ensure we don't loop forever below befcause of a bug.
	const MAX_MIGRATIONS: u32 = 30;

	#[cfg(test)]
	remove_file_lock(&db_path);

	// Loop migrations until we reach the target version.
	for _ in 0..MAX_MIGRATIONS {
		let version = try_upgrade_db_to_next_version(db_path, db_kind)?;

		#[cfg(test)]
		remove_file_lock(&db_path);

		if version == target_version {
			return Ok(())
		}
	}

	Err(Error::MigrationLoop)
}

/// Try upgrading parachain's database to the next version.
/// If successfull, it returns the current version.
pub(crate) fn try_upgrade_db_to_next_version(
	db_path: &Path,
	db_kind: DatabaseKind,
) -> Result<Version, Error> {
	let is_empty = db_path.read_dir().map_or(true, |mut d| d.next().is_none());

	let new_version = if !is_empty {
		match get_db_version(db_path)? {
			// 0 -> 1 migration
			Some(0) => migrate_from_version_0_to_1(db_path, db_kind)?,
			// 1 -> 2 migration
			Some(1) => migrate_from_version_1_to_2(db_path, db_kind)?,
			// 2 -> 3 migration
			Some(2) => migrate_from_version_2_to_3(db_path, db_kind)?,
			// 3 -> 4 migration
			Some(3) => migrate_from_version_3_to_4(db_path, db_kind)?,
			// Already at current version, do nothing.
			Some(CURRENT_VERSION) => CURRENT_VERSION,
			// This is an arbitrary future version, we don't handle it.
			Some(v) => return Err(Error::FutureVersion { current: CURRENT_VERSION, got: v }),
			// No version file. For `RocksDB` we dont need to do anything.
			None if db_kind == DatabaseKind::RocksDB => CURRENT_VERSION,
			// No version file. `ParityDB` did not previously have a version defined.
			// We handle this as a `0 -> 1` migration.
			None if db_kind == DatabaseKind::ParityDB =>
				migrate_from_version_0_to_1(db_path, db_kind)?,
			None => unreachable!(),
		}
	} else {
		CURRENT_VERSION
	};

	update_version(db_path, new_version)?;
	Ok(new_version)
}

/// Reads current database version from the file at given path.
/// If the file does not exist returns `None`, otherwise the version stored in the file.
fn get_db_version(path: &Path) -> Result<Option<Version>, Error> {
	match fs::read_to_string(version_file_path(path)) {
		Err(ref err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
		Err(err) => Err(err.into()),
		Ok(content) => u32::from_str(&content)
			.map(|v| Some(v))
			.map_err(|_| Error::CorruptedVersionFile),
	}
}

/// Writes current database version to the file.
/// Creates a new file if the version file does not exist yet.
fn update_version(path: &Path, new_version: Version) -> Result<(), Error> {
	fs::create_dir_all(path)?;
	fs::write(version_file_path(path), new_version.to_string()).map_err(Into::into)
}

/// Returns the version file path.
fn version_file_path(path: &Path) -> PathBuf {
	let mut file_path = path.to_owned();
	file_path.push(VERSION_FILE_NAME);
	file_path
}

fn migrate_from_version_0_to_1(path: &Path, db_kind: DatabaseKind) -> Result<Version, Error> {
	gum::info!(target: LOG_TARGET, "Migrating parachains db from version 0 to version 1 ...");

	match db_kind {
		DatabaseKind::ParityDB => paritydb_migrate_from_version_0_to_1(path),
		DatabaseKind::RocksDB => rocksdb_migrate_from_version_0_to_1(path),
	}
	.and_then(|result| {
		gum::info!(target: LOG_TARGET, "Migration complete! ");
		Ok(result)
	})
}

fn migrate_from_version_1_to_2(path: &Path, db_kind: DatabaseKind) -> Result<Version, Error> {
	gum::info!(target: LOG_TARGET, "Migrating parachains db from version 1 to version 2 ...");

	match db_kind {
		DatabaseKind::ParityDB => paritydb_migrate_from_version_1_to_2(path),
		DatabaseKind::RocksDB => rocksdb_migrate_from_version_1_to_2(path),
	}
	.and_then(|result| {
		gum::info!(target: LOG_TARGET, "Migration complete! ");
		Ok(result)
	})
}

// Migrade approval voting database. `OurAssignment` has been changed to support the v2 assignments.
// As these are backwards compatible, we'll convert the old entries in the new format.
fn migrate_from_version_3_to_4(path: &Path, db_kind: DatabaseKind) -> Result<Version, Error> {
	gum::info!(target: LOG_TARGET, "Migrating parachains db from version 3 to version 4 ...");
	use polkadot_node_subsystem_util::database::{
		kvdb_impl::DbAdapter as RocksDbAdapter, paritydb_impl::DbAdapter as ParityDbAdapter,
	};
	use std::sync::Arc;

	let approval_db_config =
		ApprovalDbConfig { col_approval_data: super::REAL_COLUMNS.col_approval_data };

	let _result = match db_kind {
		DatabaseKind::ParityDB => {
			let db = ParityDbAdapter::new(
				parity_db::Db::open(&paritydb_version_3_config(path))
					.map_err(|e| other_io_error(format!("Error opening db {:?}", e)))?,
				super::columns::v3::ORDERED_COL,
			);

			v1_to_v2(Arc::new(db), approval_db_config).map_err(|_| Error::MigrationFailed)?;
		},
		DatabaseKind::RocksDB => {
			let db_path = path
				.to_str()
				.ok_or_else(|| super::other_io_error("Invalid database path".into()))?;
			let db_cfg =
				kvdb_rocksdb::DatabaseConfig::with_columns(super::columns::v3::NUM_COLUMNS);
			let db = RocksDbAdapter::new(
				kvdb_rocksdb::Database::open(&db_cfg, db_path)?,
				&super::columns::v3::ORDERED_COL,
			);

			v1_to_v2(Arc::new(db), approval_db_config).map_err(|_| Error::MigrationFailed)?;
		},
	};

	gum::info!(target: LOG_TARGET, "Migration complete! ");
	Ok(CURRENT_VERSION)
}

fn migrate_from_version_2_to_3(path: &Path, db_kind: DatabaseKind) -> Result<Version, Error> {
	gum::info!(target: LOG_TARGET, "Migrating parachains db from version 2 to version 3 ...");
	match db_kind {
		DatabaseKind::ParityDB => paritydb_migrate_from_version_2_to_3(path),
		DatabaseKind::RocksDB => rocksdb_migrate_from_version_2_to_3(path),
	}
	.and_then(|result| {
		gum::info!(target: LOG_TARGET, "Migration complete! ");
		Ok(result)
	})
}

/// Migration from version 0 to version 1:
/// * the number of columns has changed from 3 to 5;
fn rocksdb_migrate_from_version_0_to_1(path: &Path) -> Result<Version, Error> {
	use kvdb_rocksdb::{Database, DatabaseConfig};

	let db_path = path
		.to_str()
		.ok_or_else(|| super::other_io_error("Invalid database path".into()))?;
	let db_cfg = DatabaseConfig::with_columns(super::columns::v0::NUM_COLUMNS);
	let mut db = Database::open(&db_cfg, db_path)?;

	db.add_column()?;
	db.add_column()?;

	Ok(1)
}

/// Migration from version 1 to version 2:
/// * the number of columns has changed from 5 to 6;
fn rocksdb_migrate_from_version_1_to_2(path: &Path) -> Result<Version, Error> {
	use kvdb_rocksdb::{Database, DatabaseConfig};

	let db_path = path
		.to_str()
		.ok_or_else(|| super::other_io_error("Invalid database path".into()))?;
	let db_cfg = DatabaseConfig::with_columns(super::columns::v1::NUM_COLUMNS);
	let mut db = Database::open(&db_cfg, db_path)?;

	db.add_column()?;

	Ok(2)
}

fn rocksdb_migrate_from_version_2_to_3(path: &Path) -> Result<Version, Error> {
	use kvdb_rocksdb::{Database, DatabaseConfig};

	let db_path = path
		.to_str()
		.ok_or_else(|| super::other_io_error("Invalid database path".into()))?;
	let db_cfg = DatabaseConfig::with_columns(super::columns::v2::NUM_COLUMNS);
	let mut db = Database::open(&db_cfg, db_path)?;

	db.remove_last_column()?;

	Ok(3)
}

// This currently clears columns which had their configs altered between versions.
// The columns to be changed are constrained by the `allowed_columns` vector.
fn paritydb_fix_columns(
	path: &Path,
	options: parity_db::Options,
	allowed_columns: Vec<u32>,
) -> io::Result<()> {
	// Figure out which columns to delete. This will be determined by inspecting
	// the metadata file.
	if let Some(metadata) = parity_db::Options::load_metadata(&path)
		.map_err(|e| other_io_error(format!("Error reading metadata {:?}", e)))?
	{
		let columns_to_clear = metadata
			.columns
			.into_iter()
			.enumerate()
			.filter(|(idx, _)| allowed_columns.contains(&(*idx as u32)))
			.filter_map(|(idx, opts)| {
				let changed = opts != options.columns[idx];
				if changed {
					gum::debug!(
						target: LOG_TARGET,
						"Column {} will be cleared. Old options: {:?}, New options: {:?}",
						idx,
						opts,
						options.columns[idx]
					);
					Some(idx)
				} else {
					None
				}
			})
			.collect::<Vec<_>>();

		if columns_to_clear.len() > 0 {
			gum::debug!(
				target: LOG_TARGET,
				"Database column changes detected, need to cleanup {} columns.",
				columns_to_clear.len()
			);
		}

		for column in columns_to_clear {
			gum::debug!(target: LOG_TARGET, "Clearing column {}", column,);
			parity_db::clear_column(path, column.try_into().expect("Invalid column ID"))
				.map_err(|e| other_io_error(format!("Error clearing column {:?}", e)))?;
		}

		// Write the updated column options.
		options
			.write_metadata(path, &metadata.salt)
			.map_err(|e| other_io_error(format!("Error writing metadata {:?}", e)))?;
	}

	Ok(())
}

/// Database configuration for version 1.
pub(crate) fn paritydb_version_1_config(path: &Path) -> parity_db::Options {
	let mut options =
		parity_db::Options::with_columns(&path, super::columns::v1::NUM_COLUMNS as u8);
	for i in columns::v4::ORDERED_COL {
		options.columns[*i as usize].btree_index = true;
	}

	options
}

/// Database configuration for version 2.
pub(crate) fn paritydb_version_2_config(path: &Path) -> parity_db::Options {
	let mut options =
		parity_db::Options::with_columns(&path, super::columns::v2::NUM_COLUMNS as u8);
	for i in columns::v4::ORDERED_COL {
		options.columns[*i as usize].btree_index = true;
	}

	options
}

/// Database configuration for version 3.
pub(crate) fn paritydb_version_3_config(path: &Path) -> parity_db::Options {
	let mut options =
		parity_db::Options::with_columns(&path, super::columns::v3::NUM_COLUMNS as u8);
	for i in columns::v3::ORDERED_COL {
		options.columns[*i as usize].btree_index = true;
	}

	options
}

/// Database configuration for version 0. This is useful just for testing.
#[cfg(test)]
pub(crate) fn paritydb_version_0_config(path: &Path) -> parity_db::Options {
	let mut options =
		parity_db::Options::with_columns(&path, super::columns::v0::NUM_COLUMNS as u8);
	options.columns[super::columns::v4::COL_AVAILABILITY_META as usize].btree_index = true;

	options
}

/// Migration from version 0 to version 1.
/// Cases covered:
/// - upgrading from v0.9.23 or earlier -> the `dispute coordinator column` was changed
/// - upgrading from v0.9.24+ -> this is a no op assuming the DB has been manually fixed as per
/// release notes
fn paritydb_migrate_from_version_0_to_1(path: &Path) -> Result<Version, Error> {
	// Delete the `dispute coordinator` column if needed (if column configuration is changed).
	paritydb_fix_columns(
		path,
		paritydb_version_1_config(path),
		vec![super::columns::v4::COL_DISPUTE_COORDINATOR_DATA],
	)?;

	Ok(1)
}

/// Migration from version 1 to version 2:
/// - add a new column for session information storage
fn paritydb_migrate_from_version_1_to_2(path: &Path) -> Result<Version, Error> {
	let mut options = paritydb_version_1_config(path);

	// Adds the session info column.
	parity_db::Db::add_column(&mut options, Default::default())
		.map_err(|e| other_io_error(format!("Error adding column {:?}", e)))?;

	Ok(2)
}

/// Migration from version 2 to version 3:
/// - drop the column used by `RollingSessionWindow`
fn paritydb_migrate_from_version_2_to_3(path: &Path) -> Result<Version, Error> {
	parity_db::Db::drop_last_column(&mut paritydb_version_2_config(path))
		.map_err(|e| other_io_error(format!("Error removing COL_SESSION_WINDOW_DATA {:?}", e)))?;
	Ok(3)
}

/// Remove the lock file. If file is locked, it will wait up to 1s.
#[cfg(test)]
pub fn remove_file_lock(path: &std::path::Path) {
	use std::{io::ErrorKind, thread::sleep, time::Duration};

	let mut lock_path = std::path::PathBuf::from(path);
	lock_path.push("lock");

	for _ in 0..10 {
		let result = std::fs::remove_file(lock_path.as_path());
		match result {
			Err(error) => match error.kind() {
				ErrorKind::WouldBlock => {
					sleep(Duration::from_millis(100));
					continue
				},
				_ => return,
			},
			Ok(_) => {},
		}
	}

	unreachable!("Database is locked, waited 1s for lock file: {:?}", lock_path);
}

#[cfg(test)]
mod tests {
	use super::{
		columns::{v2::COL_SESSION_WINDOW_DATA, v4::*},
		*,
	};
	use polkadot_node_core_approval_voting::approval_db::v2::migration_helpers::v1_to_v2_fill_test_data;

	#[test]
	fn test_paritydb_migrate_0_to_1() {
		use parity_db::Db;

		let db_dir = tempfile::tempdir().unwrap();
		let path = db_dir.path();
		{
			let db = Db::open_or_create(&paritydb_version_0_config(&path)).unwrap();

			db.commit(vec![(
				COL_AVAILABILITY_META as u8,
				b"5678".to_vec(),
				Some(b"somevalue".to_vec()),
			)])
			.unwrap();
		}

		try_upgrade_db(&path, DatabaseKind::ParityDB, 1).unwrap();

		let db = Db::open(&paritydb_version_1_config(&path)).unwrap();
		assert_eq!(
			db.get(COL_AVAILABILITY_META as u8, b"5678").unwrap(),
			Some("somevalue".as_bytes().to_vec())
		);
	}

	#[test]
	fn test_paritydb_migrate_1_to_2() {
		use parity_db::Db;

		let db_dir = tempfile::tempdir().unwrap();
		let path = db_dir.path();

		// We need to properly set db version for upgrade to work.
		fs::write(version_file_path(path), "1").expect("Failed to write DB version");

		{
			let db = Db::open_or_create(&paritydb_version_1_config(&path)).unwrap();

			// Write some dummy data
			db.commit(vec![(
				COL_DISPUTE_COORDINATOR_DATA as u8,
				b"1234".to_vec(),
				Some(b"somevalue".to_vec()),
			)])
			.unwrap();

			assert_eq!(db.num_columns(), columns::v1::NUM_COLUMNS as u8);
		}

		try_upgrade_db(&path, DatabaseKind::ParityDB, 2).unwrap();

		let db = Db::open(&paritydb_version_2_config(&path)).unwrap();

		assert_eq!(db.num_columns(), columns::v2::NUM_COLUMNS as u8);

		assert_eq!(
			db.get(COL_DISPUTE_COORDINATOR_DATA as u8, b"1234").unwrap(),
			Some("somevalue".as_bytes().to_vec())
		);

		// Test we can write the new column.
		db.commit(vec![(
			COL_SESSION_WINDOW_DATA as u8,
			b"1337".to_vec(),
			Some(b"0xdeadb00b".to_vec()),
		)])
		.unwrap();

		// Read back data from new column.
		assert_eq!(
			db.get(COL_SESSION_WINDOW_DATA as u8, b"1337").unwrap(),
			Some("0xdeadb00b".as_bytes().to_vec())
		);
	}

	#[test]
	fn test_rocksdb_migrate_1_to_2() {
		use kvdb::{DBKey, DBOp};
		use kvdb_rocksdb::{Database, DatabaseConfig};
		use polkadot_node_subsystem_util::database::{
			kvdb_impl::DbAdapter, DBTransaction, KeyValueDB,
		};

		let db_dir = tempfile::tempdir().unwrap();
		let db_path = db_dir.path().to_str().unwrap();
		let db_cfg = DatabaseConfig::with_columns(super::columns::v1::NUM_COLUMNS);
		let db = Database::open(&db_cfg, db_path).unwrap();
		assert_eq!(db.num_columns(), super::columns::v1::NUM_COLUMNS as u32);

		// We need to properly set db version for upgrade to work.
		fs::write(version_file_path(db_dir.path()), "1").expect("Failed to write DB version");
		{
			let db = DbAdapter::new(db, columns::v4::ORDERED_COL);
			db.write(DBTransaction {
				ops: vec![DBOp::Insert {
					col: COL_DISPUTE_COORDINATOR_DATA,
					key: DBKey::from_slice(b"1234"),
					value: b"0xdeadb00b".to_vec(),
				}],
			})
			.unwrap();
		}

		try_upgrade_db(&db_dir.path(), DatabaseKind::RocksDB, 2).unwrap();

		let db_cfg = DatabaseConfig::with_columns(super::columns::v2::NUM_COLUMNS);
		let db = Database::open(&db_cfg, db_path).unwrap();

		assert_eq!(db.num_columns(), super::columns::v2::NUM_COLUMNS);

		let db = DbAdapter::new(db, columns::v4::ORDERED_COL);

		assert_eq!(
			db.get(COL_DISPUTE_COORDINATOR_DATA, b"1234").unwrap(),
			Some("0xdeadb00b".as_bytes().to_vec())
		);

		// Test we can write the new column.
		db.write(DBTransaction {
			ops: vec![DBOp::Insert {
				col: COL_SESSION_WINDOW_DATA,
				key: DBKey::from_slice(b"1337"),
				value: b"0xdeadb00b".to_vec(),
			}],
		})
		.unwrap();

		// Read back data from new column.
		assert_eq!(
			db.get(COL_SESSION_WINDOW_DATA, b"1337").unwrap(),
			Some("0xdeadb00b".as_bytes().to_vec())
		);
	}

	#[test]
	fn test_migrate_3_to_4() {
		use kvdb_rocksdb::{Database, DatabaseConfig};
		use polkadot_node_core_approval_voting::approval_db::v2::migration_helpers::v1_to_v2_sanity_check;
		use polkadot_node_subsystem_util::database::kvdb_impl::DbAdapter;

		let db_dir = tempfile::tempdir().unwrap();
		let db_path = db_dir.path().to_str().unwrap();
		let db_cfg: DatabaseConfig = DatabaseConfig::with_columns(super::columns::v3::NUM_COLUMNS);

		let approval_cfg = ApprovalDbConfig {
			col_approval_data: crate::parachains_db::REAL_COLUMNS.col_approval_data,
		};

		// We need to properly set db version for upgrade to work.
		fs::write(version_file_path(db_dir.path()), "3").expect("Failed to write DB version");
		let expected_candidates = {
			let db = Database::open(&db_cfg, db_path.clone()).unwrap();
			assert_eq!(db.num_columns(), super::columns::v3::NUM_COLUMNS as u32);
			let db = DbAdapter::new(db, columns::v3::ORDERED_COL);
			// Fill the approval voting column with test data.
			v1_to_v2_fill_test_data(std::sync::Arc::new(db), approval_cfg).unwrap()
		};

		try_upgrade_db(&db_dir.path(), DatabaseKind::RocksDB, 4).unwrap();

		let db_cfg = DatabaseConfig::with_columns(super::columns::v4::NUM_COLUMNS);
		let db = Database::open(&db_cfg, db_path).unwrap();
		let db = DbAdapter::new(db, columns::v4::ORDERED_COL);

		v1_to_v2_sanity_check(std::sync::Arc::new(db), approval_cfg, expected_candidates).unwrap();
	}

	#[test]
	fn test_rocksdb_migrate_0_to_4() {
		use kvdb_rocksdb::{Database, DatabaseConfig};

		let db_dir = tempfile::tempdir().unwrap();
		let db_path = db_dir.path().to_str().unwrap();

		fs::write(version_file_path(db_dir.path()), "0").expect("Failed to write DB version");
		try_upgrade_db(&db_dir.path(), DatabaseKind::RocksDB, 4).unwrap();

		let db_cfg = DatabaseConfig::with_columns(super::columns::v4::NUM_COLUMNS);
		let db = Database::open(&db_cfg, db_path).unwrap();

		assert_eq!(db.num_columns(), columns::v4::NUM_COLUMNS);
	}

	#[test]
	fn test_paritydb_migrate_0_to_4() {
		use parity_db::Db;

		let db_dir = tempfile::tempdir().unwrap();
		let path = db_dir.path();

		// We need to properly set db version for upgrade to work.
		fs::write(version_file_path(path), "0").expect("Failed to write DB version");

		{
			let db = Db::open_or_create(&paritydb_version_0_config(&path)).unwrap();
			assert_eq!(db.num_columns(), columns::v0::NUM_COLUMNS as u8);
		}

		try_upgrade_db(&path, DatabaseKind::ParityDB, 4).unwrap();

		let db = Db::open(&paritydb_version_3_config(&path)).unwrap();
		assert_eq!(db.num_columns(), columns::v4::NUM_COLUMNS as u8);
	}

	#[test]
	fn test_paritydb_migrate_2_to_3() {
		use parity_db::Db;

		let db_dir = tempfile::tempdir().unwrap();
		let path = db_dir.path();
		let test_key = b"1337";

		// We need to properly set db version for upgrade to work.
		fs::write(version_file_path(path), "2").expect("Failed to write DB version");

		{
			let db = Db::open_or_create(&paritydb_version_2_config(&path)).unwrap();

			// Write some dummy data
			db.commit(vec![(
				COL_SESSION_WINDOW_DATA as u8,
				test_key.to_vec(),
				Some(b"0xdeadb00b".to_vec()),
			)])
			.unwrap();

			assert_eq!(db.num_columns(), columns::v2::NUM_COLUMNS as u8);
		}

		try_upgrade_db(&path, DatabaseKind::ParityDB, 3).unwrap();

		let db = Db::open(&paritydb_version_3_config(&path)).unwrap();

		assert_eq!(db.num_columns(), columns::v3::NUM_COLUMNS as u8);
	}

	#[test]
	fn test_rocksdb_migrate_2_to_3() {
		use kvdb_rocksdb::{Database, DatabaseConfig};

		let db_dir = tempfile::tempdir().unwrap();
		let db_path = db_dir.path().to_str().unwrap();
		let db_cfg = DatabaseConfig::with_columns(super::columns::v2::NUM_COLUMNS);

		{
			let db = Database::open(&db_cfg, db_path).unwrap();
			assert_eq!(db.num_columns(), super::columns::v2::NUM_COLUMNS as u32);
		}

		// We need to properly set db version for upgrade to work.
		fs::write(version_file_path(db_dir.path()), "2").expect("Failed to write DB version");

		try_upgrade_db(&db_dir.path(), DatabaseKind::RocksDB, 3).unwrap();

		let db_cfg = DatabaseConfig::with_columns(super::columns::v3::NUM_COLUMNS);
		let db = Database::open(&db_cfg, db_path).unwrap();

		assert_eq!(db.num_columns(), super::columns::v3::NUM_COLUMNS);
	}
}
