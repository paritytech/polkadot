// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use sp_runtime::RuntimeDebug;

/// Header import error.
#[derive(Clone, Copy, RuntimeDebug)]
#[cfg_attr(feature = "std", derive(PartialEq))]
pub enum Error {
	/// The header is beyond last finalized and can not be imported.
	AncientHeader = 0,
	/// The header is already imported.
	KnownHeader = 1,
	/// Seal has an incorrect format.
	InvalidSealArity = 2,
	/// Block number isn't sensible.
	RidiculousNumber = 3,
	/// Block has too much gas used.
	TooMuchGasUsed = 4,
	/// Gas limit header field is invalid.
	InvalidGasLimit = 5,
	/// Extra data is of an invalid length.
	ExtraDataOutOfBounds = 6,
	/// Timestamp header overflowed.
	TimestampOverflow = 7,
	/// The parent header is missing from the blockchain.
	MissingParentBlock = 8,
	/// The header step is missing from the header.
	MissingStep = 9,
	/// The header signature is missing from the header.
	MissingSignature = 10,
	/// Empty steps are missing from the header.
	MissingEmptySteps = 11,
	/// The same author issued different votes at the same step.
	DoubleVote = 12,
	/// Validation proof insufficient.
	InsufficientProof = 13,
	/// Difficulty header field is invalid.
	InvalidDifficulty = 14,
	/// The received block is from an incorrect proposer.
	NotValidator = 15,
	/// Missing transaction receipts for the operation.
	MissingTransactionsReceipts = 16,
	/// Redundant transaction receipts are provided.
	RedundantTransactionsReceipts = 17,
	/// Provided transactions receipts are not matching the header.
	TransactionsReceiptsMismatch = 18,
	/// Can't accept unsigned header from the far future.
	UnsignedTooFarInTheFuture = 19,
	/// Trying to finalize sibling of finalized block.
	TryingToFinalizeSibling = 20,
	/// Header timestamp is ahead of on-chain timestamp
	HeaderTimestampIsAhead = 21,
}

impl Error {
	pub fn msg(&self) -> &'static str {
		match *self {
			Error::AncientHeader => "Header is beyound last finalized and can not be imported",
			Error::KnownHeader => "Header is already imported",
			Error::InvalidSealArity => "Header has an incorrect seal",
			Error::RidiculousNumber => "Header has too large number",
			Error::TooMuchGasUsed => "Header has too much gas used",
			Error::InvalidGasLimit => "Header has invalid gas limit",
			Error::ExtraDataOutOfBounds => "Header has too large extra data",
			Error::TimestampOverflow => "Header has too large timestamp",
			Error::MissingParentBlock => "Header has unknown parent hash",
			Error::MissingStep => "Header is missing step seal",
			Error::MissingSignature => "Header is missing signature seal",
			Error::MissingEmptySteps => "Header is missing empty steps seal",
			Error::DoubleVote => "Header has invalid step in seal",
			Error::InsufficientProof => "Header has insufficient proof",
			Error::InvalidDifficulty => "Header has invalid difficulty",
			Error::NotValidator => "Header is sealed by unexpected validator",
			Error::MissingTransactionsReceipts => "The import operation requires transactions receipts",
			Error::RedundantTransactionsReceipts => "Redundant transactions receipts are provided",
			Error::TransactionsReceiptsMismatch => "Invalid transactions receipts provided",
			Error::UnsignedTooFarInTheFuture => "The unsigned header is too far in future",
			Error::TryingToFinalizeSibling => "Trying to finalize sibling of finalized block",
			Error::HeaderTimestampIsAhead => "Header timestamp is ahead of on-chain timestamp",
		}
	}

	/// Return unique error code.
	pub fn code(&self) -> u8 {
		*self as u8
	}
}
