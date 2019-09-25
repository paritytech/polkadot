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

		let chunks_ref: Vec<_> = chunks.chunks.iter()
			.map(|ErasureChunk { chunk, index, .. }| (&chunk[..], (*index as usize)))
			.collect();

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

	#[test]
	fn finalization_removes_unneeded() {
		let relay_parent = [1; 32].into();

		let para_id_1 = 5.into();
		let para_id_2 = 6.into();

		let candidate_1 = [2; 32].into();
		let candidate_2 = [3; 32].into();

		let erasure_chunk_1 = ErasureChunk {
			relay_parent,
			candidate_hash: candidate_1,
			chunk: vec![1; 8],
			index: 1,
			proof: vec![]
		};

		let erasure_chunk_2 = ErasureChunk {
			relay_parent,
			candidate_hash: candidate_2,
			chunk: vec![2; 8],
			index: 2,
			proof: vec![]
		};

		let block_1_chunks = ErasureChunks {
			n_validators: 10,
			root: Hash::default(),
			chunks: vec![
				erasure_chunk_1.clone()
			],
		};

		let block_2_chunks = ErasureChunks {
			n_validators: 10,
			root: Hash::default(),
			chunks: vec![
				erasure_chunk_2.clone()
			],
		};

		let store = Store::new_in_memory();
		store.make_available(Data {
			relay_parent,
			parachain_id: para_id_1,
			candidate_hash: candidate_1,
			erasure_chunks: block_1_chunks,
		}).unwrap();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id_2,
			candidate_hash: candidate_2,
			erasure_chunks: block_2_chunks,
		}).unwrap();

		assert_eq!(store.erasure_chunk(relay_parent, candidate_1, 1).unwrap(), erasure_chunk_1);
		assert_eq!(store.erasure_chunk(relay_parent, candidate_2, 2).unwrap(), erasure_chunk_2);

		store.candidates_finalized(relay_parent, [candidate_1].iter().cloned().collect()).unwrap();

		assert_eq!(store.erasure_chunk(relay_parent, candidate_1, 1).unwrap(), erasure_chunk_1);
		assert!(store.erasure_chunk(relay_parent, candidate_2, 2).is_none());
	}

	#[test]
	fn block_reconstructed_from_erasure() {
		let relay_parent = [1; 32].into();
		let num_validators = 10;
		let candidate_hash = [2; 32].into();

		let para_id = 10.into();

		let block_data = BlockData(vec![42; 128]);
		let message = Message(vec![1u8; 32]);
		let messages = Some(AvailableMessages(vec![(
				[5; 32].into(),
				vec![message.clone()]
			)]
		));

		let chunks = erasure::obtain_chunks(num_validators, &block_data, &messages).unwrap();
		let chunks_refs: Vec<_> = chunks.iter().map(|c| &c[..]).collect();

		let branches = erasure::branches(chunks_refs.clone());
		let root = branches.root();

		let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();

		let store = Store::new_in_memory();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id,
			candidate_hash,
			erasure_chunks: ErasureChunks {
				n_validators: num_validators as u64,
				root,
				chunks: vec![
					ErasureChunk {
						relay_parent,
						candidate_hash,
						chunk: chunks[0].clone(),
						index: 0,
						proof: proofs[0].clone()
					},
					ErasureChunk {
						relay_parent,
						candidate_hash,
						chunk: chunks[3].clone(),
						index: 3,
						proof: proofs[3].clone()
					},
					ErasureChunk {
						relay_parent,
						candidate_hash,
						chunk: chunks[5].clone(),
						index: 5,
						proof: proofs[5].clone()
					},
				],
			},
		}).unwrap();

		assert_eq!(store.block_data(relay_parent, candidate_hash), None);
		assert_eq!(store.queue_by_root(&[5u8; 32].into()), None);

		store.add_erasure_chunk(ErasureChunk {
			relay_parent,
			candidate_hash,
			chunk: chunks[9].clone(),
			index: 9,
			proof: proofs[9].clone(),
		}).unwrap();

		let block_data_res = store.block_data(relay_parent, candidate_hash).unwrap();
		let messages_res = store.queue_by_root(&[5u8; 32].into()).unwrap();

		assert_eq!(block_data_res, block_data);
		assert_eq!(messages_res, vec![message]);
	}

	#[test]
	fn broken_chunk_is_ignored() {
		let relay_parent = [1; 32].into();
		let num_validators = 10;
		let candidate_hash = [2; 32].into();

		let para_id = 10.into();

		let block_data = BlockData(vec![42; 128]);
		let message = Message(vec![1u8; 32]);

		let messages = Some(AvailableMessages(vec![(
				[5; 32].into(),
				vec![message]
			)]
		));

		let chunks = erasure::obtain_chunks(num_validators, &block_data, &messages).unwrap();
		let chunks_refs: Vec<_> = chunks.iter().map(|c| &c[..]).collect();

		let branches = erasure::branches(chunks_refs.clone());
		let root = branches.root();

		let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();

		let store = Store::new_in_memory();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id,
			candidate_hash,
			erasure_chunks: ErasureChunks {
				n_validators: num_validators as u64,
				root,
				chunks: chunks.iter().zip(proofs.iter())
							.enumerate()
							.take(3)
							.map(|(index, (chunk, proof))|
								ErasureChunk {
									relay_parent,
									candidate_hash,
									chunk: chunk.clone(),
									index: index as u32,
									proof: proof.clone()
								}).collect(),
			},
		}).unwrap();

		assert_eq!(store.block_data(relay_parent, candidate_hash), None);
		assert_eq!(store.queue_by_root(&[5u8; 32].into()), None);

		// Test that a broken chunk is rejected
		let mut broken = chunks[9].clone();
		broken.push(1);

		assert!(store.add_erasure_chunk(ErasureChunk {
											relay_parent,
											candidate_hash,
											chunk: broken,
											index: 9,
											proof: proofs[9].clone()
										}).is_err());

		// Test that a chunk with the wrong proof is rejected
		assert!(store.add_erasure_chunk(ErasureChunk {
											relay_parent,
											candidate_hash,
											chunk: chunks[9].clone(),
											index: 9,
											proof: proofs[8].clone()
										}).is_err());

		//assert_eq!(store.block_data(relay_parent, candidate_1).unwrap(), block_data_1);
		//assert_eq!(store.block_data(relay_parent, candidate_2).unwrap(), block_data_2);

		//store.candidates_finalized(relay_parent, [candidate_1].iter().cloned().collect()).unwrap();

		//assert_eq!(store.block_data(relay_parent, candidate_1).unwrap(), block_data_1);
		//assert!(store.block_data(relay_parent, candidate_2).is_none());
	}

	#[test]
	fn queues_available_by_queue_root() {
		let relay_parent = [1; 32].into();
		let para_id = 5.into();
		let candidate_hash = [2; 32].into();
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

		let chunks = erasure::obtain_chunks(n_validators, &block_data, &messages).unwrap();
		let chunks_refs: Vec<_> = chunks.iter().map(|c| &c[..]).collect();

		let branches = erasure::branches(chunks_refs.clone());
		let root = branches.root();
		let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();

		let store = Store::new_in_memory();

		store.make_available(Data {
			relay_parent,
			parachain_id: para_id,
			candidate_hash, 
			erasure_chunks: ErasureChunks {
				n_validators: n_validators as u64,
				root,
				chunks: chunks.iter().zip(proofs)
					.enumerate()
					.take(4)
					.map(|(index, (chunk, proof))|
					ErasureChunk {
						relay_parent,
						candidate_hash,
						chunk: chunk.clone(),
						index: index as u32,
						proof: proof.clone(),
					}).collect(),
			}
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
