// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! Polkadot block evaluation and evaluation errors.

use super::MAX_TRANSACTIONS_SIZE;

use codec::Encode;
use polkadot_primitives::{Block, Hash, BlockNumber};
use polkadot_primitives::parachain::{Id as ParaId, CollatorId, Retriable};

/// Result type alias for block evaluation
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for block evaluation
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Client error
	Client(sp_blockchain::Error),
	/// Too many parachain candidates in proposal
	#[display(fmt = "Proposal included {} candidates for {} parachains", expected, got)]
	TooManyCandidates { expected: usize, got: usize },
	/// Proposal included unregistered parachain
	#[display(fmt = "Proposal included unregistered parachain {:?}", _0)]
	UnknownParachain(ParaId),
	/// Proposal had wrong parent hash
	#[display(fmt = "Proposal had wrong parent hash. Expected {:?}, got {:?}", expected, got)]
	WrongParentHash { expected: Hash, got: Hash },
	/// Proposal had wrong number
	#[display(fmt = "Proposal had wrong number. Expected {:?}, got {:?}", expected, got)]
	WrongNumber { expected: BlockNumber, got: BlockNumber },
	/// Proposal exceeded the maximum size
	#[display(
		fmt = "Proposal exceeded the maximum size of {} by {} bytes.",
		MAX_TRANSACTIONS_SIZE, MAX_TRANSACTIONS_SIZE.saturating_sub(*_0)
	)]
	ProposalTooLarge(usize),
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::Client(ref err) => Some(err),
			_ => None,
		}
	}
}

/// Attempt to evaluate a substrate block as a polkadot block, returning error
/// upon any initial validity checks failing.
pub fn evaluate_initial(
	proposal: &Block,
	_now: u64,
	parent_hash: &Hash,
	parent_number: BlockNumber,
	_active_parachains: &[(ParaId, Option<(CollatorId, Retriable)>)],
) -> Result<()> {
	let transactions_size = proposal.extrinsics.iter().fold(0, |a, tx| {
		a + Encode::encode(tx).len()
	});

	if transactions_size > MAX_TRANSACTIONS_SIZE {
		return Err(Error::ProposalTooLarge(transactions_size))
	}

	if proposal.header.parent_hash != *parent_hash {
		return Err(Error::WrongParentHash {
			expected: *parent_hash,
			got: proposal.header.parent_hash
		});
	}

	if proposal.header.number != parent_number + 1 {
		return Err(Error::WrongNumber {
			expected: parent_number + 1,
			got: proposal.header.number
		});
	}

	Ok(())
}
