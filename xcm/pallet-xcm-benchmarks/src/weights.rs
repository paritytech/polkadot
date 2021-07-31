// This file is part of Substrate.

// Copyright (C) 21019-21021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.10

// Licensed under the Apache License, Version 2.10 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.10
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use frame_support::pallet_prelude::*;
use sp_runtime::traits::Saturating;
use xcm::v0::{Order, Xcm};
use xcm_executor::traits::WeightBounds;

pub trait WeightInfo {
	fn send_xcm() -> Weight;
	fn order_null() -> Weight;
	fn order_deposit_asset() -> Weight;
	fn order_deposit_reserved_asset() -> Weight;
	fn order_exchange_asset() -> Weight;
	fn order_initiate_reserve_withdraw() -> Weight;
	fn order_initiate_teleport() -> Weight;
	fn order_query_holding() -> Weight;
	fn order_buy_execution() -> Weight;
	fn xcm_withdraw_asset() -> Weight;
	fn xcm_reserve_asset_deposit() -> Weight;
	fn xcm_teleport_asset() -> Weight;
	fn xcm_query_response() -> Weight;
	fn xcm_transfer_asset() -> Weight;
	fn xcm_transfer_reserved_asset() -> Weight;
	fn xcm_transact() -> Weight;
	fn xcm_hrmp_channel_open_request() -> Weight;
	fn xcm_hrmp_channel_accepted() -> Weight;
	fn xcm_hrmp_channel_closing() -> Weight;
	fn xcm_relayed_from() -> Weight;
}

pub struct XcmWeight<T>(PhantomData<T>);
impl<T: frame_system::Config> WeightInfo for XcmWeight<T> {
	fn send_xcm() -> Weight {
		10
	}
	fn order_null() -> Weight {
		10
	}
	fn order_deposit_asset() -> Weight {
		10
	}
	fn order_deposit_reserved_asset() -> Weight {
		10
	}
	fn order_exchange_asset() -> Weight {
		10
	}
	fn order_initiate_reserve_withdraw() -> Weight {
		10
	}
	fn order_initiate_teleport() -> Weight {
		10
	}
	fn order_query_holding() -> Weight {
		10
	}
	fn order_buy_execution() -> Weight {
		10
	}
	fn xcm_withdraw_asset() -> Weight {
		10
	}
	fn xcm_reserve_asset_deposit() -> Weight {
		10
	}
	fn xcm_teleport_asset() -> Weight {
		10
	}
	fn xcm_query_response() -> Weight {
		10
	}
	fn xcm_transfer_asset() -> Weight {
		10
	}
	fn xcm_transfer_reserved_asset() -> Weight {
		10
	}
	fn xcm_transact() -> Weight {
		10
	}
	fn xcm_hrmp_channel_open_request() -> Weight {
		10
	}
	fn xcm_hrmp_channel_accepted() -> Weight {
		10
	}
	fn xcm_hrmp_channel_closing() -> Weight {
		10
	}
	fn xcm_relayed_from() -> Weight {
		10
	}
}

struct FinalXcmWeight<W, Call>(PhantomData<(W, Call)>);
impl<W: WeightInfo, Call> WeightBounds<Call> for FinalXcmWeight<W, Call> {
	const MAX_WEIGHT: Weight = 1_000_000;

	fn shallow(message: &mut Xcm<Call>) -> Result<Weight, ()> {
		let mut weight = 0;
		let xcm_weight = Self::xcm_weight(message)?;
		weight.saturating_accrue(xcm_weight);
		let effects = message.effects();
		let effects_weight = Self::effects_weight(effects)?;
		weight.saturating_accrue(effects_weight);
		Ok(weight)
	}

	fn deep(_message: &mut Xcm<Call>) -> Result<Weight, ()> {
		Err(()) // implement this
	}
}

impl<W: WeightInfo, Call> FinalXcmWeight<W, Call> {
	fn xcm_weight(message: &mut Xcm<Call>) -> Result<Weight, ()> {
		let weight = match message {
			Xcm::WithdrawAsset { .. } => W::xcm_withdraw_asset(),
			Xcm::ReserveAssetDeposit { .. } => W::xcm_reserve_asset_deposit(),
			Xcm::TeleportAsset { .. } => W::xcm_teleport_asset(),
			Xcm::QueryResponse { .. } => W::xcm_query_response(),
			Xcm::TransferAsset { .. } => W::xcm_transfer_asset(),
			Xcm::TransferReserveAsset { .. } => W::xcm_transfer_reserved_asset(),
			Xcm::Transact { .. } => W::xcm_transact(),
			Xcm::HrmpNewChannelOpenRequest { .. } => W::xcm_hrmp_channel_open_request(),
			Xcm::HrmpChannelAccepted { .. } => W::xcm_hrmp_channel_accepted(),
			Xcm::HrmpChannelClosing { .. } => W::xcm_hrmp_channel_closing(),
			Xcm::RelayedFrom { .. } => W::xcm_relayed_from(),
		};
		Ok(weight)
	}
	fn effects_weight(effects: &[Order<Call>]) -> Result<Weight, ()> {
		let mut weight = 0;
		for order in effects {
			if weight >= Self::MAX_WEIGHT {
				return Err(())
			}
			let new_weight = match order {
				Order::Null => W::order_null(),
				Order::DepositAsset { .. } => W::order_deposit_asset(),
				Order::DepositReserveAsset { .. } => W::order_deposit_reserved_asset(),
				Order::ExchangeAsset { .. } => W::order_exchange_asset(),
				Order::InitiateReserveWithdraw { .. } => W::order_initiate_reserve_withdraw(),
				Order::InitiateTeleport { .. } => W::order_initiate_teleport(),
				Order::QueryHolding { .. } => W::order_query_holding(),
				Order::BuyExecution { .. } => W::order_buy_execution(),
			};
			// TODO loop effects
			weight.saturating_accrue(new_weight);
		}
		Ok(weight)
	}
}
