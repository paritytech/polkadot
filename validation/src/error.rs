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

//! Errors that can occur during the validation process.

use polkadot_primitives::{parachain::ValidatorId, Hash};

/// Error type for validation
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Client error
	Client(sp_blockchain::Error),
	/// Consensus error
	Consensus(consensus::error::Error),
	/// A wasm-validation error.
	WasmValidation(parachain::wasm_executor::Error),
	/// An I/O error.
	Io(std::io::Error),
	/// An error in the availability erasure-coding.
	ErasureCoding(polkadot_erasure_coding::Error),
	#[display(fmt = "Invalid duty roster length: expected {}, got {}", expected, got)]
	InvalidDutyRosterLength {
		/// Expected roster length
		expected: usize,
		/// Actual roster length
		got: usize,
	},
	/// Local account not a validator at this block
	#[display(fmt = "Local account ID ({:?}) not a validator at this block.", _0)]
	NotValidator(ValidatorId),
	/// Unexpected error checking inherents
	#[display(fmt = "Unexpected error while checking inherents: {}", _0)]
	InherentError(inherents::Error),
	/// Proposer destroyed before finishing proposing or evaluating
	#[display(fmt = "Proposer destroyed before finishing proposing or evaluating")]
	PrematureDestruction,
	/// Timer failed
	#[display(fmt = "Timer failed: {}", _0)]
	Timer(std::io::Error),
	#[display(fmt = "Failed to compute deadline of now + {:?}", _0)]
	DeadlineComputeFailure(std::time::Duration),
	#[display(fmt = "Validation service is down.")]
	ValidationServiceDown,
	/// PoV-block in collation doesn't match provided.
	#[display(fmt = "PoV hash mismatch. Expected {:?}, got {:?}", _0, _1)]
	PoVHashMismatch(Hash, Hash),
	/// Collator signature is invalid.
	#[display(fmt = "Invalid collator signature on collation")]
	InvalidCollatorSignature,
	/// Head-data too large.
	#[display(fmt = "Head data size of {} exceeded maximum of {}", _0, _1)]
	HeadDataTooLarge(usize, usize),
	/// Head-data mismatch after validation.
	#[display(fmt = "Validation produced a different parachain header")]
	HeadDataMismatch,
	/// Relay parent of candidate not allowed.
	#[display(fmt = "Relay parent {} of candidate not allowed in this context.", _0)]
	DisallowedRelayParent(Hash),
	/// Commitments in candidate match commitments produced by validation.
	#[display(fmt = "Commitments in candidate receipt do not match those produced by validation")]
	CommitmentsMismatch,
	/// The parachain for which validation work is being done is not active.
	#[display(fmt = "Parachain {:?} is not active", _0)]
	InactiveParachain(polkadot_primitives::parachain::Id),
	/// Block data is too big
	#[display(fmt = "Block data is too big (maximum allowed size: {}, actual size: {})", size, max_size)]
	BlockDataTooBig { size: u64, max_size: u64 },
	Join(tokio::task::JoinError)
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::Client(ref err) => Some(err),
			Error::Consensus(ref err) => Some(err),
			Error::WasmValidation(ref err) => Some(err),
			Error::ErasureCoding(ref err) => Some(err),
			Error::Io(ref err) => Some(err),
			_ => None,
		}
	}
}
