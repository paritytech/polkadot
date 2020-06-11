// Copyright 2020 Parity Technologies (UK) Ltd.
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

#![cfg_attr(not(feature = "std"), no_std)]

//! Core Polkadot types.
//!
//! These core Polkadot types are used by the relay chain and the Parachains.

use sp_runtime::{generic, MultiSignature, traits::{Verify, BlakeTwo256, IdentifyAccount}};

/// The block number type used by Polkadot.
/// 32-bits will allow for 136 years of blocks assuming 1 block per second.
pub type BlockNumber = u32;

/// An instant or duration in time.
pub type Moment = u64;

/// Alias to type for a signature for a transaction on the relay chain. This allows one of several
/// kinds of underlying crypto to be used, so isn't a fixed size when encoded.
pub type Signature = MultiSignature;

/// Alias to the public key used for this chain, actually a `MultiSigner`. Like the signature, this
/// also isn't a fixed size when encoded, as different cryptos have different size public keys.
pub type AccountPublic = <Signature as Verify>::Signer;

/// Alias to the opaque account ID type for this chain, actually a `AccountId32`. This is always
/// 32 bytes.
pub type AccountId = <AccountPublic as IdentifyAccount>::AccountId;

/// The type for looking up accounts. We don't expect more than 4 billion of them.
pub type AccountIndex = u32;

/// Identifier for a chain. 32-bit should be plenty.
pub type ChainId = u32;

/// A hash of some data used by the relay chain.
pub type Hash = sp_core::H256;

/// Index of a transaction in the relay chain. 32-bit should be plenty.
pub type Nonce = u32;

/// The balance of an account.
/// 128-bits (or 38 significant decimal figures) will allow for 10m currency (10^7) at a resolution
/// to all for one second's worth of an annualised 50% reward be paid to a unit holder (10^11 unit
/// denomination), or 10^18 total atomic units, to grow at 50%/year for 51 years (10^9 multiplier)
/// for an eventual total of 10^27 units (27 significant decimal figures).
/// We round denomination to 10^12 (12 sdf), and leave the other redundancy at the upper end so
/// that 32 bits may be multiplied with a balance in 128 bits without worrying about overflow.
pub type Balance = u128;

/// Header type.
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
/// Block type.
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
/// Block ID.
pub type BlockId = generic::BlockId<Block>;

/// Opaque, encoded, unchecked extrinsic.
pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;

/// The information that goes alongside a transfer_into_parachain operation. Entirely opaque, it
/// will generally be used for identifying the reason for the transfer. Typically it will hold the
/// destination account to which the transfer should be credited. If still more information is
/// needed, then this should be a hash with the pre-image presented via an off-chain mechanism on
/// the parachain.
pub type Remark = [u8; 32];

/// These are special "control" messages that can be passed from the Relaychain to a parachain.
/// They should be handled by all parachains.
#[derive(codec::Encode, codec::Decode, Clone, sp_runtime::RuntimeDebug, PartialEq)]
pub enum DownwardMessage<AccountId = crate::AccountId> {
	/// Some funds were transferred into the parachain's account. The hash is the identifier that
	/// was given with the transfer.
	TransferInto(AccountId, Balance, Remark),
	/// An opaque blob of data. The relay chain must somehow know how to form this so that the
	/// destination parachain does something sensible.
	///
	/// NOTE: Be very careful not to allow users to place arbitrary size information in here.
	Opaque(sp_std::vec::Vec<u8>),
	/// XCMP message for the Parachain.
	XCMPMessage(sp_std::vec::Vec<u8>),
}
