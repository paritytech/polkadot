// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Defines primitive types for creating or validating a parachain.
//!
//! When compiled with standard library support, this crate exports a `wasm`
//! module that can be used to validate parachain WASM.
//!
//! ## Parachain WASM
//!
//! Polkadot parachain WASM is in the form of a module which imports a memory
//! instance and exports a function `validate_block`.
//!
//! `validate` accepts as input two `i32` values, representing a pointer/length pair
//! respectively, that encodes [`ValidationParams`].
//!
//! `validate` returns an `u64` which is a pointer to an `u8` array and its length.
//! The data in the array is expected to be a SCALE encoded [`ValidationResult`].
//!
//! ASCII-diagram demonstrating the return data format:
//!
//! ```ignore
//! [pointer][length]
//!   32bit   32bit
//!         ^~~ returned pointer & length
//! ```
//!
//! The wasm-api (enabled only when `std` feature is not enabled and `wasm-api` feature is enabled)
//! provides utilities for setting up a parachain WASM module in Rust.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
pub mod wasm_executor;

mod wasm_api;

use rstd::vec::Vec;

use codec::{Encode, Decode};

#[cfg(all(not(feature = "std"), feature = "wasm-api"))]
pub use wasm_api::*;

pub use polkadot_primitives::{BlockNumber, parachain::{
	AccountIdConversion,
	Id,
	IncomingMessage,
	LOWEST_USER_ID,
	ParachainDispatchOrigin,
	UpwardMessage,
}};

/// Validation parameters for evaluating the parachain validity function.
// TODO: balance downloads (https://github.com/paritytech/polkadot/issues/220)
#[derive(PartialEq, Eq, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Encode))]
pub struct ValidationParams {
	/// The collation body.
	pub block_data: Vec<u8>,
	/// Previous head-data.
	pub parent_head: Vec<u8>,
	/// Number of the current relay chain block.
	pub current_relay_block: BlockNumber,
}

/// The result of parachain validation.
// TODO: egress and balance uploads (https://github.com/paritytech/polkadot/issues/220)
#[derive(PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Debug, Decode))]
pub struct ValidationResult {
	/// New head data that should be included in the relay chain state.
	pub head_data: Vec<u8>,
}
