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

//! Version 1 of the Cross-Consensus Message format data structures.

use super::v1::{Order as OldOrder, Response as OldResponse, Xcm as OldXcm};
use crate::DoubleEncoded;
use alloc::{vec, vec::Vec};
use core::{
	convert::{TryFrom, TryInto},
	fmt::Debug,
	result,
};
use derivative::Derivative;
use parity_scale_codec::{self, Decode, Encode};

mod traits;

pub use traits::{Error, ExecuteXcm, Outcome, Result, SendError, SendResult, SendXcm};
// These parts of XCM v1 have been unchanged in XCM v2, and are re-imported here.
pub use super::v1::{
	Ancestor, AncestorThen, AssetId, AssetInstance, BodyId, BodyPart, Fungibility,
	InteriorMultiLocation, Junction, Junctions, MultiAsset, MultiAssetFilter, MultiAssets,
	MultiLocation, NetworkId, OriginKind, Parent, ParentThen, WildFungibility, WildMultiAsset,
};

#[derive(Derivative, Encode, Decode)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub struct Xcm<Call>(pub Vec<Instruction<Call>>);

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
			OriginKind, Outcome, Parent, ParentThen, Response, Result as XcmResult, SendError,
			SendResult, SendXcm,
			WeightLimit::{self, *},
			WildFungibility::{self, Fungible as WildFungible, NonFungible as WildNonFungible},
			WildMultiAsset::{self, *},
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
#[derive(Clone, Eq, PartialEq, Encode, Decode, Debug)]
pub enum Response {
	/// Some assets.
	Assets(MultiAssets),
	/// The outcome of an XCM instruction.
	ExecutionResult(result::Result<(), (u32, Error)>),
}

/// An optional weight limit.
#[derive(Clone, Eq, PartialEq, Encode, Decode, Debug)]
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

/// Cross-Consensus Message: A message from one consensus system to another.
///
/// Consensus systems that may send and receive messages include blockchains and smart contracts.
///
/// All messages are delivered from a known *origin*, expressed as a `MultiLocation`.
///
/// This is the inner XCM format and is version-sensitive. Messages are typically passed using the outer
/// XCM format, known as `VersionedXcm`.
#[derive(Derivative, Encode, Decode)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum Instruction<Call> {
	/// Withdraw asset(s) (`assets`) from the ownership of `origin` and place them into the Holding
	/// Register.
	///
	/// - `assets`: The asset(s) to be withdrawn into holding.
	///
	/// Kind: *Instruction*.
	///
	/// Errors:
	WithdrawAsset { assets: MultiAssets },

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
	ReserveAssetDeposited { assets: MultiAssets },

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
	ReceiveTeleportedAsset { assets: MultiAssets },

	/// Indication of the contents of the holding register corresponding to the `QueryHolding`
	/// order of `query_id`.
	///
	/// - `query_id`: The identifier of the query that resulted in this message being sent.
	/// - `assets`: The message content.
	///
	/// Safety: No concerns.
	///
	/// Kind: *Information*.
	///
	/// Errors:
	QueryResponse {
		#[codec(compact)]
		query_id: u64,
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
	Transact { origin_type: OriginKind, require_weight_at_most: u64, call: DoubleEncoded<Call> },

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

	/// When execution of the current XCM scope finished report its outcome to `dest`.
	///
	/// A `QueryResponse` message of type `ExecutionOutcome` is sent to `dest` with the given
	/// `query_id` and the outcome of the XCM.
	///
	/// Kind: *Instruction*
	///
	/// Errors:
	ReportOutcome {
		#[codec(compact)]
		query_id: u64,
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
	/// Errors:
	DepositAsset { assets: MultiAssetFilter, max_assets: u32, beneficiary: MultiLocation },

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
	/// Errors:
	DepositReserveAsset {
		assets: MultiAssetFilter,
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
	/// Errors:
	QueryHolding {
		#[codec(compact)]
		query_id: u64,
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
	/// Errors:
	BuyExecution { fees: MultiAsset, weight_limit: WeightLimit },

	/// Refund any surplus weight previously bought with `BuyExecution`.
	RefundSurplus,
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
			WithdrawAsset { assets } => WithdrawAsset { assets },
			ReserveAssetDeposited { assets } => ReserveAssetDeposited { assets },
			ReceiveTeleportedAsset { assets } => ReceiveTeleportedAsset { assets },
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
			ReportOutcome { query_id, dest, max_response_weight } =>
				ReportOutcome { query_id, dest, max_response_weight },
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
		}
	}
}

impl<Call> TryFrom<OldXcm<Call>> for Xcm<Call> {
	type Error = ();
	fn try_from(old: OldXcm<Call>) -> result::Result<Xcm<Call>, ()> {
		use Instruction::*;
		Ok(Xcm(match old {
			OldXcm::WithdrawAsset { assets, effects } => Some(Ok(WithdrawAsset { assets }))
				.into_iter()
				.chain(effects.into_iter().map(Instruction::try_from))
				.collect::<result::Result<Vec<_>, _>>()?,
			OldXcm::ReserveAssetDeposited { assets, effects } =>
				Some(Ok(ReserveAssetDeposited { assets }))
					.into_iter()
					.chain(Some(Ok(ClearOrigin)).into_iter())
					.chain(effects.into_iter().map(Instruction::try_from))
					.collect::<result::Result<Vec<_>, _>>()?,
			OldXcm::ReceiveTeleportedAsset { assets, effects } =>
				Some(Ok(ReceiveTeleportedAsset { assets }))
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
		}))
	}
}

impl<Call> TryFrom<OldOrder<Call>> for Instruction<Call> {
	type Error = ();
	fn try_from(old: OldOrder<Call>) -> result::Result<Instruction<Call>, ()> {
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
			ReceiveTeleportedAsset { assets: (Here, 1).into() },
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
			ReserveAssetDeposited { assets: (Here, 1).into() },
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
