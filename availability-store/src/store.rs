// Copyright 2018-2020 Parity Technologies (UK) Ltd.
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

#[cfg(not(target_os = "unknown"))]
use kvdb_rocksdb::{Database, DatabaseConfig};
use kvdb::{KeyValueDB, DBTransaction};
use codec::{Encode, Decode};
use polkadot_erasure_coding as erasure;
use polkadot_primitives::{
	Hash,
	parachain::{
		ErasureChunk, AvailableData, AbridgedCandidateReceipt,
	},
};
use parking_lot::Mutex;

use log::{trace, warn};
use std::collections::HashSet;
use std::sync::Arc;
use std::iter::FromIterator;
use std::io;

use crate::{LOG_TARGET, Config, ExecutionData};

mod columns {
	pub const DATA: u32 = 0;
	pub const META: u32 = 1;
	pub const NUM_COLUMNS: u32 = 2;
}

#[derive(Clone)]
pub struct Store {
	inner: Arc<dyn KeyValueDB>,
	candidate_descendents_lock: Arc<Mutex<()>>
}

// data keys
fn execution_data_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 0i8).encode()
}

fn erasure_chunks_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 1i8).encode()
}

fn candidate_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 2i8).encode()
}

fn candidates_with_relay_parent_key(relay_block: &Hash) -> Vec<u8> {
	(relay_block, 4i8).encode()
}

// meta keys
const AWAITED_CHUNKS_KEY: [u8; 14] = *b"awaited_chunks";

fn validator_index_and_n_validators_key(relay_parent: &Hash) -> Vec<u8> {
	(relay_parent, 1i8).encode()
}

fn available_chunks_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 2i8).encode()
}

/// An entry in the awaited frontier of chunks we are interested in.
#[derive(Encode, Decode, Debug, Hash, PartialEq, Eq, Clone)]
pub struct AwaitedFrontierEntry {
	/// The hash of the candidate for which we want to fetch a chunk for.
	/// There will be duplicate entries in the case of multiple candidates with
	/// the same erasure-root, but this is unlikely.
	pub candidate_hash: Hash,
	/// Although the relay-parent is implicitly referenced by the candidate hash,
	/// we include it here as well for convenience in pruning the set.
	pub relay_parent: Hash,
	/// The index of the validator we represent.
	pub validator_index: u32,
}

impl Store {
	/// Create a new `Store` with given condig on disk.
	#[cfg(not(target_os = "unknown"))]
	pub(super) fn new(config: Config) -> io::Result<Self> {
		let mut db_config = DatabaseConfig::with_columns(columns::NUM_COLUMNS);

		if let Some(cache_size) = config.cache_size {
			let mut memory_budget = std::collections::HashMap::new();
			for i in 0..columns::NUM_COLUMNS {
				memory_budget.insert(i, cache_size / columns::NUM_COLUMNS as usize);
			}

			db_config.memory_budget = memory_budget;
		}

		let path = config.path.to_str().ok_or_else(|| io::Error::new(
			io::ErrorKind::Other,
			format!("Bad database path: {:?}", config.path),
		))?;

		let db = Database::open(&db_config, &path)?;

		Ok(Store {
			inner: Arc::new(db),
			candidate_descendents_lock: Arc::new(Mutex::new(())),
		})
	}

	/// Create a new `Store` in-memory. Useful for tests.
	pub(super) fn new_in_memory() -> Self {
		Store {
			inner: Arc::new(::kvdb_memorydb::create(columns::NUM_COLUMNS)),
			candidate_descendents_lock: Arc::new(Mutex::new(())),
		}
	}

	/// Make some data available provisionally.
	pub(crate) fn make_available(&self, candidate_hash: Hash, available_data: AvailableData)
		-> io::Result<()>
	{
		let mut tx = DBTransaction::new();

		// at the moment, these structs are identical. later, we will also
		// keep outgoing message queues available, and these are not needed
		// for execution.
		let AvailableData { pov_block, omitted_validation } = available_data;
		let execution_data = ExecutionData {
			pov_block,
			omitted_validation,
		};

		tx.put_vec(
			columns::DATA,
			execution_data_key(&candidate_hash).as_slice(),
			execution_data.encode(),
		);

		self.inner.write(tx)
	}

	/// Get a set of all chunks we are waiting for.
	pub fn awaited_chunks(&self) -> Option<HashSet<AwaitedFrontierEntry>> {
		self.query_inner(columns::META, &AWAITED_CHUNKS_KEY).map(|vec: Vec<AwaitedFrontierEntry>| {
			HashSet::from_iter(vec.into_iter())
		})
	}

	/// Adds a set of candidates hashes that were included in a relay block by the block's parent.
	///
	/// If we already possess the receipts for these candidates _and_ our position at the specified
	/// relay chain the awaited frontier of the erasure chunks will also be extended.
	///
	/// This method modifies the erasure chunks awaited frontier by adding this validator's
	/// chunks from `candidates` to it. In order to do so the information about this validator's
	/// position at parent `relay_parent` should be known to the store prior to calling this
	/// method, in other words `note_validator_index_and_n_validators` should be called for
	/// the given `relay_parent` before calling this function.
	pub(crate) fn note_candidates_with_relay_parent(
		&self,
		relay_parent: &Hash,
		candidates: &[Hash],
	) -> io::Result<()> {
		let mut tx = DBTransaction::new();
		let dbkey = candidates_with_relay_parent_key(relay_parent);

		// This call can race against another call to `note_candidates_with_relay_parent`
		// with a different set of descendents.
		let _lock = self.candidate_descendents_lock.lock();

		if let Some((validator_index, _)) = self.get_validator_index_and_n_validators(relay_parent) {
			let candidates = candidates.clone();
			let awaited_frontier: Vec<AwaitedFrontierEntry> = self
				.query_inner(columns::META, &AWAITED_CHUNKS_KEY)
				.unwrap_or_else(|| Vec::new());

			let mut awaited_frontier: HashSet<AwaitedFrontierEntry> =
				HashSet::from_iter(awaited_frontier.into_iter());

			awaited_frontier.extend(candidates.iter().cloned().map(|candidate_hash| {
				AwaitedFrontierEntry {
					relay_parent: relay_parent.clone(),
					candidate_hash,
					validator_index,
				}
			}));
			let awaited_frontier = Vec::from_iter(awaited_frontier.into_iter());
			tx.put_vec(columns::META, &AWAITED_CHUNKS_KEY, awaited_frontier.encode());
		}

		let mut descendent_candidates = self.get_candidates_with_relay_parent(relay_parent);
		descendent_candidates.extend(candidates.iter().cloned());
		tx.put_vec(columns::DATA, &dbkey, descendent_candidates.encode());

		self.inner.write(tx)
	}

	/// Make a validator's index and a number of validators at a relay parent available.
	pub(crate) fn note_validator_index_and_n_validators(
		&self,
		relay_parent: &Hash,
		validator_index: u32,
		n_validators: u32,
	) -> io::Result<()> {
		let mut tx = DBTransaction::new();
		let dbkey = validator_index_and_n_validators_key(relay_parent);

		tx.put_vec(columns::META, &dbkey, (validator_index, n_validators).encode());

		self.inner.write(tx)
	}

	/// Query a validator's index and n_validators by relay parent.
	pub(crate) fn get_validator_index_and_n_validators(&self, relay_parent: &Hash) -> Option<(u32, u32)> {
		let dbkey = validator_index_and_n_validators_key(relay_parent);

		self.query_inner(columns::META, &dbkey)
	}

	/// Add a set of chunks.
	///
	/// The same as `add_erasure_chunk` but adds a set of chunks in one atomic transaction.
	pub fn add_erasure_chunks<I>(
		&self,
		n_validators: u32,
		candidate_hash: &Hash,
		chunks: I,
	) -> io::Result<()>
		where I: IntoIterator<Item = ErasureChunk>
	{
		if let Some(receipt) = self.get_candidate(candidate_hash) {
			let mut tx = DBTransaction::new();
			let dbkey = erasure_chunks_key(candidate_hash);

			let mut v = self.query_inner(columns::DATA, &dbkey).unwrap_or(Vec::new());

			let av_chunks_key = available_chunks_key(candidate_hash);
			let mut have_chunks = self.query_inner(columns::META, &av_chunks_key).unwrap_or(Vec::new());

			let awaited_frontier: Option<Vec<AwaitedFrontierEntry>> = self.query_inner(
				columns::META,
				&AWAITED_CHUNKS_KEY,
			);

			for chunk in chunks.into_iter() {
				if !have_chunks.contains(&chunk.index) {
					have_chunks.push(chunk.index);
				}
				v.push(chunk);
			}

			if let Some(mut awaited_frontier) = awaited_frontier {
				awaited_frontier.retain(|entry| {
					!(
						entry.relay_parent == receipt.relay_parent &&
						&entry.candidate_hash == candidate_hash &&
						have_chunks.contains(&entry.validator_index)
					)
				});
				tx.put_vec(columns::META, &AWAITED_CHUNKS_KEY, awaited_frontier.encode());
			}

			// If there are no block data in the store at this point,
			// check that they can be reconstructed now and add them to store if they can.
			if self.execution_data(&candidate_hash).is_none() {
				if let Ok(available_data) = erasure::reconstruct(
					n_validators as usize,
					v.iter().map(|chunk| (chunk.chunk.as_ref(), chunk.index as usize)),
				)
				{
					self.make_available(*candidate_hash, available_data)?;
				}
			}

			tx.put_vec(columns::DATA, &dbkey, v.encode());
			tx.put_vec(columns::META, &av_chunks_key, have_chunks.encode());

			self.inner.write(tx)
		} else {
			trace!(target: LOG_TARGET, "Candidate with hash {} not found", candidate_hash);
			Ok(())
		}
	}

	/// Queries an erasure chunk by its block's relay-parent, the candidate hash, and index.
	pub fn get_erasure_chunk(
		&self,
		candidate_hash: &Hash,
		index: usize,
	) -> Option<ErasureChunk> {
		self.query_inner(columns::DATA, &erasure_chunks_key(candidate_hash))
			.and_then(|chunks: Vec<ErasureChunk>| {
				chunks.iter()
				.find(|chunk: &&ErasureChunk| chunk.index == index as u32)
				.map(|chunk| chunk.clone())
			})
	}

	/// Stores a candidate receipt.
	pub fn add_candidate(
		&self,
		receipt: &AbridgedCandidateReceipt,
	) -> io::Result<()> {
		let candidate_hash = receipt.hash();
		let dbkey = candidate_key(&candidate_hash);
		let mut tx = DBTransaction::new();

		tx.put_vec(columns::DATA, &dbkey, receipt.encode());

		self.inner.write(tx)
	}

	/// Queries a candidate receipt by the relay parent hash and its hash.
	pub(crate) fn get_candidate(&self, candidate_hash: &Hash)
		-> Option<AbridgedCandidateReceipt>
	{
		self.query_inner(columns::DATA, &candidate_key(candidate_hash))
	}

	/// Note that a set of candidates have been included in a finalized block with given hash and parent hash.
	pub(crate) fn candidates_finalized(
		&self,
		relay_parent: Hash,
		finalized_candidates: HashSet<Hash>,
	) -> io::Result<()> {
		let mut tx = DBTransaction::new();

		let awaited_frontier: Option<Vec<AwaitedFrontierEntry>> = self
			.query_inner(columns::META, &AWAITED_CHUNKS_KEY);

		if let Some(mut awaited_frontier) = awaited_frontier {
			awaited_frontier.retain(|entry| entry.relay_parent != relay_parent);
			tx.put_vec(columns::META, &AWAITED_CHUNKS_KEY, awaited_frontier.encode());
		}

		let candidates = self.get_candidates_with_relay_parent(&relay_parent);

		for candidate in candidates.into_iter().filter(|c| !finalized_candidates.contains(c)) {
			// we only delete this data for candidates which were not finalized.
			// we keep all data for the finalized chain forever at the moment.
			tx.delete(columns::DATA, execution_data_key(&candidate).as_slice());
			tx.delete(columns::DATA, &erasure_chunks_key(&candidate));
			tx.delete(columns::DATA, &candidate_key(&candidate));

			tx.delete(columns::META, &available_chunks_key(&candidate));
		}

		self.inner.write(tx)
	}

	/// Query execution data by relay parent and candidate hash.
	pub(crate) fn execution_data(&self, candidate_hash: &Hash) -> Option<ExecutionData> {
		self.query_inner(columns::DATA, &execution_data_key(candidate_hash))
	}

	/// Get candidates which pinned to the environment of the given relay parent.
	/// Note that this is not necessarily the same as candidates that were included in a direct
	/// descendent of the given relay-parent.
	fn get_candidates_with_relay_parent(&self, relay_parent: &Hash) -> Vec<Hash> {
		let key = candidates_with_relay_parent_key(relay_parent);
		self.query_inner(columns::DATA, &key[..]).unwrap_or_default()
	}

	fn query_inner<T: Decode>(&self, column: u32, key: &[u8]) -> Option<T> {
		match self.inner.get(column, key) {
			Ok(Some(raw)) => {
				let res = T::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed");
				Some(res)
			}
			Ok(None) => None,
			Err(e) => {
				warn!(target: LOG_TARGET, "Error reading from the availability store: {:?}", e);
				None
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_erasure_coding::{self as erasure};
	use polkadot_primitives::parachain::{
		Id as ParaId, BlockData, AvailableData, PoVBlock, OmittedValidationData,
	};

	fn available_data(block_data: &[u8]) -> AvailableData {
		AvailableData {
			pov_block: PoVBlock {
				block_data: BlockData(block_data.to_vec()),
			},
			omitted_validation: OmittedValidationData {
				global_validation: Default::default(),
				local_validation: Default::default(),
			}
		}
	}

	fn execution_data(available: &AvailableData) -> ExecutionData {
		let AvailableData { pov_block, omitted_validation } = available.clone();
		ExecutionData { pov_block, omitted_validation }
	}

	#[test]
	fn finalization_removes_unneeded() {
		let relay_parent = [1; 32].into();

		let para_id_1 = 5.into();
		let para_id_2 = 6.into();

		let mut candidate_1 = AbridgedCandidateReceipt::default();
		let mut candidate_2 = AbridgedCandidateReceipt::default();

		candidate_1.parachain_index = para_id_1;
		candidate_1.commitments.erasure_root = [6; 32].into();
		candidate_1.relay_parent = relay_parent;

		candidate_2.parachain_index = para_id_2;
		candidate_2.commitments.erasure_root = [6; 32].into();
		candidate_2.relay_parent = relay_parent;


		let candidate_1_hash = candidate_1.hash();
		let candidate_2_hash = candidate_2.hash();

		let available_data_1 = available_data(&[1, 2, 3]);
		let available_data_2 = available_data(&[4, 5, 6]);

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
		store.make_available(candidate_1_hash, available_data_1.clone()).unwrap();

		store.make_available(candidate_2_hash, available_data_2.clone()).unwrap();

		store.add_candidate(&candidate_1).unwrap();
		store.add_candidate(&candidate_2).unwrap();

		store.note_candidates_with_relay_parent(&relay_parent, &[candidate_1_hash, candidate_2_hash]).unwrap();

		assert!(store.add_erasure_chunks(3, &candidate_1_hash, vec![erasure_chunk_1.clone()]).is_ok());
		assert!(store.add_erasure_chunks(3, &candidate_2_hash, vec![erasure_chunk_2.clone()]).is_ok());

		assert_eq!(store.execution_data(&candidate_1_hash).unwrap(), execution_data(&available_data_1));
		assert_eq!(store.execution_data(&candidate_2_hash).unwrap(), execution_data(&available_data_2));

		assert_eq!(store.get_erasure_chunk(&candidate_1_hash, 1).as_ref(), Some(&erasure_chunk_1));
		assert_eq!(store.get_erasure_chunk(&candidate_2_hash, 1), Some(erasure_chunk_2));

		assert_eq!(store.get_candidate(&candidate_1_hash), Some(candidate_1.clone()));
		assert_eq!(store.get_candidate(&candidate_2_hash), Some(candidate_2.clone()));

		store.candidates_finalized(relay_parent, [candidate_1_hash].iter().cloned().collect()).unwrap();

		assert_eq!(store.get_erasure_chunk(&candidate_1_hash, 1).as_ref(), Some(&erasure_chunk_1));
		assert!(store.get_erasure_chunk(&candidate_2_hash, 1).is_none());

		assert_eq!(store.get_candidate(&candidate_1_hash), Some(candidate_1));
		assert_eq!(store.get_candidate(&candidate_2_hash), None);

		assert_eq!(store.execution_data(&candidate_1_hash).unwrap(), execution_data(&available_data_1));
		assert!(store.execution_data(&candidate_2_hash).is_none());
	}

	#[test]
	fn erasure_coding() {
		let relay_parent: Hash = [1; 32].into();
		let para_id: ParaId = 5.into();
		let available_data = available_data(&[42; 8]);
		let n_validators = 5;

		let erasure_chunks = erasure::obtain_chunks(
			n_validators,
			&available_data,
		).unwrap();

		let branches = erasure::branches(erasure_chunks.as_ref());

		let mut candidate = AbridgedCandidateReceipt::default();
		candidate.parachain_index = para_id;
		candidate.commitments.erasure_root = [6; 32].into();
		candidate.relay_parent = relay_parent;

		let candidate_hash = candidate.hash();

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

		store.add_candidate(&candidate).unwrap();
		store.add_erasure_chunks(n_validators as u32, &candidate_hash, vec![chunks[0].clone()]).unwrap();
		assert_eq!(store.get_erasure_chunk(&candidate_hash, 0), Some(chunks[0].clone()));

		assert!(store.execution_data(&candidate_hash).is_none());

		store.add_erasure_chunks(n_validators as u32, &candidate_hash, chunks).unwrap();
		assert_eq!(store.execution_data(&candidate_hash), Some(execution_data(&available_data)));
	}

	#[test]
	fn add_validator_index_works() {
		let relay_parent = [42; 32].into();
		let store = Store::new_in_memory();

		store.note_validator_index_and_n_validators(&relay_parent, 42, 24).unwrap();
		assert_eq!(store.get_validator_index_and_n_validators(&relay_parent).unwrap(), (42, 24));
	}

	#[test]
	fn add_candidates_in_relay_block_works() {
		let relay_parent = [42; 32].into();
		let store = Store::new_in_memory();

		let candidates = vec![[1; 32].into(), [2; 32].into(), [3; 32].into()];

		store.note_candidates_with_relay_parent(&relay_parent, &candidates).unwrap();
		assert_eq!(store.get_candidates_with_relay_parent(&relay_parent), candidates);
	}

	#[test]
	fn awaited_chunks_works() {
		use std::iter::FromIterator;
		let validator_index = 3;
		let n_validators = 10;
		let relay_parent = [42; 32].into();
		let erasure_root_1 = [11; 32].into();
		let erasure_root_2 = [12; 32].into();
		let mut receipt_1 = AbridgedCandidateReceipt::default();
		let mut receipt_2 = AbridgedCandidateReceipt::default();


		receipt_1.parachain_index = 1.into();
		receipt_1.commitments.erasure_root = erasure_root_1;
		receipt_1.relay_parent = relay_parent;

		receipt_2.parachain_index = 2.into();
		receipt_2.commitments.erasure_root = erasure_root_2;
		receipt_2.relay_parent = relay_parent;

		let receipt_1_hash = receipt_1.hash();
		let receipt_2_hash = receipt_2.hash();

		let chunk = ErasureChunk {
			chunk: vec![1, 2, 3],
			index: validator_index,
			proof: Vec::new(),
		};
		let candidates = vec![receipt_1_hash, receipt_2_hash];

		let store = Store::new_in_memory();

		store.note_validator_index_and_n_validators(
			&relay_parent,
			validator_index,
			n_validators
		).unwrap();
		store.add_candidate(&receipt_1).unwrap();
		store.add_candidate(&receipt_2).unwrap();

		// We are waiting for chunks from two candidates.
		store.note_candidates_with_relay_parent(&relay_parent, &candidates).unwrap();

		let awaited_frontier = store.awaited_chunks().unwrap();
		warn!(target: "availability", "awaited {:?}", awaited_frontier);
		let expected: HashSet<_> = candidates
			.clone()
			.into_iter()
			.map(|c| AwaitedFrontierEntry {
				relay_parent,
				candidate_hash: c,
				validator_index,
			})
			.collect();
		assert_eq!(awaited_frontier, expected);

		// We add chunk from one of the candidates.
		store.add_erasure_chunks(n_validators, &receipt_1_hash, vec![chunk]).unwrap();

		let awaited_frontier = store.awaited_chunks().unwrap();
		// Now we wait for the other chunk that we haven't received yet.
		let expected: HashSet<_> = vec![AwaitedFrontierEntry {
			relay_parent,
			candidate_hash: receipt_2_hash,
			validator_index,
		}].into_iter().collect();

		assert_eq!(awaited_frontier, expected);

		// Finalizing removes awaited candidates from frontier.
		store.candidates_finalized(relay_parent, HashSet::from_iter(candidates.into_iter())).unwrap();

		assert_eq!(store.awaited_chunks().unwrap().len(), 0);
	}
}
