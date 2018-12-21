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
use polkadot_primitives::parachain::Id as ParaId;

error_chain! {
	links {
		Client(::client::error::Error, ::client::error::ErrorKind);
	}

	errors {
		ProposalNotForPolkadot {
			description("Proposal provided not a Polkadot block."),
			display("Proposal provided not a Polkadot block."),
		}
		TooManyCandidates(expected: usize, got: usize) {
			description("Proposal included more candidates than is possible."),
			display("Proposal included {} candidates for {} parachains", got, expected),
		}
		ParachainOutOfOrder {
			description("Proposal included parachains out of order."),
			display("Proposal included parachains out of order."),
		}
		UnknownParachain(id: ParaId) {
			description("Proposal included unregistered parachain."),
			display("Proposal included unregistered parachain {:?}", id),
		}
		WrongParentHash(expected: Hash, got: Hash) {
			description("Proposal had wrong parent hash."),
			display("Proposal had wrong parent hash. Expected {:?}, got {:?}", expected, got),
		}
		WrongNumber(expected: BlockNumber, got: BlockNumber) {
			description("Proposal had wrong number."),
			display("Proposal had wrong number. Expected {:?}, got {:?}", expected, got),
		}
		ProposalTooLarge(size: usize) {
			description("Proposal exceeded the maximum size."),
			display(
				"Proposal exceeded the maximum size of {} by {} bytes.",
				MAX_TRANSACTIONS_SIZE, MAX_TRANSACTIONS_SIZE.saturating_sub(*size)
			),
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
	_active_parachains: &[ParaId],
) -> Result<()> {
	let transactions_size = proposal.extrinsics.iter().fold(0, |a, tx| {
		a + Encode::encode(tx).len()
	});

	if transactions_size > MAX_TRANSACTIONS_SIZE {
		bail!(ErrorKind::ProposalTooLarge(transactions_size))
	}

	if proposal.header.parent_hash != *parent_hash {
		bail!(ErrorKind::WrongParentHash(*parent_hash, proposal.header.parent_hash));
	}

	if proposal.header.number != parent_number + 1 {
		bail!(ErrorKind::WrongNumber(parent_number + 1, proposal.header.number));
	}

	Ok(())
}
