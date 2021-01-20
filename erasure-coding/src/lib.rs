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

//! As part of Polkadot's availability system, certain pieces of data
//! for each block are required to be kept available.
//!
//! The way we accomplish this is by erasure coding the data into n pieces
//! and constructing a merkle root of the data.
//!
//! Each of n validators stores their piece of data. We assume n=3f+k, 0 < k ≤ 3.
//! f is the maximum number of faulty validators in the system.
//! The data is coded so any f+1 chunks can be used to reconstruct the full data.

use parity_scale_codec::{Encode, Decode};
use reed_solomon::galois_16::{self, ReedSolomon};
use primitives::v0::{self, Hash as H256, BlakeTwo256, HashT};
use primitives::v1;
use sp_core::Blake2Hasher;
use trie::{EMPTY_PREFIX, MemoryDB, Trie, TrieMut, trie_types::{TrieDBMut, TrieDB}};
use thiserror::Error;

use self::wrapped_shard::WrappedShard;

mod wrapped_shard;

// we are limited to the field order of GF(2^16), which is 65536
const MAX_VALIDATORS: usize = <galois_16::Field as reed_solomon::Field>::ORDER;

/// Errors in erasure coding.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum Error {
	/// Returned when there are too many validators.
	#[error("There are too many validators")]
	TooManyValidators,
	/// Cannot encode something for zero or one validator
	#[error("Expected at least 2 validators")]
	NotEnoughValidators,
	/// Cannot reconstruct: wrong number of validators.
	#[error("Validator count mismatches between encoding and decoding")]
	WrongValidatorCount,
	/// Not enough chunks present.
	#[error("Not enough chunks to reconstruct message")]
	NotEnoughChunks,
	/// Too many chunks present.
	#[error("Too many chunks present")]
	TooManyChunks,
	/// Chunks not of uniform length or the chunks are empty.
	#[error("Chunks are not unform, mismatch in length or are zero sized")]
	NonUniformChunks,
	/// An uneven byte-length of a shard is not valid for GF(2^16) encoding.
	#[error("Uneven length is not valid for field GF(2^16)")]
	UnevenLength,
	/// Chunk index out of bounds.
	#[error("Chunk is out of bounds: {chunk_index} not included in 0..{n_validators}")]
	ChunkIndexOutOfBounds{ chunk_index: usize, n_validators: usize },
	/// Bad payload in reconstructed bytes.
	#[error("Reconstructed payload invalid")]
	BadPayload,
	/// Invalid branch proof.
	#[error("Invalid branch proof")]
	InvalidBranchProof,
	/// Branch out of bounds.
	#[error("Branch is out of bounds")]
	BranchOutOfBounds,
}

#[derive(Debug, PartialEq)]
struct CodeParams {
	data_shards: usize,
	parity_shards: usize,
}

impl CodeParams {
	// the shard length needed for a payload with initial size `base_len`.
	fn shard_len(&self, base_len: usize) -> usize {
		// how many bytes we actually need.
		let needed_shard_len = base_len / self.data_shards
			+ (base_len % self.data_shards != 0) as usize;

		// round up to next even number
		// (no actual space overhead since we are working in GF(2^16)).
		needed_shard_len + needed_shard_len % 2
	}

	fn make_shards_for(&self, payload: &[u8]) -> Vec<WrappedShard> {
		let shard_len = self.shard_len(payload.len());
		let mut shards = vec![
			WrappedShard::new(vec![0; shard_len]);
			self.data_shards + self.parity_shards
		];

		for (data_chunk, blank_shard) in payload.chunks(shard_len).zip(&mut shards) {
			// fill the empty shards with the corresponding piece of the payload,
			// zero-padded to fit in the shards.
			let len = std::cmp::min(shard_len, data_chunk.len());
			let blank_shard: &mut [u8] = blank_shard.as_mut();
			blank_shard[..len].copy_from_slice(&data_chunk[..len]);
		}

		shards
	}

	// make a reed-solomon instance.
	fn make_encoder(&self) -> ReedSolomon {
		ReedSolomon::new(self.data_shards, self.parity_shards)
			.expect("this struct is not created with invalid shard number; qed")
	}
}

/// Returns the maximum number of allowed, faulty chunks
/// which does not prevent recovery given all other pieces
/// are correct.
const fn n_faulty(n_validators: usize) -> Result<usize, Error> {
	if n_validators > MAX_VALIDATORS { return Err(Error::TooManyValidators) }
	if n_validators <= 1 { return Err(Error::NotEnoughValidators) }

	Ok(n_validators.saturating_sub(1) / 3)
}

fn code_params(n_validators: usize) -> Result<CodeParams, Error> {
	let n_faulty = n_faulty(n_validators)?;
	let n_good = n_validators - n_faulty;

	Ok(CodeParams {
		data_shards: n_faulty + 1,
		parity_shards: n_good - 1,
	})
}

/// Obtain a threshold of chunks that should be enough to recover the data.
pub fn recovery_threshold(n_validators: usize) -> Result<usize, Error> {
	let n_faulty = n_faulty(n_validators)?;

	Ok(n_faulty + 1)
}

/// Obtain erasure-coded chunks for v0 `AvailableData`, one for each validator.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
pub fn obtain_chunks_v0(n_validators: usize, data: &v0::AvailableData)
	-> Result<Vec<Vec<u8>>, Error>
{
	obtain_chunks(n_validators, data)
}

/// Obtain erasure-coded chunks for v1 `AvailableData`, one for each validator.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
pub fn obtain_chunks_v1(n_validators: usize, data: &v1::AvailableData)
	-> Result<Vec<Vec<u8>>, Error>
{
	obtain_chunks(n_validators, data)
}

/// Obtain erasure-coded chunks, one for each validator.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
fn obtain_chunks<T: Encode>(n_validators: usize, data: &T)
	-> Result<Vec<Vec<u8>>, Error>
{
	let params = code_params(n_validators)?;
	let encoded = data.encode();

	if encoded.is_empty() {
		return Err(Error::BadPayload);
	}

	let mut shards = params.make_shards_for(&encoded[..]);

	params.make_encoder().encode(&mut shards[..])
		.expect("Payload non-empty, shard sizes are uniform, and validator numbers checked; qed");

	Ok(shards.into_iter().map(|w| w.into_inner()).collect())
}

/// Reconstruct the v0 available data from a set of chunks.
///
/// Provide an iterator containing chunk data and the corresponding index.
/// The indices of the present chunks must be indicated. If too few chunks
/// are provided, recovery is not possible.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
pub fn reconstruct_v0<'a, I: 'a>(n_validators: usize, chunks: I)
	-> Result<v0::AvailableData, Error>
	where I: IntoIterator<Item=(&'a [u8], usize)>
{
	reconstruct(n_validators, chunks)
}

/// Reconstruct the v1 available data from a set of chunks.
///
/// Provide an iterator containing chunk data and the corresponding index.
/// The indices of the present chunks must be indicated. If too few chunks
/// are provided, recovery is not possible.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
pub fn reconstruct_v1<'a, I: 'a>(n_validators: usize, chunks: I)
	-> Result<v1::AvailableData, Error>
	where I: IntoIterator<Item=(&'a [u8], usize)>
{
	reconstruct(n_validators, chunks)
}

/// Reconstruct decodable data from a set of chunks.
///
/// Provide an iterator containing chunk data and the corresponding index.
/// The indices of the present chunks must be indicated. If too few chunks
/// are provided, recovery is not possible.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
fn reconstruct<'a, I: 'a, T: Decode>(n_validators: usize, chunks: I) -> Result<T, Error>
	where I: IntoIterator<Item=(&'a [u8], usize)>
{
	let params = code_params(n_validators)?;
	let mut shards: Vec<Option<WrappedShard>> = vec![None; n_validators];
	let mut shard_len = None;
	for (chunk_data, chunk_idx) in chunks.into_iter().take(n_validators) {
		if chunk_idx >= n_validators {
			return Err(Error::ChunkIndexOutOfBounds{ chunk_index: chunk_idx, n_validators });
		}

		let shard_len = shard_len.get_or_insert_with(|| chunk_data.len());

		if *shard_len % 2 != 0 {
			return Err(Error::UnevenLength);
		}

		if *shard_len != chunk_data.len() || *shard_len == 0 {
			return Err(Error::NonUniformChunks);
		}

		shards[chunk_idx] = Some(WrappedShard::new(chunk_data.to_vec()));
	}

	if let Err(e) = params.make_encoder().reconstruct(&mut shards[..]) {
		match e {
			reed_solomon::Error::TooFewShardsPresent => Err(Error::NotEnoughChunks)?,
			reed_solomon::Error::InvalidShardFlags => Err(Error::WrongValidatorCount)?,
			reed_solomon::Error::TooManyShards => Err(Error::TooManyChunks)?,
			reed_solomon::Error::EmptyShard => panic!("chunks are all non-empty; this is checked above; qed"),
			reed_solomon::Error::IncorrectShardSize => panic!("chunks are all same len; this is checked above; qed"),
			_ => panic!("reed_solomon encoder returns no more variants for this function; qed"),
		}
	}

	// lazily decode from the data shards.
	Decode::decode(&mut ShardInput {
		remaining_len: shard_len.map(|s| s * params.data_shards).unwrap_or(0),
		cur_shard: None,
		shards: shards.iter()
			.map(|x| x.as_ref())
			.take(params.data_shards)
			.map(|x| x.expect("all data shards have been recovered; qed"))
			.map(|x| x.as_ref()),
	}).or_else(|_| Err(Error::BadPayload))
}

/// An iterator that yields merkle branches and chunk data for all chunks to
/// be sent to other validators.
pub struct Branches<'a, I> {
	trie_storage: MemoryDB<Blake2Hasher>,
	root: H256,
	chunks: &'a [I],
	current_pos: usize,
}

impl<'a, I: AsRef<[u8]>> Branches<'a, I> {
	/// Get the trie root.
	pub fn root(&self) -> H256 { self.root.clone() }
}

impl<'a, I: AsRef<[u8]>> Iterator for Branches<'a, I> {
	type Item = (Vec<Vec<u8>>, &'a [u8]);

	fn next(&mut self) -> Option<Self::Item> {
		use trie::Recorder;

		let trie = TrieDB::new(&self.trie_storage, &self.root)
			.expect("`Branches` is only created with a valid memorydb that contains all nodes for the trie with given root; qed");

		let mut recorder = Recorder::new();
		let res = (self.current_pos as u32).using_encoded(|s|
			trie.get_with(s, &mut recorder)
		);

		match res.expect("all nodes in trie present; qed") {
			Some(_) => {
				let nodes = recorder.drain().into_iter().map(|r| r.data).collect();
				let chunk = self.chunks.get(self.current_pos)
					.expect("there is a one-to-one mapping of chunks to valid merkle branches; qed");

				self.current_pos += 1;
				Some((nodes, chunk.as_ref()))
			}
			None => None,
		}
	}
}

/// Construct a trie from chunks of an erasure-coded value. This returns the root hash and an
/// iterator of merkle proofs, one for each validator.
pub fn branches<'a, I: 'a>(chunks: &'a [I]) -> Branches<'a, I>
	where I: AsRef<[u8]>,
{
	let mut trie_storage: MemoryDB<Blake2Hasher> = MemoryDB::default();
	let mut root = H256::default();

	// construct trie mapping each chunk's index to its hash.
	{
		let mut trie = TrieDBMut::new(&mut trie_storage, &mut root);
		for (i, chunk) in chunks.as_ref().iter().enumerate() {
			(i as u32).using_encoded(|encoded_index| {
				let chunk_hash = BlakeTwo256::hash(chunk.as_ref());
				trie.insert(encoded_index, chunk_hash.as_ref())
					.expect("a fresh trie stored in memory cannot have errors loading nodes; qed");
			})
		}
	}

	Branches {
		trie_storage,
		root,
		chunks: chunks,
		current_pos: 0,
	}
}

/// Verify a merkle branch, yielding the chunk hash meant to be present at that
/// index.
pub fn branch_hash(root: &H256, branch_nodes: &[Vec<u8>], index: usize) -> Result<H256, Error> {
	let mut trie_storage: MemoryDB<Blake2Hasher> = MemoryDB::default();
	for node in branch_nodes.iter() {
		(&mut trie_storage as &mut trie::HashDB<_>).insert(EMPTY_PREFIX, node.as_slice());
	}

	let trie = TrieDB::new(&trie_storage, &root).map_err(|_| Error::InvalidBranchProof)?;
	let res = (index as u32).using_encoded(|key|
		trie.get_with(key, |raw_hash: &[u8]| H256::decode(&mut &raw_hash[..]))
	);

	match res {
		Ok(Some(Ok(hash))) => Ok(hash),
		Ok(Some(Err(_))) => Err(Error::InvalidBranchProof), // hash failed to decode
		Ok(None) => Err(Error::BranchOutOfBounds),
		Err(_) => Err(Error::InvalidBranchProof),
	}
}

// input for `codec` which draws data from the data shards
struct ShardInput<'a, I> {
	remaining_len: usize,
	shards: I,
	cur_shard: Option<(&'a [u8], usize)>,
}

impl<'a, I: Iterator<Item=&'a [u8]>> parity_scale_codec::Input for ShardInput<'a, I> {
	fn remaining_len(&mut self) -> Result<Option<usize>, parity_scale_codec::Error> {
		Ok(Some(self.remaining_len))
	}

	fn read(&mut self, into: &mut [u8]) -> Result<(), parity_scale_codec::Error> {
		let mut read_bytes = 0;

		loop {
			if read_bytes == into.len() { break }

			let cur_shard = self.cur_shard.take().or_else(|| self.shards.next().map(|s| (s, 0)));
			let (active_shard, mut in_shard) = match cur_shard {
				Some((s, i)) => (s, i),
				None => break,
			};

			if in_shard >= active_shard.len() {
				continue;
			}

			let remaining_len_out = into.len() - read_bytes;
			let remaining_len_shard = active_shard.len() - in_shard;

			let write_len = std::cmp::min(remaining_len_out, remaining_len_shard);
			into[read_bytes..][..write_len]
				.copy_from_slice(&active_shard[in_shard..][..write_len]);

			in_shard += write_len;
			read_bytes += write_len;
			self.cur_shard = Some((active_shard, in_shard))
		}

		self.remaining_len -= read_bytes;
		if read_bytes == into.len() {
			Ok(())
		} else {
			Err("slice provided too big for input".into())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use primitives::v0::{AvailableData, BlockData, PoVBlock};

	#[test]
	fn field_order_is_right_size() {
		assert_eq!(MAX_VALIDATORS, 65536);
	}

	#[test]
	fn test_code_params() {
		assert_eq!(code_params(0), Err(Error::NotEnoughValidators));

		assert_eq!(code_params(1), Err(Error::NotEnoughValidators));

		assert_eq!(code_params(2), Ok(CodeParams {
			data_shards: 1,
			parity_shards: 1,
		}));

		assert_eq!(code_params(3), Ok(CodeParams {
			data_shards: 1,
			parity_shards: 2,
		}));

		assert_eq!(code_params(4), Ok(CodeParams {
			data_shards: 2,
			parity_shards: 2,
		}));

		assert_eq!(code_params(100), Ok(CodeParams {
			data_shards: 34,
			parity_shards: 66,
		}));
	}

	#[test]
	fn shard_len_is_reasonable() {
		let mut params = CodeParams {
			data_shards: 5,
			parity_shards: 0, // doesn't affect calculation.
		};

		assert_eq!(params.shard_len(100), 20);
		assert_eq!(params.shard_len(99), 20);

		// see if it rounds up to 2.
		assert_eq!(params.shard_len(95), 20);
		assert_eq!(params.shard_len(94), 20);

		assert_eq!(params.shard_len(89), 18);

		params.data_shards = 7;

		// needs 3 bytes to fit, rounded up to next even number.
		assert_eq!(params.shard_len(19), 4);
	}

    #[test]
	fn round_trip_works() {
		let pov_block = PoVBlock {
			block_data: BlockData((0..255).collect()),
		};

		let available_data = AvailableData {
			pov_block,
			omitted_validation: Default::default(),
		};
		let chunks = obtain_chunks(
			10,
			&available_data,
		).unwrap();

		assert_eq!(chunks.len(), 10);

		// any 4 chunks should work.
		let reconstructed: AvailableData = reconstruct(
			10,
			[
				(&*chunks[1], 1),
				(&*chunks[4], 4),
				(&*chunks[6], 6),
				(&*chunks[9], 9),
			].iter().cloned(),
		).unwrap();

		assert_eq!(reconstructed, available_data);
	}

	#[test]
	fn reconstruct_does_not_panic_on_low_validator_count() {
		let reconstructed = reconstruct_v1(
			1,
			[].iter().cloned(),
		);
		assert_eq!(reconstructed, Err(Error::NotEnoughValidators));
	}

	#[test]
	fn construct_valid_branches() {
		let pov_block = PoVBlock {
			block_data: BlockData(vec![2; 256]),
		};

		let available_data = AvailableData {
			pov_block,
			omitted_validation: Default::default(),
		};

		let chunks = obtain_chunks(
			10,
			&available_data,
		).unwrap();

		assert_eq!(chunks.len(), 10);

		let branches = branches(chunks.as_ref());
		let root = branches.root();

		let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();

		assert_eq!(proofs.len(), 10);

		for (i, proof) in proofs.into_iter().enumerate() {
			assert_eq!(branch_hash(&root, &proof, i).unwrap(), BlakeTwo256::hash(&chunks[i]));
		}
	}
}
