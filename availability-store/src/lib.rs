// Copyright 2018 Parity Technologies (UK) Ltd.
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

//! Persistent database for parachain data.

use codec::{Encode, Decode};
use kvdb::{KeyValueDB, DBTransaction};
use kvdb_rocksdb::{Database, DatabaseConfig};
use polkadot_primitives::Hash;
use polkadot_primitives::parachain::{Id as ParaId, BlockData, Extrinsic};
use log::warn;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::io;

mod columns {
	pub const DATA: Option<u32> = Some(0);
	pub const META: Option<u32> = Some(1);
	pub const NUM_COLUMNS: u32 = 2;
}

/// Configuration for the availability store.
pub struct Config {
	/// Cache size in bytes. If `None` default is used.
	pub cache_size: Option<usize>,
	/// Path to the database.
	pub path: PathBuf,
}

/// A record containing information about parachain extrinsic data to store.
pub struct StoredExtrinsic {
	/// The actual extrinsic data.
	pub inner: Extrinsic,
	/// The queue root of the `outgoing_messages` member of `inner`.
	///
	/// The responsibility is on the creator to set this correctly.
	pub message_queue_root: Hash,
}

/// Some data to keep available.
pub struct Data {
	/// The relay chain parent hash this should be localized to.
	pub relay_parent: Hash,
	/// The parachain index for this candidate.
	pub parachain_id: ParaId,
	/// Unique candidate receipt hash.
	pub candidate_hash: Hash,
	/// Block data.
	pub block_data: BlockData,
	/// Extrinsic data.
	pub extrinsic: Option<StoredExtrinsic>,
}

fn block_data_key(relay_parent: &Hash, candidate_hash: &Hash) -> Vec<u8> {
	(relay_parent, candidate_hash, 0i8).encode()
}

fn extrinsic_key(relay_parent: &Hash, candidate_hash: &Hash) -> Vec<u8> {
	(relay_parent, candidate_hash, 1i8).encode()
}

/// Handle to the availability store.
#[derive(Clone)]
pub struct Store {
	inner: Arc<dyn KeyValueDB>,
}

impl Store {
	/// Create a new `Store` with given config on disk.
	pub fn new(config: Config) -> io::Result<Self> {
		let mut db_config = DatabaseConfig::with_columns(Some(columns::NUM_COLUMNS));
		db_config.memory_budget = config.cache_size;

		let path = config.path.to_str().ok_or_else(|| io::Error::new(
			io::ErrorKind::Other,
			format!("Bad database path: {:?}", config.path),
		))?;

		let db = Database::open(&db_config, &path)?;

		Ok(Store {
			inner: Arc::new(db),
		})
	}

	/// Create a new `Store` in-memory. Useful for tests.
	pub fn new_in_memory() -> Self {
		Store {
			inner: Arc::new(::kvdb_memorydb::create(columns::NUM_COLUMNS)),
		}
	}

	/// Make some data available provisionally.
	pub fn make_available(&self, data: Data) -> io::Result<()> {
		let mut tx = DBTransaction::new();

		// note the meta key.
		let mut v = match self.inner.get(columns::META, data.relay_parent.as_ref()) {
			Ok(Some(raw)) => Vec::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed"),
			Ok(None) => Vec::new(),
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				Vec::new()
			}
		};

		v.push(data.candidate_hash);
		tx.put_vec(columns::META, &data.relay_parent[..], v.encode());

		tx.put_vec(
			columns::DATA,
			block_data_key(&data.relay_parent, &data.candidate_hash).as_slice(),
			data.block_data.encode()
		);

		if let Some(extrinsic) = data.extrinsic {
			let encoded = extrinsic.inner.encode();
			tx.put_vec(
				columns::DATA,
				extrinsic_key(&data.relay_parent, &data.candidate_hash).as_slice(),
				encoded.clone(),
			);

			tx.put_vec(
				columns::DATA,
				extrinsic.message_queue_root.as_ref(),
				encoded,
			);
		}

		self.inner.write(tx)
	}

	/// Note that a set of candidates have been included in a finalized block with given hash and parent hash.
	pub fn candidates_finalized(&self, parent: Hash, finalized_candidates: HashSet<Hash>) -> io::Result<()> {
		let mut tx = DBTransaction::new();

		let v = match self.inner.get(columns::META, &parent[..]) {
			Ok(Some(raw)) => Vec::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed"),
			Ok(None) => Vec::new(),
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				Vec::new()
			}
		};
		tx.delete(columns::META, &parent[..]);

		for candidate_hash in v {
			if !finalized_candidates.contains(&candidate_hash) {
				tx.delete(columns::DATA, block_data_key(&parent, &candidate_hash).as_slice());
				tx.delete(columns::DATA, extrinsic_key(&parent, &candidate_hash).as_slice());
			}
		}

		self.inner.write(tx)
	}

	/// Query block data.
	pub fn block_data(&self, relay_parent: Hash, candidate_hash: Hash) -> Option<BlockData> {
		let encoded_key = block_data_key(&relay_parent, &candidate_hash);
		match self.inner.get(columns::DATA, &encoded_key[..]) {
			Ok(Some(raw)) => Some(
				BlockData::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed")
			),
			Ok(None) => None,
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				None
			}
		}
	}

	fn extrinsic_with_key(&self, key: &[u8]) -> Option<Extrinsic> {
		match self.inner.get(columns::DATA, key) {
			Ok(Some(raw)) => Some(
				Extrinsic::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed")
			),
			Ok(None) => None,
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				None
			}
		}
	}

	/// Query extrinsic data by relay parent and candidate hash.
	pub fn extrinsic(&self, relay_parent: Hash, candidate_hash: Hash) -> Option<Extrinsic> {
		let encoded_key = extrinsic_key(&relay_parent, &candidate_hash);
		self.extrinsic_with_key(&encoded_key[..])
	}

	/// Query extrinsic data by message queue root hash.
	pub fn extrinsic_by_queue_root(&self, queue_root: &Hash) -> Option<Extrinsic> {
		self.extrinsic_with_key(queue_root.as_ref())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::parachain::OutgoingMessage;

	#[test]
	fn finalization_removes_unneeded() {
		let relay_parent = [1; 32].into();

		let para_id_1 = 5.into();
		let para_id_2 = 6.into();

		let candidate_1 = [2; 32].into();
		let candidate_2 = [3; 32].into();

		let block_data_1 = BlockData(vec![1, 2, 3]);
		let block_data_2 = BlockData(vec![4, 5, 6]);

		let store = Store::new_in_memory();
		store.make_available(Data {
			relay_parent,
			parachain_id: para_id_1,
			candidate_hash: candidate_1,
			block_data: block_data_1.clone(),
			extrinsic: Some(StoredExtrinsic {
				inner: Extrinsic { outgoing_messages: Vec::new() },
				message_queue_root: Default::default(),
			}),
		}).unwrap();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id_2,
			candidate_hash: candidate_2,
			block_data: block_data_2.clone(),
			extrinsic: Some(StoredExtrinsic {
				inner: Extrinsic { outgoing_messages: Vec::new() },
				message_queue_root: Default::default(),
			}),
		}).unwrap();

		assert_eq!(store.block_data(relay_parent, candidate_1).unwrap(), block_data_1);
		assert_eq!(store.block_data(relay_parent, candidate_2).unwrap(), block_data_2);

		assert!(store.extrinsic(relay_parent, candidate_1).is_some());
		assert!(store.extrinsic(relay_parent, candidate_2).is_some());

		store.candidates_finalized(relay_parent, [candidate_1].iter().cloned().collect()).unwrap();

		assert_eq!(store.block_data(relay_parent, candidate_1).unwrap(), block_data_1);
		assert!(store.block_data(relay_parent, candidate_2).is_none());

		assert!(store.extrinsic(relay_parent, candidate_1).is_some());
		assert!(store.extrinsic(relay_parent, candidate_2).is_none());
	}

	#[test]
	fn extrinsic_available_by_queue_root() {
		let relay_parent = [1; 32].into();
		let para_id = 5.into();
		let candidate = [2; 32].into();
		let block_data = BlockData(vec![1, 2, 3]);

		let outgoing_messages = vec![
			OutgoingMessage { target: 1.into(), data: vec![1, 2, 3, 4] },
			OutgoingMessage { target: 2.into(), data: vec![5, 6, 7, 8] },
		];
		let ex = Extrinsic { outgoing_messages };

		let message_queue_root = [0x42; 32].into();

		let store = Store::new_in_memory();
		store.make_available(Data {
			relay_parent,
			parachain_id: para_id,
			candidate_hash: candidate,
			block_data: block_data.clone(),
			extrinsic: Some(StoredExtrinsic {
				inner: ex.clone(),
				message_queue_root,
			}),
		}).unwrap();

		assert_eq!(store.extrinsic_by_queue_root(&message_queue_root), Some(ex));
	}
}
