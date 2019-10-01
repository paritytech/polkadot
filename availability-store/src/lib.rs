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

//! Persistent database for parachain data: PoV block data and outgoing messages.
//!
//! This will be written into during the block validation pipeline, and queried
//! by networking code in order to circulate required data and maintain availability
//! of it.

use codec::{Encode, Decode};
use kvdb::{KeyValueDB, DBTransaction};
use kvdb_rocksdb::{Database, DatabaseConfig};
use polkadot_primitives::{Hash, BlakeTwo256, HashT};
use polkadot_primitives::parachain::{AvailableMessages, Id as ParaId, BlockData, Message, ErasureChunk, ErasureChunks};
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
	/// Unique candidate receipt hash.
	pub candidate_hash: Hash,
	/// Block data chunks.
	pub erasure_chunks: ErasureChunks,
}

fn block_data_key(relay_parent: &Hash, candidate_hash: &Hash) -> Vec<u8> {
	(relay_parent, candidate_hash, 0i8).encode()
}

fn messages_key(relay_parent: &Hash, candidate_hash: &Hash) -> Vec<u8> {
	(relay_parent, candidate_hash, 1i8).encode()
}

fn erasure_chunks_key(relay_parent: &Hash, candidate_hash: &Hash) -> Vec<u8> {
	(relay_parent, candidate_hash, 2i8).encode()
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

		v.push(data.candidate_hash);
		tx.put_vec(columns::META, &data.relay_parent[..], v.encode());

		tx.put_vec(
			columns::DATA,
			erasure_chunks_key(&data.relay_parent, &data.candidate_hash).as_slice(),
			data.erasure_chunks.encode(),
		);

		self.inner.write(tx)?;

		// At this point we might have enough chunks, so try to reconstruct
		let encoded_key = block_data_key(&data.relay_parent, &data.candidate_hash);
		if let Ok(None) = self.inner.get(columns::DATA, &encoded_key[..]) {
			self.try_recover_block_data_and_messages(&data.relay_parent, &data.candidate_hash);
		}

		Ok(())

	}

	/// Add a single erasure chunk.
	///
	/// Adds another chunk to existing `Data`. `make_available` should be
	/// called for the corresponding data before calling this.
	pub fn add_erasure_chunk(&self, chunk: ErasureChunk) -> io::Result<()> {
		let mut tx = DBTransaction::new();
		let (relay_parent, candidate_hash) = (chunk.relay_parent.clone(), chunk.candidate_hash.clone());

		let encoded_key = erasure_chunks_key(&relay_parent, &candidate_hash);

		let mut v = match self.inner.get(columns::DATA, &encoded_key[..]) {
			Ok(Some(raw)) => ErasureChunks::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed"),
			Ok(None) => return Err(std::io::Error::new(std::io::ErrorKind::Other,
													   "no info on erasure encoding for such parent and candidate")),
			Err(e) => {
				warn!(target: "availbility", "Error reading from availability store: {:?}", e);
				return Err(std::io::Error::new(std::io::ErrorKind::Other,
											   format!("failed to query store {}", e)))
			}
		};

		match erasure::branch_hash(&v.root, &chunk.proof, chunk.index as usize) {
			Err(e) => return Err(io::Error::new(io::ErrorKind::Other, format!("branch_hash error {:?}", e))),
			Ok(hash) => {
				if hash != BlakeTwo256::hash(&chunk.chunk) {
					return Err(io::Error::new(io::ErrorKind::Other, "wrong chunk hash"));
				}
			}
		}

		v.chunks.push(chunk);

		tx.put_vec(
			columns::DATA,
			erasure_chunks_key(&relay_parent, &candidate_hash).as_slice(),
			v.encode()
		);

		self.inner.write(tx)?;

		let encoded_key = block_data_key(&relay_parent, &candidate_hash);
		if let Ok(None) = self.inner.get(columns::DATA, &encoded_key[..]) {
			self.try_recover_block_data_and_messages(&relay_parent, &candidate_hash);
		};

		Ok(())

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
				tx.delete(columns::DATA, messages_key(&parent, &candidate_hash).as_slice());
				tx.delete(columns::DATA, erasure_chunks_key(&parent, &candidate_hash).as_slice());
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

    /// Query an erasure chunk of block by it's parent and it's hash and chunk's id.
	pub fn erasure_chunk(&self, relay_parent: Hash, candidate_hash: Hash, chunk_id: usize) -> Option<ErasureChunk> {
		let encoded_key = erasure_chunks_key(&relay_parent, &candidate_hash);

		let chunks = match self.inner.get(columns::DATA, &encoded_key[..]) {
			Ok(Some(raw)) => ErasureChunks::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed"),
			Ok(None) => return None,
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				return None;
			}
		};

		chunks.chunks.iter().find_map(|c| {
			match c.index as usize == chunk_id {
				true => Some(c.clone()),
				false => None,
			}
		})
	}

	fn try_recover_block_data_and_messages(
		&self,
		relay_parent: &Hash,
		candidate_hash: &Hash
	) -> Option<(BlockData, Option<AvailableMessages>)> {
		let encoded_key = erasure_chunks_key(&relay_parent, &candidate_hash);

		let chunks = match self.inner.get(columns::DATA, &encoded_key[..]) {
			Ok(Some(raw)) => ErasureChunks::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed"),
			Ok(None) => return None,
			Err(e) => {
				warn!(target: "availability", "Error reading from availability store: {:?}", e);
				return None;
			}
		};

		let chunks_ref = chunks.chunks.iter()
			.map(|ErasureChunk { chunk, index, .. }| (&chunk[..], (*index as usize)));

		let res = erasure::reconstruct(chunks.n_validators as usize, chunks_ref);

		match res {
			Ok((b, m)) => {
				if let Err(err) = self.add_block_data_and_messages(
					relay_parent, candidate_hash, b.clone(), m.clone()) {
					warn!(target: "availability", "Error writing to availability store: {:?}", err);
				}
				Some((b, m))
			},
			Err(_) => None,
		}
	}

	fn add_block_data_and_messages(
		&self,
		relay_parent: &Hash,
		candidate_hash: &Hash,
		block_data: BlockData,
		messages: Option<AvailableMessages>,
	) -> io::Result<()> {
		let mut tx = DBTransaction::new();

		let encoded_block_key = block_data_key(&relay_parent, &candidate_hash);

		tx.put_vec(
			columns::DATA,
			encoded_block_key.as_slice(),
			block_data.encode()
		);

		if let Some(messages) = messages {
			// This is kept forever and not pruned.
			for (root, messages) in messages.0 {
				tx.put_vec(
					columns::DATA,
					root.as_ref(),
					messages.encode(),
				);
			}

		}

		self.inner.write(tx)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::parachain::AvailableMessages;

	// Adds two blocks with the same parent into availability-store with enough chunks to reconstruct
	// whole blocks and messages.
	// Checks that data is available.
	// Finalizes one of the blocks.
	// Checks that the other block's erasure chunks and block data are removed from storage, but that the
	// messages are left intact.
	#[test]
	fn finalization_removes_unneeded() {
		let relay_parent: Hash = [1; 32].into();
		let num_validators = 10;

		let para_id_1 = 5.into();
		let para_id_2 = 6.into();

		let candidate_1: Hash = [2; 32].into();
		let candidate_2: Hash = [3; 32].into();

		let block_data_1 = BlockData(vec![42; 128]);
		let block_data_2 = BlockData(vec![24; 128]);

		let message_1 = Message(vec![12u8; 32]);
		let message_2 = Message(vec![13u8; 32]);

		let queue_root_1: Hash = [44; 32].into();

		let messages_1 = Some(AvailableMessages(vec![(
				queue_root_1.clone(),
				vec![message_1.clone()]
			)]
		));

		let queue_root_2: Hash = [55; 32].into();

		let messages_2 = Some(AvailableMessages(vec![(
				queue_root_2.clone(),
				vec![message_2.clone()]
			)]
		));

		let chunks_1 = erasure::obtain_chunks(num_validators, relay_parent.clone(), candidate_1.clone(), &block_data_1, messages_1.as_ref()).unwrap();
		let chunks_2 = erasure::obtain_chunks(num_validators, relay_parent.clone(), candidate_2.clone(), &block_data_2, messages_2.as_ref()).unwrap();

		let store = Store::new_in_memory();


		store.make_available(Data {
			relay_parent,
			parachain_id: para_id_1,
			candidate_hash: candidate_1,
			erasure_chunks: ErasureChunks {
				n_validators: num_validators as u64,
				root: chunks_1.root,
				chunks: chunks_1.chunks.clone().into_iter().take(4).collect(),
			},
		}).unwrap();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id_2,
			candidate_hash: candidate_2,
			erasure_chunks: ErasureChunks {
				n_validators: num_validators as u64,
				root: chunks_2.root,
				chunks: chunks_2.chunks.clone().into_iter().take(4).collect(),
			},
		}).unwrap();

		// Check that data for each candidate is available now.
		assert_eq!(store.erasure_chunk(relay_parent, candidate_1, 1).unwrap(), chunks_1.chunks[1]);
		assert_eq!(store.erasure_chunk(relay_parent, candidate_2, 2).unwrap(), chunks_2.chunks[2]);
		assert_eq!(store.queue_by_root(&queue_root_1).unwrap(), vec![message_1.clone()]);
		assert_eq!(store.queue_by_root(&queue_root_2).unwrap(), vec![message_2.clone()]);

		store.candidates_finalized(relay_parent, [candidate_1].iter().cloned().collect()).unwrap();

		// First candidate is finalized and it's data persists in the store.
		assert_eq!(store.erasure_chunk(relay_parent, candidate_1, 1).unwrap(), chunks_1.chunks[1]);
		// Second candidate is not finalized and it's is purged from the store
		assert!(store.erasure_chunk(relay_parent, candidate_2, 2).is_none());

		// However, messages remain for both candidates.
		assert_eq!(store.queue_by_root(&queue_root_1).unwrap(), vec![message_1.clone()]);
		assert_eq!(store.queue_by_root(&queue_root_2).unwrap(), vec![message_2.clone()]);
	}

	// Checks that block data is correctly reconstructed from erasure chunks.
	// First an encoding for 10 validators is created and 3 chunks are added to the store.
	// 3 chunks are not enough to reconstruct block data and messages, this is checked.
	// Another chunk is added, 4 chunks are now enough for reconstructing block data and
	// messages, this is also checked.
	#[test]
	fn block_reconstructed_from_erasure() {
		let relay_parent: Hash = [1; 32].into();
		let num_validators = 10;
		let candidate_hash: Hash = [2; 32].into();

		let para_id = 10.into();

		let block_data = BlockData(vec![42; 128]);
		let message = Message(vec![1u8; 32]);
		let messages = Some(AvailableMessages(vec![(
				[5; 32].into(),
				vec![message.clone()]
			)]
		));

		let chunks = erasure::obtain_chunks(num_validators,
			relay_parent.clone(),
			candidate_hash.clone(),
			&block_data,
			messages.as_ref()).unwrap();

		let store = Store::new_in_memory();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id,
			candidate_hash,
			erasure_chunks: ErasureChunks {
				n_validators: num_validators as u64,
				root: chunks.root,
				chunks: chunks.chunks.clone().into_iter().take(3).collect(),
			},
		}).unwrap();

		assert_eq!(store.block_data(relay_parent, candidate_hash), None);
		assert_eq!(store.queue_by_root(&[5u8; 32].into()), None);

		store.add_erasure_chunk(chunks.chunks[9].clone()).unwrap();

		let block_data_res = store.block_data(relay_parent, candidate_hash).unwrap();
		let messages_res = store.queue_by_root(&[5u8; 32].into()).unwrap();

		assert_eq!(block_data_res, block_data);
		assert_eq!(messages_res, vec![message]);
	}

	// A broken chunk fails to be added to the store.
	// A chunk with a broken proof fails to be added to the store.
	#[test]
	fn broken_chunk_is_ignored() {
		let relay_parent: Hash = [1; 32].into();
		let num_validators = 10;
		let candidate_hash: Hash = [2; 32].into();

		let para_id = 10.into();

		let block_data = BlockData(vec![42; 128]);
		let message = Message(vec![1u8; 32]);

		let messages = Some(AvailableMessages(vec![(
				[5; 32].into(),
				vec![message]
			)]
		));

		let chunks = erasure::obtain_chunks(num_validators,
			relay_parent.clone(),
			candidate_hash.clone(),
			&block_data,
			messages.as_ref()).unwrap();

		let store = Store::new_in_memory();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id,
			candidate_hash,
			erasure_chunks: ErasureChunks {
				n_validators: num_validators as u64,
				root: chunks.root,
				chunks: chunks.chunks.clone().into_iter().take(3).collect(),
			},
		}).unwrap();

		assert_eq!(store.block_data(relay_parent, candidate_hash), None);
		assert_eq!(store.queue_by_root(&[5u8; 32].into()), None);

		// Test that a broken chunk is rejected
		let mut broken = chunks.chunks[9].chunk.clone();
		broken.push(1);

		assert!(store.add_erasure_chunk(ErasureChunk {
											relay_parent,
											candidate_hash,
											chunk: broken,
											index: 9,
											proof: chunks.chunks[9].proof.clone()
										}).is_err());

		// Test that a chunk with the wrong proof is rejected
		assert!(store.add_erasure_chunk(ErasureChunk {
											relay_parent,
											candidate_hash,
											chunk: chunks.chunks[9].chunk.clone(),
											index: 9,
											proof: chunks.chunks[8].proof.clone()
										}).is_err());
	}

	// Tests that as soon as a store has enough chunks to reconstruct
	// block data and messages, queues are available by their roots.
	#[test]
	fn queues_available_by_queue_root() {
		let relay_parent: Hash = [1; 32].into();
		let para_id = 5.into();
		let candidate_hash: Hash = [2; 32].into();
		let block_data = BlockData(vec![1, 2, 3]);
		let n_validators = 5;

		let message_queue_root_1 = [0x42; 32].into();
		let message_queue_root_2 = [0x43; 32].into();

		let message_a = Message(vec![1, 2, 3, 4]);
		let message_b = Message(vec![4, 5, 6, 7]);

		let messages = Some(AvailableMessages(vec![
			(message_queue_root_1, vec![message_a.clone()]),
			(message_queue_root_2, vec![message_b.clone()]),
		]));

		let erasure_chunks = erasure::obtain_chunks(n_validators,
			relay_parent.clone(),
			candidate_hash.clone(),
			&block_data,
			messages.as_ref()).unwrap();

		let store = Store::new_in_memory();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id,
			candidate_hash,
			erasure_chunks,
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
}
