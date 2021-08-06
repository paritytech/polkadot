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

//! Support data structures for `MultiLocation`, primarily the `Junction` datatype.

use crate::v1::{BodyId as BodyId1, BodyPart, Junction as Junction1, NetworkId};
use alloc::vec::Vec;
use parity_scale_codec::{self, Decode, Encode};

/// An identifier of a pluralistic body.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug)]
pub enum BodyId {
	/// The only body in its context.
	Unit,
	/// A named body.
	Named(Vec<u8>),
	/// An indexed body.
	// TODO: parity-scale-codec#262: Change to be a tuple.
	Index {
		#[codec(compact)]
		id: u32,
	},
	/// The unambiguous executive body (for Polkadot, this would be the Polkadot council).
	Executive,
	/// The unambiguous technical body (for Polkadot, this would be the Technical Committee).
	Technical,
	/// The unambiguous legislative body (for Polkadot, this could be considered the opinion of a majority of
	/// lock-voters).
	Legislative,
	/// The unambiguous judicial body (this doesn't exist on Polkadot, but if it were to get a "grand oracle", it
	/// may be considered as that).
	Judicial,
}

impl From<BodyId1> for BodyId {
	fn from(v1: BodyId1) -> Self {
		use BodyId1::*;
		match v1 {
			Unit => Self::Unit,
			Named(name) => Self::Named(name),
			Index(id) => Self::Index { id },
			Executive => Self::Executive,
			Technical => Self::Technical,
			Legislative => Self::Legislative,
			Judicial => Self::Judicial,
		}
	}
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
	Parachain(#[codec(compact)] u32),
	/// A 32-byte identifier for an account of a specific network that is respected as a sovereign endpoint within
	/// the context.
	///
	/// Generally used when the context is a Substrate-based chain.
	AccountId32 { network: NetworkId, id: [u8; 32] },
	/// An 8-byte index for an account of a specific network that is respected as a sovereign endpoint within
	/// the context.
	///
	/// May be used when the context is a Frame-based chain and includes e.g. an indices pallet.
	AccountIndex64 {
		network: NetworkId,
		#[codec(compact)]
		index: u64,
	},
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
	GeneralIndex {
		#[codec(compact)]
		id: u128,
	},
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
	/// A pluralistic body existing within consensus.
	///
	/// Typical to be used to represent a governance origin of a chain, but could in principle be used to represent
	/// things such as multisigs also.
	Plurality { id: BodyId, part: BodyPart },
}

impl Junction {
	/// Returns true if this junction is a `Parent` item.
	pub fn is_parent(&self) -> bool {
		match self {
			Junction::Parent => true,
			_ => false,
		}
	}

	/// Returns true if this junction can be considered an interior part of its context. This is generally `true`,
	/// except for the `Parent` item.
	pub fn is_interior(&self) -> bool {
		match self {
			Junction::Parent => false,

			Junction::Parachain(..) |
			Junction::AccountId32 { .. } |
			Junction::AccountIndex64 { .. } |
			Junction::AccountKey20 { .. } |
			Junction::PalletInstance { .. } |
			Junction::GeneralIndex { .. } |
			Junction::GeneralKey(..) |
			Junction::OnlyChild |
			Junction::Plurality { .. } => true,
		}
	}
}

impl From<Junction1> for Junction {
	fn from(v1: Junction1) -> Self {
		match v1 {
			Junction1::Parachain(id) => Self::Parachain(id),
			Junction1::AccountId32 { network, id } => Self::AccountId32 { network, id },
			Junction1::AccountIndex64 { network, index } => Self::AccountIndex64 { network, index },
			Junction1::AccountKey20 { network, key } => Self::AccountKey20 { network, key },
			Junction1::PalletInstance(index) => Self::PalletInstance(index),
			Junction1::GeneralIndex { id } => Self::GeneralIndex { id },
			Junction1::GeneralKey(key) => Self::GeneralKey(key),
			Junction1::OnlyChild => Self::OnlyChild,
			Junction1::Plurality { id, part } => Self::Plurality { id: id.into(), part },
		}
	}
}
