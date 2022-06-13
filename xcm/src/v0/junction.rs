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

//! Support data structures for `MultiLocation`, primarily the `Junction` datatype.

use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::{traits::ConstU32, WeakBoundedVec};

/// A global identifier of an account-bearing consensus system.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub enum NetworkId {
	/// Unidentified/any.
	Any,
	/// Some named network.
	Named(WeakBoundedVec<u8, ConstU32<32>>),
	/// The Polkadot Relay chain
	Polkadot,
	/// Kusama.
	Kusama,
}

/// An identifier of a pluralistic body.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub enum BodyId {
	/// The only body in its context.
	Unit,
	/// A named body.
	Named(WeakBoundedVec<u8, ConstU32<32>>),
	/// An indexed body.
	Index(#[codec(compact)] u32),
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

/// A part of a pluralistic body.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub enum BodyPart {
	/// The body's declaration, under whatever means it decides.
	Voice,
	/// A given number of members of the body.
	Members {
		#[codec(compact)]
		count: u32,
	},
	/// A given number of members of the body, out of some larger caucus.
	Fraction {
		#[codec(compact)]
		nom: u32,
		#[codec(compact)]
		denom: u32,
	},
	/// No less than the given proportion of members of the body.
	AtLeastProportion {
		#[codec(compact)]
		nom: u32,
		#[codec(compact)]
		denom: u32,
	},
	/// More than than the given proportion of members of the body.
	MoreThanProportion {
		#[codec(compact)]
		nom: u32,
		#[codec(compact)]
		denom: u32,
	},
}

impl BodyPart {
	/// Returns `true` if the part represents a strict majority (> 50%) of the body in question.
	pub fn is_majority(&self) -> bool {
		match self {
			BodyPart::Fraction { nom, denom } if *nom * 2 > *denom => true,
			BodyPart::AtLeastProportion { nom, denom } if *nom * 2 > *denom => true,
			BodyPart::MoreThanProportion { nom, denom } if *nom * 2 >= *denom => true,
			_ => false,
		}
	}
}

/// A single item in a path to describe the relative location of a consensus system.
///
/// Each item assumes a pre-existing location as its context and is defined in terms of it.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
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
	GeneralIndex(#[codec(compact)] u128),
	/// A nondescript datum acting as a key within the context location.
	///
	/// Usage will vary widely owing to its generality.
	///
	/// NOTE: Try to avoid using this and instead use a more specific item.
	GeneralKey(WeakBoundedVec<u8, ConstU32<32>>),
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

impl From<crate::v1::Junction> for Junction {
	fn from(v1: crate::v1::Junction) -> Junction {
		use crate::v1::Junction::*;
		match v1 {
			Parachain(id) => Self::Parachain(id),
			AccountId32 { network, id } => Self::AccountId32 { network, id },
			AccountIndex64 { network, index } => Self::AccountIndex64 { network, index },
			AccountKey20 { network, key } => Self::AccountKey20 { network, key },
			PalletInstance(index) => Self::PalletInstance(index),
			GeneralIndex(index) => Self::GeneralIndex(index),
			GeneralKey(key) => Self::GeneralKey(key),
			OnlyChild => Self::OnlyChild,
			Plurality { id, part } => Self::Plurality { id, part },
		}
	}
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
