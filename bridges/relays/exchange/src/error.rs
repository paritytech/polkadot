// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

//! Exchange-relay errors.

use crate::exchange::{BlockHashOf, BlockNumberOf, TransactionHashOf};

use relay_utils::MaybeConnectionError;
use std::fmt::{Debug, Display};
use thiserror::Error;

/// Error type given pipeline.
pub type ErrorOf<P> = Error<BlockHashOf<P>, BlockNumberOf<P>, TransactionHashOf<P>>;

/// Exchange-relay error type.
#[derive(Error, Debug)]
pub enum Error<Hash: Display, HeaderNumber: Display, SourceTxHash: Display> {
	/// Failed to check finality of the requested header on the target node.
	#[error("Failed to check finality of header {0}/{1} on {2} node: {3:?}")]
	Finality(HeaderNumber, Hash, &'static str, anyhow::Error),
	/// Error retrieving block from the source node.
	#[error("Error retrieving block {0} from {1} node: {2:?}")]
	RetrievingBlock(Hash, &'static str, anyhow::Error),
	/// Error retrieving transaction from the source node.
	#[error("Error retrieving transaction {0} from {1} node: {2:?}")]
	RetrievingTransaction(SourceTxHash, &'static str, anyhow::Error),
	/// Failed to check existence of header from the target node.
	#[error("Failed to check existence of header {0}/{1} on {2} node: {3:?}")]
	CheckHeaderExistence(HeaderNumber, Hash, &'static str, anyhow::Error),
	/// Failed to prepare proof for the transaction from the source node.
	#[error("Error building transaction {0} proof on {1} node: {2:?}")]
	BuildTransactionProof(String, &'static str, anyhow::Error, bool),
	/// Failed to submit the transaction proof to the target node.
	#[error("Error submitting transaction {0} proof to {1} node: {2:?}")]
	SubmitTransactionProof(String, &'static str, anyhow::Error, bool),
	/// Transaction filtering failed.
	#[error("Transaction filtering has failed with {0:?}")]
	TransactionFiltering(anyhow::Error, bool),
	/// Utilities/metrics error.
	#[error("{0}")]
	Utils(#[from] relay_utils::Error),
}

impl<T: Display, U: Display, V: Display> MaybeConnectionError for Error<T, U, V> {
	fn is_connection_error(&self) -> bool {
		match *self {
			Self::BuildTransactionProof(_, _, _, b) => b,
			Self::SubmitTransactionProof(_, _, _, b) => b,
			Self::TransactionFiltering(_, b) => b,
			_ => false,
		}
	}
}
