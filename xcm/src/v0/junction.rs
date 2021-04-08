// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Support datastructures for `MultiLocation`, primarily the `Junction` datatype.

use alloc::vec::Vec;
use parity_scale_codec::{self, Encode, Decode};

/// A global identifier of an account-bearing consensus system.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug)]
pub enum NetworkId {
	/// Unidentified/any.
	Any,
	/// Some named network.
	Named(Vec<u8>),
	/// The Polkadot Relay chain
	Polkadot,
	/// Kusama.
	Kusama,
}

/// A single item in a path to describe the relative location of a consensus system.
///
/// Each item assumes a pre-existing location as its context and is defined in terms of it.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug)]
pub enum Junction {
	/// The consensus system of which the context is a member and state-wise super-set.
	///
	/// NOTE: This item is *not* a sub-consensus item: a consensus system may not identify itself trustlessly as
	/// a location that includes this junction.
	Parent,
	/// An indexed parachain belonging to and operated by the context.
	///
	/// Generally used when the context is a Polkadot Relay-chain.
	///
	/// There is also `Parachain` which can be used in tests to avoid the faffy `{ id: ... }` syntax. Production
	/// code should use this.
	// TODO: parity-scale-codec#262: Change to be `Parachain(#[codec(compact)] u32)`
	Parachain { #[codec(compact)] id: u32 },
	/// A 32-byte identifier for an account of a specific network that is respected as a sovereign endpoint within
	/// the context.
	///
	/// Generally used when the context is a Substrate-based chain.
	AccountId32 { network: NetworkId, id: [u8; 32] },
	/// An 8-byte index for an account of a specific network that is respected as a sovereign endpoint within
	/// the context.
	///
	/// May be used when the context is a Frame-based chain and includes e.g. an indices pallet.
	AccountIndex64 { network: NetworkId, #[codec(compact)] index: u64 },
	/// A 20-byte identifier for an account of a specific network that is respected as a sovereign endpoint within
	/// the context.
	///
	/// May be used when the context is an Ethereum or Bitcoin chain or smart-contract.
	AccountKey20 { network: NetworkId, key: [u8; 20] },
	/// An instanced, indexed pallet that forms a constituent part of the context.
	///
	/// Generally used when the context is a Frame-based chain.
	PalletInstance(u8),
	/// A non-descript index within the context location.
	///
	/// Usage will vary widely owing to its generality.
	///
	/// NOTE: Try to avoid using this and instead use a more specific item.
	GeneralIndex { #[codec(compact)] id: u128 },
	/// A nondescript datum acting as a key within the context location.
	///
	/// Usage will vary widely owing to its generality.
	///
	/// NOTE: Try to avoid using this and instead use a more specific item.
	GeneralKey(Vec<u8>),
	/// The unambiguous child.
	///
	/// Not currently used except as a fallback when deriving ancestry.
	OnlyChild,
}

impl Junction {
	pub fn is_sub_consensus(&self) -> bool {
		match self {
			Junction::Parent => false,

			Junction::Parachain { .. } |
			Junction::AccountId32 { .. } |
			Junction::AccountIndex64 { .. } |
			Junction::AccountKey20 { .. } |
			Junction::PalletInstance { .. } |
			Junction::GeneralIndex { .. } |
			Junction::GeneralKey(..) |
			Junction::OnlyChild => true,
		}
	}
}
