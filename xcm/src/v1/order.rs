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

use super::{
	super::v0::Order as Order0, MultiAsset, MultiAssetFilter, MultiAssets, MultiLocation, Xcm,
};
use alloc::{vec, vec::Vec};
use core::{
	convert::{TryFrom, TryInto},
	result,
};
use derivative::Derivative;
use parity_scale_codec::{self, Decode, Encode};

/// An instruction to be executed on some or all of the assets in the Holding Register, used by
/// asset-related XCM messages.
///
/// The Holding Register is a temporary place used to keep track of all assets in motion, e.g.
/// withdrawn from accounts or teleported in via XCM.
#[derive(Derivative, Encode, Decode)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum Order<Call> {
	/// Do nothing. Not generally used.
	#[codec(index = 0)]
	Noop,

	/// Remove the asset(s) (`assets`) from the Holding Register and place equivalent assets under
	/// the ownership of `beneficiary` within this consensus system.
	///
	/// - `assets`: The asset(s) to remove from the Holding Register.
	/// - `max_assets`: The maximum number of unique assets/asset instances to remove from the
	///   Holding Register. Only the first `max_assets` assets/instances of those matched by
	///   `assets` will be removed, prioritized under standard asset ordering. Any others will
	///   remain in the Holding Register.
	/// - `beneficiary`: The new owner for the assets.
	///
	/// Errors:
	/// - Errors occurring when trying to deposit the asset(s).
	#[codec(index = 1)]
	DepositAsset { assets: MultiAssetFilter, max_assets: u32, beneficiary: MultiLocation },

	/// Remove the asset(s) (`assets`) from the Holding Register and place equivalent assets under
	/// the ownership of `dest` within this consensus system (i.e. its sovereign account).
	///
	/// Send an onward XCM message to `dest` of `ReserveAssetDeposited` with the given `effects`.
	///
	/// - `assets`: The asset(s) to remove from the Holding Register.
	/// - `max_assets`: The maximum number of unique assets/asset instances to remove from the
	///   Holding Register. Only the first `max_assets` assets/instances of those matched by
	///   `assets` will be removed, prioritized under standard asset ordering. Any others will
	///   remain in the Holding Register.
	/// - `dest`: The location whose sovereign account will own the assets and thus the effective
	///   beneficiary for the assets and the notification target for the reserve asset deposit
	///   message.
	/// - `effects`: The orders that should be contained in the `ReserveAssetDeposited` which is
	///   sent onwards to `dest`.
	///
	/// Errors:
	/// - Errors occurring when trying to deposit the asset(s).
	/// - Errors occurring when trying to send the `ReserveAssetDeposited` XCM.
	#[codec(index = 2)]
	DepositReserveAsset {
		assets: MultiAssetFilter,
		max_assets: u32,
		dest: MultiLocation,
		effects: Vec<Order<()>>,
	},

	/// Remove the asset(s) (`give`) from the Holding Register and replace them with alternative
	/// assets.
	///
	/// The minimum amount of assets to be received into the Holding Register for the order not to
	/// fail may be stated.
	///
	/// - `give`: The asset(s) to remove from the Holding Register.
	/// - `receive`: The minimum amount of assets(s) which `give` should be exchanged for.
	///
	/// Errors:
	#[codec(index = 3)]
	ExchangeAsset { give: MultiAssetFilter, receive: MultiAssets },

	/// Remove the asset(s) (`assets`) from the Holding Register and send a `WithdrawAsset` XCM
	/// message to a reserve location.
	///
	/// - `assets`: The asset(s) to remove from the Holding Register.
	/// - `reserve`: A valid location that acts as a reserve for all asset(s) in `assets`. The
	///   sovereign account of this consensus system *on the reserve location* will have appropriate
	///   assets withdrawn and `effects` will be executed on them. There will typically be only one
	///   valid location on any given asset/chain combination.
	/// - `effects`: The orders to execute on the assets once withdrawn *on the reserve location*.
	///
	/// Errors:
	/// - Errors occurring when trying to send the `WithdrawAsset` XCM.
	#[codec(index = 4)]
	InitiateReserveWithdraw {
		assets: MultiAssetFilter,
		reserve: MultiLocation,
		effects: Vec<Order<()>>,
	},

	/// Remove the asset(s) (`assets`) from the Holding Register and send a `ReceiveTeleportedAsset`
	/// XCM message to a `destination` location.
	///
	/// - `assets`: The asset(s) to remove from the Holding Register.
	/// - `dest`: A valid location that has a bi-lateral teleportation arrangement.
	/// - `effects`: The orders to execute on the assets once arrived *on the destination location*.
	///
	/// NOTE: The `destination` location *MUST* respect this origin as a valid teleportation origin
	/// for all `assets`. If it does not, then the assets may be lost.
	///
	/// Errors:
	/// - Errors occurring when trying to send the `ReceiveTeleportedAsset` XCM.
	#[codec(index = 5)]
	InitiateTeleport { assets: MultiAssetFilter, dest: MultiLocation, effects: Vec<Order<()>> },

	/// Send a `Balances` XCM message with the `assets` value equal to the Holding Register
	/// contents, or a portion thereof.
	///
	/// - `query_id`: An identifier that will be replicated into the returned XCM message.
	/// - `dest`: A valid destination for the returned XCM message. This may be limited to the
	///   current origin.
	/// - `assets`: A filter for the assets that should be reported back. The assets reported back
	///   will be, asset- wise, *the lesser of this value and the Holding Register*. No wildcards
	///   will be used when reporting assets back.
	///
	/// Errors:
	/// - Errors occurring when trying to send the `QueryResponse` XCM.
	#[codec(index = 6)]
	QueryHolding {
		#[codec(compact)]
		query_id: u64,
		dest: MultiLocation,
		assets: MultiAssetFilter,
	},

	/// Pay for the execution of some XCM `instructions` and `orders` with up to `weight`
	/// picoseconds of execution time, paying for this with up to `fees` from the Holding Register.
	///
	/// - `fees`: The asset(s) to remove from the Holding Register to pay for fees.
	/// - `weight`: The amount of weight to purchase; this should be at least the shallow weight of
	///   `orders` and `instructions`.
	/// - `debt`: The amount of weight-debt already incurred to be paid off; this should be equal to
	///   the unpaid weight of any surrounding operations/orders.
	/// - `halt_on_error`: If `true`, the execution of the `orders` and `instructions` will halt on
	///   the first failure. If `false`, then execution will continue regardless.
	/// - `orders`: Orders to be executed with the existing the Holding Register; execution of these
	///   orders happens PRIOR to execution of the `instructions`. The (shallow) weight for these
	///   must be paid for with the `weight` purchased.
	/// - `instructions`: XCM instructions to be executed outside of the context of the current
	///   Holding Register; execution of these instructions happens AFTER the execution of the
	///   `orders`. The (shallow) weight for these must be paid for with the `weight` purchased.
	///
	/// Errors:
	/// - `Overflow`, if weight calculation overflows
	/// - `NotHoldingFees`, if the fees are not present in the Holding Register
	/// - Errors returned by the execution of `orders` and `instructions`.
	#[codec(index = 7)]
	BuyExecution {
		fees: MultiAsset,
		weight: u64,
		debt: u64,
		halt_on_error: bool,
		orders: Vec<Order<Call>>,
		instructions: Vec<Xcm<Call>>,
	},
}

pub mod opaque {
	pub type Order = super::Order<()>;
}

impl<Call> Order<Call> {
	pub fn into<C>(self) -> Order<C> {
		Order::from(self)
	}
	pub fn from<C>(order: Order<C>) -> Self {
		use Order::*;
		match order {
			Noop => Noop,
			DepositAsset { assets, max_assets, beneficiary } =>
				DepositAsset { assets, max_assets, beneficiary },
			DepositReserveAsset { assets, max_assets, dest, effects } =>
				DepositReserveAsset { assets, max_assets, dest, effects },
			ExchangeAsset { give, receive } => ExchangeAsset { give, receive },
			InitiateReserveWithdraw { assets, reserve, effects } =>
				InitiateReserveWithdraw { assets, reserve, effects },
			InitiateTeleport { assets, dest, effects } =>
				InitiateTeleport { assets, dest, effects },
			QueryHolding { query_id, dest, assets } => QueryHolding { query_id, dest, assets },
			BuyExecution { fees, weight, debt, halt_on_error, orders, instructions } => {
				let orders = orders.into_iter().map(Order::from).collect();
				let instructions = instructions.into_iter().map(Xcm::from).collect();
				BuyExecution { fees, weight, debt, halt_on_error, orders, instructions }
			},
		}
	}
}

impl<Call> TryFrom<Order0<Call>> for Order<Call> {
	type Error = ();
	fn try_from(old: Order0<Call>) -> result::Result<Order<Call>, ()> {
		use Order::*;
		Ok(match old {
			Order0::Null => Noop,
			Order0::DepositAsset { assets, dest } => DepositAsset {
				assets: assets.try_into()?,
				max_assets: 1,
				beneficiary: dest.try_into()?,
			},
			Order0::DepositReserveAsset { assets, dest, effects } => DepositReserveAsset {
				assets: assets.try_into()?,
				max_assets: 1,
				dest: dest.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::<()>::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			Order0::ExchangeAsset { give, receive } =>
				ExchangeAsset { give: give.try_into()?, receive: receive.try_into()? },
			Order0::InitiateReserveWithdraw { assets, reserve, effects } =>
				InitiateReserveWithdraw {
					assets: assets.try_into()?,
					reserve: reserve.try_into()?,
					effects: effects
						.into_iter()
						.map(Order::<()>::try_from)
						.collect::<result::Result<_, _>>()?,
				},
			Order0::InitiateTeleport { assets, dest, effects } => InitiateTeleport {
				assets: assets.try_into()?,
				dest: dest.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::<()>::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			Order0::QueryHolding { query_id, dest, assets } =>
				QueryHolding { query_id, dest: dest.try_into()?, assets: assets.try_into()? },
			Order0::BuyExecution { fees, weight, debt, halt_on_error, xcm } => {
				let instructions =
					xcm.into_iter().map(Xcm::<Call>::try_from).collect::<result::Result<_, _>>()?;
				BuyExecution {
					fees: fees.try_into()?,
					weight,
					debt,
					halt_on_error,
					orders: vec![],
					instructions,
				}
			},
		})
	}
}
