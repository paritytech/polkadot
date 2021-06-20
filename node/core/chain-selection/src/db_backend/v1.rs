// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! A database [`Backend`][crate::backend::Backend] for the chain selection subsystem.
//!
//! This stores the following schema:
//!
//! ```
//! ("CS_block_entry", Hash) -> BlockEntry;
//! ("CS_block_height", BigEndianBlockNumber) -> Vec<Hash>;
//! ("CS_stagnant_at", BigEndianTimestamp) -> Vec<Hash>;
//! ```
//!
//! The big-endian encoding is used for creating iterators over the key-value DB which are
//! accessible by prefix, to find the earlist block number stored as well as the all stagnant
//! blocks.
//!
//! The `Vec`s stored are always non-empty. Empty `Vec`s are not stored on disk so there is no
//! semantic difference between `None` and an empty `Vec`.

use crate::backend::{Backend, BackendWriteOp};
use crate::{BlockEntry, Error, Timestamp};

use polkadot_primitives::v1::{BlockNumber, Hash};

use kvdb::{DBTransaction, KeyValueDB};
use parity_scale_codec::{Encode, Decode};

use std::sync::Arc;

const BLOCK_ENTRY_PREFIX: &[u8; 14] = b"CS_block_entry";
const BLOCK_HEIGHT_PREFIX: &[u8; 15] = b"CS_block_height";
const STAGNANT_AT_PREFIX: &[u8; 14] = b"CS_stagnant_at";

/// Configuration for the database backend.
#[derive(Debug, Clone, Copy)]
pub struct Config {
	/// The column where block metadata is stored.
	pub col_data: u32,
}

/// The database backend.
pub struct DbBackend {
	inner: Arc<dyn KeyValueDB>,
	config: Config,
}

fn block_entry_key(hash: &Hash) -> [u8; 14 + 32] {
	let mut key = [0; 14 + 32];
	key[..14].copy_from_slice(BLOCK_ENTRY_PREFIX);
	hash.using_encoded(|s| key[14..].copy_from_slice(s));
	key
}

fn block_height_key(number: BlockNumber) -> [u8; 15 + 4] {
	let mut key = [0; 15 + 4];
	key[..15].copy_from_slice(BLOCK_HEIGHT_PREFIX);
	key[15..].copy_from_slice(&number.to_be_bytes());
	key
}

fn stagnant_at_key(timestamp: Timestamp) -> [u8; 14 + 8] {
	let mut key = [0; 14 + 8];
	key[..14].copy_from_slice(STAGNANT_AT_PREFIX);
	key[14..].copy_from_slice(&timestamp.to_be_bytes());
	key
}

fn decode_block_height_key(key: &[u8]) -> Option<BlockNumber> {
	if key.len() != 15 + 4 { return None }
	if !key.starts_with(BLOCK_HEIGHT_PREFIX) { return None }

	let mut bytes = [0; 4];
	bytes.copy_from_slice(&key[15..]);
	Some(BlockNumber::from_be_bytes(bytes))
}

fn decode_stagnant_at_key(key: &[u8]) -> Option<Timestamp> {
	if key.len() != 14 + 8 { return None }
	if !key.starts_with(STAGNANT_AT_PREFIX) { return None }

	let mut bytes = [0; 8];
	bytes.copy_from_slice(&key[14..]);
	Some(Timestamp::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn block_height_key_decodes() {
		let key = block_height_key(5);
		assert_eq!(decode_block_height_key(&key), Some(5));
	}

	#[test]
	fn stagnant_at_key_decodes() {
		let key = stagnant_at_key(5);
		assert_eq!(decode_stagnant_at_key(&key), Some(5));
	}
}
