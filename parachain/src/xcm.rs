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

use sp_std::{boxed::Box, vec::Vec, convert::TryFrom};
use sp_runtime::RuntimeDebug;
use codec::{self, Encode, Decode, Input, Output};
use crate::primitives::ParachainDispatchOrigin;

/// A single XCM message, together with its version code.
#[derive(Clone, Eq, PartialEq, Encode, Decode)]
pub enum VersionedXcm {
	V0(v0::Xcm),
}

/// A versioned multi-location, a relative location of a cross-consensus system identifier.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum VersionedMultiLocation {
	V0(v0::MultiLocation),
}

/// A versioned multi-asset, an identifier for an asset within a consensus system.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum VersionedMultiAsset {
	V0(v0::MultiAsset),
}

pub mod v0 {
	use super::*;

	pub type XcmError = ();
	pub type XcmResult = Result<(), XcmError>;

	pub trait ExecuteXcm {
		fn execute_xcm(origin: MultiLocation, msg: Xcm) -> XcmResult;
	}

	pub trait SendXcm {
		fn send_xcm(dest: MultiLocation, msg: Xcm) -> XcmResult;
	}

	/// Basically just the XCM (more general) version of `ParachainDispatchOrigin`.
	#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
	pub enum MultiOrigin {
		/// Origin should just be the native origin for the sender. For Cumulus/Frame chains this is
		/// the `Parachain` origin.
		Native,
		/// Origin should just be the standard account-based origin with the sovereign account of
		/// the sender. For Cumulus/Frame chains, this is the `Signed` origin.
		SovereignAccount,
		/// Origin should be the super-user. For Cumulus/Frame chains, this is the `Root` origin.
		/// This will not usually be an available option.
		Superuser,
	}

	impl From<ParachainDispatchOrigin> for MultiOrigin {
		fn from(o: ParachainDispatchOrigin) -> Self {
			match o {
				ParachainDispatchOrigin::Parachain => MultiOrigin::Native,
				ParachainDispatchOrigin::Signed => MultiOrigin::SovereignAccount,
				ParachainDispatchOrigin::Root => MultiOrigin::Superuser,
			}
		}
	}

	impl From<MultiOrigin> for ParachainDispatchOrigin {
		fn from(o: MultiOrigin) -> Self {
			match o {
				MultiOrigin::Native => ParachainDispatchOrigin::Parachain,
				MultiOrigin::SovereignAccount => ParachainDispatchOrigin::Signed,
				MultiOrigin::Superuser => ParachainDispatchOrigin::Root,
			}
		}
	}

	#[derive(Clone, Eq, PartialEq, Ord, PartialOrd,, Encode, Decode, RuntimeDebug)]
	pub enum MultiNetwork {
		Wildcard,
		Identified(Vec<u8>),
	}

	#[derive(Clone, Eq, PartialEq, Ord, PartialOrd,, Encode, Decode, RuntimeDebug)]
	pub enum MultiLocation {
		Null,
		X1(Junction),
		X2(Junction, Junction),
		X3(Junction, Junction, Junction),
		X4(Junction, Junction, Junction, Junction),
	}

	impl MultiLocation {
		pub fn first(&self) -> Option<&Junction> {
			match &self {
				MultiLocation::Null => None,
				MultiLocation::X1(ref a) => Some(a),
				MultiLocation::X2(ref a, ..) => Some(a),
				MultiLocation::X3(ref a, ..) => Some(a),
				MultiLocation::X4(ref a, ..) => Some(a),
			}
		}
		pub fn split_last(self) -> (MultiLocation, Option<Junction>) {
			match self {
				MultiLocation::Null => (MultiLocation::Null, None),
				MultiLocation::X1(a) => (MultiLocation::Null, Some(a)),
				MultiLocation::X2(a, b) => (MultiLocation::X1(a), Some(b)),
				MultiLocation::X3(a, b, c) => (MultiLocation::X2(a, b), Some(c)),
				MultiLocation::X4(a, b, c ,d) => (MultiLocation::X3(a, b, c), Some(d)),
			}
		}
		pub fn pushed_with(self, new: Junction) -> Result<Self, Self> {
			Ok(match self {
				MultiLocation::Null => MultiLocation::X1(new),
				MultiLocation::X1(a) => MultiLocation::X2(a, new),
				MultiLocation::X2(a, b) => MultiLocation::X3(a, b, new),
				MultiLocation::X3(a, b, c) => MultiLocation::X4(a, b, c, new),
				s => Err(s)?,
			})
		}
	}

	#[derive(Clone, Eq, PartialEq, Ord, PartialOrd,, Encode, Decode, RuntimeDebug)]
	pub enum Junction {
		Parent,
		Parachain { #[codec(compact)] id: u32 },
		OpaqueRemark(Vec<u8>),
		AccountId32 { network: MultiNetwork, id: [u8; 32] },
		AccountIndex64 { network: MultiNetwork, #[codec(compact)] index: u64 },
		AccountKey20 { network: MultiNetwork, key: [u8; 20] },
		/// An instanced Pallet on a Frame-based chain.
		PalletInstance { id: u8 },
		/// A nondescript index within the context location.
		GeneralIndex { #[codec(compact)] id: u128 },
		/// A nondescript datum acting as a key within the context location.
		GeneralKey(Vec<u8>),
	}

	impl From<Junction> for MultiLocation {
		fn from(x: Junction) -> Self {
			MultiLocation::X1(x)
		}
	}

	#[derive(Clone, Eq, PartialEq, Ord, PartialOrd,, Encode, Decode, RuntimeDebug)]
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

	pub type MultiAssets = Vec<MultiAsset>;
	// TODO: Efficient encoding, using initial byte values 128+ to encode the number of items in the vector.

	#[derive(Clone, Eq, PartialEq, Encode, Decode)]
	pub enum Ai {
		Null,
		DepositAsset { assets: MultiAssets, dest: MultiLocation },
		ExchangeAsset { give: MultiAssets, receive: MultiAssets },
		InitiateReserveTransfer { assets: MultiAssets, dest: MultiLocation, effects: Ais },
		InitiateTeleport { assets: MultiAssets, dest: MultiLocation, effects: Ais },
		QueryHolding { #[codec(compact)] query_id: u64, dest: MultiLocation, assets: Vec<MultiAssets> },
	}

	pub type Ais = Vec<Ai>;
	// TODO: Efficient encoding, using initial byte values 128+ to encode the number of items in the vector.

	#[derive(Clone, Eq, PartialEq, Encode, Decode)]
	pub enum Xcm {
		WithdrawAsset { assets: MultiAssets, effects: Ais },
		// Equivalent to WithdrawAsset{asset, Ai::InitiateReserveTransfer{asset, dest, effect}
		ReserveAssetTransfer { assets: MultiAssets, dest: MultiLocation, effects: Ais },
		ReserveAssetCredit { assets: MultiAssets, effects: Ais },
		TeleportAsset { assets: MultiAssets, effects: Ais },
		Balances { query_id: Vec<u8>, assets: MultiAssets },
		Transact { origin_type: MultiOrigin, call: Vec<u8> },
		// these won't be staying here for long. only v0 parachains with HRMP.
		ForwardToParachain { id: u32, inner: Box<VersionedXcm> },
		ForwardedFromParachain { id: u32, inner: Box<VersionedXcm> },
	}

	impl From<Xcm> for VersionedXcm {
		fn from(x: Xcm) -> Self {
			VersionedXcm::V0(x)
		}
	}

	impl TryFrom<VersionedXcm> for Xcm {
		type Error = ();
		fn try_from(x: VersionedXcm) -> Result<Self, ()> {
			match x {
				VersionedXcm::V0(x) => Ok(x),
			}
		}
	}

	impl From<MultiLocation> for VersionedMultiLocation {
		fn from(x: MultiLocation) -> Self {
			VersionedMultiLocation::V0(x)
		}
	}

	impl TryFrom<VersionedMultiLocation> for MultiLocation {
		type Error = ();
		fn try_from(x: VersionedMultiLocation) -> Result<Self, ()> {
			match x {
				VersionedMultiLocation::V0(x) => Ok(x),
			}
		}
	}

	impl From<MultiAsset> for VersionedMultiAsset {
		fn from(x: MultiAsset) -> Self {
			VersionedMultiAsset::V0(x)
		}
	}

	impl TryFrom<VersionedMultiAsset> for MultiAsset {
		type Error = ();
		fn try_from(x: VersionedMultiAsset) -> Result<Self, ()> {
			match x {
				VersionedMultiAsset::V0(x) => Ok(x),
			}
		}
	}
}
