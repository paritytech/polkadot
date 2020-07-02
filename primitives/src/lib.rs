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

//! Polkadot types shared between the runtime and the Node-side code.

#![warn(missing_docs)]

#![cfg_attr(not(feature = "std"), no_std)]

pub use runtime_primitives::traits::{BlakeTwo256, Hash as HashT, Verify, IdentifyAccount};
pub use polkadot_core_primitives::*;

pub mod inclusion_inherent;
pub mod parachain;

pub use parity_scale_codec::Compact;

/// Custom validity errors used in Polkadot while validating transactions.
#[repr(u8)]
pub enum ValidityError {
	/// The Ethereum signature is invalid.
	InvalidEthereumSignature = 0,
	/// The signer has no claim.
	SignerHasNoClaim = 1,
	/// No permission to execute the call.
	NoPermission = 2,
	/// An invalid statement was made for a claim.
	InvalidStatement = 3,
}

impl From<ValidityError> for u8 {
	fn from(err: ValidityError) -> Self {
		err as u8
	}
}

/// App-specific crypto used for reporting equivocation/misbehavior in BABE,
/// GRANDPA and Parachains, described in the white paper as the fisherman role.
/// Any rewards for misbehavior reporting will be paid out to this account.
pub mod fisherman {
	use super::{Signature, Verify};
	use primitives::crypto::KeyTypeId;

	/// Key type for the reporting module. Used for reporting BABE, GRANDPA
	/// and Parachain equivocations.
	pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"fish");

	mod app {
		use application_crypto::{app_crypto, sr25519};
		app_crypto!(sr25519, super::KEY_TYPE);
	}

	/// Identity of the equivocation/misbehavior reporter.
	pub type FishermanId = app::Public;

	/// An `AppCrypto` type to allow submitting signed transactions using the fisherman
	/// application key as signer.
	pub struct FishermanAppCrypto;
	impl frame_system::offchain::AppCrypto<<Signature as Verify>::Signer, Signature> for FishermanAppCrypto {
		type RuntimeAppPublic = FishermanId;
		type GenericSignature = primitives::sr25519::Signature;
		type GenericPublic = primitives::sr25519::Public;
	}
}
