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
use super::{ValidationParams, ValidationResult, Message};

mod ll {
	extern "C" {
		pub(super) fn ext_post_message(target: u32, data_ptr: *const u8, data_len: u32);
	}
}

/// Load the validation params from memory when implementing a Rust parachain.
///
/// Offset and length must have been provided by the validation
/// function's entry point.
pub unsafe fn load_params(offset: usize, len: usize) -> ValidationParams {
	let mut slice = ::core::slice::from_raw_parts(offset as *const u8, len);

	ValidationParams::decode(&mut slice).expect("Invalid input data")
}

/// Allocate the validation result in memory, getting the return-pointer back.
///
/// As described in the crate docs, this is a pointer to the appended length
/// of the vector.
pub fn write_result(result: ValidationResult) -> usize {
	let mut encoded = result.encode();
	let len = encoded.len();

	assert!(len <= u32::max_value() as usize, "Len too large for parachain-WASM abi");
	(len as u32).using_encoded(|s| encoded.extend(s));

	// do not alter `encoded` beyond this point. may reallocate.
	let end_ptr = &encoded[len] as *const u8 as usize;

	// leak so it doesn't get zeroed.
	::core::mem::forget(encoded);
	end_ptr
}

/// Post a message to another parachain.
pub fn post_message(message: &Message) {
	let data_ptr = message.data.as_ptr();
	let data_len = message.data.len();

	unsafe { ll::ext_post_message(message.target, data_ptr, data_len as u32) }
}
