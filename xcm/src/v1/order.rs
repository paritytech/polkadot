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

//! Version 1 of the Cross-Consensus Message format data structures.

use super::{MultiAsset, MultiAssetFilter, MultiAssets, MultiLocation, Xcm};
use crate::{v0::Order as OldOrder, v2::Instruction};
use alloc::{vec, vec::Vec};
use core::result;
use derivative::Derivative;
use parity_scale_codec::{self, Decode, Encode};
use scale_info::TypeInfo;

/// An instruction to be executed on some or all of the assets in holding, used by asset-related XCM messages.
#[derive(Derivative, Encode, Decode, TypeInfo)]
#[derivative(Clone(bound = ""), Eq(bound = ""), PartialEq(bound = ""), Debug(bound = ""))]
#[codec(encode_bound())]
#[codec(decode_bound())]
#[scale_info(bounds(), skip_type_params(RuntimeCall))]
pub enum Order<RuntimeCall> {
	/// Do nothing. Not generally used.
	#[codec(index = 0)]
	Noop,

	/// Remove the asset(s) (`assets`) from holding and place equivalent assets under the ownership of `beneficiary`
	/// within this consensus system.
	///
	/// - `assets`: The asset(s) to remove from holding.
	/// - `max_assets`: The maximum number of unique assets/asset instances to remove from holding. Only the first
	///   `max_assets` assets/instances of those matched by `assets` will be removed, prioritized under standard asset
	///   ordering. Any others will remain in holding.
	/// - `beneficiary`: The new owner for the assets.
	///
	/// Errors:
	#[codec(index = 1)]
	DepositAsset { assets: MultiAssetFilter, max_assets: u32, beneficiary: MultiLocation },

	/// Remove the asset(s) (`assets`) from holding and place equivalent assets under the ownership of `dest` within
	/// this consensus system (i.e. its sovereign account).
	///
	/// Send an onward XCM message to `dest` of `ReserveAssetDeposited` with the given `effects`.
	///
	/// - `assets`: The asset(s) to remove from holding.
	/// - `max_assets`: The maximum number of unique assets/asset instances to remove from holding. Only the first
	///   `max_assets` assets/instances of those matched by `assets` will be removed, prioritized under standard asset
	///   ordering. Any others will remain in holding.
	/// - `dest`: The location whose sovereign account will own the assets and thus the effective beneficiary for the
	///   assets and the notification target for the reserve asset deposit message.
	/// - `effects`: The orders that should be contained in the `ReserveAssetDeposited` which is sent onwards to
	///   `dest`.
	///
	/// Errors:
	#[codec(index = 2)]
	DepositReserveAsset {
		assets: MultiAssetFilter,
		max_assets: u32,
		dest: MultiLocation,
		effects: Vec<Order<()>>,
	},

	/// Remove the asset(s) (`give`) from holding and replace them with alternative assets.
	///
	/// The minimum amount of assets to be received into holding for the order not to fail may be stated.
	///
	/// - `give`: The asset(s) to remove from holding.
	/// - `receive`: The minimum amount of assets(s) which `give` should be exchanged for.
	///
	/// Errors:
	#[codec(index = 3)]
	ExchangeAsset { give: MultiAssetFilter, receive: MultiAssets },

	/// Remove the asset(s) (`assets`) from holding and send a `WithdrawAsset` XCM message to a reserve location.
	///
	/// - `assets`: The asset(s) to remove from holding.
	/// - `reserve`: A valid location that acts as a reserve for all asset(s) in `assets`. The sovereign account
	///   of this consensus system *on the reserve location* will have appropriate assets withdrawn and `effects` will
	///   be executed on them. There will typically be only one valid location on any given asset/chain combination.
	/// - `effects`: The orders to execute on the assets once withdrawn *on the reserve location*.
	///
	/// Errors:
	#[codec(index = 4)]
	InitiateReserveWithdraw {
		assets: MultiAssetFilter,
		reserve: MultiLocation,
		effects: Vec<Order<()>>,
	},

	/// Remove the asset(s) (`assets`) from holding and send a `ReceiveTeleportedAsset` XCM message to a `destination`
	/// location.
	///
	/// - `assets`: The asset(s) to remove from holding.
	/// - `destination`: A valid location that has a bi-lateral teleportation arrangement.
	/// - `effects`: The orders to execute on the assets once arrived *on the destination location*.
	///
	/// NOTE: The `destination` location *MUST* respect this origin as a valid teleportation origin for all `assets`.
	/// If it does not, then the assets may be lost.
	///
	/// Errors:
	#[codec(index = 5)]
	InitiateTeleport { assets: MultiAssetFilter, dest: MultiLocation, effects: Vec<Order<()>> },

	/// Send a `Balances` XCM message with the `assets` value equal to the holding contents, or a portion thereof.
	///
	/// - `query_id`: An identifier that will be replicated into the returned XCM message.
	/// - `dest`: A valid destination for the returned XCM message. This may be limited to the current origin.
	/// - `assets`: A filter for the assets that should be reported back. The assets reported back will be, asset-
	///   wise, *the lesser of this value and the holding register*. No wildcards will be used when reporting assets
	///   back.
	///
	/// Errors:
	#[codec(index = 6)]
	QueryHolding {
		#[codec(compact)]
		query_id: u64,
		dest: MultiLocation,
		assets: MultiAssetFilter,
	},

	/// Pay for the execution of some XCM `instructions` and `orders` with up to `weight` picoseconds of execution time,
	/// paying for this with up to `fees` from the Holding Register.
	///
	/// - `fees`: The asset(s) to remove from holding to pay for fees.
	/// - `weight`: The amount of weight to purchase; this should be at least the shallow weight of `effects` and `xcm`.
	/// - `debt`: The amount of weight-debt already incurred to be paid off; this should be equal to the unpaid weight of
	///   any surrounding operations/orders.
	/// - `halt_on_error`: If `true`, the execution of the `orders` and `operations` will halt on the first failure. If
	///   `false`, then execution will continue regardless.
	/// - `instructions`: XCM instructions to be executed outside of the context of the current Holding Register;
	///   execution of these instructions happens AFTER the execution of the `orders`. The (shallow) weight for these
	///   must be paid for with the `weight` purchased.
	/// Errors:
	#[codec(index = 7)]
	BuyExecution {
		fees: MultiAsset,
		weight: u64,
		debt: u64,
		halt_on_error: bool,
		instructions: Vec<Xcm<RuntimeCall>>,
	},
}

pub mod opaque {
	pub type Order = super::Order<()>;
}

impl<RuntimeCall> Order<RuntimeCall> {
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
			BuyExecution { fees, weight, debt, halt_on_error, instructions } => {
				let instructions = instructions.into_iter().map(Xcm::from).collect();
				BuyExecution { fees, weight, debt, halt_on_error, instructions }
			},
		}
	}
}

impl<RuntimeCall> TryFrom<OldOrder<RuntimeCall>> for Order<RuntimeCall> {
	type Error = ();
	fn try_from(old: OldOrder<RuntimeCall>) -> result::Result<Order<RuntimeCall>, ()> {
		use Order::*;
		Ok(match old {
			OldOrder::Null => Noop,
			OldOrder::DepositAsset { assets, dest } => DepositAsset {
				assets: assets.try_into()?,
				max_assets: 1,
				beneficiary: dest.try_into()?,
			},
			OldOrder::DepositReserveAsset { assets, dest, effects } => DepositReserveAsset {
				assets: assets.try_into()?,
				max_assets: 1,
				dest: dest.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::<()>::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			OldOrder::ExchangeAsset { give, receive } =>
				ExchangeAsset { give: give.try_into()?, receive: receive.try_into()? },
			OldOrder::InitiateReserveWithdraw { assets, reserve, effects } =>
				InitiateReserveWithdraw {
					assets: assets.try_into()?,
					reserve: reserve.try_into()?,
					effects: effects
						.into_iter()
						.map(Order::<()>::try_from)
						.collect::<result::Result<_, _>>()?,
				},
			OldOrder::InitiateTeleport { assets, dest, effects } => InitiateTeleport {
				assets: assets.try_into()?,
				dest: dest.try_into()?,
				effects: effects
					.into_iter()
					.map(Order::<()>::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			OldOrder::QueryHolding { query_id, dest, assets } =>
				QueryHolding { query_id, dest: dest.try_into()?, assets: assets.try_into()? },
			OldOrder::BuyExecution { fees, weight, debt, halt_on_error, xcm } => {
				let instructions = xcm
					.into_iter()
					.map(Xcm::<RuntimeCall>::try_from)
					.collect::<result::Result<_, _>>()?;
				BuyExecution { fees: fees.try_into()?, weight, debt, halt_on_error, instructions }
			},
		})
	}
}

impl<RuntimeCall> TryFrom<Instruction<RuntimeCall>> for Order<RuntimeCall> {
	type Error = ();
	fn try_from(old: Instruction<RuntimeCall>) -> result::Result<Order<RuntimeCall>, ()> {
		use Order::*;
		Ok(match old {
			Instruction::DepositAsset { assets, max_assets, beneficiary } =>
				DepositAsset { assets, max_assets, beneficiary },
			Instruction::DepositReserveAsset { assets, max_assets, dest, xcm } =>
				DepositReserveAsset {
					assets,
					max_assets,
					dest,
					effects: xcm
						.0
						.into_iter()
						.map(Order::<()>::try_from)
						.collect::<result::Result<_, _>>()?,
				},
			Instruction::ExchangeAsset { give, receive } => ExchangeAsset { give, receive },
			Instruction::InitiateReserveWithdraw { assets, reserve, xcm } =>
				InitiateReserveWithdraw {
					assets,
					reserve,
					effects: xcm
						.0
						.into_iter()
						.map(Order::<()>::try_from)
						.collect::<result::Result<_, _>>()?,
				},
			Instruction::InitiateTeleport { assets, dest, xcm } => InitiateTeleport {
				assets,
				dest,
				effects: xcm
					.0
					.into_iter()
					.map(Order::<()>::try_from)
					.collect::<result::Result<_, _>>()?,
			},
			Instruction::QueryHolding { query_id, dest, assets, max_response_weight } => {
				// Cannot handle special response weights.
				if max_response_weight > 0 {
					return Err(())
				}
				QueryHolding { query_id, dest, assets }
			},
			Instruction::BuyExecution { fees, weight_limit } => {
				let instructions = vec![];
				let halt_on_error = true;
				let weight = 0;
				let debt = Option::<u64>::from(weight_limit).ok_or(())?;
				BuyExecution { fees, weight, debt, halt_on_error, instructions }
			},
			_ => return Err(()),
		})
	}
}
