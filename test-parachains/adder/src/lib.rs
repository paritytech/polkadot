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

//! Basic parachain that adds a number as part of its state.

#![no_std]

use parity_codec::{Encode, Decode};

/// Head data for this parachain.
#[derive(Default, Clone, Hash, Eq, PartialEq, Encode, Decode)]
pub struct HeadData {
	/// Block number
	pub number: u64,
	/// parent block keccak256
	pub parent_hash: [u8; 32],
	/// hash of post-execution state.
	pub post_state: [u8; 32],
}

impl HeadData {
	pub fn hash(&self) -> [u8; 32] {
		::tiny_keccak::keccak256(&self.encode())
	}
}

/// Block data for this parachain.
#[derive(Default, Clone, Encode, Decode)]
pub struct BlockData {
	/// State to begin from.
	pub state: u64,
	/// Amount to add (overflowing)
	pub add: u64,
}

pub fn hash_state(state: u64) -> [u8; 32] {
	::tiny_keccak::keccak256(state.encode().as_slice())
}

#[derive(Default, Encode, Decode)]
pub struct AddMessage {
	/// The amount to add based on this message.
	pub amount: u64,
}

/// Start state mismatched with parent header's state hash.
#[derive(Debug)]
pub struct StateMismatch;

/// Process all incoming messages, yielding the amount of addition from messages.
///
/// Ignores unknown message kinds.
pub fn process_messages<I, T>(iterable: I) -> u64
	where I: IntoIterator<Item=T>, T: AsRef<[u8]>
{
	iterable.into_iter()
		.filter_map(|data| AddMessage::decode(&mut data.as_ref()))
		.fold(0u64, |a, c| a.overflowing_add(c.amount).0)
}

/// Execute a block body on top of given parent head, producing new parent head
/// if valid.
pub fn execute(
	parent_hash: [u8; 32],
	parent_head: HeadData,
	block_data: &BlockData,
	from_messages: u64,
) -> Result<HeadData, StateMismatch> {
	debug_assert_eq!(parent_hash, parent_head.hash());

	if hash_state(block_data.state) != parent_head.post_state {
		return Err(StateMismatch);
	}

	let new_state = block_data.state.overflowing_add(block_data.add).0;
	let new_state = new_state.overflowing_add(from_messages).0;

	Ok(HeadData {
		number: parent_head.number + 1,
		parent_hash,
		post_state: hash_state(new_state),
	})
}
