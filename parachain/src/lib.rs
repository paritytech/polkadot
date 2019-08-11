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
pub use codec;

#[cfg(feature = "std")]
pub mod wasm_executor;

#[cfg(feature = "wasm-api")]
pub mod wasm_api;

use rstd::vec::Vec;

use codec::{Encode, Decode};

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
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Default, Clone, Copy, Encode, Decode)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize, Debug))]
pub struct Id(u32);

impl codec::CompactAs for Id {
	type As = u32;
	fn encode_as(&self) -> &u32 {
		&self.0
	}
	fn decode_from(x: u32) -> Self {
		Self(x)
	}
}
impl From<codec::Compact<Id>> for Id {
	fn from(x: codec::Compact<Id>) -> Id {
		x.0
	}
}

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

// TODO: Remove all of this, move sr-primitives::AccountIdConversion to own crate and and use that.
// #360
struct TrailingZeroInput<'a>(&'a [u8]);
impl<'a> codec::Input for TrailingZeroInput<'a> {
	fn remaining_len(&mut self) -> Result<Option<usize>, codec::Error> {
		Ok(None)
	}

	fn read(&mut self, into: &mut [u8]) -> Result<(), codec::Error> {
		let len = into.len().min(self.0.len());
		into[..len].copy_from_slice(&self.0[..len]);
		for i in &mut into[len..] {
			*i = 0;
		}
		self.0 = &self.0[len..];
		Ok(())
	}
}

/// This type can be converted into and possibly from an AccountId (which itself is generic).
pub trait AccountIdConversion<AccountId>: Sized {
	/// Convert into an account ID. This is infallible.
	fn into_account(&self) -> AccountId;

 	/// Try to convert an account ID into this type. Might not succeed.
	fn try_from_account(a: &AccountId) -> Option<Self>;
}

 /// Format is b"para" ++ encode(parachain ID) ++ 00.... where 00... is indefinite trailing zeroes to fill AccountId.
impl<T: Encode + Decode + Default> AccountIdConversion<T> for Id {
	fn into_account(&self) -> T {
		(b"para", self).using_encoded(|b|
			T::decode(&mut TrailingZeroInput(b))
		).unwrap_or_default()
	}

 	fn try_from_account(x: &T) -> Option<Self> {
		x.using_encoded(|d| {
			if &d[0..4] != b"para" { return None }
			let mut cursor = &d[4..];
			let result = Decode::decode(&mut cursor).ok()?;
			if cursor.iter().all(|x| *x == 0) {
				Some(result)
			} else {
				None
			}
		})
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

/// Which origin a parachain's message to the relay chain should be dispatched from.
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
#[repr(u8)]
pub enum ParachainDispatchOrigin {
	/// As a simple `Origin::Signed`, using `ParaId::account_id` as its value. This is good when
	/// interacting with standard modules such as `balances`.
	Signed,
	/// As the special `Origin::Parachain(ParaId)`. This is good when interacting with parachain-
	/// aware modules which need to succinctly verify that the origin is a parachain.
	Parachain,
}

impl rstd::convert::TryFrom<u8> for ParachainDispatchOrigin {
	type Error = ();
	fn try_from(x: u8) -> core::result::Result<ParachainDispatchOrigin, ()> {
		const SIGNED: u8 = ParachainDispatchOrigin::Signed as u8;
		const PARACHAIN: u8 = ParachainDispatchOrigin::Parachain as u8;
		Ok(match x {
			SIGNED => ParachainDispatchOrigin::Signed,
			PARACHAIN => ParachainDispatchOrigin::Parachain,
			_ => return Err(()),
		})
	}
}

/// A reference to an upward message.
pub struct UpwardMessageRef<'a> {
	/// The origin type.
	pub origin: ParachainDispatchOrigin,
	/// Underlying data of the message.
	pub data: &'a [u8],
}

/// A message from a parachain to its Relay Chain.
#[derive(Clone, PartialEq, Eq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct UpwardMessage {
	/// The origin for the message to be sent from.
	pub origin: ParachainDispatchOrigin,
	/// The message data.
	pub data: Vec<u8>,
}
