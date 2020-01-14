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

use rstd::{vec::Vec, cmp::Ordering};

use codec::{Encode, Decode, CompactAs};
use sp_core::{RuntimeDebug, TypeId};

#[cfg(all(not(feature = "std"), feature = "wasm-api"))]
pub use wasm_api::*;

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
#[derive(
	Clone, CompactAs, Copy, Decode, Default, Encode, Eq,
	Hash, Ord, PartialEq, PartialOrd, RuntimeDebug
)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct Id(u32);

impl TypeId for Id {
	const TYPE_ID: [u8; 4] = *b"para";
}

/// Type for determining the active set of parachains.
pub trait ActiveThreads {
	/// Return the current ordered set of `Id`s of active parathreads.
	fn active_threads() -> Vec<Id>;
}

impl From<Id> for u32 {
	fn from(x: Id) -> Self { x.0 }
}

impl From<u32> for Id {
	fn from(x: u32) -> Self { Id(x) }
}

const USER_INDEX_START: u32 = 1000;

/// The ID of the first user (non-system) parachain.
pub const LOWEST_USER_ID: Id = Id(USER_INDEX_START);

impl Id {
	/// Create an `Id`.
	pub const fn new(id: u32) -> Self {
		Self(id)
	}

	/// Returns `true` if this parachain runs with system-level privileges.
	pub fn is_system(&self) -> bool { self.0 < USER_INDEX_START }
}

impl rstd::ops::Add<u32> for Id {
	type Output = Self;

	fn add(self, other: u32) -> Self {
		Self(self.0 + other)
	}
}

// TODO: Remove all of this, move sp-runtime::AccountIdConversion to own crate and and use that.
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

/// Format is b"para" ++ encode(parachain ID) ++ 00.... where 00... is indefinite trailing
/// zeroes to fill AccountId.
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
	/// As the simple, superuser `Origin::Root`. This can only be done on specially permissioned
	/// parachains.
	Root,
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

/// A message from a parachain to its Relay Chain.
#[derive(Clone, PartialEq, Eq, Encode, Decode, sp_runtime_interface::pass_by::PassByCodec)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct UpwardMessage {
	/// The origin for the message to be sent from.
	pub origin: ParachainDispatchOrigin,
	/// The message data.
	pub data: Vec<u8>,
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

/// A message targeted to a specific parachain.
#[derive(Clone, PartialEq, Eq, Encode, Decode, sp_runtime_interface::pass_by::PassByCodec)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize, Debug))]
#[cfg_attr(feature = "std", serde(rename_all = "camelCase"))]
#[cfg_attr(feature = "std", serde(deny_unknown_fields))]
pub struct TargetedMessage {
	/// The target parachain.
	pub target: Id,
	/// The message data.
	pub data: Vec<u8>,
}

impl AsRef<[u8]> for TargetedMessage {
	fn as_ref(&self) -> &[u8] {
		&self.data[..]
	}
}

impl PartialOrd for TargetedMessage {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.target.cmp(&other.target))
	}
}

impl Ord for TargetedMessage {
	fn cmp(&self, other: &Self) -> Ordering {
		self.target.cmp(&other.target)
	}
}
