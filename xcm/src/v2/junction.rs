// Copyright (C) Parity Technologies (UK) Ltd.
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

use super::{BodyId, BodyPart, Junctions, MultiLocation, NetworkId};
use crate::v3::Junction as NewJunction;
use bounded_collections::{ConstU32, WeakBoundedVec};
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

/// A single item in a path to describe the relative location of a consensus system.
///
/// Each item assumes a pre-existing location as its context and is defined in terms of it.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub enum Junction {
	/// An indexed parachain belonging to and operated by the context.
	///
	/// Generally used when the context is a Polkadot Relay-chain.
	Parachain(#[codec(compact)] u32),
	/// A 32-byte identifier for an account of a specific network that is respected as a sovereign
	/// endpoint within the context.
	///
	/// Generally used when the context is a Substrate-based chain.
	AccountId32 { network: NetworkId, id: [u8; 32] },
	/// An 8-byte index for an account of a specific network that is respected as a sovereign
	/// endpoint within the context.
	///
	/// May be used when the context is a Frame-based chain and includes e.g. an indices pallet.
	AccountIndex64 {
		network: NetworkId,
		#[codec(compact)]
		index: u64,
	},
	/// A 20-byte identifier for an account of a specific network that is respected as a sovereign
	/// endpoint within the context.
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
	/// Typical to be used to represent a governance origin of a chain, but could in principle be
	/// used to represent things such as multisigs also.
	Plurality { id: BodyId, part: BodyPart },
}

impl TryFrom<NewJunction> for Junction {
	type Error = ();

	fn try_from(value: NewJunction) -> Result<Self, Self::Error> {
		use NewJunction::*;
		Ok(match value {
			Parachain(id) => Self::Parachain(id),
			AccountId32 { network, id } => Self::AccountId32 { network: network.try_into()?, id },
			AccountIndex64 { network, index } =>
				Self::AccountIndex64 { network: network.try_into()?, index },
			AccountKey20 { network, key } =>
				Self::AccountKey20 { network: network.try_into()?, key },
			PalletInstance(index) => Self::PalletInstance(index),
			GeneralIndex(id) => Self::GeneralIndex(id),
			GeneralKey { length, data } => Self::GeneralKey(
				data[0..data.len().min(length as usize)]
					.to_vec()
					.try_into()
					.expect("key is bounded to 32 and so will never be out of bounds; qed"),
			),
			OnlyChild => Self::OnlyChild,
			Plurality { id, part } => Self::Plurality { id: id.into(), part: part.into() },
			_ => return Err(()),
		})
	}
}

impl Junction {
	/// Convert `self` into a `MultiLocation` containing 0 parents.
	///
	/// Similar to `Into::into`, except that this method can be used in a const evaluation context.
	pub const fn into(self) -> MultiLocation {
		MultiLocation { parents: 0, interior: Junctions::X1(self) }
	}

	/// Convert `self` into a `MultiLocation` containing `n` parents.
	///
	/// Similar to `Self::into`, with the added ability to specify the number of parent junctions.
	pub const fn into_exterior(self, n: u8) -> MultiLocation {
		MultiLocation { parents: n, interior: Junctions::X1(self) }
	}
}
