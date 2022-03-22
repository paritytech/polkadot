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

		fn get_by_prefix(&self, col: u32, prefix: &[u8]) -> Option<Box<[u8]>> {
			self.ensure_is_indexed(col);
			self.db.get_by_prefix(col, prefix)
		}

		fn write(&self, transaction: DBTransaction) -> Result<()> {
			self.ensure_ops_indexing(&transaction);
			self.db.write(transaction)
		}

		fn iter<'a>(&'a self, col: u32) -> Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a> {
			self.ensure_is_indexed(col);
			self.db.iter(col)
		}

		fn iter_with_prefix<'a>(
			&'a self,
			col: u32,
			prefix: &'a [u8],
		) -> Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a> {
			self.ensure_is_indexed(col);
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
			self.ensure_is_indexed(col);
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

/// Utilities for using parity-db database.
pub mod paritydb_impl {
	use super::{DBTransaction, DBValue, Database, KeyValueDB};
	use kvdb::{DBOp, IoStats, IoStatsKind};
	use linked_hash_map::{Entry, LinkedHashMap};
	use parity_db::Db;
	use parking_lot::{Mutex, RwLock};
	use std::{collections::BTreeSet, hash::Hash as StdHash, io::Result, sync::Arc};

	type ColumnId = u32;

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
		cache: Cache,
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
			if let Some(res) = self.cache.get(col, key) {
				return Ok(res)
			}
			let res = map_err(self.db.get(col as u8, key));
			if let Ok(res) = res.as_ref() {
				self.cache.add_val(col, key, res.as_ref());
			}
			res
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
			self.cache.write(&transaction)?;
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
		pub fn new(db: Db, indexed_columns: &[u32], cache_sizes: &[usize]) -> Self {
			let write_lock = Arc::new(Mutex::new(()));
			let mut cache = Cache::new();
			for (i, c) in cache_sizes.iter().enumerate() {
				if c > &0 {
					cache.configure_cache(i as u32, Some(*c));
				}
			}

			DbAdapter {
				db,
				indexed_columns: indexed_columns.iter().cloned().collect(),
				write_lock,
				cache,
			}
		}
	}

	/// Simple lru cache for a colum.
	#[derive(Clone)]
	struct Cache {
		has_lru: BTreeSet<ColumnId>,
		// Note that design really only work for trie node (we do not cache
		// deletion).
		// TODO a RwLock per column!!
		lru: Option<Arc<RwLock<Vec<Option<LRUMap<Vec<u8>, Option<Vec<u8>>>>>>>>,
	}

	impl Cache {
		/// New db with unconfigured cache.
		pub fn new() -> Self {
			Cache { has_lru: Default::default(), lru: None }
		}

		/// define cache for a given column.
		pub fn configure_cache(&mut self, column: ColumnId, size: Option<usize>) {
			if size.is_some() {
				self.has_lru.insert(column);
			} else {
				self.has_lru.remove(&column);
			}
			let new_cache = size.map(|size| LRUMap::new(size));
			if self.lru.is_none() {
				if new_cache.is_none() {
					return
				}
				let mut caches = Vec::with_capacity(column as usize + 1);
				while caches.len() < column as usize + 1 {
					caches.push(None);
				}
				caches[column as usize] = new_cache;
				self.lru = Some(Arc::new(RwLock::new(caches)));
			} else {
				self.lru.as_ref().map(|lru| {
					let mut lru = lru.write();
					while lru.len() < column as usize + 1 {
						lru.push(None);
					}
					lru[column as usize] = new_cache;
				});
			}
		}

		fn add_val(&self, col: ColumnId, key: &[u8], value: Option<&Vec<u8>>) {
			if let Some(lru) = self.lru.as_ref() {
				if self.has_lru.contains(&col) {
					let mut lru = lru.write();
					if let Some(Some(lru)) = lru.get_mut(col as usize) {
						lru.add(key.to_vec(), value.cloned());
					}
				}
			}
		}
		/*
				fn add_val_slice(&self, col: ColumnId, key: &[u8], value: &[u8]) {
					if let Some(lru) = self.lru.as_ref()  {
						if self.has_lru.contains(&col) {
							let mut lru = lru.write();
							if let Some(Some(lru)) = lru.get_mut(col as usize) {
								lru.add(key.to_vec(), Some(value.to_vec()));
							}
						}
					}
				}
		*/
		fn write(&self, transaction: &DBTransaction) -> Result<()> {
			let mut delete_prefix = BTreeSet::<u32>::new();
			let cache_update: Vec<_> = if self.lru.is_some() {
				transaction
					.ops
					.iter()
					.filter_map(|change| {
						if let DBOp::Insert { col, key, value } = change {
							self.has_lru
								.contains(&col)
								.then(|| (col, key.clone(), Some(value.clone())))
						} else if let DBOp::Delete { col, key } = change {
							self.has_lru.contains(&col).then(|| (col, key.clone(), None))
						} else if let DBOp::DeletePrefix { col, .. } = change {
							delete_prefix.insert(*col);
							None
						} else {
							None
						}
					})
					.collect()
			} else {
				Vec::new()
			};

			// TODO can also clear cache on commit error (those are rare enough to avoid putting all
			// update in mem.
			if let Some(lru) = self.lru.as_ref() {
				let mut lru = lru.write();
				for (col, key, value) in cache_update {
					if !delete_prefix.contains(col) {
						if let Some(Some(lru)) = lru.get_mut(*col as usize) {
							if value.is_some() {
								lru.add(key.to_vec(), value);
							} else {
								// small change it will be queried again, remove.
								lru.remove(&key.to_vec());
							}
						}
					}
				}
				for col in delete_prefix {
					if let Some(Some(lru)) = lru.get_mut(col as usize) {
						lru.clear();
					}
				}
			}

			Ok(())
		}

		fn get(&self, col: ColumnId, key: &[u8]) -> Option<Option<Vec<u8>>> {
			if let Some(lru) = self.lru.as_ref() {
				if self.has_lru.contains(&col) {
					let mut lru = lru.write();
					if let Some(Some(lru)) = lru.get_mut(col as usize) {
						if let Some(node) = lru.get(key) {
							return Some(node.clone())
						}
					}
				}
			}
			None
		}
		/*
				fn contains(&self, col: ColumnId, key: &[u8]) -> Option<bool> {
					if let Some(lru) = self.lru.as_ref()  {
						if self.has_lru.contains(&col) {
							let mut lru = lru.write();
							if let Some(Some(lru)) = lru.get_mut(col as usize) {
								if let Some(cache) = lru.get(key) {
									return Some(cache.is_some());
								}
							}
						}
					}

					None
				}

				fn value_size(&self, col: ColumnId, key: &[u8]) -> Option<Option<usize>> {
					if let Some(lru) = self.lru.as_ref()  {
						if self.has_lru.contains(&col) {
							let mut lru = lru.write();
							if let Some(Some(lru)) = lru.get_mut(col as usize) {
								if let Some(node) = lru.get(key) {
									return Some(node.as_ref().map(|value| value.len()));
								}
							}
						}
					}

					None
				}
		*/
	}

	struct LRUMap<K, V>(LinkedHashMap<K, V>, usize, usize);

	impl<K, V> LRUMap<K, V>
	where
		K: std::hash::Hash + Eq,
	{
		pub(crate) fn new(size_limit: usize) -> Self {
			LRUMap(LinkedHashMap::new(), 0, size_limit)
		}
	}

	/// Internal trait similar to `heapsize` but using
	/// a simply estimation.
	///
	/// This should not be made public, it is implementation
	/// detail trait. If it need to become public please
	/// consider using `malloc_size_of`.
	pub(crate) trait EstimateSize {
		/// Return a size estimation of additional size needed
		/// to cache this struct (in bytes).
		fn estimate_size(&self) -> usize;
	}

	impl EstimateSize for Vec<u8> {
		fn estimate_size(&self) -> usize {
			self.capacity()
		}
	}

	impl EstimateSize for Option<Vec<u8>> {
		fn estimate_size(&self) -> usize {
			self.as_ref().map(|v| v.capacity()).unwrap_or(0)
		}
	}

	struct OptionHOut<T: AsRef<[u8]>>(Option<T>);

	impl<T: AsRef<[u8]>> EstimateSize for OptionHOut<T> {
		fn estimate_size(&self) -> usize {
			// capacity would be better
			self.0.as_ref().map(|v| v.as_ref().len()).unwrap_or(0)
		}
	}

	impl<T: EstimateSize> EstimateSize for (T, T) {
		fn estimate_size(&self) -> usize {
			self.0.estimate_size() + self.1.estimate_size()
		}
	}

	impl<K: EstimateSize + Eq + StdHash, V: EstimateSize> LRUMap<K, V> {
		fn clear(&mut self) {
			self.0.clear();
			self.1 = 0;
		}

		fn remove(&mut self, k: &K) {
			let map = &mut self.0;
			let storage_used_size = &mut self.1;
			if let Some(v) = map.remove(k) {
				*storage_used_size -= k.estimate_size();
				*storage_used_size -= v.estimate_size();
			}
		}

		pub(crate) fn add(&mut self, k: K, v: V) {
			let lmap = &mut self.0;
			let storage_used_size = &mut self.1;
			let limit = self.2;
			let klen = k.estimate_size();
			*storage_used_size += v.estimate_size();
			// TODO assert k v size fit into limit?? to avoid insert remove?
			match lmap.entry(k) {
				Entry::Occupied(mut entry) => {
					// note that in this case we are not running pure lru as
					// it would require to remove first
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
					// can happen fairly often as we get value from multiple lru
					// and only remove from a single lru
					break
				}
			}
		}

		pub(crate) fn get<Q: ?Sized>(&mut self, k: &Q) -> Option<&mut V>
		where
			K: std::borrow::Borrow<Q>,
			Q: StdHash + Eq,
		{
			self.0.get_refresh(k)
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
