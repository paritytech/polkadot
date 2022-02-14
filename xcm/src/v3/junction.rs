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

use super::{BodyId, BodyPart, Junctions, MultiLocation};
use crate::{
	v2::{Junction as OldJunction, NetworkId as OldNetworkId},
	VersionedMultiLocation,
};
use alloc::vec::Vec;
use core::convert::TryFrom;
use parity_scale_codec::{self, Decode, Encode};
use scale_info::TypeInfo;

/// A global identifier of a data structure existing within consensus.
///
/// Maintenance note: Networks with global consensus and which are practically bridgeable within the
/// Polkadot ecosystem are given preference over explicit naming in this enumeration.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo)]
pub enum NetworkId {
	/// Network specified by the first 32 bytes of its genesis block.
	ByGenesis([u8; 32]),
	/// Network defined by the first 32-bytes of the hash and number of some block it contains.
	ByFork { block_number: u64, block_hash: [u8; 32] },
	/// The Polkadot mainnet Relay-chain.
	Polkadot,
	/// The Kusama canary-net Relay-chain.
	Kusama,
	/// The Westend testnet Relay-chain.
	Westend,
	/// The Rococo testnet Relay-chain.
	Rococo,
	/// The Wococo testnet Relay-chain.
	Wococo,
	/// The Ethereum network, including hard-forks supported by the Etheruem Foundation.
	EthereumFoundation,
	/// The Ethereum network, including hard-forks supported by Ethereum Classic developers.
	EthereumClassic,
	/// The Bitcoin network, including hard-forks supported by Bitcoin Core development team.
	BitcoinCore,
	/// The Bitcoin network, including hard-forks supported by Bitcoin Cash developers.
	BitcoinCash,
}

impl From<OldNetworkId> for Option<NetworkId> {
	fn from(old: OldNetworkId) -> Option<NetworkId> {
		use OldNetworkId::*;
		match old {
			Any => None,
			Named(_) => None,
			Polkadot => Some(NetworkId::Polkadot),
			Kusama => Some(NetworkId::Kusama),
		}
	}
}

/// A single item in a path to describe the relative location of a consensus system.
///
/// Each item assumes a pre-existing location as its context and is defined in terms of it.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, Debug, TypeInfo)]
pub enum Junction {
	/// An indexed parachain belonging to and operated by the context.
	///
	/// Generally used when the context is a Polkadot Relay-chain.
	Parachain(#[codec(compact)] u32),
	/// A 32-byte identifier for an account of a specific network that is respected as a sovereign endpoint within
	/// the context.
	///
	/// Generally used when the context is a Substrate-based chain.
	AccountId32 { network: Option<NetworkId>, id: [u8; 32] },
	/// An 8-byte index for an account of a specific network that is respected as a sovereign endpoint within
	/// the context.
	///
	/// May be used when the context is a Frame-based chain and includes e.g. an indices pallet.
	AccountIndex64 {
		network: Option<NetworkId>,
		#[codec(compact)]
		index: u64,
	},
	/// A 20-byte identifier for an account of a specific network that is respected as a sovereign endpoint within
	/// the context.
	///
	/// May be used when the context is an Ethereum or Bitcoin chain or smart-contract.
	AccountKey20 { network: Option<NetworkId>, key: [u8; 20] },
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
	GeneralKey(Vec<u8>),
	/// The unambiguous child.
	///
	/// Not currently used except as a fallback when deriving context.
	OnlyChild,
	/// A pluralistic body existing within consensus.
	///
	/// Typical to be used to represent a governance origin of a chain, but could in principle be used to represent
	/// things such as multisigs also.
	Plurality { id: BodyId, part: BodyPart },
	/// A global network capable of externalizing its own consensus. This is not generally
	/// meaningful outside of the universal level.
	GlobalConsensus(NetworkId),
}

impl From<NetworkId> for Junction {
	fn from(n: NetworkId) -> Self {
		Self::GlobalConsensus(n)
	}
}

impl From<[u8; 32]> for Junction {
	fn from(id: [u8; 32]) -> Self {
		Self::AccountId32 { network: None, id }
	}
}

impl From<[u8; 20]> for Junction {
	fn from(key: [u8; 20]) -> Self {
		Self::AccountKey20 { network: None, key }
	}
}

impl From<u64> for Junction {
	fn from(index: u64) -> Self {
		Self::AccountIndex64 { network: None, index }
	}
}

impl From<u128> for Junction {
	fn from(id: u128) -> Self {
		Self::GeneralIndex(id)
	}
}

impl From<Vec<u8>> for Junction {
	fn from(id: Vec<u8>) -> Self {
		Self::GeneralKey(id)
	}
}

impl TryFrom<OldJunction> for Junction {
	type Error = ();
	fn try_from(value: OldJunction) -> Result<Self, ()> {
		use OldJunction::*;
		Ok(match value {
			Parachain(id) => Self::Parachain(id),
			AccountId32 { network, id } => Self::AccountId32 { network: network.into(), id },
			AccountIndex64 { network, index } =>
				Self::AccountIndex64 { network: network.into(), index },
			AccountKey20 { network, key } => Self::AccountKey20 { network: network.into(), key },
			PalletInstance(index) => Self::PalletInstance(index),
			GeneralIndex(id) => Self::GeneralIndex(id),
			GeneralKey(key) => Self::GeneralKey(key),
			OnlyChild => Self::OnlyChild,
			Plurality { id, part } => Self::Plurality { id: id.into(), part },
		})
	}
}

impl Junction {
	/// Convert `self` into a `MultiLocation` containing 0 parents.
	///
	/// Similar to `Into::into`, except that this method can be used in a const evaluation context.
	pub const fn into_location(self) -> MultiLocation {
		MultiLocation { parents: 0, interior: Junctions::X1(self) }
	}

	/// Convert `self` into a `MultiLocation` containing `n` parents.
	///
	/// Similar to `Self::into_location`, with the added ability to specify the number of parent junctions.
	pub const fn into_exterior(self, n: u8) -> MultiLocation {
		MultiLocation { parents: n, interior: Junctions::X1(self) }
	}

	/// Convert `self` into a `VersionedMultiLocation` containing 0 parents.
	///
	/// Similar to `Into::into`, except that this method can be used in a const evaluation context.
	pub const fn into_versioned(self) -> VersionedMultiLocation {
		self.into_location().into_versioned()
	}
}
