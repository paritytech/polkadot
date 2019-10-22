// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Utilities for writing parachain WASM.

use codec::{Encode, Decode};
use super::{ValidationParams, ValidationResult, MessageRef, UpwardMessageRef};

mod ll {
	extern "C" {
		pub(super) fn ext_post_message(target: u32, data_ptr: *const u8, data_len: u32);
		pub(super) fn ext_post_upward_message(origin: u32, data_ptr: *const u8, data_len: u32);
	}
}

/// Load the validation params from memory when implementing a Rust parachain.
///
/// Offset and length must have been provided by the validation
/// function's entry point.
pub unsafe fn load_params(params: *const u8, len: usize) -> ValidationParams {
	let mut slice = rstd::slice::from_raw_parts(params, len);

	ValidationParams::decode(&mut slice).expect("Invalid input data")
}

/// Allocate the validation result in memory, getting the return-pointer back.
///
/// As described in the crate docs, this is a value holding the pointer to the encoded data and the
/// length of this data.
pub fn write_result(result: ValidationResult) -> u64 {
	let mut encoded = result.encode();
	let len = encoded.len();

	assert!(len <= u32::max_value() as usize, "Len too large for parachain-WASM abi");
	let res = encoded.as_ptr() as u64 | ((encoded.len() as u64) << 32);

	// leak so it doesn't get zeroed.
	rstd::mem::forget(encoded);

	res
}

/// Post a message to another parachain.
pub fn post_message(message: MessageRef) {
	let data_ptr = message.data.as_ptr();
	let data_len = message.data.len();

	unsafe { ll::ext_post_message(message.target.into(), data_ptr, data_len as u32) }
}

/// Post a message to this parachain's relay chain.
pub fn post_upward_message(message: UpwardMessageRef) {
	let data_ptr = message.data.as_ptr();
	let data_len = message.data.len();

	unsafe { ll::ext_post_upward_message(u32::from(message.origin as u8), data_ptr, data_len as u32) }
}
