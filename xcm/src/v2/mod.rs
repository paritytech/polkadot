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

//! # XCM Version 2
//! Version 2 of the Cross-Consensus Message format data structures. The comprehensive list of
//! changes can be found in
//! [this PR description](https://github.com/paritytech/polkadot/pull/3629#issue-968428279).
//!
//! ## Changes to be aware of
//! The biggest change here is the restructuring of XCM messages: instead of having `Order` and
//! `Xcm` types, the `Xcm` type now simply wraps a `Vec` containing `Instruction`s. However, most
//! changes should still be automatically convertible via the `try_from` and `from` conversion
//! functions.
//!
//! ### Junction
//! - No special attention necessary
//!
//! ### `MultiLocation`
//! - No special attention necessary
//!
//! ### `MultiAsset`
//! - No special attention necessary
//!
//! ### XCM and Order
//! - `Xcm` and `Order` variants are now combined under a single `Instruction` enum.
//! - `Order` is now obsolete and replaced entirely by `Instruction`.
//! - `Xcm` is now a simple wrapper around a `Vec<Instruction>`.
//! - During conversion from `Order` to `Instruction`, we do not handle `BuyExecution`s that have
//!   nested XCMs, i.e. if the `instructions` field in the `BuyExecution` enum struct variant is
//!   not empty, then the conversion will fail. To address this, rewrite the XCM using
//!   `Instruction`s in chronological order.
//! - During conversion from `Xcm` to `Instruction`, we do not handle `RelayedFrom` messages at
//!   all.
//!
//! ### XCM Pallet
//! - The `Weigher` configuration item must have sensible weights defined for `BuyExecution` and
//!   `DepositAsset` instructions. Failing that, dispatch calls to `teleport_assets` and
//!   `reserve_transfer_assets` will fail with `UnweighableMessage`.

use super::v1::{Order as OldOrder, Response as OldResponse, Xcm as OldXcm};
use crate::{DoubleEncoded, GetWeight};
use alloc::{vec, vec::Vec};
use core::{fmt::Debug, result};
use derivative::Derivative;
use parity_scale_codec::{self, Decode, Encode};
use scale_info::TypeInfo;

mod traits;

pub use traits::{Error, ExecuteXcm, Outcome, Result, SendError, SendResult, SendXcm};
// These parts of XCM v1 have been unchanged in XCM v2, and are re-imported here.
pub use super::v1::{
	Ancestor, AncestorThen, AssetId, AssetInstance, BodyId, BodyPart, Fungibility,
	InteriorMultiLocation, Junction, Junctions, MultiAsset, MultiAssetFilter, MultiAssets,
	MultiLocation, NetworkId, OriginKind, Parent, ParentThen, WildFungibility, WildMultiAsset,
};

/// This module's XCM version.
pub const VERSION: super::Version = 2;

/// An identifier for a query.
pub type QueryId = u64;

#[derive(Derivative, Default, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
#[scale_info(bounds(), skip_type_params(RuntimeCall))]
pub struct Xcm<RuntimeCall>(pub Vec<Instruction<RuntimeCall>>);

impl<RuntimeCall> Xcm<RuntimeCall> {
	/// Create an empty instance.
	pub fn new() -> Self {
		Self(vec![])
	}

	/// Return `true` if no instructions are held in `self`.
	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}

	/// Return the number of instructions held in `self`.
	pub fn len(&self) -> usize {
		self.0.len()
	}

	/// Consume and either return `self` if it contains some instructions, or if it's empty, then
	/// instead return the result of `f`.
	pub fn or_else(self, f: impl FnOnce() -> Self) -> Self {
		if self.0.is_empty() {
			f()
		} else {
			self
		}
	}

	/// Return the first instruction, if any.
	pub fn first(&self) -> Option<&Instruction<RuntimeCall>> {
		self.0.first()
	}

	/// Return the last instruction, if any.
	pub fn last(&self) -> Option<&Instruction<RuntimeCall>> {
		self.0.last()
	}

	/// Return the only instruction, contained in `Self`, iff only one exists (`None` otherwise).
	pub fn only(&self) -> Option<&Instruction<RuntimeCall>> {
		if self.0.len() == 1 {
			self.0.first()
		} else {
			None
		}
	}

	/// Return the only instruction, contained in `Self`, iff only one exists (returns `self`
	/// otherwise).
	pub fn into_only(mut self) -> core::result::Result<Instruction<RuntimeCall>, Self> {
		if self.0.len() == 1 {
			self.0.pop().ok_or(self)
		} else {
			Err(self)
		}
	}
}

/// A prelude for importing all types typically used when interacting with XCM messages.
pub mod prelude {
	mod contents {
		pub use super::super::{
			Ancestor, AncestorThen,
			AssetId::{self, *},
			AssetInstance::{self, *},
			BodyId, BodyPart, Error as XcmError, ExecuteXcm,
			Fungibility::{self, *},
			Instruction::*,
			InteriorMultiLocation,
			Junction::{self, *},
			Junctions::{self, *},
			MultiAsset,
			MultiAssetFilter::{self, *},
			MultiAssets, MultiLocation,
			NetworkId::{self, *},
			OriginKind, Outcome, Parent, ParentThen, QueryId, Response, Result as XcmResult,
			SendError, SendResult, SendXcm,
			WeightLimit::{self, *},
			WildFungibility::{self, Fungible as WildFungible, NonFungible as WildNonFungible},
			WildMultiAsset::{self, *},
			XcmWeightInfo, VERSION as XCM_VERSION,
		};
	}
	pub use super::{Instruction, Xcm};
	pub use contents::*;
	pub mod opaque {
		pub use super::{
			super::opaque::{Instruction, Xcm},
			contents::*,
		};
	}
}

/// Response data to a query.
#[derive(Clone, Eq, PartialEq, Encode, Decode, Debug, TypeInfo)]
pub enum Response {
	/// No response. Serves as a neutral default.
	Null,
	/// Some assets.
	Assets(MultiAssets),
	/// The outcome of an XCM instruction.
	ExecutionResult(Option<(u32, Error)>),
	/// An XCM version.
	Version(super::Version),
}

impl Default for Response {
	fn default() -> Self {
		Self::Null
	}
}

/// An optional weight limit.
#[derive(Clone, Eq, PartialEq, Encode, Decode, Debug, TypeInfo)]
pub enum WeightLimit {
	/// No weight limit imposed.
	Unlimited,
	/// Weight limit imposed of the inner value.
	Limited(#[codec(compact)] u64),
}

impl From<Option<u64>> for WeightLimit {
	fn from(x: Option<u64>) -> Self {
		match x {
			Some(w) => WeightLimit::Limited(w),
			None => WeightLimit::Unlimited,
		}
	}
}

impl From<WeightLimit> for Option<u64> {
	fn from(x: WeightLimit) -> Self {
		match x {
			WeightLimit::Limited(w) => Some(w),
			WeightLimit::Unlimited => None,
		}
	}
}

/// Local weight type; execution time in picoseconds.
pub type Weight = u64;

/// Cross-Consensus Message: A message from one consensus system to another.
///
/// Consensus systems that may send and receive messages include blockchains and smart contracts.
///
/// All messages are delivered from a known *origin*, expressed as a `MultiLocation`.
///
/// This is the inner XCM format and is version-sensitive. Messages are typically passed using the outer
/// XCM format, known as `VersionedXcm`.
#[derive(Derivative, Encode, Decode, TypeInfo, xcm_procedural::XcmWeightInfoTrait)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
#[scale_info(bounds(), skip_type_params(RuntimeCall))]
pub enum Instruction<RuntimeCall> {
	/// Withdraw asset(s) (`assets`) from the ownership of `origin` and place them into the Holding
	/// Register.
	///
	/// - `assets`: The asset(s) to be withdrawn into holding.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	WithdrawAsset(MultiAssets),

	/// Asset(s) (`assets`) have been received into the ownership of this system on the `origin`
	/// system and equivalent derivatives should be placed into the Holding Register.
	///
	/// - `assets`: The asset(s) that are minted into holding.
	///
	/// Safety: `origin` must be trusted to have received and be storing `assets` such that they
	/// may later be withdrawn should this system send a corresponding message.
	///
	/// Kind: *Trusted Indication*.
	///
	/// Errors:
	ReserveAssetDeposited(MultiAssets),

	/// Asset(s) (`assets`) have been destroyed on the `origin` system and equivalent assets should
	/// be created and placed into the Holding Register.
	///
	/// - `assets`: The asset(s) that are minted into the Holding Register.
	///
	/// Safety: `origin` must be trusted to have irrevocably destroyed the corresponding `assets`
	/// prior as a consequence of sending this message.
	///
	/// Kind: *Trusted Indication*.
	///
	/// Errors:
	ReceiveTeleportedAsset(MultiAssets),

	/// Respond with information that the local system is expecting.
	///
	/// - `query_id`: The identifier of the query that resulted in this message being sent.
	/// - `response`: The message content.
	/// - `max_weight`: The maximum weight that handling this response should take.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Information*.
	///
	/// Errors:
	QueryResponse {
		#[codec(compact)]
		query_id: QueryId,
		response: Response,
		#[codec(compact)]
		max_weight: u64,
	},

	/// Withdraw asset(s) (`assets`) from the ownership of `origin` and place equivalent assets
	/// under the ownership of `beneficiary`.
	///
	/// - `assets`: The asset(s) to be withdrawn.
	/// - `beneficiary`: The new owner for the assets.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	TransferAsset { assets: MultiAssets, beneficiary: MultiLocation },

	/// Withdraw asset(s) (`assets`) from the ownership of `origin` and place equivalent assets
	/// under the ownership of `dest` within this consensus system (i.e. its sovereign account).
	///
	/// Send an onward XCM message to `dest` of `ReserveAssetDeposited` with the given
	/// `xcm`.
	///
	/// - `assets`: The asset(s) to be withdrawn.
	/// - `dest`: The location whose sovereign account will own the assets and thus the effective
	///   beneficiary for the assets and the notification target for the reserve asset deposit
	///   message.
	/// - `xcm`: The instructions that should follow the `ReserveAssetDeposited`
	///   instruction, which is sent onwards to `dest`.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	TransferReserveAsset { assets: MultiAssets, dest: MultiLocation, xcm: Xcm<()> },

	/// Apply the encoded transaction `call`, whose dispatch-origin should be `origin` as expressed
	/// by the kind of origin `origin_type`.
	///
	/// - `origin_type`: The means of expressing the message origin as a dispatch origin.
	/// - `max_weight`: The weight of `call`; this should be at least the chain's calculated weight
	///   and will be used in the weight determination arithmetic.
	/// - `call`: The encoded transaction to be applied.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	Transact {
		origin_type: OriginKind,
		#[codec(compact)]
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
	HrmpChannelAccepted {
		// NOTE: We keep this as a structured item to a) keep it consistent with the other Hrmp
		// items; and b) because the field's meaning is not obvious/mentioned from the item name.
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
	HrmpChannelClosing {
		#[codec(compact)]
		initiator: u32,
		#[codec(compact)]
		sender: u32,
		#[codec(compact)]
		recipient: u32,
	},

	/// Clear the origin.
	///
	/// This may be used by the XCM author to ensure that later instructions cannot command the
	/// authority of the origin (e.g. if they are being relayed from an untrusted source, as often
	/// the case with `ReserveAssetDeposited`).
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	ClearOrigin,

	/// Mutate the origin to some interior location.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	DescendOrigin(InteriorMultiLocation),

	/// Immediately report the contents of the Error Register to the given destination via XCM.
	///
	/// A `QueryResponse` message of type `ExecutionOutcome` is sent to `dest` with the given
	/// `query_id` and the outcome of the XCM.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	ReportError {
		#[codec(compact)]
		query_id: QueryId,
		dest: MultiLocation,
		#[codec(compact)]
		max_response_weight: u64,
	},

	/// Remove the asset(s) (`assets`) from the Holding Register and place equivalent assets under
	/// the ownership of `beneficiary` within this consensus system.
	///
	/// - `assets`: The asset(s) to remove from holding.
	/// - `max_assets`: The maximum number of unique assets/asset instances to remove from holding.
	///   Only the first `max_assets` assets/instances of those matched by `assets` will be removed,
	///   prioritized under standard asset ordering. Any others will remain in holding.
	/// - `beneficiary`: The new owner for the assets.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	DepositAsset {
		assets: MultiAssetFilter,
		#[codec(compact)]
		max_assets: u32,
		beneficiary: MultiLocation,
	},

	/// Remove the asset(s) (`assets`) from the Holding Register and place equivalent assets under
	/// the ownership of `dest` within this consensus system (i.e. deposit them into its sovereign
	/// account).
	///
	/// Send an onward XCM message to `dest` of `ReserveAssetDeposited` with the given `effects`.
	///
	/// - `assets`: The asset(s) to remove from holding.
	/// - `max_assets`: The maximum number of unique assets/asset instances to remove from holding.
	///   Only the first `max_assets` assets/instances of those matched by `assets` will be removed,
	///   prioritized under standard asset ordering. Any others will remain in holding.
	/// - `dest`: The location whose sovereign account will own the assets and thus the effective
	///   beneficiary for the assets and the notification target for the reserve asset deposit
	///   message.
	/// - `xcm`: The orders that should follow the `ReserveAssetDeposited` instruction
	///   which is sent onwards to `dest`.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	DepositReserveAsset {
		assets: MultiAssetFilter,
		#[codec(compact)]
		max_assets: u32,
		dest: MultiLocation,
		xcm: Xcm<()>,
	},

	/// Remove the asset(s) (`give`) from the Holding Register and replace them with alternative
	/// assets.
	///
	/// The minimum amount of assets to be received into the Holding Register for the order not to
	/// fail may be stated.
	///
	/// - `give`: The asset(s) to remove from holding.
	/// - `receive`: The minimum amount of assets(s) which `give` should be exchanged for.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	ExchangeAsset { give: MultiAssetFilter, receive: MultiAssets },

	/// Remove the asset(s) (`assets`) from holding and send a `WithdrawAsset` XCM message to a
	/// reserve location.
	///
	/// - `assets`: The asset(s) to remove from holding.
	/// - `reserve`: A valid location that acts as a reserve for all asset(s) in `assets`. The
	///   sovereign account of this consensus system *on the reserve location* will have appropriate
	///   assets withdrawn and `effects` will be executed on them. There will typically be only one
	///   valid location on any given asset/chain combination.
	/// - `xcm`: The instructions to execute on the assets once withdrawn *on the reserve
	///   location*.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	InitiateReserveWithdraw { assets: MultiAssetFilter, reserve: MultiLocation, xcm: Xcm<()> },

	/// Remove the asset(s) (`assets`) from holding and send a `ReceiveTeleportedAsset` XCM message
	/// to a `dest` location.
	///
	/// - `assets`: The asset(s) to remove from holding.
	/// - `dest`: A valid location that respects teleports coming from this location.
	/// - `xcm`: The instructions to execute on the assets once arrived *on the destination
	///   location*.
	///
	/// NOTE: The `dest` location *MUST* respect this origin as a valid teleportation origin for all
	/// `assets`. If it does not, then the assets may be lost.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	InitiateTeleport { assets: MultiAssetFilter, dest: MultiLocation, xcm: Xcm<()> },

	/// Send a `Balances` XCM message with the `assets` value equal to the holding contents, or a
	/// portion thereof.
	///
	/// - `query_id`: An identifier that will be replicated into the returned XCM message.
	/// - `dest`: A valid destination for the returned XCM message. This may be limited to the
	///   current origin.
	/// - `assets`: A filter for the assets that should be reported back. The assets reported back
	///   will be, asset-wise, *the lesser of this value and the holding register*. No wildcards
	///   will be used when reporting assets back.
	/// - `max_response_weight`: The maximum amount of weight that the `QueryResponse` item which
	///   is sent as a reply may take to execute. NOTE: If this is unexpectedly large then the
	///   response may not execute at all.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	QueryHolding {
		#[codec(compact)]
		query_id: QueryId,
		dest: MultiLocation,
		assets: MultiAssetFilter,
		#[codec(compact)]
		max_response_weight: u64,
	},

	/// Pay for the execution of some XCM `xcm` and `orders` with up to `weight`
	/// picoseconds of execution time, paying for this with up to `fees` from the Holding Register.
	///
	/// - `fees`: The asset(s) to remove from the Holding Register to pay for fees.
	/// - `weight_limit`: The maximum amount of weight to purchase; this must be at least the
	///   expected maximum weight of the total XCM to be executed for the
	///   `AllowTopLevelPaidExecutionFrom` barrier to allow the XCM be executed.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	BuyExecution { fees: MultiAsset, weight_limit: WeightLimit },

	/// Refund any surplus weight previously bought with `BuyExecution`.
	///
	/// Kind: *Instruction*
	///
	/// Errors: None.
	RefundSurplus,

	/// Set the Error Handler Register. This is code that should be called in the case of an error
	/// happening.
	///
	/// An error occurring within execution of this code will _NOT_ result in the error register
	/// being set, nor will an error handler be called due to it. The error handler and appendix
	/// may each still be set.
	///
	/// The apparent weight of this instruction is inclusive of the inner `Xcm`; the executing
	/// weight however includes only the difference between the previous handler and the new
	/// handler, which can reasonably be negative, which would result in a surplus.
	///
	/// Kind: *Instruction*
	///
	/// Errors: None.
	SetErrorHandler(Xcm<RuntimeCall>),

	/// Set the Appendix Register. This is code that should be called after code execution
	/// (including the error handler if any) is finished. This will be called regardless of whether
	/// an error occurred.
	///
	/// Any error occurring due to execution of this code will result in the error register being
	/// set, and the error handler (if set) firing.
	///
	/// The apparent weight of this instruction is inclusive of the inner `Xcm`; the executing
	/// weight however includes only the difference between the previous appendix and the new
	/// appendix, which can reasonably be negative, which would result in a surplus.
	///
	/// Kind: *Instruction*
	///
	/// Errors: None.
	SetAppendix(Xcm<RuntimeCall>),

	/// Clear the Error Register.
	///
	/// Kind: *Instruction*
	///
	/// Errors: None.
	ClearError,

	/// Create some assets which are being held on behalf of the origin.
	///
	/// - `assets`: The assets which are to be claimed. This must match exactly with the assets
	///   claimable by the origin of the ticket.
	/// - `ticket`: The ticket of the asset; this is an abstract identifier to help locate the
	///   asset.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	ClaimAsset { assets: MultiAssets, ticket: MultiLocation },

	/// Always throws an error of type `Trap`.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	/// - `Trap`: All circumstances, whose inner value is the same as this item's inner value.
	Trap(#[codec(compact)] u64),

	/// Ask the destination system to respond with the most recent version of XCM that they
	/// support in a `QueryResponse` instruction. Any changes to this should also elicit similar
	/// responses when they happen.
	///
	/// Kind: *Instruction*
	SubscribeVersion {
		#[codec(compact)]
		query_id: QueryId,
		#[codec(compact)]
		max_response_weight: u64,
	},

	/// Cancel the effect of a previous `SubscribeVersion` instruction.
	///
	/// Kind: *Instruction*
	UnsubscribeVersion,
}

impl<RuntimeCall> Xcm<RuntimeCall> {
	pub fn into<C>(self) -> Xcm<C> {
		Xcm::from(self)
	}
	pub fn from<C>(xcm: Xcm<C>) -> Self {
		Self(xcm.0.into_iter().map(Instruction::<RuntimeCall>::from).collect())
	}
}

impl<RuntimeCall> Instruction<RuntimeCall> {
	pub fn into<C>(self) -> Instruction<C> {
		Instruction::from(self)
	}
	pub fn from<C>(xcm: Instruction<C>) -> Self {
		use Instruction::*;
		match xcm {
			WithdrawAsset(assets) => WithdrawAsset(assets),
			ReserveAssetDeposited(assets) => ReserveAssetDeposited(assets),
			ReceiveTeleportedAsset(assets) => ReceiveTeleportedAsset(assets),
			QueryResponse { query_id, response, max_weight } =>
				QueryResponse { query_id, response, max_weight },
			TransferAsset { assets, beneficiary } => TransferAsset { assets, beneficiary },
			TransferReserveAsset { assets, dest, xcm } =>
				TransferReserveAsset { assets, dest, xcm },
			HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity } =>
				HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity },
			HrmpChannelAccepted { recipient } => HrmpChannelAccepted { recipient },
			HrmpChannelClosing { initiator, sender, recipient } =>
				HrmpChannelClosing { initiator, sender, recipient },
			Transact { origin_type, require_weight_at_most, call } =>
				Transact { origin_type, require_weight_at_most, call: call.into() },
			ReportError { query_id, dest, max_response_weight } =>
				ReportError { query_id, dest, max_response_weight },
			DepositAsset { assets, max_assets, beneficiary } =>
				DepositAsset { assets, max_assets, beneficiary },
			DepositReserveAsset { assets, max_assets, dest, xcm } =>
				DepositReserveAsset { assets, max_assets, dest, xcm },
			ExchangeAsset { give, receive } => ExchangeAsset { give, receive },
			InitiateReserveWithdraw { assets, reserve, xcm } =>
				InitiateReserveWithdraw { assets, reserve, xcm },
			InitiateTeleport { assets, dest, xcm } => InitiateTeleport { assets, dest, xcm },
			QueryHolding { query_id, dest, assets, max_response_weight } =>
				QueryHolding { query_id, dest, assets, max_response_weight },
			BuyExecution { fees, weight_limit } => BuyExecution { fees, weight_limit },
			ClearOrigin => ClearOrigin,
			DescendOrigin(who) => DescendOrigin(who),
			RefundSurplus => RefundSurplus,
			SetErrorHandler(xcm) => SetErrorHandler(xcm.into()),
			SetAppendix(xcm) => SetAppendix(xcm.into()),
			ClearError => ClearError,
			ClaimAsset { assets, ticket } => ClaimAsset { assets, ticket },
			Trap(code) => Trap(code),
			SubscribeVersion { query_id, max_response_weight } =>
				SubscribeVersion { query_id, max_response_weight },
			UnsubscribeVersion => UnsubscribeVersion,
		}
	}
}

// TODO: Automate Generation
impl<RuntimeCall, W: XcmWeightInfo<RuntimeCall>> GetWeight<W> for Instruction<RuntimeCall> {
	fn weight(&self) -> Weight {
		use Instruction::*;
		match self {
			WithdrawAsset(assets) => W::withdraw_asset(assets),
			ReserveAssetDeposited(assets) => W::reserve_asset_deposited(assets),
			ReceiveTeleportedAsset(assets) => W::receive_teleported_asset(assets),
			QueryResponse { query_id, response, max_weight } =>
				W::query_response(query_id, response, max_weight),
			TransferAsset { assets, beneficiary } => W::transfer_asset(assets, beneficiary),
			TransferReserveAsset { assets, dest, xcm } =>
				W::transfer_reserve_asset(&assets, dest, xcm),
			Transact { origin_type, require_weight_at_most, call } =>
				W::transact(origin_type, require_weight_at_most, call),
			HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity } =>
				W::hrmp_new_channel_open_request(sender, max_message_size, max_capacity),
			HrmpChannelAccepted { recipient } => W::hrmp_channel_accepted(recipient),
			HrmpChannelClosing { initiator, sender, recipient } =>
				W::hrmp_channel_closing(initiator, sender, recipient),
			ClearOrigin => W::clear_origin(),
			DescendOrigin(who) => W::descend_origin(who),
			ReportError { query_id, dest, max_response_weight } =>
				W::report_error(query_id, dest, max_response_weight),
			DepositAsset { assets, max_assets, beneficiary } =>
				W::deposit_asset(assets, max_assets, beneficiary),
			DepositReserveAsset { assets, max_assets, dest, xcm } =>
				W::deposit_reserve_asset(assets, max_assets, dest, xcm),
			ExchangeAsset { give, receive } => W::exchange_asset(give, receive),
			InitiateReserveWithdraw { assets, reserve, xcm } =>
				W::initiate_reserve_withdraw(assets, reserve, xcm),
			InitiateTeleport { assets, dest, xcm } => W::initiate_teleport(assets, dest, xcm),
			QueryHolding { query_id, dest, assets, max_response_weight } =>
				W::query_holding(query_id, dest, assets, max_response_weight),
			BuyExecution { fees, weight_limit } => W::buy_execution(fees, weight_limit),
			RefundSurplus => W::refund_surplus(),
			SetErrorHandler(xcm) => W::set_error_handler(xcm),
			SetAppendix(xcm) => W::set_appendix(xcm),
			ClearError => W::clear_error(),
			ClaimAsset { assets, ticket } => W::claim_asset(assets, ticket),
			Trap(code) => W::trap(code),
			SubscribeVersion { query_id, max_response_weight } =>
				W::subscribe_version(query_id, max_response_weight),
			UnsubscribeVersion => W::unsubscribe_version(),
		}
	}
}

pub mod opaque {
	/// The basic concrete type of `Xcm`, which doesn't make any assumptions about the
	/// format of a call other than it is pre-encoded.
	pub type Xcm = super::Xcm<()>;

	/// The basic concrete type of `Instruction`, which doesn't make any assumptions about the
	/// format of a call other than it is pre-encoded.
	pub type Instruction = super::Instruction<()>;
}

// Convert from a v1 response to a v2 response
impl TryFrom<OldResponse> for Response {
	type Error = ();
	fn try_from(old_response: OldResponse) -> result::Result<Self, ()> {
		match old_response {
			OldResponse::Assets(assets) => Ok(Self::Assets(assets)),
			OldResponse::Version(version) => Ok(Self::Version(version)),
		}
	}
}

impl<RuntimeCall> TryFrom<OldXcm<RuntimeCall>> for Xcm<RuntimeCall> {
	type Error = ();
	fn try_from(old: OldXcm<RuntimeCall>) -> result::Result<Xcm<RuntimeCall>, ()> {
		use Instruction::*;
		Ok(Xcm(match old {
			OldXcm::WithdrawAsset { assets, effects } => Some(Ok(WithdrawAsset(assets)))
				.into_iter()
				.chain(effects.into_iter().map(Instruction::try_from))
				.collect::<result::Result<Vec<_>, _>>()?,
			OldXcm::ReserveAssetDeposited { assets, effects } =>
				Some(Ok(ReserveAssetDeposited(assets)))
					.into_iter()
					.chain(Some(Ok(ClearOrigin)).into_iter())
					.chain(effects.into_iter().map(Instruction::try_from))
					.collect::<result::Result<Vec<_>, _>>()?,
			OldXcm::ReceiveTeleportedAsset { assets, effects } =>
				Some(Ok(ReceiveTeleportedAsset(assets)))
					.into_iter()
					.chain(Some(Ok(ClearOrigin)).into_iter())
					.chain(effects.into_iter().map(Instruction::try_from))
					.collect::<result::Result<Vec<_>, _>>()?,
			OldXcm::QueryResponse { query_id, response } => vec![QueryResponse {
				query_id,
				response: response.try_into()?,
				max_weight: 50_000_000,
			}],
			OldXcm::TransferAsset { assets, beneficiary } =>
				vec![TransferAsset { assets, beneficiary }],
			OldXcm::TransferReserveAsset { assets, dest, effects } => vec![TransferReserveAsset {
				assets,
				dest,
				xcm: Xcm(effects
					.into_iter()
					.map(Instruction::<()>::try_from)
					.collect::<result::Result<_, _>>()?),
			}],
			OldXcm::HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity } =>
				vec![HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity }],
			OldXcm::HrmpChannelAccepted { recipient } => vec![HrmpChannelAccepted { recipient }],
			OldXcm::HrmpChannelClosing { initiator, sender, recipient } =>
				vec![HrmpChannelClosing { initiator, sender, recipient }],
			OldXcm::Transact { origin_type, require_weight_at_most, call } =>
				vec![Transact { origin_type, require_weight_at_most, call }],
			// We don't handle this one at all due to nested XCM.
			OldXcm::RelayedFrom { .. } => return Err(()),
			OldXcm::SubscribeVersion { query_id, max_response_weight } =>
				vec![SubscribeVersion { query_id, max_response_weight }],
			OldXcm::UnsubscribeVersion => vec![UnsubscribeVersion],
		}))
	}
}

impl<RuntimeCall> TryFrom<OldOrder<RuntimeCall>> for Instruction<RuntimeCall> {
	type Error = ();
	fn try_from(old: OldOrder<RuntimeCall>) -> result::Result<Instruction<RuntimeCall>, ()> {
		use Instruction::*;
		Ok(match old {
			OldOrder::Noop => return Err(()),
			OldOrder::DepositAsset { assets, max_assets, beneficiary } =>
				DepositAsset { assets, max_assets, beneficiary },
			OldOrder::DepositReserveAsset { assets, max_assets, dest, effects } =>
				DepositReserveAsset {
					assets,
					max_assets,
					dest,
					xcm: Xcm(effects
						.into_iter()
						.map(Instruction::<()>::try_from)
						.collect::<result::Result<_, _>>()?),
				},
			OldOrder::ExchangeAsset { give, receive } => ExchangeAsset { give, receive },
			OldOrder::InitiateReserveWithdraw { assets, reserve, effects } =>
				InitiateReserveWithdraw {
					assets,
					reserve,
					xcm: Xcm(effects
						.into_iter()
						.map(Instruction::<()>::try_from)
						.collect::<result::Result<_, _>>()?),
				},
			OldOrder::InitiateTeleport { assets, dest, effects } => InitiateTeleport {
				assets,
				dest,
				xcm: Xcm(effects
					.into_iter()
					.map(Instruction::<()>::try_from)
					.collect::<result::Result<_, _>>()?),
			},
			OldOrder::QueryHolding { query_id, dest, assets } =>
				QueryHolding { query_id, dest, assets, max_response_weight: 0 },
			OldOrder::BuyExecution { fees, debt, instructions, .. } => {
				// We don't handle nested XCM.
				if !instructions.is_empty() {
					return Err(())
				}
				BuyExecution { fees, weight_limit: WeightLimit::Limited(debt) }
			},
		})
	}
}

#[cfg(test)]
mod tests {
	use super::{prelude::*, *};

	#[test]
	fn basic_roundtrip_works() {
		let xcm =
			Xcm::<()>(vec![TransferAsset { assets: (Here, 1).into(), beneficiary: Here.into() }]);
		let old_xcm =
			OldXcm::<()>::TransferAsset { assets: (Here, 1).into(), beneficiary: Here.into() };
		assert_eq!(old_xcm, OldXcm::<()>::try_from(xcm.clone()).unwrap());
		let new_xcm: Xcm<()> = old_xcm.try_into().unwrap();
		assert_eq!(new_xcm, xcm);
	}

	#[test]
	fn teleport_roundtrip_works() {
		let xcm = Xcm::<()>(vec![
			ReceiveTeleportedAsset((Here, 1).into()),
			ClearOrigin,
			DepositAsset { assets: Wild(All), max_assets: 1, beneficiary: Here.into() },
		]);
		let old_xcm: OldXcm<()> = OldXcm::<()>::ReceiveTeleportedAsset {
			assets: (Here, 1).into(),
			effects: vec![OldOrder::DepositAsset {
				assets: Wild(All),
				max_assets: 1,
				beneficiary: Here.into(),
			}],
		};
		assert_eq!(old_xcm, OldXcm::<()>::try_from(xcm.clone()).unwrap());
		let new_xcm: Xcm<()> = old_xcm.try_into().unwrap();
		assert_eq!(new_xcm, xcm);
	}

	#[test]
	fn reserve_deposit_roundtrip_works() {
		let xcm = Xcm::<()>(vec![
			ReserveAssetDeposited((Here, 1).into()),
			ClearOrigin,
			BuyExecution { fees: (Here, 1).into(), weight_limit: Some(1).into() },
			DepositAsset { assets: Wild(All), max_assets: 1, beneficiary: Here.into() },
		]);
		let old_xcm: OldXcm<()> = OldXcm::<()>::ReserveAssetDeposited {
			assets: (Here, 1).into(),
			effects: vec![
				OldOrder::BuyExecution {
					fees: (Here, 1).into(),
					debt: 1,
					weight: 0,
					instructions: vec![],
					halt_on_error: true,
				},
				OldOrder::DepositAsset {
					assets: Wild(All),
					max_assets: 1,
					beneficiary: Here.into(),
				},
			],
		};
		assert_eq!(old_xcm, OldXcm::<()>::try_from(xcm.clone()).unwrap());
		let new_xcm: Xcm<()> = old_xcm.try_into().unwrap();
		assert_eq!(new_xcm, xcm);
	}
}
