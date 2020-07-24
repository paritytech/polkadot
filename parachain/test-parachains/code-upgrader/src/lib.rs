// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Test parachain WASM which implements code ugprades.

#![no_std]

#![cfg_attr(not(feature = "std"), feature(core_intrinsics, lang_items, core_panic_info, alloc_error_handler))]

use codec::{Encode, Decode};
use parachain::primitives::{RelayChainBlockNumber, ValidationCode};

#[cfg(not(feature = "std"))]
mod wasm_validation;

#[cfg(not(feature = "std"))]
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

#[cfg(feature = "std")]
/// Wasm binary unwrapped. If built with `BUILD_DUMMY_WASM_BINARY`, the function panics.
pub fn wasm_binary_unwrap() -> &'static [u8] {
	WASM_BINARY.expect("Development wasm binary is not available. Testing is only \
						supported with the flag disabled.")
}

#[derive(Encode, Decode, Clone, Default)]
pub struct State {
	/// The current code that is "active" in this chain.
	pub code: ValidationCode,
	/// Code upgrade that is pending.
	pub pending_code: Option<(ValidationCode, RelayChainBlockNumber)>,
}

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
		tiny_keccak::keccak256(&self.encode())
	}
}

/// Block data for this parachain.
#[derive(Default, Clone, Encode, Decode)]
pub struct BlockData {
	/// State to begin from.
	pub state: State,
	/// Code to upgrade to.
	pub new_validation_code: Option<ValidationCode>,
}

pub fn hash_state(state: &State) -> [u8; 32] {
	tiny_keccak::keccak256(state.encode().as_slice())
}

#[derive(Debug)]
pub enum Error {
	/// Start state mismatched with parent header's state hash.
	StateMismatch,
	/// New validation code too large.
	NewCodeTooLarge,
	/// Code upgrades not allowed at this time.
	CodeUpgradeDisallowed,
}

pub struct ValidationResult {
	/// The new head data.
	pub head_data: HeadData,
	/// The new validation code.
	pub new_validation_code: Option<ValidationCode>,
}

pub struct RelayChainParams {
	/// Whether a code upgrade is allowed and at what relay-chain block number
	/// to process it after.
	pub code_upgrade_allowed: Option<RelayChainBlockNumber>,
	/// The maximum code size allowed for an upgrade.
	pub max_code_size: u32,
	/// The relay-chain block number.
	pub relay_chain_block_number: RelayChainBlockNumber,
}

/// Execute a block body on top of given parent head, producing new parent head
/// if valid.
pub fn execute(
	parent_hash: [u8; 32],
	parent_head: HeadData,
	block_data: BlockData,
	relay_params: &RelayChainParams,
) -> Result<ValidationResult, Error> {
	debug_assert_eq!(parent_hash, parent_head.hash());

	if hash_state(&block_data.state) != parent_head.post_state {
		return Err(Error::StateMismatch);
	}

	let mut new_state = block_data.state;

	if let Some((pending_code, after)) = new_state.pending_code.take() {
		if after <= relay_params.relay_chain_block_number {
			// code applied.
			new_state.code = pending_code;
		} else {
			// reinstate.
			new_state.pending_code = Some((pending_code, after));
		}
	}

	let new_validation_code = if let Some(ref new_validation_code) = block_data.new_validation_code {
		if new_validation_code.0.len() as u32 > relay_params.max_code_size {
			return Err(Error::NewCodeTooLarge);
		}

		// replace the code if allowed and we don't have an upgrade pending.
		match (new_state.pending_code.is_some(), relay_params.code_upgrade_allowed) {
			(_, None) => return Err(Error::CodeUpgradeDisallowed),
			(false, Some(after)) => {
				new_state.pending_code = Some((new_validation_code.clone(), after));
				Some(new_validation_code.clone())
			}
			(true, Some(_)) => None,
		}
	} else {
		None
	};

	let head_data = HeadData {
		number: parent_head.number + 1,
		parent_hash,
		post_state: hash_state(&new_state),
	};

	Ok(ValidationResult {
		head_data,
		new_validation_code: new_validation_code,
	})
}
