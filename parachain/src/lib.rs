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

//! Defines primitive types for creating or validating a parachain.
//!
//! When compiled with standard library support, this crate exports a `wasm`
//! module that can be used to validate parachain WASM.
//!
//! ## Parachain WASM
//!
//! Polkadot parachain WASM is in the form of a module which imports a memory
//! instance and exports a function `validate`.
//!
//! `validate` accepts as input two `i32` values, representing a pointer/length pair
//! respectively, that encodes `ValidationParams`.
//!
//! `validate` returns an `i32` which is a pointer to a little-endian 32-bit integer denoting a length.
//! Subtracting the length from the initial pointer will give a new pointer to the actual return data,
//!
//! ASCII-diagram demonstrating the return data format:
//!
//! ```ignore
//! [return data][len (LE-u32)]
//!              ^~~returned pointer
//! ```
//!
//! The `wasm_api` module (enabled only with the wasm-api feature) provides utilities
//!  for setting up a parachain WASM module in Rust.

#![cfg_attr(not(feature = "std"), no_std)]

/// Re-export of parity-codec.
pub extern crate parity_codec as codec;

#[macro_use]
extern crate parity_codec_derive;

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(feature = "std")]
extern crate core;

#[cfg(feature = "std")]
extern crate wasmi;

#[cfg(feature = "std")]
#[macro_use]
extern crate error_chain;

#[cfg(feature = "std")]
extern crate serde;

#[cfg(feature = "std")]
#[macro_use]
extern crate serde_derive;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

#[cfg(feature = "std")]
pub mod wasm_executor;

#[cfg(feature = "wasm-api")]
pub mod wasm_api;

/// Validation parameters for evaluating the parachain validity function.
// TODO: balance downloads (https://github.com/paritytech/polkadot/issues/220)
#[derive(PartialEq, Eq, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Encode))]
pub struct ValidationParams {
	/// The collation body.
	pub block_data: Vec<u8>,
	/// Previous head-data.
	pub parent_head: Vec<u8>,
	/// Incoming messages.
	pub ingress: Vec<IncomingMessage>,
}

/// The result of parachain validation.
// TODO: egress and balance uploads (https://github.com/paritytech/polkadot/issues/220)
#[derive(PartialEq, Eq, Encode)]
#[cfg_attr(feature = "std", derive(Debug, Decode))]
pub struct ValidationResult {
	/// New head data that should be included in the relay chain state.
	pub head_data: Vec<u8>,
}

/// Unique identifier of a parachain.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize, Debug))]
pub struct Id(u32);

impl From<Id> for u32 {
	fn from(x: Id) -> Self { x.0 }
}

impl From<u32> for Id {
	fn from(x: u32) -> Self { Id(x) }
}

impl Id {
	/// Convert this Id into its inner representation.
	pub fn into_inner(self) -> u32 {
		self.0
	}
}

/// An incoming message.
#[derive(PartialEq, Eq, Decode)]
#[cfg_attr(feature = "std", derive(Debug, Encode))]
pub struct IncomingMessage {
	/// The source parachain.
	pub source: Id,
	/// The data of the message.
	pub data: Vec<u8>,
}

/// A reference to a message.
pub struct MessageRef<'a> {
	/// The target parachain.
	pub target: Id,
	/// Underlying data of the message.
	pub data: &'a [u8],
}
