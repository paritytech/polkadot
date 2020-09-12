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

//! Cross-Consensus Message format data structures.

use sp_std::{result, vec::Vec, convert::TryFrom};
use sp_runtime::RuntimeDebug;
use codec::{self, Encode, Decode};
use super::{MultiLocation, VersionedMultiAsset};

/// A general identifier for an instance of a non-fungible asset class.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug)]
pub enum AssetInstance {
	/// Undefined - used if the NFA class has only one instance.
	Undefined,

	/// A compact index. Technically this could be greater than u128, but this implementation supports only
	/// values up to `2**128 - 1`.
	Index { #[codec(compact)] id: u128 },

	/// A 4-byte fixed-length datum.
	Array4([u8; 4]),

	/// An 8-byte fixed-length datum.
	Array8([u8; 8]),

	/// A 16-byte fixed-length datum.
	Array16([u8; 16]),

	/// A 32-byte fixed-length datum.
	Array32([u8; 32]),

	/// An arbitrary piece of data. Use only when necessary.
	Blob(Vec<u8>),
}

/// A single general identifier for an asset.
///
/// Represents both fungible and non-fungible assets. May only be used to represent a single asset class.
///
/// Wildcards may or may not be allowed by the interpreting context.
///
/// Assets classes may be identified in one of two ways: either an abstract identifier or a concrete identifier.
/// Implementations may support only one of these.
///
/// Abstract identifiers are absolute identifiers that represent a notional asset which can exist within multiple
/// consensus systems. These tend to be simpler to deal with (since they stay the same between consensus systems).
/// However, in the attempt to provide uniformity across consensus systems, they may conflate different instantiations
/// of some notional asset (e.g. the reserve asset and a local reserve-backed derivative of it) under the same name,
/// leading to confusion. It also implies that one notional asset is accounted for locally in only one way. This may
/// not be the case, e.g. where there are multiple bridge instances each providing a bridged "BTC" token yet none
/// being fungible between the others.
///
/// Concrete identifiers are relative identifiers that specifically identify a single asset through its location in
/// a consensus system relative to the context interpreting. Use of a `MultiLocation` ensures that similar but non
/// fungible variants of the same underlying asset can be properly distinguished, and obviates the need for any kind
/// of central registry. The limitation is that the asset identifier cannot be trivially copied between consensus
/// systems and must instead be "reanchored" whenever being moved to a new consensus system, using the two systems'
/// relative paths.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug)]
pub enum MultiAsset {
	/// No assets. Rarely used.
	None,

	/// All assets. Typically used for the subset of assets to be used for an `Order`, and in that context means
	/// "all assets currently in holding".
	All,

	/// All fungible assets. Typically used for the subset of assets to be used for an `Order`, and in that context
	/// means "all fungible assets currently in holding".
	AllFungible,

	/// All non-fungible assets. Typically used for the subset of assets to be used for an `Order`, and in that
	/// context means "all non-fungible assets currently in holding".
	AllNonFungible,

	/// All fungible assets of a given abstract asset `id`entifier.
	AllAbstractFungible { id: Vec<u8> },

	/// All non-fungible assets of a given abstract asset `class`.
	AllAbstractNonFungible { class: Vec<u8> },

	/// All fungible assets of a given concrete asset `id`entifier.
	AllConcreteFungible { id: MultiLocation },

	/// All non-fungible assets of a given concrete asset `class`.
	AllConcreteNonFungible { class: MultiLocation },

	/// Some specific `amount` of the fungible asset identified by an abstract `id`.
	AbstractFungible { id: Vec<u8>, #[codec(compact)] amount: u128 },

	/// Some specific `instance` of the non-fungible asset whose `class` is identified abstractly.
	AbstractNonFungible { class: Vec<u8>, instance: AssetInstance },

	/// Some specific `amount` of the fungible asset identified by an concrete `id`.
	ConcreteFungible { id: MultiLocation, #[codec(compact)] amount: u128 },

	/// Some specific `instance` of the non-fungible asset whose `class` is identified concretely.
	ConcreteNonFungible { class: MultiLocation, instance: AssetInstance },
}

impl From<MultiAsset> for VersionedMultiAsset {
	fn from(x: MultiAsset) -> Self {
		VersionedMultiAsset::V0(x)
	}
}

impl TryFrom<VersionedMultiAsset> for MultiAsset {
	type Error = ();
	fn try_from(x: VersionedMultiAsset) -> result::Result<Self, ()> {
		match x {
			VersionedMultiAsset::V0(x) => Ok(x),
		}
	}
}
