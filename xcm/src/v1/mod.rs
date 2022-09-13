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

//! # XCM Version 1
//! Version 1 of the Cross-Consensus Message format data structures. The comprehensive list of
//! changes can be found in
//! [this PR description](https://github.com/paritytech/polkadot/pull/2815#issue-608567900).
//!
//! ## Changes to be aware of
//! Most changes should automatically be resolved via the conversion traits (i.e. `TryFrom` and
//! `From`). The list here is mostly for incompatible changes that result in an `Err(())` when
//! attempting to convert XCM objects from v0.
//!
//! ### Junction
//! - `v0::Junction::Parent` cannot be converted to v1, because the way we represent parents in v1
//!   has changed - instead of being a property of the junction, v1 `MultiLocation`s now have an
//!   extra field representing the number of parents that the `MultiLocation` contains.
//!
//! ### `MultiLocation`
//! - The `try_from` conversion method will always canonicalize the v0 `MultiLocation` before
//!   attempting to do the proper conversion. Since canonicalization is not a fallible operation,
//!   we do not expect v0 `MultiLocation` to ever fail to be upgraded to v1.
//!
//! ### `MultiAsset`
//! - Stronger typing to differentiate between a single class of `MultiAsset` and several classes
//!   of `MultiAssets` is introduced. As the name suggests, a `Vec<MultiAsset>` that is used on all
//!   APIs will instead be using a new type called `MultiAssets` (note the `s`).
//! - All `MultiAsset` variants whose name contains "All" in it, namely `v0::MultiAsset::All`,
//!   `v0::MultiAsset::AllFungible`, `v0::MultiAsset::AllNonFungible`,
//!   `v0::MultiAsset::AllAbstractFungible`, `v0::MultiAsset::AllAbstractNonFungible`,
//!   `v0::MultiAsset::AllConcreteFungible` and `v0::MultiAsset::AllConcreteNonFungible`, will fail
//!   to convert to v1 `MultiAsset`, since v1 does not contain these variants.
//! - Similarly, all `MultiAsset` variants whose name contains "All" in it can be converted into a
//!   `WildMultiAsset`.
//! - `v0::MultiAsset::None` is not represented at all in v1.
//!
//! ### XCM
//! - No special attention necessary
//!
//! ### Order
//! - `v1::Order::DepositAsset` and `v1::Order::DepositReserveAsset` both introduced a new
//!   `max_asset` field that limits the maximum classes of assets that can be deposited. During
//!   conversion from v0, the `max_asset` field defaults to 1.
//! - v1 Orders that contain `MultiAsset` as argument(s) will need to explicitly specify the amount
//!   and details of assets. This is to prevent accidental misuse of `All` to possibly transfer,
//!   spend or otherwise perform unintended operations on `All` assets.
//! - v1 Orders that do allow the notion of `All` to be used as wildcards, will instead use a new
//!   type called `MultiAssetFilter`.

use super::{
	v0::{Response as OldResponse, Xcm as OldXcm},
	v2::{Instruction, Response as NewResponse, Xcm as NewXcm},
};
use crate::DoubleEncoded;
use alloc::vec::Vec;
use core::{fmt::Debug, result};
use derivative::Derivative;
use parity_scale_codec::{self, Decode, Encode};
use scale_info::TypeInfo;

mod junction;
mod multiasset;
mod multilocation;
mod order;
mod traits; // the new multiasset.

pub use junction::Junction;
pub use multiasset::{
	AssetId, AssetInstance, Fungibility, MultiAsset, MultiAssetFilter, MultiAssets,
	WildFungibility, WildMultiAsset,
};
pub use multilocation::{
	Ancestor, AncestorThen, InteriorMultiLocation, Junctions, MultiLocation, Parent, ParentThen,
};
pub use order::Order;
pub use traits::{Error, ExecuteXcm, Outcome, Result, SendXcm};

// These parts of XCM v0 have been unchanged in XCM v1, and are re-imported here.
pub use super::v0::{BodyId, BodyPart, NetworkId, OriginKind};

/// A prelude for importing all types typically used when interacting with XCM messages.
pub mod prelude {
	pub use super::{
		junction::Junction::{self, *},
		opaque,
		order::Order::{self, *},
		Ancestor, AncestorThen,
		AssetId::{self, *},
		AssetInstance::{self, *},
		BodyId, BodyPart, Error as XcmError, ExecuteXcm,
		Fungibility::{self, *},
		InteriorMultiLocation,
		Junctions::{self, *},
		MultiAsset,
		MultiAssetFilter::{self, *},
		MultiAssets, MultiLocation,
		NetworkId::{self, *},
		OriginKind, Outcome, Parent, ParentThen, Response, Result as XcmResult, SendXcm,
		WildFungibility::{self, Fungible as WildFungible, NonFungible as WildNonFungible},
		WildMultiAsset::{self, *},
		Xcm::{self, *},
	};
}

/// Response data to a query.
#[derive(Clone, Eq, PartialEq, Encode, Decode, Debug, TypeInfo)]
pub enum Response {
	/// Some assets.
	Assets(MultiAssets),
	/// An XCM version.
	Version(super::Version),
}

/// Cross-Consensus Message: A message from one consensus system to another.
///
/// Consensus systems that may send and receive messages include blockchains and smart contracts.
///
/// All messages are delivered from a known *origin*, expressed as a `MultiLocation`.
///
/// This is the inner XCM format and is version-sensitive. Messages are typically passed using the outer
/// XCM format, known as `VersionedXcm`.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
#[scale_info(bounds(), skip_type_params(RuntimeCall))]
pub enum Xcm<RuntimeCall> {
	/// Withdraw asset(s) (`assets`) from the ownership of `origin` and place them into `holding`. Execute the
	/// orders (`effects`).
	///
	/// - `assets`: The asset(s) to be withdrawn into holding.
	/// - `effects`: The order(s) to execute on the holding register.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	#[codec(index = 0)]
	WithdrawAsset { assets: MultiAssets, effects: Vec<Order<RuntimeCall>> },

	/// Asset(s) (`assets`) have been received into the ownership of this system on the `origin` system.
	///
	/// Some orders are given (`effects`) which should be executed once the corresponding derivative assets have
	/// been placed into `holding`.
	///
	/// - `assets`: The asset(s) that are minted into holding.
	/// - `effects`: The order(s) to execute on the holding register.
	///
	/// Safety: `origin` must be trusted to have received and be storing `assets` such that they may later be
	/// withdrawn should this system send a corresponding message.
	///
	/// Kind: *Trusted Indication*.
	///
	/// Errors:
	#[codec(index = 1)]
	ReserveAssetDeposited { assets: MultiAssets, effects: Vec<Order<RuntimeCall>> },

	/// Asset(s) (`assets`) have been destroyed on the `origin` system and equivalent assets should be
	/// created on this system.
	///
	/// Some orders are given (`effects`) which should be executed once the corresponding derivative assets have
	/// been placed into the Holding Register.
	///
	/// - `assets`: The asset(s) that are minted into the Holding Register.
	/// - `effects`: The order(s) to execute on the Holding Register.
	///
	/// Safety: `origin` must be trusted to have irrevocably destroyed the corresponding `assets` prior as a consequence
	/// of sending this message.
	///
	/// Kind: *Trusted Indication*.
	///
	/// Errors:
	#[codec(index = 2)]
	ReceiveTeleportedAsset { assets: MultiAssets, effects: Vec<Order<RuntimeCall>> },

	/// Indication of the contents of the holding register corresponding to the `QueryHolding` order of `query_id`.
	///
	/// - `query_id`: The identifier of the query that resulted in this message being sent.
	/// - `assets`: The message content.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Information*.
	///
	/// Errors:
	#[codec(index = 3)]
	QueryResponse {
		#[codec(compact)]
		query_id: u64,
		response: Response,
	},

	/// Withdraw asset(s) (`assets`) from the ownership of `origin` and place equivalent assets under the
	/// ownership of `beneficiary`.
	///
	/// - `assets`: The asset(s) to be withdrawn.
	/// - `beneficiary`: The new owner for the assets.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	#[codec(index = 4)]
	TransferAsset { assets: MultiAssets, beneficiary: MultiLocation },

	/// Withdraw asset(s) (`assets`) from the ownership of `origin` and place equivalent assets under the
	/// ownership of `dest` within this consensus system (i.e. its sovereign account).
	///
	/// Send an onward XCM message to `dest` of `ReserveAssetDeposited` with the given `effects`.
	///
	/// - `assets`: The asset(s) to be withdrawn.
	/// - `dest`: The location whose sovereign account will own the assets and thus the effective beneficiary for the
	///   assets and the notification target for the reserve asset deposit message.
	/// - `effects`: The orders that should be contained in the `ReserveAssetDeposited` which is sent onwards to
	///   `dest`.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	#[codec(index = 5)]
	TransferReserveAsset { assets: MultiAssets, dest: MultiLocation, effects: Vec<Order<()>> },

	/// Apply the encoded transaction `call`, whose dispatch-origin should be `origin` as expressed by the kind
	/// of origin `origin_type`.
	///
	/// - `origin_type`: The means of expressing the message origin as a dispatch origin.
	/// - `max_weight`: The weight of `call`; this should be at least the chain's calculated weight and will
	///   be used in the weight determination arithmetic.
	/// - `call`: The encoded transaction to be applied.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	#[codec(index = 6)]
	Transact {
		origin_type: OriginKind,
		require_weight_at_most: u64,
		call: DoubleEncoded<RuntimeCall>,
	},

	/// A message to notify about a new incoming HRMP channel. This message is meant to be sent by the
	/// relay-chain to a para.
	///
	/// - `sender`: The sender in the to-be opened channel. Also, the initiator of the channel opening.
	/// - `max_message_size`: The maximum size of a message proposed by the sender.
	/// - `max_capacity`: The maximum number of messages that can be queued in the channel.
	///
	/// Safety: The message should originate directly from the relay-chain.
	///
	/// Kind: *System Notification*
	#[codec(index = 7)]
	HrmpNewChannelOpenRequest {
		#[codec(compact)]
		sender: u32,
		#[codec(compact)]
		max_message_size: u32,
		#[codec(compact)]
		max_capacity: u32,
	},

	/// A message to notify about that a previously sent open channel request has been accepted by
	/// the recipient. That means that the channel will be opened during the next relay-chain session
	/// change. This message is meant to be sent by the relay-chain to a para.
	///
	/// Safety: The message should originate directly from the relay-chain.
	///
	/// Kind: *System Notification*
	///
	/// Errors:
	#[codec(index = 8)]
	HrmpChannelAccepted {
		#[codec(compact)]
		recipient: u32,
	},

	/// A message to notify that the other party in an open channel decided to close it. In particular,
	/// `initiator` is going to close the channel opened from `sender` to the `recipient`. The close
	/// will be enacted at the next relay-chain session change. This message is meant to be sent by
	/// the relay-chain to a para.
	///
	/// Safety: The message should originate directly from the relay-chain.
	///
	/// Kind: *System Notification*
	///
	/// Errors:
	#[codec(index = 9)]
	HrmpChannelClosing {
		#[codec(compact)]
		initiator: u32,
		#[codec(compact)]
		sender: u32,
		#[codec(compact)]
		recipient: u32,
	},

	/// A message to indicate that the embedded XCM is actually arriving on behalf of some consensus
	/// location within the origin.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	#[codec(index = 10)]
	RelayedFrom { who: InteriorMultiLocation, message: alloc::boxed::Box<Xcm<RuntimeCall>> },

	/// Ask the destination system to respond with the most recent version of XCM that they
	/// support in a `QueryResponse` instruction. Any changes to this should also elicit similar
	/// responses when they happen.
	///
	/// Kind: *Instruction*
	#[codec(index = 11)]
	SubscribeVersion {
		#[codec(compact)]
		query_id: u64,
		#[codec(compact)]
		max_response_weight: u64,
	},

	/// Cancel the effect of a previous `SubscribeVersion` instruction.
	///
	/// Kind: *Instruction*
	#[codec(index = 12)]
	UnsubscribeVersion,
}

impl<RuntimeCall> Xcm<RuntimeCall> {
	pub fn into<C>(self) -> Xcm<C> {
		Xcm::from(self)
	}
	pub fn from<C>(xcm: Xcm<C>) -> Self {
		use Xcm::*;
		match xcm {
			WithdrawAsset { assets, effects } =>
				WithdrawAsset { assets, effects: effects.into_iter().map(Order::into).collect() },
			ReserveAssetDeposited { assets, effects } => ReserveAssetDeposited {
				assets,
				effects: effects.into_iter().map(Order::into).collect(),
			},
			ReceiveTeleportedAsset { assets, effects } => ReceiveTeleportedAsset {
				assets,
				effects: effects.into_iter().map(Order::into).collect(),
			},
			QueryResponse { query_id, response } => QueryResponse { query_id, response },
			TransferAsset { assets, beneficiary } => TransferAsset { assets, beneficiary },
			TransferReserveAsset { assets, dest, effects } =>
				TransferReserveAsset { assets, dest, effects },
			HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity } =>
				HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity },
			HrmpChannelAccepted { recipient } => HrmpChannelAccepted { recipient },
			HrmpChannelClosing { initiator, sender, recipient } =>
				HrmpChannelClosing { initiator, sender, recipient },
			Transact { origin_type, require_weight_at_most, call } =>
				Transact { origin_type, require_weight_at_most, call: call.into() },
			RelayedFrom { who, message } =>
				RelayedFrom { who, message: alloc::boxed::Box::new((*message).into()) },
			SubscribeVersion { query_id, max_response_weight } =>
				SubscribeVersion { query_id, max_response_weight },
			UnsubscribeVersion => UnsubscribeVersion,
		}
	}
}

pub mod opaque {
	/// The basic concrete type of `generic::Xcm`, which doesn't make any assumptions about the format of a
	/// call other than it is pre-encoded.
	pub type Xcm = super::Xcm<()>;

	pub use super::order::opaque::*;
}

// Convert from a v0 response to a v1 response
impl TryFrom<OldResponse> for Response {
	type Error = ();
	fn try_from(old_response: OldResponse) -> result::Result<Self, ()> {
		match old_response {
			OldResponse::Assets(assets) => Ok(Self::Assets(assets.try_into()?)),
		}
	}
}

impl<RuntimeCall> TryFrom<OldXcm<RuntimeCall>> for Xcm<RuntimeCall> {
	type Error = ();
	fn try_from(old: OldXcm<RuntimeCall>) -> result::Result<Xcm<RuntimeCall>, ()> {
		use Xcm::*;
		Ok(match old {
			OldXcm::WithdrawAsset { assets, effects } => WithdrawAsset {
				assets: assets.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			OldXcm::ReserveAssetDeposit { assets, effects } => ReserveAssetDeposited {
				assets: assets.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			OldXcm::TeleportAsset { assets, effects } => ReceiveTeleportedAsset {
				assets: assets.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			OldXcm::QueryResponse { query_id, response } =>
				QueryResponse { query_id, response: response.try_into()? },
			OldXcm::TransferAsset { assets, dest } =>
				TransferAsset { assets: assets.try_into()?, beneficiary: dest.try_into()? },
			OldXcm::TransferReserveAsset { assets, dest, effects } => TransferReserveAsset {
				assets: assets.try_into()?,
				dest: dest.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			OldXcm::HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity } =>
				HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity },
			OldXcm::HrmpChannelAccepted { recipient } => HrmpChannelAccepted { recipient },
			OldXcm::HrmpChannelClosing { initiator, sender, recipient } =>
				HrmpChannelClosing { initiator, sender, recipient },
			OldXcm::Transact { origin_type, require_weight_at_most, call } =>
				Transact { origin_type, require_weight_at_most, call: call.into() },
			OldXcm::RelayedFrom { who, message } => RelayedFrom {
				who: MultiLocation::try_from(who)?.try_into()?,
				message: alloc::boxed::Box::new((*message).try_into()?),
			},
		})
	}
}

impl<RuntimeCall> TryFrom<NewXcm<RuntimeCall>> for Xcm<RuntimeCall> {
	type Error = ();
	fn try_from(old: NewXcm<RuntimeCall>) -> result::Result<Xcm<RuntimeCall>, ()> {
		use Xcm::*;
		let mut iter = old.0.into_iter();
		let instruction = iter.next().ok_or(())?;
		Ok(match instruction {
			Instruction::WithdrawAsset(assets) => {
				let effects = iter.map(Order::try_from).collect::<result::Result<_, _>>()?;
				WithdrawAsset { assets, effects }
			},
			Instruction::ReserveAssetDeposited(assets) => {
				if !matches!(iter.next(), Some(Instruction::ClearOrigin)) {
					return Err(())
				}
				let effects = iter.map(Order::try_from).collect::<result::Result<_, _>>()?;
				ReserveAssetDeposited { assets, effects }
			},
			Instruction::ReceiveTeleportedAsset(assets) => {
				if !matches!(iter.next(), Some(Instruction::ClearOrigin)) {
					return Err(())
				}
				let effects = iter.map(Order::try_from).collect::<result::Result<_, _>>()?;
				ReceiveTeleportedAsset { assets, effects }
			},
			Instruction::QueryResponse { query_id, response, max_weight } => {
				// Cannot handle special response weights.
				if max_weight > 0 {
					return Err(())
				}
				QueryResponse { query_id, response: response.try_into()? }
			},
			Instruction::TransferAsset { assets, beneficiary } =>
				TransferAsset { assets, beneficiary },
			Instruction::TransferReserveAsset { assets, dest, xcm } => TransferReserveAsset {
				assets,
				dest,
				effects: xcm
					.0
					.into_iter()
					.map(Order::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			Instruction::HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity } =>
				HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity },
			Instruction::HrmpChannelAccepted { recipient } => HrmpChannelAccepted { recipient },
			Instruction::HrmpChannelClosing { initiator, sender, recipient } =>
				HrmpChannelClosing { initiator, sender, recipient },
			Instruction::Transact { origin_type, require_weight_at_most, call } =>
				Transact { origin_type, require_weight_at_most, call },
			Instruction::SubscribeVersion { query_id, max_response_weight } =>
				SubscribeVersion { query_id, max_response_weight },
			Instruction::UnsubscribeVersion => UnsubscribeVersion,
			_ => return Err(()),
		})
	}
}

// Convert from a v1 response to a v2 response
impl TryFrom<NewResponse> for Response {
	type Error = ();
	fn try_from(response: NewResponse) -> result::Result<Self, ()> {
		match response {
			NewResponse::Assets(assets) => Ok(Self::Assets(assets)),
			NewResponse::Version(version) => Ok(Self::Version(version)),
			_ => Err(()),
		}
	}
}
