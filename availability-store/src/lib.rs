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
use polkadot_primitives::parachain::{
	Id as ParaId, BlockData, CandidateReceipt, Message, AvailableMessages, ErasureChunk
};
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

fn erasure_chunks_key(relay_parent: &Hash, block_data_hash: &Hash) -> Vec<u8> {
	(relay_parent, block_data_hash, 1i8).encode()
}

fn awaited_chunks_key() -> Vec<u8> {
	"awaited_chunks_key".encode()
}

fn available_chunks_key(relay_parent: &Hash, erasure_root: &Hash) -> Vec<u8> {
	(relay_parent, erasure_root, 2i8).encode()
}

fn block_to_candidate_key(block_data_hash: &Hash) -> Vec<u8> {
	(block_data_hash, 1i8).encode()
}


fn candidate_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 2i8).encode()
}

fn validator_index_and_n_validators_key(relay_parent: &Hash) -> Vec<u8> {
	(relay_parent, 3i8).encode()
}

fn candidates_in_relay_chain_block_key(relay_block: &Hash) -> Vec<u8> {
	(relay_block, 4i8).encode()
}

fn erasure_roots_in_relay_chain_block_key(relay_block: &Hash) -> Vec<u8> {
	(relay_block, 5i8).encode()
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
		let mut v = self.query_inner(columns::META, data.relay_parent.as_ref()).unwrap_or(Vec::new());
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

	/// Get a set of all chunks we are waiting for grouped by (relay_parent, candidate_hash, our_id).
	pub fn awaited_chunks(&self) -> Option<HashSet<(Hash, Hash, u32)>> {
		use std::iter::FromIterator;
		self.query_inner(columns::META, &awaited_chunks_key()).map(|vec: Vec<(Hash, Hash, u32)>| {
			HashSet::from_iter(vec.into_iter())
		})
	}

	/// Adds a set of candidates hashes that were included in a relay block by the block's parent.
	pub fn add_candidates_in_relay_block(&self, relay_parent: &Hash, candidates: Vec<Hash>) -> io::Result<()> {
		use std::iter::FromIterator;

		let mut tx = DBTransaction::new();
		let dbkey = candidates_in_relay_chain_block_key(relay_parent);

		if let Some((validator_index, _)) = self.get_validator_index_and_n_validators(relay_parent) {
			let candidates = candidates.clone();
			let awaited_frontier: Vec<(Hash, Hash, u32)> = self
				.query_inner(columns::META, &awaited_chunks_key())
				.unwrap_or_else(|| Vec::new());

			let mut awaited_frontier: HashSet<(Hash, Hash, u32)> = HashSet::from_iter(awaited_frontier.into_iter());

			awaited_frontier.extend(candidates.into_iter().map(|candidate| {
				(relay_parent.clone(), candidate, validator_index)
			}));
			let awaited_frontier = Vec::from_iter(awaited_frontier.into_iter());
			tx.put_vec(columns::META, &awaited_chunks_key(), awaited_frontier.encode());
		}
		tx.put_vec(columns::DATA, &dbkey, candidates.encode());

		self.inner.write(tx)
	}

	/// Qery which candidates were included in the relay chain block by block's parent.
	pub fn get_candidates_in_relay_block(&self, relay_block: &Hash) -> Option<Vec<Hash>> {
		let dbkey = candidates_in_relay_chain_block_key(relay_block);

		self.query_inner(columns::DATA, &dbkey)
	}

	/// Adds a set of erasure chunk roots that were included in a relay block by block's parent.
	pub fn add_erasure_roots_in_relay_block(&self, relay_parent: &Hash, erasure_roots: Vec<Hash>) -> io::Result<()> {
		let mut tx = DBTransaction::new();
		let dbkey = erasure_roots_in_relay_chain_block_key(relay_parent);

		tx.put_vec(columns::DATA, &dbkey, erasure_roots.encode());

		self.inner.write(tx)
	}

	/// Query erasure roots included in the relay chain block by block's parent.
	pub fn get_erasure_roots_in_relay_block(&self, relay_parent: &Hash) -> Option<Vec<Hash>> {
		let dbkey = erasure_roots_in_relay_chain_block_key(relay_parent);

		self.query_inner(columns::DATA, &dbkey)
	}

	/// Make a validator's index and a number of validators at a relay parent available.
	// TODO: The tuple of two value is used here because for the most part thay are used together,
	// however, maybe they should be stored/queried by separate methods.
	pub fn add_validator_index_and_n_validators(
		&self,
		relay_parent: &Hash,
		validator_index: u32,
		n_validators: u32
	) -> io::Result<()> {
		let mut tx = DBTransaction::new();
		let dbkey = validator_index_and_n_validators_key(relay_parent);

		tx.put_vec(columns::META, &dbkey, (validator_index, n_validators).encode());

		self.inner.write(tx)
	}

	/// Query a validator's index and n_validators by relay parent.
	pub fn get_validator_index_and_n_validators(&self, relay_parent: &Hash) -> Option<(u32, u32)> {
		let dbkey = validator_index_and_n_validators_key(relay_parent);

		self.query_inner(columns::META, &dbkey)
	}

	/// Adds an erasure chunk to storage.
	///
	/// The chunk should be checked for validity against the root of encoding
	/// and its proof prior to calling this.
	pub fn add_erasure_chunk(
		&self,
		n_validators: u32,
		relay_parent: &Hash,
		receipt: &CandidateReceipt,
		chunk: ErasureChunk
	) -> io::Result<()> {
		let mut tx = DBTransaction::new();
		let block_data_hash = &receipt.block_data_hash;
		let chunk_index = chunk.index;

		let dbkey = erasure_chunks_key(relay_parent, block_data_hash);
		let mut v = self.query_inner(columns::DATA, &dbkey).unwrap_or(Vec::new());
		v.push(chunk);

		// If there are no block data and messages in the store at this point,
		// check that they can be reconstructed now and add them to store if they can.
		if let Ok(None) = self.inner.get(columns::DATA, &block_data_key(&relay_parent, &block_data_hash)) {
			if let Ok((block_data, outgoing_queues)) = erasure::reconstruct(n_validators as usize,
				v.iter().map(|chunk| (chunk.chunk.as_ref(), chunk.index as usize))) {
				self.make_available(Data {
					relay_parent: *relay_parent,
					parachain_id: receipt.parachain_index,
					block_data,
					outgoing_queues,
				})?;
			}
		}

		let av_chunks_key = available_chunks_key(relay_parent, &receipt.erasure_root);
		let mut have_chunks = self.query_inner(columns::META, &av_chunks_key).unwrap_or(Vec::new());
		if !have_chunks.contains(&chunk_index) {
			have_chunks.push(chunk_index);
		}

		let awaited_frontier: Option<Vec<(Hash, Hash, u32)>> = self.query_inner(columns::META, &awaited_chunks_key());

		if let Some(mut awaited_frontier) = awaited_frontier {
			awaited_frontier.retain(|&(p, c, index)| {
				!(*relay_parent == p && c == receipt.hash() && index == chunk_index)
			});
			tx.put_vec(columns::META, &awaited_chunks_key(), awaited_frontier.encode());
		}

		tx.put_vec(columns::DATA, &dbkey, v.encode());
		tx.put_vec(columns::META, &av_chunks_key, have_chunks.encode());

		self.inner.write(tx)
	}

	/// Add a set of chunks.
	///
	/// The same as `add_erasure_chunk` but adds a set of chunks in one atomic transaction.
	/// Checks that all chunks have the same `relay_parent`, `block_data_hash` and `parachain_id` fields.
	pub fn add_erasure_chunks<I>(
		&self,
		n_validators: usize,
		relay_parent: &Hash,
		receipt: &CandidateReceipt,
		chunks: I)
	-> io::Result<()>
		where I: IntoIterator<Item = ErasureChunk>
	{
		let mut tx = DBTransaction::new();
		let dbkey = erasure_chunks_key(relay_parent, &receipt.block_data_hash);

		let mut v = self.query_inner(columns::DATA, &dbkey).unwrap_or(Vec::new());

		let av_chunks_key = available_chunks_key(relay_parent, &receipt.erasure_root);
		let mut have_chunks = self.query_inner(columns::META, &av_chunks_key).unwrap_or(Vec::new());

		for chunk in chunks.into_iter() {
			if !have_chunks.contains(&chunk.index) {
				have_chunks.push(chunk.index);
			}
			v.push(chunk);
		}

		// If therea are no block data and messages in the store at this point,
		// check that they can be reconstructed now and add them to store if they can.
		if let Ok(None) = self.inner.get(columns::DATA, &block_data_key(&relay_parent, &receipt.block_data_hash)) {
			if let Ok((block_data, outgoing_queues)) = erasure::reconstruct(n_validators as usize,
				v.iter().map(|chunk| (chunk.chunk.as_ref(), chunk.index as usize))) {
				self.make_available(Data {
					relay_parent: *relay_parent,
					parachain_id: receipt.parachain_index,
					block_data,
					outgoing_queues,
				})?;
			}
		}

		tx.put_vec(columns::DATA, &dbkey, v.encode());
		tx.put_vec(columns::META, &av_chunks_key, have_chunks.encode());

		self.inner.write(tx)
	}

	/// Queries an erasure chunk by its block's parent and hash and index.
	pub fn get_erasure_chunk(&self, relay_parent: &Hash, block_data_hash: Hash, index: usize) -> Option<ErasureChunk> {
		self.query_inner(columns::DATA, &erasure_chunks_key(&relay_parent, &block_data_hash))
			.and_then(|chunks: Vec<ErasureChunk>| {
				chunks.iter()
				.find(|chunk: &&ErasureChunk| chunk.index == index as u32)
				.map(|chunk| chunk.clone())
			})
	}

	/// Stores a candidate receipt.
	pub fn add_candidate(&self, receipt: &CandidateReceipt) -> io::Result<()> {
		let dbkey = candidate_key(&receipt.hash());
		let mut tx = DBTransaction::new();

		tx.put_vec(columns::DATA, &dbkey, receipt.encode());
		tx.put_vec(columns::META, &block_to_candidate_key(&receipt.block_data_hash), receipt.hash().encode());

		self.inner.write(tx)
	}

	/// Queries a candidate receipt by it's hash.
	pub fn get_candidate(&self, candidate_hash: &Hash) -> Option<CandidateReceipt> {
		self.query_inner(columns::DATA, &candidate_key(candidate_hash))
	}

	/// Note that a set of candidates have been included in a finalized block with given hash and parent hash.
	pub fn candidates_finalized(&self, parent: Hash, finalized_candidates: HashSet<Hash>) -> io::Result<()> {
		let mut tx = DBTransaction::new();

		let v = self.query_inner(columns::META, &parent[..]).unwrap_or(Vec::new());
		tx.delete(columns::META, &parent[..]);

		let awaited_frontier: Option<Vec<(Hash, Hash, u32)>> = self.query_inner(columns::META, &awaited_chunks_key());

		if let Some(mut awaited_frontier) = awaited_frontier {
			awaited_frontier.retain(|&(p, c, _)| (p != parent && !finalized_candidates.contains(&c)));
			tx.put_vec(columns::META, &awaited_chunks_key(), awaited_frontier.encode());
		}

		for block_data_hash in v {
			if let Some(candidate_hash) = self.block_hash_to_candidate_hash(block_data_hash) {
				if !finalized_candidates.contains(&candidate_hash) {
					tx.delete(columns::DATA, block_data_key(&parent, &block_data_hash).as_slice());
					tx.delete(columns::DATA, &erasure_chunks_key(&parent, &block_data_hash));
					tx.delete(columns::DATA, &candidate_key(&candidate_hash));
					tx.delete(columns::META, &block_to_candidate_key(&block_data_hash));
				}
			}
		}

		self.inner.write(tx)
	}

	/// Query block data.
	pub fn block_data(&self, relay_parent: Hash, block_data_hash: Hash) -> Option<BlockData> {
		self.query_inner(columns::DATA, &block_data_key(&relay_parent, &block_data_hash))
	}

	/// Query block data by corresponding candidate receipt's hash.
	pub fn block_data_by_candidate(&self, relay_parent: Hash, candidate_hash: Hash) -> Option<BlockData> {
		let receipt_key = candidate_key(&candidate_hash);

		self.query_inner(columns::DATA, &receipt_key[..]).and_then(|receipt: CandidateReceipt| {
			self.block_data(relay_parent, receipt.block_data_hash)
		})
	}

	/// Query message queue data by message queue root hash.
	pub fn queue_by_root(&self, queue_root: &Hash) -> Option<Vec<Message>> {
		self.query_inner(columns::DATA, queue_root.as_ref())
	}

	fn block_hash_to_candidate_hash(&self, block_hash: Hash) -> Option<Hash> {
		self.query_inner(columns::META, &block_to_candidate_key(&block_hash))
	}

	fn query_inner<T: Decode>(&self, column: Option<u32>, key: &[u8]) -> Option<T> {
		match self.inner.get(column, key) {
			Ok(Some(raw)) => {
				let res = T::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed");
				Some(res)
			}
			Ok(None) => None,
			Err(e) => {
				warn!(target: "availability", "Error reading from the availability store: {:?}", e);
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

		let block_data_1 = BlockData(vec![1, 2, 3]);
		let block_data_2 = BlockData(vec![4, 5, 6]);

		let erasure_chunk_1 = ErasureChunk {
			chunk: vec![10, 20, 30],
			index: 1,
			proof: vec![],
		};

		let erasure_chunk_2 = ErasureChunk {
			chunk: vec![40, 50, 60],
			index: 1,
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

		let candidate_1 = CandidateReceipt {
			parachain_index: para_id_1,
			collator: Default::default(),
			signature: Default::default(),
			head_data: Default::default(),
			egress_queue_roots: Vec::new(),
			fees: 0,
			block_data_hash: block_data_1.hash(),
			upward_messages: Vec::new(),
			erasure_root: [6; 32].into(),
		};

		let candidate_2 = CandidateReceipt {
			parachain_index: para_id_2,
			collator: Default::default(),
			signature: Default::default(),
			head_data: Default::default(),
			egress_queue_roots: Vec::new(),
			fees: 0,
			block_data_hash: block_data_2.hash(),
			upward_messages: Vec::new(),
			erasure_root: [6; 32].into(),
		};

		assert!(store.add_erasure_chunk(3, &relay_parent, &candidate_1, erasure_chunk_1.clone()).is_ok());
		assert!(store.add_erasure_chunk(3, &relay_parent, &candidate_2, erasure_chunk_2.clone()).is_ok());
		assert_eq!(store.block_data(relay_parent, block_data_1.hash()).unwrap(), block_data_1);
		assert_eq!(store.block_data(relay_parent, block_data_2.hash()).unwrap(), block_data_2);

		assert_eq!(store.get_erasure_chunk(&relay_parent, block_data_1.hash(), 1).as_ref(), Some(&erasure_chunk_1));
		assert_eq!(store.get_erasure_chunk(&relay_parent, block_data_2.hash(), 1), Some(erasure_chunk_2));

		assert!(store.block_data_by_candidate(relay_parent, candidate_1.hash()).is_none());
		assert!(store.block_data_by_candidate(relay_parent, candidate_2.hash()).is_none());

		store.add_candidate(&candidate_1).unwrap();
		store.add_candidate(&candidate_2).unwrap();

		assert_eq!(store.get_candidate(&candidate_1.hash()), Some(candidate_1.clone()));
		assert_eq!(store.get_candidate(&candidate_2.hash()), Some(candidate_2.clone()));


		assert_eq!(store.block_data_by_candidate(relay_parent, candidate_1.hash()).unwrap(), block_data_1);
		assert_eq!(store.block_data_by_candidate(relay_parent, candidate_2.hash()).unwrap(), block_data_2);

		store.candidates_finalized(relay_parent, [candidate_1.hash()].iter().cloned().collect()).unwrap();

		assert_eq!(store.get_erasure_chunk(&relay_parent, block_data_1.hash(), 1).as_ref(), Some(&erasure_chunk_1));
		assert!(store.get_erasure_chunk(&relay_parent, block_data_2.hash(), 1).is_none());

		assert_eq!(store.get_candidate(&candidate_1.hash()), Some(candidate_1));
		assert_eq!(store.get_candidate(&candidate_2.hash()), None);

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

		let candidate = CandidateReceipt {
			parachain_index: para_id,
			collator: Default::default(),
			signature: Default::default(),
			head_data: Default::default(),
			egress_queue_roots: Vec::new(),
			fees: 0,
			block_data_hash: block_data.hash(),
			upward_messages: Vec::new(),
			erasure_root: [6; 32].into(),
		};

		let chunks: Vec<_> = erasure_chunks
			.iter()
			.zip(branches.map(|(proof, _)| proof))
			.enumerate()
			.map(|(index, (chunk, proof))| ErasureChunk {
				chunk: chunk.clone(),
				proof,
				index: index as u32,
			})
			.collect();

		let store = Store::new_in_memory();

		store.add_erasure_chunk(n_validators as u32, &relay_parent, &candidate, chunks[0].clone()).unwrap();
		assert_eq!(store.get_erasure_chunk(&relay_parent, block_data_hash, 0), Some(chunks[0].clone()));

		assert!(store.block_data(relay_parent, block_data_hash).is_none());

		store.add_erasure_chunks(n_validators, &relay_parent, &candidate, chunks).unwrap();
		assert_eq!(store.block_data(relay_parent, block_data_hash), Some(block_data));
	}

	#[test]
	fn add_validator_index_works() {
		let relay_parent = [42; 32].into();
		let store = Store::new_in_memory();

		store.add_validator_index_and_n_validators(&relay_parent, 42, 24).unwrap();
		assert_eq!(store.get_validator_index_and_n_validators(&relay_parent).unwrap(), (42, 24));
	}

	#[test]
	fn add_candidates_in_relay_block_works() {
		let relay_parent = [42; 32].into();
		let store = Store::new_in_memory();

		let candidates = vec![[1; 32].into(), [2; 32].into(), [3; 32].into()];

		store.add_candidates_in_relay_block(&relay_parent, candidates.clone()).unwrap();
		assert_eq!(store.get_candidates_in_relay_block(&relay_parent).unwrap(), candidates);
	}

	#[test]
	fn add_erasure_roots_in_relay_block_works() {
		let relay_parent = [42; 32].into();
		let store = Store::new_in_memory();

		let erasure_roots = vec![[1; 32].into(), [2; 32].into(), [3; 32].into()];

		store.add_erasure_roots_in_relay_block(&relay_parent, erasure_roots.clone()).unwrap();
		assert_eq!(store.get_erasure_roots_in_relay_block(&relay_parent).unwrap(), erasure_roots);
	}

	#[test]
	fn awaited_chunks_works() {
		use std::iter::FromIterator;
		let validator_index = 3;
		let n_validators = 10;
		let relay_parent = [42; 32].into();
		let erasure_root = [11; 32].into();
		let receipt = CandidateReceipt {
			parachain_index: 1.into(),
			collator: Default::default(),
			signature: Default::default(),
			head_data: Default::default(),
			egress_queue_roots: vec![],
			fees: 0,
			block_data_hash: Hash::default(),
			upward_messages: vec![],
			erasure_root,
		};
		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: Vec::new(),
		};
		let candidates = vec![receipt.hash(), [2; 32].into()];

		let store = Store::new_in_memory();

		store.add_validator_index_and_n_validators(
			&relay_parent,
			validator_index,
			n_validators
		).unwrap();

		// We are waiting for chunks from two candidates.
		store.add_candidates_in_relay_block(&relay_parent, candidates.clone()).unwrap();

		let awaited_frontier = store.awaited_chunks().unwrap();
		let expected: HashSet<_> = candidates
			.clone()
			.into_iter()
			.map(|c| (relay_parent, c, validator_index))
			.collect();
		assert_eq!(awaited_frontier, expected);

		// We add chunk from one of the candidates.
		store.add_erasure_chunk(n_validators, &relay_parent, &receipt, chunk).unwrap();

		let awaited_frontier = store.awaited_chunks().unwrap();
		// Now we wait for the other chunk that we haven't received yet.
		let expected: HashSet<_> = vec![(relay_parent, candidates[1], validator_index)].into_iter().collect();
		assert_eq!(awaited_frontier, expected);

		// Finalizing removes awaited candidates from frontier.
		store.candidates_finalized(relay_parent, HashSet::from_iter(candidates.into_iter())).unwrap();

		assert_eq!(store.awaited_chunks().unwrap().len(), 0);
	}
}
