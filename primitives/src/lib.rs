// Copyright 2017-2019 Parity Technologies (UK) Ltd.
// This file is part of Parity Polkadot.

// Parity Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Shareable Polkadot types.

#![warn(missing_docs)]

#![cfg_attr(not(feature = "std"), no_std)]

use runtime_primitives::{generic, AnySignature};
use primitives::ed25519;

pub use runtime_primitives::traits::{BlakeTwo256, Hash as HashT, Verify};

pub mod parachain;

pub use parity_codec::Compact;

/// An index to a block.
/// 32-bits will allow for 136 years of blocks assuming 1 block per second.
/// TODO: switch to u32 (https://github.com/paritytech/polkadot/issues/221)
pub type BlockNumber = u64;

/// Alias to 512-bit hash when used in the context of a signature on the relay chain.
/// Equipped with logic for possibly "unsigned" messages.
pub type Signature = AnySignature;

/// Alias to an sr25519 or ed25519 key.
pub type AccountId = <Signature as Verify>::Signer;

/// The type for looking up accounts. We don't expect more than 4 billion of them, but you
/// never know...
pub type AccountIndex = u32;

/// Identifier for a chain. 32-bit should be plenty.
pub type ChainId = u32;

/// A hash of some data used by the relay chain.
pub type Hash = primitives::H256;

/// Index of a transaction in the relay chain. 32-bit should be plenty.
pub type Nonce = u64;

/// Signature with which authorities sign blocks.
pub type SessionSignature = ed25519::Signature;

/// Identity that authorities use.
pub type SessionKey = ed25519::Public;

/// The balance of an account.
/// 128-bits (or 38 significant decimal figures) will allow for 10m currency (10^7) at a resolution
/// to all for one second's worth of an annualised 50% reward be paid to a unit holder (10^11 unit
/// denomination), or 10^18 total atomic units, to grow at 50%/year for 51 years (10^9 multiplier)
/// for an eventual total of 10^27 units (27 significant decimal figures).
/// We round denomination to 10^12 (12 sdf), and leave the other redundancy at the upper end so
/// that 32 bits may be multiplied with a balance in 128 bits without worrying about overflow.
pub type Balance = u128;

/// The aura crypto scheme defined via the keypair type.
#[cfg(feature = "std")]
pub type AuraPair = ed25519::Pair;

/// The Ed25519 pub key of an session that belongs to an Aura authority of the chain.
pub type AuraId = ed25519::Public;

/// Header type.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
/// Block type.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// Block ID.
pub type BlockId = generic::BlockId<Block>;

/// Opaque, encoded, unchecked extrinsic.
pub use runtime_primitives::OpaqueExtrinsic as UncheckedExtrinsic;
