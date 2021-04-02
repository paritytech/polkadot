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

#![cfg_attr(not(feature = "std"), no_std)]

use sp_std::{prelude::*, marker::PhantomData};
use frame_support::{
	ensure, weights::GetDispatchInfo,
	dispatch::{Weight, Dispatchable}
};
use parity_scale_codec::{self as codec, Encode, Decode};
use xcm::v0::{
	Xcm, Order, ExecuteXcm, SendXcm, Error as XcmError, Result as XcmResult,
	MultiLocation, MultiAsset, XcmGeneric, OrderGeneric,
};

pub mod traits;
use traits::{TransactAsset, ConvertOrigin, FilterAssetLocation, InvertLocation};

mod assets;
pub use assets::{Assets, AssetId};
mod config;
pub use config::{Config, WeightOf, BuyWeight, ShouldExecute};

pub struct XcmExecutor<Config>(PhantomData<Config>);

// TODO: Introduce max_weight and return used_weight.
// TODO: Use XcmGeneric/VersionedXcmGeneric and a new `EncodedCall` type which can decode when needed for weight.

impl<Config: config::Config> ExecuteXcm for XcmExecutor<Config> {
	fn execute_xcm(origin: MultiLocation, message: Xcm) -> XcmResult {
		// TODO: We should identify recursive bombs here and bail.
		let message = XcmGeneric::<Config::Call>::from(message);
		Self::do_execute_xcm(origin, true, message, &mut 0)
	}
}

impl<Config: config::Config> XcmExecutor<Config> {
	fn reanchored(mut assets: Assets, dest: &MultiLocation) -> Vec<MultiAsset> {
		let inv_dest = Config::LocationInverter::invert_location(&dest);
		assets.reanchor(&inv_dest);
		assets.into_assets_iter().collect::<Vec<_>>()
	}

	fn do_execute_xcm(
		origin: MultiLocation,
		top_level: bool,
		mut message: XcmGeneric<Config::Call>,
		weight_credit: &mut Weight,
	) -> XcmResult {
		// This is the weight of everything that cannot be paid for. This basically means all computation
		// except any XCM which is behind an Order::BuyExecution.
		let unpaid_weight = Config::Weigher::weight_of(&mut message)
			.map_err(|_| XcmError::Undefined)?;

		Config::Barrier::should_execute(&origin, top_level, &message,unpaid_weight, weight_credit)
			.map_err(|()| XcmError::Undefined)?;

		let (mut holding, effects) = match (origin.clone(), message) {
			(origin, XcmGeneric::WithdrawAsset { assets, effects }) => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let mut holding = Assets::default();
				for asset in assets {
					let withdrawn = Config::AssetTransactor::withdraw_asset(&asset, &origin)?;
					holding.saturating_subsume(withdrawn);
				}
				(holding, effects)
			}
			(origin, XcmGeneric::ReserveAssetDeposit { assets, effects }) => {
				// check whether we trust origin to be our reserve location for this asset.
				if assets.iter().all(|asset| Config::IsReserve::filter_asset_location(asset, &origin)) {
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					(Assets::from(assets), effects)
				} else {
					Err(XcmError::UntrustedReserveLocation)?
				}
			}
			(origin, XcmGeneric::TeleportAsset { assets, effects }) => {
				// check whether we trust origin to teleport this asset to us via config trait.
				// TODO: should de-wildcard `assets` before passing in.
				log::debug!(target: "runtime::xcm-executor", "Teleport from {:?}", origin);
				if assets.iter().all(|asset| Config::IsTeleporter::filter_asset_location(asset, &origin)) {
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					(Assets::from(assets), effects)
				} else {
					Err(XcmError::UntrustedTeleportLocation)?
				}
			}
			(origin, XcmGeneric::Transact { origin_type, max_weight,  mut call }) => {
				// We assume that the Relay-chain is allowed to use transact on this parachain.

				// TODO: allow this to be configurable in the trait.
				// TODO: allow the trait to issue filters for the relay-chain
				let message_call = call.take_decoded().map_err(|_| XcmError::FailedToDecode)?;
				let dispatch_origin = Config::OriginConverter::convert_origin(origin, origin_type)
					.map_err(|_| XcmError::BadOrigin)?;
				let weight = message_call.get_dispatch_info().weight;
				ensure!(max_weight >= weight, XcmError::Undefined);
				let actual_weight = match message_call.dispatch(dispatch_origin) {
					Ok(post_info) => post_info.actual_weight,
					Err(error_and_info) => {
						// Not much to do with the result as it is. It's up to the parachain to ensure that the
						// message makes sense.
						error_and_info.post_info.actual_weight
					}
				}.unwrap_or(weight);
				let _surplus_weight = max_weight - actual_weight;

				// TODO: reduce used_weight by surplus_weight.
				// FUTURE: Here is where we could provide surplus_weight to Config::Barrier if we wanted to
				// support transact weight surplus crediting.
				return Ok(());
			}
			_ => Err(XcmError::UnhandledXcmMessage)?,	// Unhandled XCM message.
		};

		for effect in effects.into_iter() {
			let _ = Self::execute_effects(&origin, &mut holding, effect)?;
		}

		Ok(())
	}

	fn execute_effects(origin: &MultiLocation, holding: &mut Assets, effect: OrderGeneric<Config::Call>) -> XcmResult {
		match effect {
			OrderGeneric::DepositAsset { assets, dest } => {
				let deposited = holding.saturating_take(assets);
				for asset in deposited.into_assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
				Ok(())
			},
			OrderGeneric::DepositReserveAsset { assets, dest, effects } => {
				let deposited = holding.saturating_take(assets);
				for asset in deposited.assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
				let assets = Self::reanchored(deposited, &dest);
				Config::XcmSender::send_xcm(dest, Xcm::ReserveAssetDeposit { assets, effects })
			},
			OrderGeneric::InitiateReserveWithdraw { assets, reserve, effects} => {
				let assets = Self::reanchored(holding.saturating_take(assets), &reserve);
				Config::XcmSender::send_xcm(reserve, Xcm::WithdrawAsset { assets, effects })
			}
			OrderGeneric::InitiateTeleport { assets, dest, effects} => {
				let assets = Self::reanchored(holding.saturating_take(assets), &dest);
				Config::XcmSender::send_xcm(dest, Xcm::TeleportAsset { assets, effects })
			}
			OrderGeneric::QueryHolding { query_id, dest, assets } => {
				let assets = Self::reanchored(holding.min(assets.iter()), &dest);
				Config::XcmSender::send_xcm(dest, Xcm::Balances { query_id, assets })
			}
			OrderGeneric::BuyExecution { fees, weight, debt, halt_on_error, xcm } => {
				// pay for `weight` using up to `fees` of the holding account.
				let desired_weight = Weight::from(weight + debt);
				let max_fee = holding.checked_sub_assign(fees).map_err(|()| XcmError::Undefined)?;
				let surplus = Config::Sale::buy_weight(desired_weight, max_fee)?;
				holding.saturating_subsume_all(surplus);
				let mut remaining_weight= desired_weight;
				for message in xcm.into_iter() {
					match Self::do_execute_xcm(origin.clone(), false, message, &mut remaining_weight) {
						Err(e) if halt_on_error => return Err(e),
						_ => {}
					}
				}
				Ok(())

				// FUTURE: Here is where we would provide surplus_weight to Config::Barrier if we wanted to
				// support weight refunding
			}
			_ => Err(XcmError::UnhandledEffect)?,
		}
	}
}
