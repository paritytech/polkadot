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

use frame_support::{
	dispatch::{Dispatchable, Weight},
	ensure,
	weights::GetDispatchInfo,
};
use sp_std::{marker::PhantomData, prelude::*};
use xcm::latest::{
	Error as XcmError, ExecuteXcm, MultiAssets, MultiLocation, Order, Outcome, Response, SendXcm,
	Xcm,
};

pub mod traits;
use traits::{
	ConvertOrigin, FilterAssetLocation, InvertLocation, OnResponse, ShouldExecute, TransactAsset,
	VersionChangeNotifier, WeightBounds, WeightTrader,
};

mod assets;
pub use assets::Assets;
mod config;
pub use config::Config;

/// The XCM executor.
pub struct XcmExecutor<Config>(PhantomData<Config>);

/// The maximum recursion limit for `execute_xcm` and `execute_effects`.
pub const MAX_RECURSION_LIMIT: u32 = 8;

impl<Config: config::Config> ExecuteXcm<Config::Call> for XcmExecutor<Config> {
	fn execute_xcm_in_credit(
		origin: MultiLocation,
		message: Xcm<Config::Call>,
		weight_limit: Weight,
		mut weight_credit: Weight,
	) -> Outcome {
		log::trace!(
			target: "xcm::execute_xcm_in_credit",
			"origin: {:?}, message: {:?}, weight_limit: {:?}, weight_credit: {:?}",
			origin,
			message,
			weight_limit,
			weight_credit,
		);
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
			return Outcome::Error(XcmError::WeightLimitReached(maximum_weight))
		}
		let mut trader = Config::Trader::new();
		let result = Self::do_execute_xcm(
			origin,
			true,
			message,
			&mut weight_credit,
			Some(shallow_weight),
			&mut trader,
			0,
		);
		drop(trader);
		log::trace!(target: "xcm::execute_xcm", "result: {:?}", &result);
		match result {
			Ok(surplus) => Outcome::Complete(maximum_weight.saturating_sub(surplus)),
			// TODO: #2841 #REALWEIGHT We can do better than returning `maximum_weight` here, and we should otherwise
			//  we'll needlessly be disregarding block execution time.
			Err(e) => Outcome::Incomplete(maximum_weight, e),
		}
	}
}

impl<Config: config::Config> XcmExecutor<Config> {
	fn reanchored(mut assets: Assets, dest: &MultiLocation) -> MultiAssets {
		let inv_dest = Config::LocationInverter::invert_location(&dest);
		assets.prepend_location(&inv_dest);
		assets.into_assets_iter().collect::<Vec<_>>().into()
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
		num_recursions: u32,
	) -> Result<Weight, XcmError> {
		log::trace!(
			target: "xcm::do_execute_xcm",
			"origin: {:?}, top_level: {:?}, message: {:?}, weight_credit: {:?}, maybe_shallow_weight: {:?}, recursion: {:?}",
			origin,
			top_level,
			message,
			weight_credit,
			maybe_shallow_weight,
			num_recursions,
		);

		if num_recursions > MAX_RECURSION_LIMIT {
			return Err(XcmError::RecursionLimitReached)
		}

		// This is the weight of everything that cannot be paid for. This basically means all computation
		// except any XCM which is behind an Order::BuyExecution.
		let shallow_weight = maybe_shallow_weight
			.or_else(|| Config::Weigher::shallow(&mut message).ok())
			.ok_or(XcmError::WeightNotComputable)?;

		Config::Barrier::should_execute(
			&origin,
			top_level,
			&message,
			shallow_weight,
			weight_credit,
		)
		.map_err(|()| XcmError::Barrier)?;

		// The surplus weight, defined as the amount by which `shallow_weight` plus all nested
		// `shallow_weight` values (ensuring no double-counting and also known as `deep_weight`) is an
		// over-estimate of the actual weight consumed.
		let mut total_surplus: Weight = 0;

		let maybe_holding_effects = match (origin.clone(), message) {
			(origin, Xcm::WithdrawAsset { assets, effects }) => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let mut holding = Assets::default();
				for asset in assets.inner() {
					let withdrawn = Config::AssetTransactor::withdraw_asset(&asset, &origin)?;
					holding.subsume_assets(withdrawn);
				}
				Some((holding, effects))
			},
			(origin, Xcm::ReserveAssetDeposited { assets, effects }) => {
				// check whether we trust origin to be our reserve location for this asset.
				for asset in assets.inner() {
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					ensure!(
						Config::IsReserve::filter_asset_location(asset, &origin),
						XcmError::UntrustedReserveLocation
					);
				}
				Some((assets.into(), effects))
			},
			(origin, Xcm::TransferAsset { assets, beneficiary }) => {
				// Take `assets` from the origin account (on-chain) and place into dest account.
				for asset in assets.inner() {
					Config::AssetTransactor::beam_asset(&asset, &origin, &beneficiary)?;
				}
				None
			},
			(origin, Xcm::TransferReserveAsset { mut assets, dest, effects }) => {
				// Take `assets` from the origin account (on-chain) and place into dest account.
				let inv_dest = Config::LocationInverter::invert_location(&dest);
				for asset in assets.inner() {
					Config::AssetTransactor::beam_asset(asset, &origin, &dest)?;
				}
				assets.reanchor(&inv_dest)?;
				Config::XcmSender::send_xcm(dest, Xcm::ReserveAssetDeposited { assets, effects })?;
				None
			},
			(origin, Xcm::ReceiveTeleportedAsset { assets, effects }) => {
				// check whether we trust origin to teleport this asset to us via config trait.
				for asset in assets.inner() {
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					ensure!(
						Config::IsTeleporter::filter_asset_location(asset, &origin),
						XcmError::UntrustedTeleportLocation
					);
					// We should check that the asset can actually be teleported in (for this to be in error, there
					// would need to be an accounting violation by one of the trusted chains, so it's unlikely, but we
					// don't want to punish a possibly innocent chain/user).
					Config::AssetTransactor::can_check_in(&origin, asset)?;
				}
				for asset in assets.inner() {
					Config::AssetTransactor::check_in(&origin, asset);
				}
				Some((Assets::from(assets), effects))
			},
			(origin, Xcm::Transact { origin_type, require_weight_at_most, mut call }) => {
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
					},
				}
				.unwrap_or(weight);
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
			},
			(origin, Xcm::QueryResponse { query_id, response }) => {
				Config::ResponseHandler::on_response(origin, query_id, response);
				None
			},
			(origin, Xcm::RelayedFrom { who, message }) => {
				let mut origin = origin;
				origin.append_with(who).map_err(|_| XcmError::MultiLocationFull)?;
				let surplus = Self::do_execute_xcm(
					origin,
					top_level,
					*message,
					weight_credit,
					None,
					trader,
					num_recursions + 1,
				)?;
				total_surplus = total_surplus.saturating_add(surplus);
				None
			},
			(origin, Xcm::SubscribeVersion { query_id, max_response_weight }) => {
				// We don't allow derivative origins to subscribe since it would otherwise pose a
				// DoS risk.
				ensure!(top_level, XcmError::BadOrigin);
				Config::SubscriptionService::start(&origin, query_id, max_response_weight)?;
				None
			},
			(origin, Xcm::UnsubscribeVersion) => {
				ensure!(top_level, XcmError::BadOrigin);
				Config::SubscriptionService::stop(&origin)?;
				None
			},
			_ => Err(XcmError::UnhandledXcmMessage)?, // Unhandled XCM message.
		};

		if let Some((mut holding, effects)) = maybe_holding_effects {
			for effect in effects.into_iter() {
				total_surplus += Self::execute_orders(
					&origin,
					&mut holding,
					effect,
					trader,
					num_recursions + 1,
				)?;
			}
		}

		Ok(total_surplus)
	}

	fn execute_orders(
		origin: &MultiLocation,
		holding: &mut Assets,
		order: Order<Config::Call>,
		trader: &mut Config::Trader,
		num_recursions: u32,
	) -> Result<Weight, XcmError> {
		log::trace!(
			target: "xcm::execute_orders",
			"origin: {:?}, holding: {:?}, order: {:?}, recursion: {:?}",
			origin,
			holding,
			order,
			num_recursions,
		);

		if num_recursions > MAX_RECURSION_LIMIT {
			return Err(XcmError::RecursionLimitReached)
		}

		let mut total_surplus = 0;
		match order {
			Order::DepositAsset { assets, max_assets, beneficiary } => {
				let deposited = holding.limited_saturating_take(assets, max_assets as usize);
				for asset in deposited.into_assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &beneficiary)?;
				}
			},
			Order::DepositReserveAsset { assets, max_assets, dest, effects } => {
				let deposited = holding.limited_saturating_take(assets, max_assets as usize);
				for asset in deposited.assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
				let assets = Self::reanchored(deposited, &dest);
				Config::XcmSender::send_xcm(dest, Xcm::ReserveAssetDeposited { assets, effects })?;
			},
			Order::InitiateReserveWithdraw { assets, reserve, effects } => {
				let assets = Self::reanchored(holding.saturating_take(assets), &reserve);
				Config::XcmSender::send_xcm(reserve, Xcm::WithdrawAsset { assets, effects })?;
			},
			Order::InitiateTeleport { assets, dest, effects } => {
				// We must do this first in order to resolve wildcards.
				let assets = holding.saturating_take(assets);
				for asset in assets.assets_iter() {
					Config::AssetTransactor::check_out(&origin, &asset);
				}
				let assets = Self::reanchored(assets, &dest);
				Config::XcmSender::send_xcm(dest, Xcm::ReceiveTeleportedAsset { assets, effects })?;
			},
			Order::QueryHolding { query_id, dest, assets } => {
				let assets = Self::reanchored(holding.min(&assets), &dest);
				Config::XcmSender::send_xcm(
					dest,
					Xcm::QueryResponse { query_id, response: Response::Assets(assets) },
				)?;
			},
			Order::BuyExecution { fees, weight, debt, halt_on_error, instructions } => {
				// pay for `weight` using up to `fees` of the holding register.
				let purchasing_weight =
					Weight::from(weight.checked_add(debt).ok_or(XcmError::Overflow)?);
				let max_fee =
					holding.try_take(fees.into()).map_err(|_| XcmError::NotHoldingFees)?;
				let unspent = trader.buy_weight(purchasing_weight, max_fee)?;
				holding.subsume_assets(unspent);

				let mut remaining_weight = weight;
				for instruction in instructions.into_iter() {
					match Self::do_execute_xcm(
						origin.clone(),
						false,
						instruction,
						&mut remaining_weight,
						None,
						trader,
						num_recursions + 1,
					) {
						Err(e) if halt_on_error => return Err(e),
						Err(_) => {},
						Ok(surplus) => total_surplus += surplus,
					}
				}
				if let Some(w) = trader.refund_weight(remaining_weight) {
					holding.subsume(w);
				}
			},
			_ => return Err(XcmError::UnhandledEffect)?,
		}
		Ok(total_surplus)
	}
}
