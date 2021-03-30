// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Primitive types used on the node-side.
//!
//! Unlike the `polkadot-primitives` crate, these primitives are only used on the node-side,
//! not shared between the node and the runtime. This crate builds on top of the primitives defined
//! there.

#![deny(missing_docs)]

use std::pin::Pin;

use serde::{Serialize, Deserialize};
use futures::Future;
use parity_scale_codec::{Decode, Encode};

pub use sp_core::traits::SpawnNamed;
pub use sp_consensus_babe::{
	Epoch as BabeEpoch, BabeEpochConfiguration, AllowedSlots as BabeAllowedSlots,
};

use polkadot_primitives::v1::{CandidateCommitments, CandidateHash, CollatorPair, CommittedCandidateReceipt, CompactStatement, EncodeAs, Hash, HeadData, Id as ParaId, OutboundHrmpMessage, PersistedValidationData, Signed, UpwardMessage, ValidationCode, BlakeTwo256, HashT, ValidatorIndex};
pub use polkadot_parachain::primitives::BlockData;


pub mod approval;

/// A statement, where the candidate receipt is included in the `Seconded` variant.
///
/// This is the committed candidate receipt instead of the bare candidate receipt. As such,
/// it gives access to the commitments to validators who have not executed the candidate. This
/// is necessary to allow a block-producing validator to include candidates from outside of the para
/// it is assigned to.
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
pub enum Statement {
	/// A statement that a validator seconds a candidate.
	#[codec(index = 1)]
	Seconded(CommittedCandidateReceipt),
	/// A statement that a validator has deemed a candidate valid.
	#[codec(index = 2)]
	Valid(CandidateHash),
}

impl std::fmt::Debug for Statement {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Statement::Seconded(seconded) => write!(f, "Seconded: {:?}", seconded.descriptor),
			Statement::Valid(hash) => write!(f, "Valid: {:?}", hash),
		}
	}
}

impl Statement {
	/// Get the candidate hash referenced by this statement.
	///
	/// If this is a `Statement::Seconded`, this does hash the candidate receipt, which may be expensive
	/// for large candidates.
	pub fn candidate_hash(&self) -> CandidateHash {
		match *self {
			Statement::Valid(ref h) => *h,
			Statement::Seconded(ref c) => c.hash(),
		}
	}

	/// Transform this statement into its compact version, which references only the hash
	/// of the candidate.
	pub fn to_compact(&self) -> CompactStatement {
		match *self {
			Statement::Seconded(ref c) => CompactStatement::Seconded(c.hash()),
			Statement::Valid(hash) => CompactStatement::Valid(hash),
		}
	}
}

impl From<&'_ Statement> for CompactStatement {
	fn from(stmt: &Statement) -> Self {
		stmt.to_compact()
	}
}

impl EncodeAs<CompactStatement> for Statement {
	fn encode_as(&self) -> Vec<u8> {
		self.to_compact().encode()
	}
}

/// A statement, the corresponding signature, and the index of the sender.
///
/// Signing context and validator set should be apparent from context.
///
/// This statement is "full" in the sense that the `Seconded` variant includes the candidate receipt.
/// Only the compact `SignedStatement` is suitable for submission to the chain.
pub type SignedFullStatement = Signed<Statement, CompactStatement>;

/// Candidate invalidity details
#[derive(Debug)]
pub enum InvalidCandidate {
	/// Failed to execute.`validate_block`. This includes function panicking.
	ExecutionError(String),
	/// Validation outputs check doesn't pass.
	InvalidOutputs,
	/// Execution timeout.
	Timeout,
	/// Validation input is over the limit.
	ParamsTooLarge(u64),
	/// Code size is over the limit.
	CodeTooLarge(u64),
	/// Validation function returned invalid data.
	BadReturn,
	/// Invalid relay chain parent.
	BadParent,
	/// POV hash does not match.
	HashMismatch,
	/// Bad collator signature.
	BadSignature,
	/// Para head hash does not match.
	ParaHeadHashMismatch,
	/// Validation code hash does not match.
	CodeHashMismatch,
}

/// Result of the validation of the candidate.
#[derive(Debug)]
pub enum ValidationResult {
	/// Candidate is valid. The validation process yields these outputs and the persisted validation
	/// data used to form inputs.
	Valid(CandidateCommitments, PersistedValidationData),
	/// Candidate is invalid.
	Invalid(InvalidCandidate),
}

/// Maximum PoV size we support right now.
pub const MAX_POV_SIZE: u32 = 50 * 1024 * 1024;

/// Very conservative (compression ratio of 1).
///
/// Experiments showed that we have a typical compression ratio of 3.4.
/// https://github.com/ordian/bench-compression-algorithms/
///
/// So this could be reduced if deemed necessary.
pub const MAX_COMPRESSED_POV_SIZE: u32 = MAX_POV_SIZE;

/// A Proof-of-Validity
#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug)]
pub struct PoV {
	/// The block witness data.
	pub block_data: BlockData,
}

impl PoV {
	/// Get the blake2-256 hash of the PoV.
	pub fn hash(&self) -> Hash {
		BlakeTwo256::hash_of(self)
	}
}

/// SCALE and Zstd encoded [`PoV`].
#[derive(Clone, Encode, Decode, PartialEq, Eq)]
pub struct CompressedPoV(Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[allow(missing_docs)]
pub enum CompressedPoVError {
	#[error("Failed to compress a PoV")]
	Compress,
	#[error("Failed to decompress a PoV")]
	Decompress,
	#[error("Failed to decode the uncompressed PoV")]
	Decode,
	#[error("Architecture is not supported")]
	NotSupported,
}

impl CompressedPoV {
	/// Compress the given [`PoV`] and returns a [`CompressedPoV`].
	#[cfg(not(target_os = "unknown"))]
	pub fn compress(pov: &PoV) -> Result<Self, CompressedPoVError> {
		zstd::encode_all(pov.encode().as_slice(), 3).map_err(|_| CompressedPoVError::Compress).map(Self)
	}

	/// Compress the given [`PoV`] and returns a [`CompressedPoV`].
	#[cfg(target_os = "unknown")]
	pub fn compress(_: &PoV) -> Result<Self, CompressedPoVError> {
		Err(CompressedPoVError::NotSupported)
	}

	/// Decompress `self` and returns the [`PoV`] on success.
	#[cfg(not(target_os = "unknown"))]
	pub fn decompress(&self) -> Result<PoV, CompressedPoVError> {
		use std::io::Read;

		struct InputDecoder<'a, T: std::io::BufRead>(&'a mut zstd::Decoder<T>, usize);
		impl<'a, T: std::io::BufRead> parity_scale_codec::Input for InputDecoder<'a, T> {
			fn read(&mut self, into: &mut [u8]) -> Result<(), parity_scale_codec::Error> {
				self.1 = self.1.saturating_add(into.len());
				if self.1 > MAX_POV_SIZE as usize {
					return Err("pov block too big".into())
				}
				self.0.read_exact(into).map_err(Into::into)
			}
			fn remaining_len(&mut self) -> Result<Option<usize>, parity_scale_codec::Error> {
				Ok(None)
			}
		}

		let mut decoder = zstd::Decoder::new(self.0.as_slice()).map_err(|_| CompressedPoVError::Decompress)?;
		PoV::decode(&mut InputDecoder(&mut decoder, 0)).map_err(|_| CompressedPoVError::Decode)
	}

	/// Decompress `self` and returns the [`PoV`] on success.
	#[cfg(target_os = "unknown")]
	pub fn decompress(&self) -> Result<PoV, CompressedPoVError> {
		Err(CompressedPoVError::NotSupported)
	}

	/// Get compressed data size.
	pub fn len(&self) -> usize {
		self.0.len()
	}
}

impl std::fmt::Debug for CompressedPoV {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "CompressedPoV({} bytes)", self.0.len())
	}
}

/// The output of a collator.
///
/// This differs from `CandidateCommitments` in two ways:
///
/// - does not contain the erasure root; that's computed at the Polkadot level, not at Cumulus
/// - contains a proof of validity.
#[derive(Clone, Encode, Decode)]
pub struct Collation<BlockNumber = polkadot_primitives::v1::BlockNumber> {
	/// Messages destined to be interpreted by the Relay chain itself.
	pub upward_messages: Vec<UpwardMessage>,
	/// The horizontal messages sent by the parachain.
	pub horizontal_messages: Vec<OutboundHrmpMessage<ParaId>>,
	/// New validation code.
	pub new_validation_code: Option<ValidationCode>,
	/// The head-data produced as a result of execution.
	pub head_data: HeadData,
	/// Proof to verify the state transition of the parachain.
	pub proof_of_validity: PoV,
	/// The number of messages processed from the DMQ.
	pub processed_downward_messages: u32,
	/// The mark which specifies the block number up to which all inbound HRMP messages are processed.
	pub hrmp_watermark: BlockNumber,
	/// The hash of validation code
	pub validation_code_hash: Hash,
}

/// Result of the [`CollatorFn`] invocation.
pub struct CollationResult {
	/// The collation that was build.
	pub collation: Collation,
	/// An optional result sender that should be informed about a successfully seconded collation.
	///
	/// There is no guarantee that this sender is informed ever about any result, it is completly okay to just drop it.
	/// However, if it is called, it should be called with the signed statement of a parachain validator seconding the
	/// collation.
	pub result_sender: Option<futures::channel::oneshot::Sender<SignedFullStatement>>,
}

impl CollationResult {
	/// Convert into the inner values.
	pub fn into_inner(self) -> (Collation, Option<futures::channel::oneshot::Sender<SignedFullStatement>>) {
		(self.collation, self.result_sender)
	}
}

/// Collation function.
///
/// Will be called with the hash of the relay chain block the parachain block should be build on and the
/// [`ValidationData`] that provides information about the state of the parachain on the relay chain.
///
/// Returns an optional [`CollationResult`].
pub type CollatorFn = Box<
	dyn Fn(Hash, &PersistedValidationData) -> Pin<Box<dyn Future<Output = Option<CollationResult>> + Send>>
		+ Send
		+ Sync,
>;

/// Configuration for the collation generator
pub struct CollationGenerationConfig {
	/// Collator's authentication key, so it can sign things.
	pub key: CollatorPair,
	/// Collation function. See [`CollatorFn`] for more details.
	pub collator: CollatorFn,
	/// The parachain that this collator collates for
	pub para_id: ParaId,
}

impl std::fmt::Debug for CollationGenerationConfig {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "CollationGenerationConfig {{ ... }}")
	}
}

/// This is the data we keep available for each candidate included in the relay chain.
#[derive(Clone, Encode, Decode, PartialEq, Eq, Debug)]
pub struct AvailableData {
	/// The Proof-of-Validation of the candidate.
	pub pov: std::sync::Arc<PoV>,
	/// The persisted validation data needed for secondary checks.
	pub validation_data: PersistedValidationData,
}

/// A chunk of erasure-encoded block data.
#[derive(PartialEq, Eq, Clone, Encode, Decode, Serialize, Deserialize, Debug, Hash)]
pub struct ErasureChunk {
	/// The erasure-encoded chunk of data belonging to the candidate block.
	pub chunk: Vec<u8>,
	/// The index of this erasure-encoded chunk of data.
	pub index: ValidatorIndex,
	/// Proof for this chunk's branch in the Merkle tree.
	pub proof: Vec<Vec<u8>>,
}

#[cfg(test)]
mod test {
	use super::{CompressedPoV, CompressedPoVError, PoV};

	#[test]
	fn decompress_huge_pov_block_fails() {
		let pov = PoV { block_data: vec![0; 63 * 1024 * 1024].into() };

		let compressed = CompressedPoV::compress(&pov).unwrap();
		assert_eq!(CompressedPoVError::Decode, compressed.decompress().unwrap_err());
	}
}
