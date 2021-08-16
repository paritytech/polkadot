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

use super::{super::v1::Order as Order1, MultiAsset, MultiLocation, Xcm};
use alloc::vec::Vec;
use core::{
	convert::{TryFrom, TryInto},
	result,
};
use derivative::Derivative;
use parity_scale_codec::{self, Decode, Encode};

/// An instruction to be executed on some or all of the assets in the Holding Register, used by
/// asset-related XCM messages.
///
/// The Holding Register is a temporary place used to keep track of all assets in motion,
/// e.g. withdrawn from accounts or teleported in via XCM.
#[derive(Derivative, Encode, Decode)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
pub enum Order<Call> {
	/// Do nothing. Not generally used.
	#[codec(index = 0)]
	Null,

	/// Remove the asset(s) (`assets`) from the Holding Register and place equivalent assets under the ownership of `dest` within
	/// this consensus system.
	///
	/// - `assets`: The asset(s) to remove from the Holding Register.
	/// - `dest`: The new owner for the assets.
	///
	/// Errors:
	/// - Errors occurring when trying to deposit the asset(s).
	#[codec(index = 1)]
	DepositAsset { assets: Vec<MultiAsset>, dest: MultiLocation },

	/// Remove the asset(s) (`assets`) from the Holding Register and place equivalent assets under the ownership of `dest` within
	/// this consensus system.
	///
	/// Send an onward XCM message to `dest` of `ReserveAssetDeposit` with the given `effects`.
	///
	/// - `assets`: The asset(s) to remove from the Holding Register.
	/// - `dest`: The new owner for the assets.
	/// - `effects`: The orders that should be contained in the `ReserveAssetDeposit` which is sent onwards to
	///   `dest`.
	///
	/// Errors:
	/// - Errors occurring when trying to deposit the asset(s).
	/// - Errors occurring when trying to send the `ReserveAssetDeposited` XCM.
	#[codec(index = 2)]
	DepositReserveAsset { assets: Vec<MultiAsset>, dest: MultiLocation, effects: Vec<Order<()>> },

	/// Remove the asset(s) (`give`) from the Holding Register and replace them with alternative assets.
	///
	/// The minimum amount of assets to be received into the Holding Register for the order not to fail may be stated.
	///
	/// - `give`: The asset(s) to remove from the Holding Register.
	/// - `receive`: The minimum amount of assets(s) which `give` should be exchanged for. The meaning of wildcards
	///   is undefined and they should be not be used.
	///
	/// Errors:
	#[codec(index = 3)]
	ExchangeAsset { give: Vec<MultiAsset>, receive: Vec<MultiAsset> },

	/// Remove the asset(s) (`assets`) from the Holding Register and send a `WithdrawAsset` XCM message to a reserve location.
	///
	/// - `assets`: The asset(s) to remove from the Holding Register.
	/// - `reserve`: A valid location that acts as a reserve for all asset(s) in `assets`. The sovereign account
	///   of this consensus system *on the reserve location* will have appropriate assets withdrawn and `effects` will
	///   be executed on them. There will typically be only one valid location on any given asset/chain combination.
	/// - `effects`: The orders to execute on the assets once withdrawn *on the reserve location*.
	///
	/// Errors:
	/// - Errors occurring when trying to send the `WithdrawAsset` XCM.
	#[codec(index = 4)]
	InitiateReserveWithdraw {
		assets: Vec<MultiAsset>,
		reserve: MultiLocation,
		effects: Vec<Order<()>>,
	},

	/// Remove the asset(s) (`assets`) from the Holding Register and send a `TeleportAsset` XCM message to a destination location.
	///
	/// - `assets`: The asset(s) to remove from the Holding Register.
	/// - `dest`: A valid location that has a bi-lateral teleportation arrangement.
	/// - `effects`: The orders to execute on the assets once arrived *on the destination location*.
	///
	/// Errors:
	/// - Errors occurring when trying to send the `ReceiveTeleportedAsset` XCM.
	#[codec(index = 5)]
	InitiateTeleport { assets: Vec<MultiAsset>, dest: MultiLocation, effects: Vec<Order<()>> },

	/// Send a `Balances` XCM message with the `assets` value equal to the the Holding Register contents, or a portion thereof.
	///
	/// - `query_id`: An identifier that will be replicated into the returned XCM message.
	/// - `dest`: A valid destination for the returned XCM message. This may be limited to the current origin.
	/// - `assets`: A filter for the assets that should be reported back. The assets reported back will be, asset-
	///   wise, *the lesser of this value and the Holding Register*. No wildcards will be used when reporting assets
	///   back.
	///
	/// Errors:
	/// - Errors occurring when trying to send the `QueryResponse` XCM.
	#[codec(index = 6)]
	QueryHolding {
		#[codec(compact)]
		query_id: u64,
		dest: MultiLocation,
		assets: Vec<MultiAsset>,
	},

	/// Pay for the execution of some XCM with up to `weight` picoseconds of execution time, paying
	/// for this with up to `fees` from the Holding Register.
	///
	/// - `fees`: The asset(s) to remove from the Holding Register to pay for fees.
	/// - `weight`: The amount of weight to purchase; this should be at least the shallow weight of
	///   `orders` and `instructions`.
	/// - `debt`: The amount of weight-debt already incurred to be paid off; this should be equal to
	///   the unpaid weight of any surrounding operations/orders.
	/// - `halt_on_error`: If `true`, the execution of the `xcm`s will halt on the first failure. If
	///   `false`, then execution will continue regardless.
	/// - `xcm`: XCM instructions to be executed outside of the context of the current Holding
	///   Register.
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
		xcm: Vec<Xcm<Call>>,
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
			Null => Null,
			DepositAsset { assets, dest } => DepositAsset { assets, dest },
			DepositReserveAsset { assets, dest, effects } =>
				DepositReserveAsset { assets, dest, effects },
			ExchangeAsset { give, receive } => ExchangeAsset { give, receive },
			InitiateReserveWithdraw { assets, reserve, effects } =>
				InitiateReserveWithdraw { assets, reserve, effects },
			InitiateTeleport { assets, dest, effects } =>
				InitiateTeleport { assets, dest, effects },
			QueryHolding { query_id, dest, assets } => QueryHolding { query_id, dest, assets },
			BuyExecution { fees, weight, debt, halt_on_error, xcm } => {
				let xcm = xcm.into_iter().map(Xcm::from).collect();
				BuyExecution { fees, weight, debt, halt_on_error, xcm }
			},
		}
	}
}

impl<Call> TryFrom<Order1<Call>> for Order<Call> {
	type Error = ();
	fn try_from(old: Order1<Call>) -> result::Result<Order<Call>, ()> {
		use Order::*;
		Ok(match old {
			Order1::Noop => Null,
			Order1::DepositAsset { assets, beneficiary, .. } =>
				DepositAsset { assets: assets.try_into()?, dest: beneficiary.try_into()? },
			Order1::DepositReserveAsset { assets, dest, effects, .. } => DepositReserveAsset {
				assets: assets.try_into()?,
				dest: dest.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::<()>::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			Order1::ExchangeAsset { give, receive } =>
				ExchangeAsset { give: give.try_into()?, receive: receive.try_into()? },
			Order1::InitiateReserveWithdraw { assets, reserve, effects } =>
				InitiateReserveWithdraw {
					assets: assets.try_into()?,
					reserve: reserve.try_into()?,
					effects: effects
						.into_iter()
						.map(Order::<()>::try_from)
						.collect::<result::Result<_, _>>()?,
				},
			Order1::InitiateTeleport { assets, dest, effects } => InitiateTeleport {
				assets: assets.try_into()?,
				dest: dest.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::<()>::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			Order1::QueryHolding { query_id, dest, assets } =>
				QueryHolding { query_id, dest: dest.try_into()?, assets: assets.try_into()? },
			Order1::BuyExecution { fees, weight, debt, halt_on_error, orders, instructions } => {
				if !orders.is_empty() {
					return Err(())
				}
				let xcm = instructions
					.into_iter()
					.map(Xcm::<Call>::try_from)
					.collect::<result::Result<_, _>>()?;
				BuyExecution { fees: fees.try_into()?, weight, debt, halt_on_error, xcm }
			},
		})
	}
}
