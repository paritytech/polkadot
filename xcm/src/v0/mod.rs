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

//! Version 0 of the Cross-Consensus Message format data structures.

use sp_std::{result, boxed::Box, vec::Vec, convert::TryFrom};
use sp_runtime::RuntimeDebug;
use codec::{self, Encode, Decode};
use super::{VersionedXcm, VersionedMultiAsset};

mod junction;
mod multi_asset;
mod multi_location;
mod order;
mod traits;
pub use junction::Junction;
pub use multi_asset::{MultiAsset, AssetInstance};
pub use multi_location::MultiLocation;
pub use order::Order;
pub use traits::{Error, Result, SendXcm, ExecuteXcm};

// TODO: Efficient encodings for Vec<MultiAsset>, Vec<Order>, using initial byte values 128+ to encode the number of
//   items in the vector.

/// Basically just the XCM (more general) version of `ParachainDispatchOrigin`.
#[derive(Copy, Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum OriginKind {
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

/// Cross-Consensus Message: A message from one consensus system to another.
///
/// Consensus systems that may send and receive messages include blockchains and smart contracts.
///
/// All messages are delivered from a known *origin*, expressed as a `MultiLocation`.
///
/// This is the inner XCM format and is version-sensitive. Messages are typically passed using the outer
/// XCM format, known as `VersionedXcm`.
#[derive(Clone, Eq, PartialEq, Encode, Decode, RuntimeDebug)]
pub enum Xcm {
	/// Withdraw asset(s) (`assets`) from the ownership of `origin` and place them into `holding`. Execute the
	/// orders (`effects`).
	///
	/// - `assets`:
	/// - `effects`:
	///
	/// Kind: *Instruction*.
	WithdrawAsset { assets: Vec<MultiAsset>, effects: Vec<Order> },
	/// Asset(s) (`assets`) have been received into the ownership of this system on the `origin` system.
	///
	/// Some orders are given (`effects`) which should be executed once the corresponding derivative assets have
	/// been placed into `holding`.
	///
	/// - `assets`:
	/// - `effects`:
	///
	/// Safety: `origin` must be trusted to have received and be storing `assets` such that they may later be
	/// withdrawn should this system send a corresponding message.
	///
	/// Kind: *Trusted Indication*.
	ReserveAssetDeposit { assets: Vec<MultiAsset>, effects: Vec<Order> },
	/// Asset(s) (`assets`) have been destroyed on the `origin` system and equivalent assets should be
	/// created on this system.
	///
	/// Some orders are given (`effects`) which should be executed once the corresponding derivative assets have
	/// been placed into `holding`.
	///
	/// - `assets`:
	/// - `effects`:
	///
	/// Safety: `origin` must be trusted to have irrevocably destroyed the `assets` prior as a consequence of
	/// sending this message.
	///
	/// Kind: *Trusted Indication*.
	TeleportAsset { assets: Vec<MultiAsset>, effects: Vec<Order> },
	/// Indication of the contents of the holding account corresponding to the `QueryHolding` order of `query_id`.
	///
	/// - `query_id`:
	/// - `assets`:
	///
	/// Safety: No concerns.
	///
	/// Kind: *Information*.
	Balances { #[codec(compact)] query_id: u64, assets: Vec<MultiAsset> },
	/// Apply the encoded transaction `call`, whose dispatch-origin should be `origin` as expressed by the kind
	/// of origin `origin_type`.
	///
	/// - `origin_type`:
	/// - `call`:
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	Transact { origin_type: OriginKind, call: Vec<u8> },
	/// Relay an inner message (`inner`) to the parachain destination ID `id`.
	///
	/// The message sent to the parachain will be wrapped into a `RelayedFrom` message, with the `superorigin`
	/// being this parachain.
	///
	/// - `id`:
	/// - `inner`:
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	RelayToParachain { id: u32, inner: Box<VersionedXcm> },
	/// A message (`inner`) was sent to `origin` from `superorigin` with the intention of being relayed.
	///
	/// - `superorigin`: The location of the `inner` message origin, **relative to `origin`**.
	/// - `inner`: The message sent by the super origin.
	///
	/// Safety: `superorigin` must express a sub-consensus only.
	///
	/// Kind: *Trusted Indication*.
	RelayedFrom { superorigin: MultiLocation, inner: Box<VersionedXcm> },
}

impl From<Xcm> for VersionedXcm {
	fn from(x: Xcm) -> Self {
		VersionedXcm::V0(x)
	}
}

impl TryFrom<VersionedXcm> for Xcm {
	type Error = ();
	fn try_from(x: VersionedXcm) -> result::Result<Self, ()> {
		match x {
			VersionedXcm::V0(x) => Ok(x),
		}
	}
}
