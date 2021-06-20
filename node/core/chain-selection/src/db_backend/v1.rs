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

use std::sync::Arc;

const BLOCK_ENTRY_PREFIX: &[u8; 14] = b"CS_block_entry";
const BLOCK_HEIGHT_PREFIX: &[u8; 15] = b"CS_block_height";
const BLOCK_STAGNANT_AT: &[u8; 14] = b"CS_stagnant_at";

/// Configuration for the database backend.
#[derive(Debug, Clone, Copy)]
pub struct Config {
	/// The column where block metadata is stored.
	pub col_data: u32,
}

/// The database backend.
pub struct DbBackend {
	inner: Arc<KeyValueDB>,
	config: Config,
}
