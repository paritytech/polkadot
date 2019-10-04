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

//! Persistent database for parachain data: PoV block data, erasure-coding chunks and outgoing messages.
//!
//! This will be written into during the block validation pipeline, and queried
//! by networking code in order to circulate required data and maintain availability
//! of it.

use codec::{Encode, Decode};
use kvdb::{KeyValueDB, DBTransaction};
use kvdb_rocksdb::{Database, DatabaseConfig};
use polkadot_primitives::Hash;
use polkadot_primitives::parachain::{Id as ParaId, BlockData, Message, AvailableMessages, ErasureChunk};
use polkadot_erasure_coding::{self as erasure};
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

/// Some data to keep available about a parachain block candidate.
pub struct Data {
	/// The relay chain parent hash this should be localized to.
	pub relay_parent: Hash,
	/// The parachain index for this candidate.
	pub parachain_id: ParaId,
	/// Block data.
	pub block_data: BlockData,
	/// Outgoing message queues from execution of the block, if any.
	///
	/// The tuple pairs the message queue root and the queue data.
	pub outgoing_queues: Option<AvailableMessages>,
}

fn block_data_key(relay_parent: &Hash, block_data_hash: &Hash) -> Vec<u8> {
	(relay_parent, block_data_hash, 0i8).encode()
}

fn candidate_to_block_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 0i8).encode()
}

fn block_to_candidate_key(block_data_hash: &Hash) -> Vec<u8> {
	(block_data_hash, 1i8).encode()
}

fn erasure_chunks_key(relay_parent: &Hash, block_data_hash: &Hash) -> Vec<u8> {
	(relay_parent, block_data_hash, 1i8).encode()
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
	///
	/// Validators with the responsibility of maintaining availability
	/// for a block or collators collating a block will call this function
	/// in order to persist that data to disk and so it can be queried and provided
	/// to other nodes in the network.
	///
	/// The message data of `Data` is optional but is expected
	/// to be present with the exception of the case where there is no message data
	/// due to the block's invalidity. Determination of invalidity is beyond the
	/// scope of this function.
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

		v.push(data.block_data.hash());
		tx.put_vec(columns::META, &data.relay_parent[..], v.encode());

		tx.put_vec(
			columns::DATA,
			block_data_key(&data.relay_parent, &data.block_data.hash()).as_slice(),
			data.block_data.encode()
		);

		if let Some(outgoing_queues) = data.outgoing_queues {
			// This is kept forever and not pruned.
			for (root, messages) in outgoing_queues.0 {
				tx.put_vec(
					columns::DATA,
					root.as_ref(),
					messages.encode(),
				);
			}

		}

		self.inner.write(tx)
	}

	/// Adds an erasure chunk to storage.
	///
	/// The chunk should be checked for validness against the root of encoding
	/// and it's proof prior to calling this.
	pub fn add_erasure_chunk(&self, parachain_id: ParaId, chunk: ErasureChunk) -> io::Result<()> {
		let mut tx = DBTransaction::new();
		let relay_parent = chunk.relay_parent;
		let block_data_hash = chunk.block_data_hash;

		let key = erasure_chunks_key(&chunk.relay_parent, &chunk.block_data_hash);

		let mut v = match self.inner.get(columns::DATA, &key) {
			Ok(Some(raw)) => Vec::decode(&mut &raw[..]).expect("all sorted data serialized correctly; qed"),
			Ok(None) => Vec::new(),
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				Vec::new()
			}
		};

		v.push(chunk);

		// If therea are no block data and messages in the store at this point,
		// check that they can be reconstructed now and add them to store if they can.
		if let Ok(None) = self.inner.get(columns::DATA, &block_data_key(&relay_parent, &block_data_hash)) {
			if let Ok((block_data, outgoing_queues)) = erasure::reconstruct(v[0].n_validators as usize,
				v.iter().map(|chunk| (chunk.chunk.as_ref(), chunk.index as usize))) {
				self.make_available(Data {
					relay_parent,
					parachain_id,
					block_data,
					outgoing_queues,
				})?;
			}
		}

		tx.put_vec(columns::DATA, &key, v.encode());

		self.inner.write(tx)
	}

	/// Queries an erasure chunk by it's block's parent and hash and index.
	pub fn get_erasure_chunk(&self, relay_parent: Hash, block_data_hash: Hash, index: usize) -> Option<ErasureChunk> {
		let key = erasure_chunks_key(&relay_parent, &block_data_hash);

		let v = match self.inner.get(columns::DATA, &key) {
			Ok(Some(raw)) => Vec::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed"),
			Ok(None) => return None,
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				return None;
			}
		};

		v.iter()
			.find(|chunk: &&ErasureChunk| chunk.index == index as u32)
			.map(|chunk| chunk.clone())
	}

	/// Adds a mapping from candidate's receipt hash to block data hash.
	pub fn add_candidate_for_block(&self, candidate_hash: Hash, block_data_hash: Hash) -> io::Result<()> {
		let mut tx = DBTransaction::new();

		tx.put_vec(columns::META, &block_to_candidate_key(&block_data_hash), candidate_hash.encode());
		tx.put_vec(columns::META, &candidate_to_block_key(&candidate_hash), block_data_hash.encode());

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

		for block_data_hash in v {
			if let Some(candidate_hash) = self.block_hash_to_candidate_hash(block_data_hash) {
				if !finalized_candidates.contains(&candidate_hash) {
					tx.delete(columns::DATA, block_data_key(&parent, &block_data_hash).as_slice());
					tx.delete(columns::DATA, &erasure_chunks_key(&parent, &block_data_hash));
					tx.delete(columns::META, &block_to_candidate_key(&block_data_hash));
					tx.delete(columns::META, &candidate_to_block_key(&candidate_hash));
				}
			}
		}

		self.inner.write(tx)
	}

	/// Query block data.
	pub fn block_data(&self, relay_parent: Hash, block_data_hash: Hash) -> Option<BlockData> {
		let encoded_key = block_data_key(&relay_parent, &block_data_hash);
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

	/// Query block data by corresponding candidate receipt's hash.
	pub fn block_data_by_candidate(&self, relay_parent: Hash, candidate_hash: Hash) -> Option<BlockData> {
		let block_data_hash = self.candidate_hash_to_block_hash(candidate_hash)?;

		self.block_data(relay_parent, block_data_hash)
	}

	/// Query message queue data by message queue root hash.
	pub fn queue_by_root(&self, queue_root: &Hash) -> Option<Vec<Message>> {
		match self.inner.get(columns::DATA, queue_root.as_ref()) {
			Ok(Some(raw)) => Some(
				<_>::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed")
			),
			Ok(None) => None,
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				None
			}
		}
	}

	fn candidate_hash_to_block_hash(&self, candidate_hash: Hash) -> Option<Hash> {
		match self.inner.get(columns::META, &candidate_to_block_key(&candidate_hash)) {
			Ok(Some(raw)) => Some(Hash::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed")),
			Ok(None) => None,
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				None
			}
		}
	}

	fn block_hash_to_candidate_hash(&self, block_hash: Hash) -> Option<Hash> {
		match self.inner.get(columns::META, &block_to_candidate_key(&block_hash)) {
			Ok(Some(raw)) => Some(Hash::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed")),
				Ok(None) => None,
				Err(e) => {
					warn!(target: "availability", "Error reading from availability store: {:?}", e);
				None
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
    use polkadot_erasure_coding::{self as erasure};

	#[test]
	fn finalization_removes_unneeded() {
		let relay_parent = [1; 32].into();

		let para_id_1 = 5.into();
		let para_id_2 = 6.into();

		let candidate_1 = [2; 32].into();
		let candidate_2 = [3; 32].into();

		let block_data_1 = BlockData(vec![1, 2, 3]);
		let block_data_2 = BlockData(vec![4, 5, 6]);

		let erasure_chunk_1 = ErasureChunk {
			relay_parent,
			chunk: vec![10, 20, 30],
			block_data_hash: block_data_1.hash(),
			index: 1,
			n_validators: 3,
			parachain_id: para_id_1,
			proof: vec![],
		};

		let erasure_chunk_2 = ErasureChunk {
			relay_parent,
			chunk: vec![40, 50, 60],
			block_data_hash: block_data_2.hash(),
			index: 1,
			parachain_id: para_id_2,
			n_validators: 3,
			proof: vec![],
		};

		let store = Store::new_in_memory();
		store.make_available(Data {
			relay_parent,
			parachain_id: para_id_1,
			block_data: block_data_1.clone(),
			outgoing_queues: None,
		}).unwrap();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id_2,
			block_data: block_data_2.clone(),
			outgoing_queues: None,
		}).unwrap();

		assert!(store.add_erasure_chunk(1.into(), erasure_chunk_1.clone()).is_ok());
		assert!(store.add_erasure_chunk(1.into(), erasure_chunk_2.clone()).is_ok());
		assert_eq!(store.block_data(relay_parent, block_data_1.hash()).unwrap(), block_data_1);
		assert_eq!(store.block_data(relay_parent, block_data_2.hash()).unwrap(), block_data_2);

		assert_eq!(store.get_erasure_chunk(relay_parent, block_data_1.hash(), 1).as_ref(), Some(&erasure_chunk_1));
		assert_eq!(store.get_erasure_chunk(relay_parent, block_data_2.hash(), 1), Some(erasure_chunk_2));

		assert!(store.block_data_by_candidate(relay_parent, candidate_1).is_none());
		assert!(store.block_data_by_candidate(relay_parent, candidate_2).is_none());

		store.add_candidate_for_block(candidate_1, block_data_1.hash()).unwrap();
		store.add_candidate_for_block(candidate_2, block_data_2.hash()).unwrap();

		assert_eq!(store.block_data_by_candidate(relay_parent, candidate_1).unwrap(), block_data_1);
		assert_eq!(store.block_data_by_candidate(relay_parent, candidate_2).unwrap(), block_data_2);

		store.candidates_finalized(relay_parent, [candidate_1].iter().cloned().collect()).unwrap();

		assert_eq!(store.get_erasure_chunk(relay_parent, block_data_1.hash(), 1).as_ref(), Some(&erasure_chunk_1));
		assert!(store.get_erasure_chunk(relay_parent, block_data_2.hash(), 1).is_none());

		assert_eq!(store.block_data(relay_parent, block_data_1.hash()).unwrap(), block_data_1);
		assert!(store.block_data(relay_parent, block_data_2.hash()).is_none());
	}

	#[test]
	fn queues_available_by_queue_root() {
		let relay_parent = [1; 32].into();
		let para_id = 5.into();
		let block_data = BlockData(vec![1, 2, 3]);

		let message_queue_root_1 = [0x42; 32].into();
		let message_queue_root_2 = [0x43; 32].into();

		let message_a = Message(vec![1, 2, 3, 4]);
		let message_b = Message(vec![4, 5, 6, 7]);

		let outgoing_queues = AvailableMessages(vec![
			(message_queue_root_1, vec![message_a.clone()]),
			(message_queue_root_2, vec![message_b.clone()]),
		]);

		let store = Store::new_in_memory();
		store.make_available(Data {
			relay_parent,
			parachain_id: para_id,
			block_data: block_data.clone(),
			outgoing_queues: Some(outgoing_queues),
		}).unwrap();

		assert_eq!(
			store.queue_by_root(&message_queue_root_1),
			Some(vec![message_a]),
		);

		assert_eq!(
			store.queue_by_root(&message_queue_root_2),
			Some(vec![message_b]),
		);
	}

	#[test]
	fn erasure_coding() {
		let relay_parent: Hash = [1; 32].into();
		let para_id: ParaId = 5.into();
		let block_data = BlockData(vec![42; 8]);
		let block_data_hash = block_data.hash();
		let n_validators = 5;

		let message_queue_root_1 = [0x42; 32].into();
		let message_queue_root_2 = [0x43; 32].into();

		let message_a = Message(vec![1, 2, 3, 4]);
		let message_b = Message(vec![5, 6, 7, 8]);

		let outgoing_queues = Some(AvailableMessages(vec![
				(message_queue_root_1, vec![message_a.clone()]),
				(message_queue_root_2, vec![message_b.clone()]),
		]));

		let erasure_chunks = erasure::obtain_chunks(
			n_validators,
			&block_data,
			outgoing_queues.as_ref()).unwrap();

		let branches = erasure::branches(erasure_chunks.as_ref());

		let chunks: Vec<_> = erasure_chunks
			.iter()
			.zip(branches.map(|(proof, _)| proof))
			.enumerate()
			.map(|(index, (chunk, proof))| ErasureChunk {
				relay_parent,
				chunk: chunk.clone(),
				block_data_hash,
				n_validators: n_validators as u32,
				parachain_id: para_id,
				proof,
				index: index as u32,
			})
		.collect();

	let store = Store::new_in_memory();

	store.add_erasure_chunk(para_id, chunks[0].clone()).unwrap();
	assert_eq!(store.get_erasure_chunk(relay_parent, block_data_hash, 0), Some(chunks[0].clone()));

	assert!(store.block_data(relay_parent, block_data_hash).is_none());

	for chunk in chunks {
		store.add_erasure_chunk(para_id, chunk.clone()).unwrap();
	}
	assert_eq!(store.block_data(relay_parent, block_data_hash), Some(block_data));
	}
}
