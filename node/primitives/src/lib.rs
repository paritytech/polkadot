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

use polkadot_primitives::v1::{BlakeTwo256, CandidateCommitments, CandidateHash, CollatorPair, CommittedCandidateReceipt, CompactStatement, EncodeAs, Hash, HashT, HeadData, Id as ParaId, OutboundHrmpMessage, PersistedValidationData, Signed, UncheckedSigned, UpwardMessage, ValidationCode, ValidatorIndex};
pub use polkadot_parachain::primitives::BlockData;

pub mod approval;

/// The bomb limit for decompressing code blobs.
pub const VALIDATION_CODE_BOMB_LIMIT: usize = 16 * 1024 * 1024;

/// Maximum PoV size we support right now.
pub const MAX_POV_SIZE: u32 = 20 * 1024 * 1024;

/// The bomb limit for decompressing PoV blobs.
pub const POV_BOMB_LIMIT: usize = MAX_POV_SIZE as usize;

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

/// Variant of `SignedFullStatement` where the signature has not yet been verified.
pub type UncheckedSignedFullStatement = UncheckedSigned<Statement, CompactStatement>;

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
	/// Code does not decompress correctly.
	CodeDecompressionFailure,
	/// PoV does not decompress correctly.
	PoVDecompressionFailure,
	/// Validation function returned invalid data.
	BadReturn,
	/// Invalid relay chain parent.
	BadParent,
	/// POV hash does not match.
	PoVHashMismatch,
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

/// Compress a PoV, unless it exceeds the [`POV_BOMB_LIMIT`].
#[cfg(not(target_os = "unknown"))]
pub fn maybe_compress_pov(pov: PoV) -> PoV {
	let PoV { block_data: BlockData(raw) } = pov;
	let raw = sp_maybe_compressed_blob::compress(&raw, POV_BOMB_LIMIT)
		.unwrap_or(raw);

	let pov = PoV { block_data: BlockData(raw) };
	pov
}
