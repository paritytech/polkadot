// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

use crate::{TargetedMessage, UpwardMessage};
use sp_runtime_interface::runtime_interface;
#[cfg(feature = "std")]
use sp_externalities::ExternalitiesExt;

/// The parachain api for posting messages.
// Either activate on `std` to get access to the `HostFunctions` or when `wasm-api` is given and on
// `no_std`.
#[cfg(any(feature = "std", all(not(feature = "std"), feature = "wasm-api")))]
#[runtime_interface]
pub trait Parachain {
	/// Post a message to another parachain.
	fn post_message(&mut self, msg: TargetedMessage) {
		self.extension::<crate::wasm_executor::ParachainExt>()
			.expect("No `ParachainExt` associated with the current context.")
			.post_message(msg)
			.expect("Failed to post message")
	}

	/// Post a message to this parachain's relay chain.
	fn post_upward_message(&mut self, msg: UpwardMessage) {
		self.extension::<crate::wasm_executor::ParachainExt>()
			.expect("No `ParachainExt` associated with the current context.")
			.post_upward_message(msg)
			.expect("Failed to post upward message")
	}
}

/// Load the validation params from memory when implementing a Rust parachain.
///
/// Offset and length must have been provided by the validation
/// function's entry point.
#[cfg(not(feature = "std"))]
pub unsafe fn load_params(params: *const u8, len: usize) -> crate::ValidationParams {
	let mut slice = rstd::slice::from_raw_parts(params, len);

	codec::Decode::decode(&mut slice).expect("Invalid input data")
}

/// Allocate the validation result in memory, getting the return-pointer back.
///
/// As described in the crate docs, this is a pointer to the appended length
/// of the vector.
#[cfg(not(feature = "std"))]
pub fn write_result(result: &crate::ValidationResult) -> u64 {
	sp_core::to_substrate_wasm_fn_return_value(&result)
}
