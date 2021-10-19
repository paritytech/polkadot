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

//! Version 3 of the Cross-Consensus Message format data structures.

use super::v2::{Instruction as OldInstruction, Response as OldResponse, Xcm as OldXcm};
use crate::{DoubleEncoded, GetWeight};
use alloc::{vec, vec::Vec};
use core::{
	convert::{TryFrom, TryInto},
	fmt::Debug,
	result,
};
use derivative::Derivative;
use parity_scale_codec::{self, Decode, Encode};
use scale_info::TypeInfo;

mod traits;

pub use traits::{
	Error, ExecuteXcm, Outcome, Result, SendError, SendResult, SendXcm, Weight, XcmWeightInfo,
};
// These parts of XCM v1 have been unchanged in XCM v2, and are re-imported here.
pub use super::v2::{
	Ancestor, AncestorThen, AssetId, AssetInstance, BodyId, BodyPart, Fungibility,
	InteriorMultiLocation, Junction, Junctions, MultiAsset, MultiAssetFilter, MultiAssets,
	MultiLocation, NetworkId, OriginKind, Parent, ParentThen, WeightLimit, WildFungibility,
	WildMultiAsset,
};

/// This module's XCM version.
pub const VERSION: super::Version = 3;

/// An identifier for a query.
pub type QueryId = u64;

#[derive(Derivative, Default, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
#[scale_info(bounds(), skip_type_params(Call))]
pub struct Xcm<Call>(pub Vec<Instruction<Call>>);

impl<Call> Xcm<Call> {
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
	pub fn first(&self) -> Option<&Instruction<Call>> {
		self.0.first()
	}

	/// Return the last instruction, if any.
	pub fn last(&self) -> Option<&Instruction<Call>> {
		self.0.last()
	}

	/// Return the only instruction, contained in `Self`, iff only one exists (`None` otherwise).
	pub fn only(&self) -> Option<&Instruction<Call>> {
		if self.0.len() == 1 {
			self.0.first()
		} else {
			None
		}
	}

	/// Return the only instruction, contained in `Self`, iff only one exists (returns `self`
	/// otherwise).
	pub fn into_only(mut self) -> core::result::Result<Instruction<Call>, Self> {
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

#[derive(Clone, Eq, PartialEq, Encode, Decode, Debug, TypeInfo)]
pub struct PalletInfo {
	#[codec(compact)]
	pub index: u32,
	pub name: Vec<u8>,
	pub module_name: Vec<u8>,
	#[codec(compact)]
	pub major: u32,
	#[codec(compact)]
	pub minor: u32,
	#[codec(compact)]
	pub patch: u32,
}

#[derive(Clone, Eq, PartialEq, Encode, Decode, Debug, TypeInfo)]
pub enum MaybeErrorCode {
	Success,
	Error(Vec<u8>),
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
	/// The index, instance name, pallet name and version of some pallets.
	PalletsInfo(Vec<PalletInfo>),
	/// The error of a dispatch attempt, or `None` if the dispatch executed without error.
	DispatchResult(MaybeErrorCode),
}

impl Default for Response {
	fn default() -> Self {
		Self::Null
	}
}

/// Information regarding the composition of a query response.
#[derive(Clone, Eq, PartialEq, Encode, Decode, Debug, TypeInfo)]
pub struct QueryResponseInfo {
	/// The destination to which the query response message should be send.
	pub destination: MultiLocation,
	/// The `query_id` field of the `QueryResponse` message.
	#[codec(compact)]
	pub query_id: QueryId,
	/// The `max_weight` field of the `QueryResponse` message.
	#[codec(compact)]
	pub max_weight: Weight,
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
#[scale_info(bounds(), skip_type_params(Call))]
pub enum Instruction<Call> {
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
		max_weight: Weight,
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
		call: DoubleEncoded<Call>,
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
	/// - `query_id`: An identifier that will be replicated into the returned XCM message.
	/// - `dest`: A valid destination for the returned XCM message.
	/// - `max_response_weight`: The maximum amount of weight that the `QueryResponse` item which
	///   is sent as a reply may take to execute. NOTE: If this is unexpectedly large then the
	///   response may not execute at all.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	ReportError {
		#[codec(compact)]
		query_id: QueryId,
		dest: MultiLocation,
		#[codec(compact)]
		max_response_weight: Weight,
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
		max_response_weight: Weight,
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
	SetErrorHandler(Xcm<Call>),

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
	SetAppendix(Xcm<Call>),

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
	/// - `query_id`: An identifier that will be replicated into the returned XCM message.
	/// - `max_response_weight`: The maximum amount of weight that the `QueryResponse` item which
	///   is sent as a reply may take to execute. NOTE: If this is unexpectedly large then the
	///   response may not execute at all.
	///
	/// Kind: *Instruction*
	///
	/// Errors: *Fallible*
	SubscribeVersion {
		#[codec(compact)]
		query_id: QueryId,
		#[codec(compact)]
		max_response_weight: Weight,
	},

	/// Cancel the effect of a previous `SubscribeVersion` instruction.
	///
	/// Kind: *Instruction*
	///
	/// Errors: *Fallible*
	UnsubscribeVersion,

	/// Reduce Holding by up to the given assets.
	///
	/// Holding is reduced by up to the assets in the parameter. If this is less than the
	///
	/// Kind: *Instruction*
	///
	/// Errors: *Fallible*
	BurnAsset(MultiAssets),

	/// Throw an error if Holding does not contain at least the given assets.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	/// - `ExpectationFalse`: If Holding Register does not contain the assets in the parameter.
	ExpectAsset(MultiAssets),

	/// Ensure that the Origin Register equals some given value and throw an error if not.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	/// - `ExpectationFalse`: If Origin Register is not some value, or if that value is not equal to
	///   the parameter.
	ExpectOrigin(MultiLocation),

	/// Ensure that the Error Register equals some given value and throw an error if not.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	/// - `ExpectationFalse`: If the value of the Error Register is not equal to the parameter.
	ExpectError(Option<(u32, Error)>),

	/// Query the existence of a particular pallet type.
	///
	/// - `name`: The name of the pallet to query.
	/// - `response_info`: Information for making the response.
	///
	/// Sends a `QueryResponse` to Origin whose data field `PalletsInfo` containing the information
	/// of all pallets on the local chain whose name is equal to `name`. This is empty in the case
	/// that the local chain is not based on Substrate Frame.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*
	///
	/// Errors: *Fallible*.
	QueryPallet { name: Vec<u8>, response_info: QueryResponseInfo },

	/// Dispatch a call into a pallet in the Frame system. This provides a means of ensuring that
	/// the pallet continues to exist with a known version.
	///
	/// - `origin_kind`: The means of expressing the message origin as a dispatch origin.
	/// - `name`: The name of the pallet to which to dispatch a message.
	/// - `major`, `minor`: The major and minor version of the pallet. The major version
	///   must be equal and the minor version of the pallet must be at least as great.
	/// - `pallet_index`: The index of the pallet to be called.
	/// - `call_index`: The index of the dispatchable to be called.
	/// - `params`: The encoded parameters of the dispatchable.
	/// - `query_id`: If present, then a `QueryResponse`
	///   whose `query_id` and `max_weight` are the given `QueryId`and `Weight` values is sent to
	///   the given `MultiLocation` value with a `DispatchResult` response corresponding to the
	///   error status of the "inner" dispatch. This only happens if the dispatch was actually
	///   made - if an error happened prior to dispatch, then the Error Register is set and the
	///   operation aborted as usual.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Instruction*
	///
	/// Errors: *Fallible*.
	Dispatch {
		origin_kind: OriginKind,
		#[codec(compact)]
		require_weight_at_most: Weight,
		name: Vec<u8>,
		#[codec(compact)]
		major: u32,
		#[codec(compact)]
		minor: u32,
		#[codec(compact)]
		pallet_index: u32,
		#[codec(compact)]
		call_index: u32,
		params: Vec<u8>,
		response_info: Option<QueryResponseInfo>,
	},
}

impl<Call> Xcm<Call> {
	pub fn into<C>(self) -> Xcm<C> {
		Xcm::from(self)
	}
	pub fn from<C>(xcm: Xcm<C>) -> Self {
		Self(xcm.0.into_iter().map(Instruction::<Call>::from).collect())
	}
}

impl<Call> Instruction<Call> {
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
			BurnAsset(assets) => BurnAsset(assets),
			ExpectAsset(assets) => ExpectAsset(assets),
			ExpectOrigin(origin) => ExpectOrigin(origin),
			ExpectError(error) => ExpectError(error),
			QueryPallet { name, response_info } => QueryPallet { name, response_info },
			Dispatch {
				origin_kind,
				require_weight_at_most,
				name,
				major,
				minor,
				pallet_index,
				call_index,
				params,
				response_info,
			} => Dispatch {
				origin_kind,
				require_weight_at_most,
				name,
				major,
				minor,
				pallet_index,
				call_index,
				params,
				response_info,
			},
		}
	}
}

// TODO: Automate Generation
impl<Call, W: XcmWeightInfo<Call>> GetWeight<W> for Instruction<Call> {
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
			BurnAsset(assets) => W::burn_asset(assets),
			ExpectAsset(assets) => W::expect_asset(assets),
			ExpectOrigin(origin) => W::expect_origin(origin),
			ExpectError(error) => W::expect_error(error),
			QueryPallet { .. } => W::query_pallet(),
			Dispatch {
				origin_kind,
				require_weight_at_most,
				pallet_index,
				call_index,
				params,
				response_info,
				..
			} => W::dispatch(
				origin_kind,
				require_weight_at_most,
				pallet_index,
				call_index,
				params,
				response_info,
			),
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

// Convert from a v2 response to a v3 response.
impl TryFrom<OldResponse> for Response {
	type Error = ();
	fn try_from(old_response: OldResponse) -> result::Result<Self, ()> {
		match old_response {
			OldResponse::Assets(assets) => Ok(Self::Assets(assets)),
			OldResponse::Version(version) => Ok(Self::Version(version)),
			OldResponse::ExecutionResult(error) => Ok(Self::ExecutionResult(match error {
				Some((i, e)) => Some((i, e.try_into()?)),
				None => None,
			})),
			OldResponse::Null => Ok(Self::Null),
		}
	}
}

// Convert from a v2 XCM to a v3 XCM.
impl<Call> TryFrom<OldXcm<Call>> for Xcm<Call> {
	type Error = ();
	fn try_from(old_xcm: OldXcm<Call>) -> result::Result<Self, ()> {
		Ok(Xcm(old_xcm.0.into_iter().map(TryInto::try_into).collect::<result::Result<_, _>>()?))
	}
}

// Convert from a v2 instruction to a v3 instruction.
impl<Call> TryFrom<OldInstruction<Call>> for Instruction<Call> {
	type Error = ();
	fn try_from(old_instruction: OldInstruction<Call>) -> result::Result<Self, ()> {
		use OldInstruction::*;
		Ok(match old_instruction {
			WithdrawAsset(assets) => Self::WithdrawAsset(assets),
			ReserveAssetDeposited(assets) => Self::ReserveAssetDeposited(assets),
			ReceiveTeleportedAsset(assets) => Self::ReceiveTeleportedAsset(assets),
			QueryResponse { query_id, response, max_weight } =>
				Self::QueryResponse { query_id, response: response.try_into()?, max_weight },
			TransferAsset { assets, beneficiary } => Self::TransferAsset { assets, beneficiary },
			TransferReserveAsset { assets, dest, xcm } =>
				Self::TransferReserveAsset { assets, dest, xcm: xcm.try_into()? },
			HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity } =>
				Self::HrmpNewChannelOpenRequest { sender, max_message_size, max_capacity },
			HrmpChannelAccepted { recipient } => Self::HrmpChannelAccepted { recipient },
			HrmpChannelClosing { initiator, sender, recipient } =>
				Self::HrmpChannelClosing { initiator, sender, recipient },
			Transact { origin_type, require_weight_at_most, call } =>
				Self::Transact { origin_type, require_weight_at_most, call: call.into() },
			ReportError { query_id, dest, max_response_weight } =>
				Self::ReportError { query_id, dest, max_response_weight },
			DepositAsset { assets, max_assets, beneficiary } =>
				Self::DepositAsset { assets, max_assets, beneficiary },
			DepositReserveAsset { assets, max_assets, dest, xcm } =>
				Self::DepositReserveAsset { assets, max_assets, dest, xcm: xcm.try_into()? },
			ExchangeAsset { give, receive } => Self::ExchangeAsset { give, receive },
			InitiateReserveWithdraw { assets, reserve, xcm } =>
				Self::InitiateReserveWithdraw { assets, reserve, xcm: xcm.try_into()? },
			InitiateTeleport { assets, dest, xcm } =>
				Self::InitiateTeleport { assets, dest, xcm: xcm.try_into()? },
			QueryHolding { query_id, dest, assets, max_response_weight } =>
				Self::QueryHolding { query_id, dest, assets, max_response_weight },
			BuyExecution { fees, weight_limit } => Self::BuyExecution { fees, weight_limit },
			ClearOrigin => Self::ClearOrigin,
			DescendOrigin(who) => Self::DescendOrigin(who),
			RefundSurplus => Self::RefundSurplus,
			SetErrorHandler(xcm) => Self::SetErrorHandler(xcm.try_into()?),
			SetAppendix(xcm) => Self::SetAppendix(xcm.try_into()?),
			ClearError => Self::ClearError,
			ClaimAsset { assets, ticket } => Self::ClaimAsset { assets, ticket },
			Trap(code) => Self::Trap(code),
			SubscribeVersion { query_id, max_response_weight } =>
				Self::SubscribeVersion { query_id, max_response_weight },
			UnsubscribeVersion => Self::UnsubscribeVersion,
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
		let old_xcm = OldXcm::<()>(vec![OldInstruction::TransferAsset {
			assets: (Here, 1).into(),
			beneficiary: Here.into(),
		}]);
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
		let old_xcm: OldXcm<()> = OldXcm::<()>(vec![
			OldInstruction::ReceiveTeleportedAsset((Here, 1).into()),
			OldInstruction::ClearOrigin,
			OldInstruction::DepositAsset {
				assets: Wild(All),
				max_assets: 1,
				beneficiary: Here.into(),
			},
		]);
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
		let old_xcm = OldXcm::<()>(vec![
			OldInstruction::ReserveAssetDeposited((Here, 1).into()),
			OldInstruction::ClearOrigin,
			OldInstruction::BuyExecution { fees: (Here, 1).into(), weight_limit: Some(1).into() },
			OldInstruction::DepositAsset {
				assets: Wild(All),
				max_assets: 1,
				beneficiary: Here.into(),
			},
		]);
		assert_eq!(old_xcm, OldXcm::<()>::try_from(xcm.clone()).unwrap());
		let new_xcm: Xcm<()> = old_xcm.try_into().unwrap();
		assert_eq!(new_xcm, xcm);
	}
}
