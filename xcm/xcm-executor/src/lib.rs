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
use xcm::v0::{
	ExecuteXcm, SendXcm, Error as XcmError, Outcome,
	MultiLocation, MultiAsset, Xcm, Order, Response,
};

pub mod traits;
use traits::{
	TransactAsset, ConvertOrigin, FilterAssetLocation, InvertLocation, WeightBounds, WeightTrader,
	ShouldExecute, OnResponse
};

mod assets;
pub use assets::{Assets, AssetId};
mod config;
pub use config::Config;

/// The XCM executor.
pub struct XcmExecutor<Config>(PhantomData<Config>);

impl<Config: config::Config> ExecuteXcm<Config::Call> for XcmExecutor<Config> {
	fn execute_xcm_in_credit(
		origin: MultiLocation,
		message: Xcm<Config::Call>,
		weight_limit: Weight,
		mut weight_credit: Weight,
	) -> Outcome {
		// TODO: #2841 #HARDENXCM We should identify recursive bombs here and bail.
		let mut message = Xcm::<Config::Call>::from(message);
		let shallow_weight = match Config::Weigher::shallow(&mut message) {
			Ok(x) => x,
			Err(()) => return Outcome::Error(XcmError::WeightNotComputable),
		};
		let deep_weight = match Config::Weigher::deep(&mut message) {
			Ok(x) => x,
			Err(()) => return Outcome::Error(XcmError::WeightNotComputable),
		};
		let maximum_weight = match shallow_weight.checked_add(deep_weight) {
			Some(x) => x,
			None => return Outcome::Error(XcmError::Overflow),
		};
		if maximum_weight > weight_limit {
			return Outcome::Error(XcmError::WeightLimitReached(maximum_weight));
		}
		let mut trader = Config::Trader::new();
		let result = Self::do_execute_xcm(origin, true, message, &mut weight_credit, Some(shallow_weight), &mut trader);
		drop(trader);
		match result {
			Ok(surplus) => Outcome::Complete(maximum_weight.saturating_sub(surplus)),
			// TODO: #2841 #REALWEIGHT We can do better than returning `maximum_weight` here, and we should otherwise
			//  we'll needlessly be disregarding block execution time.
			Err(e) => Outcome::Incomplete(maximum_weight, e),
		}
	}
}

impl<Config: config::Config> XcmExecutor<Config> {
	fn reanchored(mut assets: Assets, dest: &MultiLocation) -> Vec<MultiAsset> {
		let inv_dest = Config::LocationInverter::invert_location(&dest);
		assets.prepend_location(&inv_dest);
		assets.into_assets_iter().collect::<Vec<_>>()
	}

	/// Execute the XCM and return the portion of weight of `shallow_weight + deep_weight` that `message` did not use.
	///
	/// NOTE: The amount returned must be less than `shallow_weight + deep_weight` of `message`.
	fn do_execute_xcm(
		origin: MultiLocation,
		top_level: bool,
		mut message: Xcm<Config::Call>,
		weight_credit: &mut Weight,
		maybe_shallow_weight: Option<Weight>,
		trader: &mut Config::Trader,
	) -> Result<Weight, XcmError> {
		// This is the weight of everything that cannot be paid for. This basically means all computation
		// except any XCM which is behind an Order::BuyExecution.
		let shallow_weight = maybe_shallow_weight
			.or_else(|| Config::Weigher::shallow(&mut message).ok())
			.ok_or(XcmError::WeightNotComputable)?;

		Config::Barrier::should_execute(&origin, top_level, &message, shallow_weight, weight_credit)
			.map_err(|()| XcmError::Barrier)?;

		// The surplus weight, defined as the amount by which `shallow_weight` plus all nested
		// `shallow_weight` values (ensuring no double-counting and also known as `deep_weight`) is an
		// over-estimate of the actual weight consumed.
		let mut total_surplus: Weight = 0;

		let maybe_holding_effects = match (origin.clone(), message) {
			(origin, Xcm::WithdrawAsset { assets, effects }) => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let mut holding = Assets::default();
				for asset in assets {
					ensure!(!asset.is_wildcard(), XcmError::Wildcard);
					let withdrawn = Config::AssetTransactor::withdraw_asset(&asset, &origin)?;
					holding.saturating_subsume_all(withdrawn);
				}
				Some((holding, effects))
			}
			(origin, Xcm::ReserveAssetDeposit { assets, effects }) => {
				// check whether we trust origin to be our reserve location for this asset.
				for asset in assets.iter() {
					ensure!(!asset.is_wildcard(), XcmError::Wildcard);
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					ensure!(Config::IsReserve::filter_asset_location(asset, &origin), XcmError::UntrustedReserveLocation);
				}
				Some((Assets::from(assets), effects))
			}
			(origin, Xcm::TransferAsset { assets, dest }) => {
				// Take `assets` from the origin account (on-chain) and place into dest account.
				for asset in assets {
					ensure!(!asset.is_wildcard(), XcmError::Wildcard);
					Config::AssetTransactor::teleport_asset(&asset, &origin, &dest)?;
				}
				None
			}
			(origin, Xcm::TransferReserveAsset { mut assets, dest, effects }) => {
				// Take `assets` from the origin account (on-chain) and place into dest account.
				let inv_dest = Config::LocationInverter::invert_location(&dest);
				for asset in assets.iter_mut() {
					ensure!(!asset.is_wildcard(), XcmError::Wildcard);
					Config::AssetTransactor::teleport_asset(&asset, &origin, &dest)?;
					asset.reanchor(&inv_dest)?;
				}
				Config::XcmSender::send_xcm(dest, Xcm::ReserveAssetDeposit { assets, effects })?;
				None
			}
			(origin, Xcm::TeleportAsset { assets, effects }) => {
				// check whether we trust origin to teleport this asset to us via config trait.
				for asset in assets.iter() {
					ensure!(!asset.is_wildcard(), XcmError::Wildcard);
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					ensure!(Config::IsTeleporter::filter_asset_location(asset, &origin), XcmError::UntrustedTeleportLocation);
					// We should check that the asset can actually be teleported in (for this to be in error, there
					// would need to be an accounting violation by one of the trusted chains, so it's unlikely, but we
					// don't want to punish a possibly innocent chain/user).
					Config::AssetTransactor::can_check_in(&origin, asset)?;
				}
				for asset in assets.iter() {
					Config::AssetTransactor::check_in(&origin, asset);
				}
				Some((Assets::from(assets), effects))
			}
			(origin, Xcm::Transact { origin_type, require_weight_at_most,  mut call }) => {
				// We assume that the Relay-chain is allowed to use transact on this parachain.

				// TODO: #2841 #TRANSACTFILTER allow the trait to issue filters for the relay-chain
				let message_call = call.take_decoded().map_err(|_| XcmError::FailedToDecode)?;
				let dispatch_origin = Config::OriginConverter::convert_origin(origin, origin_type)
					.map_err(|_| XcmError::BadOrigin)?;
				let weight = message_call.get_dispatch_info().weight;
				ensure!(weight <= require_weight_at_most, XcmError::TooMuchWeightRequired);
				let actual_weight = match message_call.dispatch(dispatch_origin) {
					Ok(post_info) => post_info.actual_weight,
					Err(error_and_info) => {
						// Not much to do with the result as it is. It's up to the parachain to ensure that the
						// message makes sense.
						error_and_info.post_info.actual_weight
					}
				}.unwrap_or(weight);
				let surplus = weight.saturating_sub(actual_weight);
				// Credit any surplus weight that we bought. This should be safe since it's work we
				// didn't realise that we didn't have to do.
				// It works because we assume that the `Config::Weigher` will always count the `call`'s
				// `get_dispatch_info` weight into its `shallow` estimate.
				*weight_credit = weight_credit.saturating_add(surplus);
				// Do the same for the total surplus, which is reported to the caller and eventually makes its way
				// back up the stack to be subtracted from the deep-weight.
				total_surplus = total_surplus.saturating_add(surplus);
				// Return the overestimated amount so we can adjust our expectations on how much this entire
				// execution has taken.
				None
			}
			(origin, Xcm::QueryResponse { query_id, response }) => {
				Config::ResponseHandler::on_response(origin, query_id, response);
				None
			}
			(origin, Xcm::RelayedFrom { who, message }) => {
				ensure!(who.is_interior(), XcmError::EscalationOfPrivilege);
				let mut origin = origin;
				origin.append_with(who).map_err(|_| XcmError::MultiLocationFull)?;
				let surplus = Self::do_execute_xcm(origin, top_level, *message, weight_credit, None, trader)?;
				total_surplus = total_surplus.saturating_add(surplus);
				None
			}
			_ => Err(XcmError::UnhandledXcmMessage)?,	// Unhandled XCM message.
		};

		if let Some((mut holding, effects)) = maybe_holding_effects {
			for effect in effects.into_iter() {
				total_surplus += Self::execute_effects(&origin, &mut holding, effect, trader)?;
			}
		}

		Ok(total_surplus)
	}

	fn execute_effects(
		origin: &MultiLocation,
		holding: &mut Assets,
		effect: Order<Config::Call>,
		trader: &mut Config::Trader,
	) -> Result<Weight, XcmError> {
		let mut total_surplus = 0;
		match effect {
			Order::DepositAsset { assets, dest } => {
				let deposited = holding.saturating_take(assets);
				for asset in deposited.into_assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
			},
			Order::DepositReserveAsset { assets, dest, effects } => {
				let deposited = holding.saturating_take(assets);
				for asset in deposited.assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
				let assets = Self::reanchored(deposited, &dest);
				Config::XcmSender::send_xcm(dest, Xcm::ReserveAssetDeposit { assets, effects })?;
			},
			Order::InitiateReserveWithdraw { assets, reserve, effects} => {
				let assets = Self::reanchored(holding.saturating_take(assets), &reserve);
				Config::XcmSender::send_xcm(reserve, Xcm::WithdrawAsset { assets, effects })?;
			}
			Order::InitiateTeleport { assets, dest, effects} => {
				// We must do this first in order to resolve wildcards.
				let assets = holding.saturating_take(assets);
				for asset in assets.assets_iter() {
					Config::AssetTransactor::check_out(&origin, &asset);
				}
				let assets = Self::reanchored(assets, &dest);
				Config::XcmSender::send_xcm(dest, Xcm::TeleportAsset { assets, effects })?;
			}
			Order::QueryHolding { query_id, dest, assets } => {
				let assets = Self::reanchored(holding.min(assets.iter()), &dest);
				Config::XcmSender::send_xcm(dest, Xcm::QueryResponse { query_id, response: Response::Assets(assets) })?;
			}
			Order::BuyExecution { fees, weight, debt, halt_on_error, xcm } => {
				// pay for `weight` using up to `fees` of the holding account.
				let purchasing_weight = Weight::from(weight.checked_add(debt).ok_or(XcmError::Overflow)?);
				let max_fee = holding.try_take(fees).map_err(|()| XcmError::NotHoldingFees)?;
				let unspent = trader.buy_weight(purchasing_weight, max_fee)?;
				holding.saturating_subsume_all(unspent);

				let mut remaining_weight = weight;
				for message in xcm.into_iter() {
					match Self::do_execute_xcm(origin.clone(), false, message, &mut remaining_weight, None, trader) {
						Err(e) if halt_on_error => return Err(e),
						Err(_) => {}
						Ok(surplus) => { total_surplus += surplus }
					}
				}
				holding.saturating_subsume(trader.refund_weight(remaining_weight));
			}
			_ => return Err(XcmError::UnhandledEffect)?,
		}
		Ok(total_surplus)
	}
}
