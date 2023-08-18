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

//! Database trait for polkadot db.

pub use kvdb::{DBKeyValue, DBTransaction, DBValue, KeyValueDB};

/// Database trait with ordered key capacity.
pub trait Database: KeyValueDB {
	/// Check if column allows content iteration
	/// and removal by prefix.
	fn is_indexed_column(&self, col: u32) -> bool;
}

/// Implementation for database supporting `KeyValueDB` already.
pub mod kvdb_impl {
	use super::{DBKeyValue, DBTransaction, DBValue, Database, KeyValueDB};
	use kvdb::{DBOp, IoStats, IoStatsKind};
	use std::{collections::BTreeSet, io::Result};

	/// Adapter implementing subsystem database
	/// for `KeyValueDB`.
	#[derive(Clone)]
	pub struct DbAdapter<D> {
		db: D,
		indexed_columns: BTreeSet<u32>,
	}

	impl<D: KeyValueDB> DbAdapter<D> {
		/// Instantiate new subsystem database, with
		/// the columns that allow ordered iteration.
		pub fn new(db: D, indexed_columns: &[u32]) -> Self {
			DbAdapter { db, indexed_columns: indexed_columns.iter().cloned().collect() }
		}

		fn ensure_is_indexed(&self, col: u32) {
			debug_assert!(
				self.is_indexed_column(col),
				"Invalid configuration of database, column {} is not ordered.",
				col
			);
		}

		fn ensure_ops_indexing(&self, transaction: &DBTransaction) {
			debug_assert!({
				let mut pass = true;
				for op in &transaction.ops {
					if let DBOp::DeletePrefix { col, .. } = op {
						if !self.is_indexed_column(*col) {
							pass = false;
							break
						}
					}
				}
				pass
			})
		}
	}

	impl<D: KeyValueDB> Database for DbAdapter<D> {
		fn is_indexed_column(&self, col: u32) -> bool {
			self.indexed_columns.contains(&col)
		}
	}

	impl<D: KeyValueDB> KeyValueDB for DbAdapter<D> {
		fn transaction(&self) -> DBTransaction {
			self.db.transaction()
		}

		fn get(&self, col: u32, key: &[u8]) -> Result<Option<DBValue>> {
			self.db.get(col, key)
		}

		fn get_by_prefix(&self, col: u32, prefix: &[u8]) -> Result<Option<DBValue>> {
			self.ensure_is_indexed(col);
			self.db.get_by_prefix(col, prefix)
		}

		fn write(&self, transaction: DBTransaction) -> Result<()> {
			self.ensure_ops_indexing(&transaction);
			self.db.write(transaction)
		}

		fn iter<'a>(&'a self, col: u32) -> Box<dyn Iterator<Item = Result<DBKeyValue>> + 'a> {
			self.ensure_is_indexed(col);
			self.db.iter(col)
		}

		fn iter_with_prefix<'a>(
			&'a self,
			col: u32,
			prefix: &'a [u8],
		) -> Box<dyn Iterator<Item = Result<DBKeyValue>> + 'a> {
			self.ensure_is_indexed(col);
			self.db.iter_with_prefix(col, prefix)
		}

		fn io_stats(&self, kind: IoStatsKind) -> IoStats {
			self.db.io_stats(kind)
		}

		fn has_key(&self, col: u32, key: &[u8]) -> Result<bool> {
			self.db.has_key(col, key)
		}

		fn has_prefix(&self, col: u32, prefix: &[u8]) -> Result<bool> {
			self.ensure_is_indexed(col);
			self.db.has_prefix(col, prefix)
		}
	}
}

/// Utilities for using parity-db database.
pub mod paritydb_impl {
	use super::{DBKeyValue, DBTransaction, DBValue, Database, KeyValueDB};
	use kvdb::DBOp;
	use parity_db::Db;
	use parking_lot::Mutex;
	use std::{collections::BTreeSet, io::Result, sync::Arc};

	fn handle_err<T>(result: parity_db::Result<T>) -> T {
		match result {
			Ok(r) => r,
			Err(e) => {
				panic!("Critical database error: {:?}", e);
			},
		}
	}

	fn map_err<T>(result: parity_db::Result<T>) -> Result<T> {
		result.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{:?}", e)))
	}

	/// Implementation of of `Database` for parity-db adapter.
	pub struct DbAdapter {
		db: Db,
		indexed_columns: BTreeSet<u32>,
		write_lock: Arc<Mutex<()>>,
	}

	impl KeyValueDB for DbAdapter {
		fn transaction(&self) -> DBTransaction {
			DBTransaction::new()
		}

		fn get(&self, col: u32, key: &[u8]) -> Result<Option<DBValue>> {
			map_err(self.db.get(col as u8, key))
		}

		fn get_by_prefix(&self, col: u32, prefix: &[u8]) -> Result<Option<DBValue>> {
			self.iter_with_prefix(col, prefix)
				.next()
				.transpose()
				.map(|mb| mb.map(|(_, v)| v))
		}

		fn iter<'a>(&'a self, col: u32) -> Box<dyn Iterator<Item = Result<DBKeyValue>> + 'a> {
			let mut iter = match self.db.iter(col as u8) {
				Ok(iter) => iter,
				Err(e) => return Box::new(std::iter::once(map_err(Err(e)))),
			};
			Box::new(std::iter::from_fn(move || {
				iter.next().transpose().map(|r| map_err(r.map(|(k, v)| (k.into(), v))))
			}))
		}

		fn iter_with_prefix<'a>(
			&'a self,
			col: u32,
			prefix: &'a [u8],
		) -> Box<dyn Iterator<Item = Result<DBKeyValue>> + 'a> {
			if prefix.len() == 0 {
				return self.iter(col)
			}
			let mut iter = match self.db.iter(col as u8) {
				Ok(iter) => iter,
				Err(e) => return Box::new(std::iter::once(map_err(Err(e)))),
			};
			if let Err(e) = iter.seek(prefix) {
				return Box::new(std::iter::once(map_err(Err(e))))
			}
			Box::new(std::iter::from_fn(move || {
				iter.next().transpose().and_then(|r| {
					map_err(r.map(|(k, v)| k.starts_with(prefix).then(|| (k.into(), v))))
						.transpose()
				})
			}))
		}

		fn write(&self, transaction: DBTransaction) -> Result<()> {
			let mut ops = transaction.ops.into_iter();
			// TODO using a key iterator or native delete here would be faster.
			let mut current_prefix_iter: Option<(parity_db::BTreeIterator, u8, Vec<u8>)> = None;
			let current_prefix_iter = &mut current_prefix_iter;
			let transaction = std::iter::from_fn(move || loop {
				if let Some((prefix_iter, col, prefix)) = current_prefix_iter {
					if let Some((key, _value)) = handle_err(prefix_iter.next()) {
						if key.starts_with(prefix) {
							return Some((*col, key.to_vec(), None))
						}
					}
					*current_prefix_iter = None;
				}
				return match ops.next() {
					None => None,
					Some(DBOp::Insert { col, key, value }) =>
						Some((col as u8, key.to_vec(), Some(value))),
					Some(DBOp::Delete { col, key }) => Some((col as u8, key.to_vec(), None)),
					Some(DBOp::DeletePrefix { col, prefix }) => {
						let col = col as u8;
						let mut iter = handle_err(self.db.iter(col));
						handle_err(iter.seek(&prefix[..]));
						*current_prefix_iter = Some((iter, col, prefix.to_vec()));
						continue
					},
				}
			});

			// Locking is required due to possible racy change of the content of a deleted prefix.
			let _lock = self.write_lock.lock();
			map_err(self.db.commit(transaction))
		}
	}

	impl Database for DbAdapter {
		fn is_indexed_column(&self, col: u32) -> bool {
			self.indexed_columns.contains(&col)
		}
	}

	impl DbAdapter {
		/// Implementation of of `Database` for parity-db adapter.
		pub fn new(db: Db, indexed_columns: &[u32]) -> Self {
			let write_lock = Arc::new(Mutex::new(()));
			DbAdapter { db, indexed_columns: indexed_columns.iter().cloned().collect(), write_lock }
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use kvdb_shared_tests as st;
		use std::io;
		use tempfile::Builder as TempfileBuilder;

		fn create(num_col: u32) -> io::Result<(DbAdapter, tempfile::TempDir)> {
			let tempdir = TempfileBuilder::new().prefix("").tempdir()?;
			let mut options = parity_db::Options::with_columns(tempdir.path(), num_col as u8);
			for i in 0..num_col {
				options.columns[i as usize].btree_index = true;
			}

			let db = parity_db::Db::open_or_create(&options)
				.map_err(|err| io::Error::new(io::ErrorKind::Other, format!("{:?}", err)))?;

			let db = DbAdapter::new(db, &[0]);
			Ok((db, tempdir))
		}

		#[test]
		fn put_and_get() -> io::Result<()> {
			let (db, _temp_file) = create(1)?;
			st::test_put_and_get(&db)
		}

		#[test]
		fn delete_and_get() -> io::Result<()> {
			let (db, _temp_file) = create(1)?;
			st::test_delete_and_get(&db)
		}

		#[test]
		fn delete_prefix() -> io::Result<()> {
			let (db, _temp_file) = create(st::DELETE_PREFIX_NUM_COLUMNS)?;
			st::test_delete_prefix(&db)
		}

		#[test]
		fn iter() -> io::Result<()> {
			let (db, _temp_file) = create(1)?;
			st::test_iter(&db)
		}

		#[test]
		fn iter_with_prefix() -> io::Result<()> {
			let (db, _temp_file) = create(1)?;
			st::test_iter_with_prefix(&db)
		}

		#[test]
		fn complex() -> io::Result<()> {
			let (db, _temp_file) = create(1)?;
			st::test_complex(&db)
		}
	}
}
