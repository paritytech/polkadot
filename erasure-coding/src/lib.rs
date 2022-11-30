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
//! Each of n validators stores their piece of data. We assume `n = 3f + k`, `0 < k ≤ 3`.
//! f is the maximum number of faulty validators in the system.
//! The data is coded so any f+1 chunks can be used to reconstruct the full data.

use parity_scale_codec::{Decode, Encode};
use polkadot_node_primitives::{AvailableData, Proof};
use polkadot_primitives::v2::{BlakeTwo256, Hash as H256, HashT};
use sp_core::Blake2Hasher;
use sp_trie::{
	trie_types::{TrieDBBuilder, TrieDBMutBuilderV0 as TrieDBMutBuilder},
	LayoutV0, MemoryDB, Trie, TrieMut, EMPTY_PREFIX,
};
use thiserror::Error;

use novelpoly::{CodeParams, WrappedShard};

// we are limited to the field order of GF(2^16), which is 65536
const MAX_VALIDATORS: usize = novelpoly::f2e16::FIELD_SIZE;

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
	#[error("Chunks are not uniform, mismatch in length or are zero sized")]
	NonUniformChunks,
	/// An uneven byte-length of a shard is not valid for `GF(2^16)` encoding.
	#[error("Uneven length is not valid for field GF(2^16)")]
	UnevenLength,
	/// Chunk index out of bounds.
	#[error("Chunk is out of bounds: {chunk_index} not included in 0..{n_validators}")]
	ChunkIndexOutOfBounds { chunk_index: usize, n_validators: usize },
	/// Bad payload in reconstructed bytes.
	#[error("Reconstructed payload invalid")]
	BadPayload,
	/// Invalid branch proof.
	#[error("Invalid branch proof")]
	InvalidBranchProof,
	/// Branch out of bounds.
	#[error("Branch is out of bounds")]
	BranchOutOfBounds,
	/// Unknown error
	#[error("An unknown error has appeared when reconstructing erasure code chunks")]
	UnknownReconstruction,
	/// Unknown error
	#[error("An unknown error has appeared when deriving code parameters from validator count")]
	UnknownCodeParam,
}

/// Obtain a threshold of chunks that should be enough to recover the data.
pub const fn recovery_threshold(n_validators: usize) -> Result<usize, Error> {
	if n_validators > MAX_VALIDATORS {
		return Err(Error::TooManyValidators)
	}
	if n_validators <= 1 {
		return Err(Error::NotEnoughValidators)
	}

	let needed = n_validators.saturating_sub(1) / 3;
	Ok(needed + 1)
}

fn code_params(n_validators: usize) -> Result<CodeParams, Error> {
	// we need to be able to reconstruct from 1/3 - eps

	let n_wanted = n_validators;
	let k_wanted = recovery_threshold(n_wanted)?;

	if n_wanted > MAX_VALIDATORS as usize {
		return Err(Error::TooManyValidators)
	}

	CodeParams::derive_parameters(n_wanted, k_wanted).map_err(|e| match e {
		novelpoly::Error::WantedShardCountTooHigh(_) => Error::TooManyValidators,
		novelpoly::Error::WantedShardCountTooLow(_) => Error::NotEnoughValidators,
		_ => Error::UnknownCodeParam,
	})
}

/// Obtain erasure-coded chunks for v1 `AvailableData`, one for each validator.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
pub fn obtain_chunks_v1(n_validators: usize, data: &AvailableData) -> Result<Vec<Vec<u8>>, Error> {
	obtain_chunks(n_validators, data)
}

/// Obtain erasure-coded chunks, one for each validator.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
pub fn obtain_chunks<T: Encode>(n_validators: usize, data: &T) -> Result<Vec<Vec<u8>>, Error> {
	let params = code_params(n_validators)?;
	let encoded = data.encode();

	if encoded.is_empty() {
		return Err(Error::BadPayload)
	}

	let shards = params
		.make_encoder()
		.encode::<WrappedShard>(&encoded[..])
		.expect("Payload non-empty, shard sizes are uniform, and validator numbers checked; qed");

	Ok(shards.into_iter().map(|w: WrappedShard| w.into_inner()).collect())
}

/// Reconstruct the v1 available data from a set of chunks.
///
/// Provide an iterator containing chunk data and the corresponding index.
/// The indices of the present chunks must be indicated. If too few chunks
/// are provided, recovery is not possible.
///
/// Works only up to 65536 validators, and `n_validators` must be non-zero.
pub fn reconstruct_v1<'a, I: 'a>(n_validators: usize, chunks: I) -> Result<AvailableData, Error>
where
	I: IntoIterator<Item = (&'a [u8], usize)>,
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
pub fn reconstruct<'a, I: 'a, T: Decode>(n_validators: usize, chunks: I) -> Result<T, Error>
where
	I: IntoIterator<Item = (&'a [u8], usize)>,
{
	let params = code_params(n_validators)?;
	let mut received_shards: Vec<Option<WrappedShard>> = vec![None; n_validators];
	let mut shard_len = None;
	for (chunk_data, chunk_idx) in chunks.into_iter().take(n_validators) {
		if chunk_idx >= n_validators {
			return Err(Error::ChunkIndexOutOfBounds { chunk_index: chunk_idx, n_validators })
		}

		let shard_len = shard_len.get_or_insert_with(|| chunk_data.len());

		if *shard_len % 2 != 0 {
			return Err(Error::UnevenLength)
		}

		if *shard_len != chunk_data.len() || *shard_len == 0 {
			return Err(Error::NonUniformChunks)
		}

		received_shards[chunk_idx] = Some(WrappedShard::new(chunk_data.to_vec()));
	}

	let res = params.make_encoder().reconstruct(received_shards);

	let payload_bytes = match res {
		Err(e) => match e {
			novelpoly::Error::NeedMoreShards { .. } => return Err(Error::NotEnoughChunks),
			novelpoly::Error::ParamterMustBePowerOf2 { .. } => return Err(Error::UnevenLength),
			novelpoly::Error::WantedShardCountTooHigh(_) => return Err(Error::TooManyValidators),
			novelpoly::Error::WantedShardCountTooLow(_) => return Err(Error::NotEnoughValidators),
			novelpoly::Error::PayloadSizeIsZero { .. } => return Err(Error::BadPayload),
			novelpoly::Error::InconsistentShardLengths { .. } =>
				return Err(Error::NonUniformChunks),
			_ => return Err(Error::UnknownReconstruction),
		},
		Ok(payload_bytes) => payload_bytes,
	};

	Decode::decode(&mut &payload_bytes[..]).or_else(|_e| Err(Error::BadPayload))
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
	pub fn root(&self) -> H256 {
		self.root
	}
}

impl<'a, I: AsRef<[u8]>> Iterator for Branches<'a, I> {
	type Item = (Proof, &'a [u8]);

	fn next(&mut self) -> Option<Self::Item> {
		use sp_trie::Recorder;

		let mut recorder = Recorder::<LayoutV0<Blake2Hasher>>::new();
		let res = {
			let trie = TrieDBBuilder::new(&self.trie_storage, &self.root)
				.with_recorder(&mut recorder)
				.build();

			(self.current_pos as u32).using_encoded(|s| trie.get(s))
		};

		match res.expect("all nodes in trie present; qed") {
			Some(_) => {
				let nodes: Vec<Vec<u8>> = recorder.drain().into_iter().map(|r| r.data).collect();
				let chunk = self.chunks.get(self.current_pos).expect(
					"there is a one-to-one mapping of chunks to valid merkle branches; qed",
				);
				self.current_pos += 1;
				Proof::try_from(nodes).ok().map(|proof| (proof, chunk.as_ref()))
			},
			None => None,
		}
	}
}

/// Construct a trie from chunks of an erasure-coded value. This returns the root hash and an
/// iterator of merkle proofs, one for each validator.
pub fn branches<'a, I: 'a>(chunks: &'a [I]) -> Branches<'a, I>
where
	I: AsRef<[u8]>,
{
	let mut trie_storage: MemoryDB<Blake2Hasher> = MemoryDB::default();
	let mut root = H256::default();

	// construct trie mapping each chunk's index to its hash.
	{
		let mut trie = TrieDBMutBuilder::new(&mut trie_storage, &mut root).build();
		for (i, chunk) in chunks.as_ref().iter().enumerate() {
			(i as u32).using_encoded(|encoded_index| {
				let chunk_hash = BlakeTwo256::hash(chunk.as_ref());
				trie.insert(encoded_index, chunk_hash.as_ref())
					.expect("a fresh trie stored in memory cannot have errors loading nodes; qed");
			})
		}
	}

	Branches { trie_storage, root, chunks, current_pos: 0 }
}

/// Verify a merkle branch, yielding the chunk hash meant to be present at that
/// index.
pub fn branch_hash(root: &H256, branch_nodes: &Proof, index: usize) -> Result<H256, Error> {
	let mut trie_storage: MemoryDB<Blake2Hasher> = MemoryDB::default();
	for node in branch_nodes.iter() {
		(&mut trie_storage as &mut sp_trie::HashDB<_>).insert(EMPTY_PREFIX, node);
	}

	let trie = TrieDBBuilder::new(&trie_storage, &root).build();
	let res = (index as u32).using_encoded(|key| {
		trie.get_with(key, |raw_hash: &[u8]| H256::decode(&mut &raw_hash[..]))
	});

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

impl<'a, I: Iterator<Item = &'a [u8]>> parity_scale_codec::Input for ShardInput<'a, I> {
	fn remaining_len(&mut self) -> Result<Option<usize>, parity_scale_codec::Error> {
		Ok(Some(self.remaining_len))
	}

	fn read(&mut self, into: &mut [u8]) -> Result<(), parity_scale_codec::Error> {
		let mut read_bytes = 0;

		loop {
			if read_bytes == into.len() {
				break
			}

			let cur_shard = self.cur_shard.take().or_else(|| self.shards.next().map(|s| (s, 0)));
			let (active_shard, mut in_shard) = match cur_shard {
				Some((s, i)) => (s, i),
				None => break,
			};

			if in_shard >= active_shard.len() {
				continue
			}

			let remaining_len_out = into.len() - read_bytes;
			let remaining_len_shard = active_shard.len() - in_shard;

			let write_len = std::cmp::min(remaining_len_out, remaining_len_shard);
			into[read_bytes..][..write_len].copy_from_slice(&active_shard[in_shard..][..write_len]);

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
	use polkadot_node_primitives::{AvailableData, BlockData, PoV};

	// In order to adequately compute the number of entries in the Merkle
	// trie, we must account for the fixed 16-ary trie structure.
	const KEY_INDEX_NIBBLE_SIZE: usize = 4;

	#[test]
	fn field_order_is_right_size() {
		assert_eq!(MAX_VALIDATORS, 65536);
	}

	#[test]
	fn round_trip_works() {
		let pov = PoV { block_data: BlockData((0..255).collect()) };

		let available_data = AvailableData { pov: pov.into(), validation_data: Default::default() };
		let chunks = obtain_chunks(10, &available_data).unwrap();

		assert_eq!(chunks.len(), 10);

		// any 4 chunks should work.
		let reconstructed: AvailableData = reconstruct(
			10,
			[(&*chunks[1], 1), (&*chunks[4], 4), (&*chunks[6], 6), (&*chunks[9], 9)]
				.iter()
				.cloned(),
		)
		.unwrap();

		assert_eq!(reconstructed, available_data);
	}

	#[test]
	fn reconstruct_does_not_panic_on_low_validator_count() {
		let reconstructed = reconstruct_v1(1, [].iter().cloned());
		assert_eq!(reconstructed, Err(Error::NotEnoughValidators));
	}

	fn generate_trie_and_generate_proofs(magnitude: u32) {
		let n_validators = 2_u32.pow(magnitude) as usize;
		let pov = PoV { block_data: BlockData(vec![2; n_validators / KEY_INDEX_NIBBLE_SIZE]) };

		let available_data = AvailableData { pov: pov.into(), validation_data: Default::default() };

		let chunks = obtain_chunks(magnitude as usize, &available_data).unwrap();

		assert_eq!(chunks.len() as u32, magnitude);

		let branches = branches(chunks.as_ref());
		let root = branches.root();

		let proofs: Vec<_> = branches.map(|(proof, _)| proof).collect();
		assert_eq!(proofs.len() as u32, magnitude);
		for (i, proof) in proofs.into_iter().enumerate() {
			let encode = Encode::encode(&proof);
			let decode = Decode::decode(&mut &encode[..]).unwrap();
			assert_eq!(proof, decode);
			assert_eq!(encode, Encode::encode(&decode));

			assert_eq!(branch_hash(&root, &proof, i).unwrap(), BlakeTwo256::hash(&chunks[i]));
		}
	}

	#[test]
	fn roundtrip_proof_encoding() {
		for i in 2..16 {
			generate_trie_and_generate_proofs(i);
		}
	}
}
