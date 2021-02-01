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
pub mod primitives;

mod wasm_api;

#[cfg(all(not(feature = "std"), feature = "wasm-api"))]
pub use wasm_api::*;

use sp_std::{vec::Vec, any::{TypeId, Any}, boxed::Box};
use crate::primitives::{ValidationParams, ValidationResult, ValidationCode};

use sp_runtime_interface::runtime_interface;

#[runtime_interface]
pub trait Validation {
	/// TODO
	fn validation_code(&mut self) -> Vec<u8> {
		use sp_externalities::ExternalitiesExt;
		let extension = self.extension::<ValidationExt>()
			.expect("Cannot set capacity without dynamic runtime dispatcher (RuntimeSpawnExt)");
		extension.validation_code()
	}
}

#[cfg(feature = "std")]
sp_externalities::decl_extension! {
	///  executor extension.
	pub struct ValidationExt(Box<dyn Validation>);
}

#[cfg(feature = "std")]
impl ValidationExt {
	/// New instance of task executor extension.
	pub fn new(validation_ext: impl Validation) -> Self {
		Self(Box::new(validation_ext))
	}
}

/// Base methods to implement validation extension.
pub trait Validation: Send + 'static {
	/// Get the validation code currently running.
	/// This can be use to check validity or to complete
	/// proofs.
	fn validation_code(&self) -> Vec<u8>;
}

#[cfg(feature = "std")]
impl Validation for std::sync::Arc<ValidationCode> {
	fn validation_code(&self) -> Vec<u8> {
		self.0.clone()
	}
}

impl Validation for &'static [u8] {
	fn validation_code(&self) -> Vec<u8> {
		self.to_vec()
	}
}
