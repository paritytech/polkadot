// Copyright 2021-2020 Parity Technologies (UK) Ltd.
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

pub use kvdb::{DBTransaction, DBValue, KeyValueDB};

/// Database trait with ordered key capacity.
pub trait Database: KeyValueDB {
	/// Check if column allows content iteration
	/// and removal by prefix.
	fn is_indexed_column(&self, col: u32) -> bool;
}

/// Implementation for database supporting `KeyValueDB` already.
pub mod kvdb_impl {
	use super::{DBTransaction, DBValue, Database, KeyValueDB};
	use kvdb::{DBOp, IoStats, IoStatsKind};
	use parity_util_mem::{MallocSizeOf, MallocSizeOfOps};
	use std::{collections::BTreeSet, io::Result};

	/// Adapter implementing subsystem database
	/// for `KeyValueDB`.
	#[derive(Clone)]
	pub struct DbAdapter<D> {
		db: D,
		allowed_iter: BTreeSet<u32>,
	}

	impl<D: KeyValueDB> DbAdapter<D> {
		/// Instantiate new subsystem database, with
		/// definition of collections allowing iteration.
		pub fn new(db: D, allowed_iter: &[u32]) -> Self {
			DbAdapter { db, allowed_iter: allowed_iter.iter().cloned().collect() }
		}

		fn filter_iter(&self, col: u32) {
			debug_assert!(
				self.is_indexed_column(col),
				"Invalid configuration of database, column {} is not ordered.",
				col
			);
		}

		fn filter_tx(&self, transaction: &DBTransaction) {
			debug_assert!({
				for op in &transaction.ops {
					if let DBOp::DeletePrefix { col, .. } = op {
						self.filter_iter(*col);
					}
				}
				true
			})
		}
	}

	impl<D: KeyValueDB> Database for DbAdapter<D> {
		fn is_indexed_column(&self, col: u32) -> bool {
			self.allowed_iter.contains(&col)
		}
	}

	impl<D: KeyValueDB> KeyValueDB for DbAdapter<D> {
		fn transaction(&self) -> DBTransaction {
			self.db.transaction()
		}

		fn get(&self, col: u32, key: &[u8]) -> Result<Option<DBValue>> {
			self.db.get(col, key)
		}

		fn get_by_prefix(&self, col: u32, prefix: &[u8]) -> Option<Box<[u8]>> {
			self.filter_iter(col);
			self.db.get_by_prefix(col, prefix)
		}

		fn write(&self, transaction: DBTransaction) -> Result<()> {
			self.filter_tx(&transaction);
			self.db.write(transaction)
		}

		fn iter<'a>(&'a self, col: u32) -> Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a> {
			self.filter_iter(col);
			self.db.iter(col)
		}

		fn iter_with_prefix<'a>(
			&'a self,
			col: u32,
			prefix: &'a [u8],
		) -> Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a> {
			self.filter_iter(col);
			self.db.iter_with_prefix(col, prefix)
		}

		fn restore(&self, _new_db: &str) -> Result<()> {
			unimplemented!("restore is unsupported")
		}

		fn io_stats(&self, kind: IoStatsKind) -> IoStats {
			self.db.io_stats(kind)
		}

		fn has_key(&self, col: u32, key: &[u8]) -> Result<bool> {
			self.db.has_key(col, key)
		}

		fn has_prefix(&self, col: u32, prefix: &[u8]) -> bool {
			self.filter_iter(col);
			self.db.has_prefix(col, prefix)
		}
	}

	impl<D: KeyValueDB> MallocSizeOf for DbAdapter<D> {
		fn size_of(&self, ops: &mut MallocSizeOfOps) -> usize {
			// ignore filter set
			self.db.size_of(ops)
		}
	}
}

/// Utils to use parity-db base database.
pub mod paritydb_impl {
	use super::{lru_map::LRUMap, DBTransaction, DBValue, Database, KeyValueDB};
	use kvdb::{DBOp, IoStats, IoStatsKind};
	use parity_db::Db;
	use parking_lot::{RwLock, Mutex};
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
		allowed_iter: BTreeSet<u32>,
		lru: Vec<Option<Arc<RwLock<LRUMap<Vec<u8>, Option<Vec<u8>>>>>>>,
		write_lock: Arc<Mutex<()>>,
	}

	impl parity_util_mem::MallocSizeOf for DbAdapter {
		fn size_of(&self, _ops: &mut parity_util_mem::MallocSizeOfOps) -> usize {
			unimplemented!("size_of is not supported for parity_db")
		}
	}

	impl KeyValueDB for DbAdapter {
		fn transaction(&self) -> DBTransaction {
			DBTransaction::new()
		}

		fn get(&self, col: u32, key: &[u8]) -> Result<Option<DBValue>> {
			if let Some(lru) = self.lru.get(col as usize).map(Option::as_ref).flatten() {
				if let Some(cached) = lru.write().get(key) {
					return Ok(cached.clone())
				}
			}
			map_err(self.db.get(col as u8, key))
		}

		fn get_by_prefix(&self, col: u32, prefix: &[u8]) -> Option<Box<[u8]>> {
			self.iter_with_prefix(col, prefix).next().map(|(_, v)| v)
		}

		fn iter<'a>(&'a self, col: u32) -> Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a> {
			let mut iter = handle_err(self.db.iter(col as u8));
			Box::new(std::iter::from_fn(move || {
				if let Some((key, value)) = handle_err(iter.next()) {
					Some((key.into_boxed_slice(), value.into_boxed_slice()))
				} else {
					None
				}
			}))
		}

		fn iter_with_prefix<'a>(
			&'a self,
			col: u32,
			prefix: &'a [u8],
		) -> Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a> {
			if prefix.len() == 0 {
				return self.iter(col)
			}
			let mut iter = handle_err(self.db.iter(col as u8));
			handle_err(iter.seek(prefix));
			Box::new(std::iter::from_fn(move || {
				if let Some((key, value)) = handle_err(iter.next()) {
					key.starts_with(prefix)
						.then(|| (key.into_boxed_slice(), value.into_boxed_slice()))
				} else {
					None
				}
			}))
		}

		fn restore(&self, _new_db: &str) -> Result<()> {
			unimplemented!("restore is unsupported")
		}

		fn io_stats(&self, _kind: IoStatsKind) -> IoStats {
			unimplemented!("io_stats not supported by parity_db");
		}

		fn has_key(&self, col: u32, key: &[u8]) -> Result<bool> {
			map_err(self.db.get_size(col as u8, key).map(|r| r.is_some()))
		}

		fn has_prefix(&self, col: u32, prefix: &[u8]) -> bool {
			self.get_by_prefix(col, prefix).is_some()
		}

		fn write(&self, transaction: DBTransaction) -> std::io::Result<()> {
			let mut ops = transaction.ops.into_iter();
			// TODO using a key iterator or native delete here would be faster.
			let mut current_prefix_iter: Option<(parity_db::BTreeIterator, u8, Vec<u8>)> = None;
			let current_prefix_iter = &mut current_prefix_iter;
			let transaction = std::iter::from_fn(move || loop {
				if let Some((prefix_iter, col, prefix)) = current_prefix_iter {
					if let Some((key, _value)) = handle_err(prefix_iter.next()) {
						if key.starts_with(prefix) {
							self.set_lru(*col as u32, &key[..], &None);
							return Some((*col, key.to_vec(), None))
						}
					}
					*current_prefix_iter = None;
				}
				return match ops.next() {
					None => None,
					Some(DBOp::Insert { col, key, value }) => {
						let value = Some(value);
						self.set_lru(col, &key[..], &value);
						Some((col as u8, key.to_vec(), value))
					},
					Some(DBOp::Delete { col, key }) => {
						self.set_lru(col, &key[..], &None);
						Some((col as u8, key.to_vec(), None))
					},
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
			self.allowed_iter.contains(&col)
		}
	}

	impl DbAdapter {
		/// Implementation of of `Database` for parity-db adapter.
		pub fn new(db: Db, allowed_iter: &[u32], caches_size: &[(u32, usize)]) -> Self {
			let mut lru = Vec::<Option<Arc<RwLock<LRUMap<Vec<u8>, Option<Vec<u8>>>>>>>::new();
			for (col, cache_size) in caches_size {
				if *cache_size > 0 {
					lru.resize(*col as usize + 1, None);
					lru[*col as usize] = Some(Arc::new(RwLock::new(LRUMap::new(*cache_size))));
				}
			}
			let write_lock = Arc::new(Mutex::new(()));
			DbAdapter { db, allowed_iter: allowed_iter.iter().cloned().collect(), lru, write_lock }
		}

		fn set_lru(&self, col: u32, key: &[u8], value: &Option<Vec<u8>>) {
			if let Some(lru) = self.lru.get(col as usize).map(Option::as_ref).flatten() {
				lru.write().add(key.to_vec(), value.clone());
			}
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use kvdb_shared_tests as st;
		use std::io;
		use tempfile::Builder as TempfileBuilder;

		fn create(num_col: u32, with_cache: bool) -> io::Result<(DbAdapter, tempfile::TempDir)> {
			let tempdir = TempfileBuilder::new().prefix("").tempdir()?;
			let mut options = parity_db::Options::with_columns(tempdir.path(), num_col as u8);
			for i in 0..num_col {
				options.columns[i as usize].btree_index = true;
			}

			let db = parity_db::Db::open_or_create(&options)
				.map_err(|err| io::Error::new(io::ErrorKind::Other, format!("{:?}", err)))?;

			let cache = if with_cache {
				&[(0, 100)]
			} else {
				&[(0, 0)]
			};
			let db = DbAdapter::new(db, &[0], cache);
			Ok((db, tempdir))
		}

		#[test]
		fn put_and_get() -> io::Result<()> {
			let (db, _temp_file) = create(1, false)?;
			st::test_put_and_get(&db)?;
			let (db, _temp_file) = create(1, true)?;
			st::test_put_and_get(&db)
		}

		#[test]
		fn delete_and_get() -> io::Result<()> {
			let (db, _temp_file) = create(1, false)?;
			st::test_delete_and_get(&db)?;
			let (db, _temp_file) = create(1, true)?;
			st::test_delete_and_get(&db)
		}

		#[test]
		fn delete_prefix() -> io::Result<()> {
			let (db, _temp_file) = create(st::DELETE_PREFIX_NUM_COLUMNS, false)?;
			st::test_delete_prefix(&db)?;
			let (db, _temp_file) = create(st::DELETE_PREFIX_NUM_COLUMNS, true)?;
			st::test_delete_prefix(&db)
		}

		#[test]
		fn iter() -> io::Result<()> {
			let (db, _temp_file) = create(1, false)?;
			st::test_iter(&db)?;
			let (db, _temp_file) = create(1, true)?;
			st::test_iter(&db)
		}

		#[test]
		fn iter_with_prefix() -> io::Result<()> {
			let (db, _temp_file) = create(1, false)?;
			st::test_iter_with_prefix(&db)?;
			let (db, _temp_file) = create(1, true)?;
			st::test_iter_with_prefix(&db)
		}

		#[test]
		fn complex() -> io::Result<()> {
			let (db, _temp_file) = create(1, false)?;
			st::test_complex(&db)?;
			let (db, _temp_file) = create(1, true)?;
			st::test_complex(&db)
		}
	}
}

mod lru_map {
	use linked_hash_map::{Entry, LinkedHashMap};
	use std::hash::Hash as StdHash;

	pub(super) struct LRUMap<K, V>(LinkedHashMap<K, V>, usize, usize);

	impl<K, V> LRUMap<K, V>
	where
		K: std::hash::Hash + Eq,
	{
		pub(crate) fn new(size_limit: usize) -> Self {
			LRUMap(LinkedHashMap::new(), 0, size_limit)
		}
	}

	/// Internal trait similar to `heapsize` but using
	/// a simple estimation.
	///
	/// This should not be made public, it is implementation
	/// detail trait. If it need to become public please
	/// consider using `malloc_size_of`.
	pub(super) trait EstimateSize {
		/// Return a size estimation of additional size needed
		/// to cache this struct (in bytes).
		fn estimate_size(&self) -> usize;
	}

	impl EstimateSize for Vec<u8> {
		fn estimate_size(&self) -> usize {
			self.len()
		}
	}

	impl EstimateSize for Option<Vec<u8>> {
		fn estimate_size(&self) -> usize {
			self.as_ref().map(|v| v.capacity()).unwrap_or(0)
		}
	}

	impl<K: EstimateSize + Eq + StdHash, V: EstimateSize> LRUMap<K, V> {
		pub(super) fn add(&mut self, k: K, v: V) {
			let lmap = &mut self.0;
			let storage_used_size = &mut self.1;
			let limit = self.2;
			let klen = k.estimate_size();
			*storage_used_size += v.estimate_size();
			match lmap.entry(k) {
				Entry::Occupied(mut entry) => {
					*storage_used_size -= entry.get().estimate_size();
					entry.insert(v);
				},
				Entry::Vacant(entry) => {
					*storage_used_size += klen;
					entry.insert(v);
				},
			};

			while *storage_used_size > limit {
				if let Some((k, v)) = lmap.pop_front() {
					*storage_used_size -= k.estimate_size();
					*storage_used_size -= v.estimate_size();
				} else {
					break
				}
			}
		}

		pub(super) fn get<Q: ?Sized>(&mut self, k: &Q) -> Option<&mut V>
		where
			K: std::borrow::Borrow<Q>,
			Q: StdHash + Eq,
		{
			self.0.get_refresh(k)
		}
	}
}
