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

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug)]
pub enum AssetInstance {
	Undefined,
	Index8(u8),
	Index16 { #[codec(compact)] id: u16 },
	Index32 { #[codec(compact)] id: u32 },
	Index64 { #[codec(compact)] id: u64 },
	Index128 { #[codec(compact)] id: u128 },
	Array4([u8; 4]),
	Array8([u8; 8]),
	Array16([u8; 16]),
	Array32([u8; 32]),
	Blob(Vec<u8>),
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode, RuntimeDebug)]
pub enum MultiAsset {
	None,
	All,
	AllFungible,
	AllNonFungible,
	AllAbstractFungible { id: Vec<u8> },
	AllAbstractNonFungible { class: Vec<u8> },
	AllConcreteFungible { id: MultiLocation },
	AllConcreteNonFungible { class: MultiLocation },
	AbstractFungible { id: Vec<u8>, #[codec(compact)] amount: u128 },
	AbstractNonFungible { class: Vec<u8>, instance: AssetInstance },
	ConcreteFungible { id: MultiLocation, #[codec(compact)] amount: u128 },
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
