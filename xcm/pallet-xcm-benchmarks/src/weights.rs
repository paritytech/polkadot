// Copyright 2021 Parity Technologies (UK) Ltd.
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

use frame_support::{dispatch::GetDispatchInfo, pallet_prelude::*};
use sp_runtime::traits::Saturating;
use xcm::v0::{GetWeight, Order, Xcm};
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

struct FinalXcmWeight<W, C>(PhantomData<(W, C)>);
impl<W, C> WeightBounds<C> for FinalXcmWeight<W, C>
where
	W: WeightInfo,
	C: Decode + GetDispatchInfo,
	Xcm<C>: GetWeight<W>,
	Order<C>: GetWeight<W>,
{
	fn shallow(message: &mut Xcm<C>) -> Result<Weight, ()> {
		let weight = match message {
			Xcm::RelayedFrom { ref mut message, .. } => {
				let relay_message_weight = Self::shallow(message.as_mut())?;
				message.weight().saturating_add(relay_message_weight)
			},
			// These XCM
			Xcm::WithdrawAsset { effects, .. } |
			Xcm::ReserveAssetDeposit { effects, .. } |
			Xcm::TeleportAsset { effects, .. } => {
				let inner: Weight = effects.iter_mut().map(|effect| effect.weight()).sum();
				message.weight().saturating_add(inner)
			},
			// The shallow weight of `Transact` is the full weight of the message, thus there is no
			// deeper weight.
			Xcm::Transact { call, .. } => {
				let call_weight = call.ensure_decoded()?.get_dispatch_info().weight;
				message.weight().saturating_add(call_weight)
			},
			// These
			Xcm::QueryResponse { .. } |
			Xcm::TransferAsset { .. } |
			Xcm::TransferReserveAsset { .. } |
			Xcm::HrmpNewChannelOpenRequest { .. } |
			Xcm::HrmpChannelAccepted { .. } |
			Xcm::HrmpChannelClosing { .. } => message.weight(),
		};

		Ok(weight)
	}

	fn deep(message: &mut Xcm<C>) -> Result<Weight, ()> {
		let weight = match message {
			// `RelayFrom` needs to account for the deep weight of the internal message.
			Xcm::RelayedFrom { ref mut message, .. } => Self::deep(message.as_mut())?,
			// These XCM have internal effects which are not accounted for in the `shallow` weight.
			Xcm::WithdrawAsset { effects, .. } |
			Xcm::ReserveAssetDeposit { effects, .. } |
			Xcm::TeleportAsset { effects, .. } => {
				let mut extra = 0;
				for effect in effects.iter_mut() {
					match effect {
						Order::BuyExecution { xcm, .. } =>
							for message in xcm.iter_mut() {
								extra.saturating_accrue({
									let shallow = Self::shallow(message)?;
									let deep = Self::deep(message)?;
									shallow.saturating_add(deep)
								});
							},
						_ => {},
					}
				}
				extra
			},
			// These XCM do not have any deeper weight.
			Xcm::Transact { .. } |
			Xcm::QueryResponse { .. } |
			Xcm::TransferAsset { .. } |
			Xcm::TransferReserveAsset { .. } |
			Xcm::HrmpNewChannelOpenRequest { .. } |
			Xcm::HrmpChannelAccepted { .. } |
			Xcm::HrmpChannelClosing { .. } => 0,
		};

		Ok(weight)
	}
}
