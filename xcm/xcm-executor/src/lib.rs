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
	Error as XcmError, ExecuteXcm,
	Instruction::{self, *},
	MultiAssets, MultiLocation, Outcome, Response, SendXcm, Xcm,
};

pub mod traits;
use traits::{
	ConvertOrigin, FilterAssetLocation, InvertLocation, OnResponse, ShouldExecute, TransactAsset,
	WeightBounds, WeightTrader,
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
		let max_weight = match Config::Weigher::weight(&mut message) {
			Ok(x) => x,
			Err(()) => return Outcome::Error(XcmError::WeightNotComputable),
		};
		if max_weight > weight_limit {
			return Outcome::Error(XcmError::WeightLimitReached(max_weight))
		}
		let mut trader = Config::Trader::new();
		let mut holding = Assets::new();
		let result = Self::do_execute_xcm(
			Some(origin),
			true,
			message,
			&mut weight_credit,
			Some(max_weight),
			&mut trader,
			0,
			&mut holding,
		);
		if let Ok(ref surplus) = result {
			// Refund any unused weight.
			if let Some(w) = trader.refund_weight(*surplus) {
				holding.subsume(w);
			}
		}
		// TODO #2841: Do something with holding? (Fail-safe AssetTrap?)
		drop(trader);
		log::trace!(target: "xcm::execute_xcm", "result: {:?}", &result);
		match result {
			Ok(surplus) => Outcome::Complete(max_weight.saturating_sub(surplus)),
			// TODO: #2841 #REALWEIGHT We can do better than returning `maximum_weight` here, and we should otherwise
			//  we'll needlessly be disregarding block execution time.
			Err(e) => Outcome::Incomplete(max_weight, e),
		}
	}
}

impl<Config: config::Config> XcmExecutor<Config> {
	fn reanchored(mut assets: Assets, dest: &MultiLocation) -> MultiAssets {
		let inv_dest = Config::LocationInverter::invert_location(&dest);
		assets.prepend_location(&inv_dest);
		assets.into_assets_iter().collect::<Vec<_>>().into()
	}

	/// Execute the XCM and return the portion of weight of `max_weight` that `message` did not use.
	///
	/// NOTE: The amount returned must be less than `max_weight` of `message`.
	fn do_execute_xcm(
		mut origin: Option<MultiLocation>,
		top_level: bool,
		mut xcm: Xcm<Config::Call>,
		weight_credit: &mut Weight,
		maybe_max_weight: Option<Weight>,
		trader: &mut Config::Trader,
		num_recursions: u32,
		holding: &mut Assets,
	) -> Result<Weight, XcmError> {
		log::trace!(
			target: "xcm::do_execute_xcm",
			"origin: {:?}, top_level: {:?}, weight_credit: {:?}, maybe_max_weight: {:?}, recursion: {:?}",
			origin,
			top_level,
			weight_credit,
			maybe_max_weight,
			num_recursions,
		);

		if num_recursions > MAX_RECURSION_LIMIT {
			return Err(XcmError::RecursionLimitReached)
		}

		// This is the weight of everything that cannot be paid for. This basically means all computation
		// except any XCM which is behind an Order::BuyExecution.
		let max_weight = maybe_max_weight
			.or_else(|| Config::Weigher::weight(&mut xcm).ok())
			.ok_or(XcmError::WeightNotComputable)?;

		Config::Barrier::should_execute(&origin, top_level, &mut xcm, max_weight, weight_credit)
			.map_err(|()| XcmError::Barrier)?;

		let mut process = |instr: Instruction<Config::Call>,
		                   holding: &mut Assets,
		                   origin: &mut Option<MultiLocation>,
		                   report_outcome: &mut Option<_>,
		                   total_surplus: &mut u64,
		                   total_refunded: &mut u64| match instr {
			WithdrawAsset { assets } => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let origin = origin.as_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.drain().into_iter() {
					Config::AssetTransactor::withdraw_asset(&asset, origin)?;
					holding.subsume(asset);
				}
				Ok(())
			},
			ReserveAssetDeposited { assets } => {
				// check whether we trust origin to be our reserve location for this asset.
				let origin = origin.as_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.drain().into_iter() {
					// Must ensure that we recognise the asset as being managed by the origin.
					ensure!(
						Config::IsReserve::filter_asset_location(&asset, origin),
						XcmError::UntrustedReserveLocation
					);
					holding.subsume(asset);
				}
				Ok(())
			},
			TransferAsset { assets, beneficiary } => {
				// Take `assets` from the origin account (on-chain) and place into dest account.
				let origin = origin.as_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.inner() {
					Config::AssetTransactor::beam_asset(&asset, origin, &beneficiary)?;
				}
				Ok(())
			},
			TransferReserveAsset { mut assets, dest, xcm } => {
				let origin = origin.as_ref().ok_or(XcmError::BadOrigin)?;
				// Take `assets` from the origin account (on-chain) and place into dest account.
				let inv_dest = Config::LocationInverter::invert_location(&dest);
				for asset in assets.inner() {
					Config::AssetTransactor::beam_asset(asset, origin, &dest)?;
				}
				assets.reanchor(&inv_dest)?;
				let mut message = vec![ReserveAssetDeposited { assets }, ClearOrigin];
				message.extend(xcm.0.into_iter());
				Config::XcmSender::send_xcm(dest, Xcm(message)).map_err(Into::into)
			},
			ReceiveTeleportedAsset { assets } => {
				let origin = origin.as_ref().ok_or(XcmError::BadOrigin)?;
				// check whether we trust origin to teleport this asset to us via config trait.
				for asset in assets.inner() {
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					ensure!(
						Config::IsTeleporter::filter_asset_location(asset, origin),
						XcmError::UntrustedTeleportLocation
					);
					// We should check that the asset can actually be teleported in (for this to be in error, there
					// would need to be an accounting violation by one of the trusted chains, so it's unlikely, but we
					// don't want to punish a possibly innocent chain/user).
					Config::AssetTransactor::can_check_in(&origin, asset)?;
				}
				for asset in assets.drain().into_iter() {
					Config::AssetTransactor::check_in(origin, &asset);
					holding.subsume(asset);
				}
				Ok(())
			},
			Transact { origin_type, require_weight_at_most, mut call } => {
				// We assume that the Relay-chain is allowed to use transact on this parachain.
				let origin = origin.clone().ok_or(XcmError::BadOrigin)?;

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
				*total_surplus = total_surplus.saturating_add(surplus);
				// Return the overestimated amount so we can adjust our expectations on how much this entire
				// execution has taken.
				Ok(())
			},
			QueryResponse { query_id, response, max_weight } => {
				let origin = origin.as_ref().ok_or(XcmError::BadOrigin)?;
				Config::ResponseHandler::on_response(origin, query_id, response, max_weight);
				Ok(())
			},
			DescendOrigin(who) => origin
				.as_mut()
				.ok_or(XcmError::BadOrigin)?
				.append_with(who)
				.map_err(|_| XcmError::MultiLocationFull),
			ClearOrigin => {
				*origin = None;
				Ok(())
			},
			ReportOutcome { query_id, dest, max_response_weight } => {
				*report_outcome = Some((dest, query_id, max_response_weight));
				Ok(())
			},
			DepositAsset { assets, max_assets, beneficiary } => {
				let deposited = holding.limited_saturating_take(assets, max_assets as usize);
				for asset in deposited.into_assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &beneficiary)?;
				}
				Ok(())
			},
			DepositReserveAsset { assets, max_assets, dest, xcm } => {
				let deposited = holding.limited_saturating_take(assets, max_assets as usize);
				for asset in deposited.assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
				let assets = Self::reanchored(deposited, &dest);
				let mut message = vec![ReserveAssetDeposited { assets }, ClearOrigin];
				message.extend(xcm.0.into_iter());
				Config::XcmSender::send_xcm(dest, Xcm(message)).map_err(Into::into)
			},
			InitiateReserveWithdraw { assets, reserve, xcm } => {
				let assets = Self::reanchored(holding.saturating_take(assets), &reserve);
				let mut message = vec![WithdrawAsset { assets }, ClearOrigin];
				message.extend(xcm.0.into_iter());
				Config::XcmSender::send_xcm(reserve, Xcm(message)).map_err(Into::into)
			},
			InitiateTeleport { assets, dest, xcm } => {
				// We must do this first in order to resolve wildcards.
				let assets = holding.saturating_take(assets);
				for asset in assets.assets_iter() {
					Config::AssetTransactor::check_out(&dest, &asset);
				}
				let assets = Self::reanchored(assets, &dest);
				let mut message = vec![ReceiveTeleportedAsset { assets }, ClearOrigin];
				message.extend(xcm.0.into_iter());
				Config::XcmSender::send_xcm(dest, Xcm(message)).map_err(Into::into)
			},
			QueryHolding { query_id, dest, assets, max_response_weight } => {
				let assets = Self::reanchored(holding.min(&assets), &dest);
				let max_weight = max_response_weight;
				let response = Response::Assets(assets);
				let instruction = QueryResponse { query_id, response, max_weight };
				Config::XcmSender::send_xcm(dest, Xcm(vec![instruction])).map_err(Into::into)
			},
			BuyExecution { fees, weight_limit } => {
				// There is no need to buy any weight is `weight_limit` is `Unlimited` since it
				// would indicate that `AllowTopLevelPaidExecutionFrom` was unused for execution
				// and thus there is some other reason why it has been determined that this XCM
				// should be executed.
				if let Some(weight) = Option::<u64>::from(weight_limit) {
					// pay for `weight` using up to `fees` of the holding register.
					let max_fee =
						holding.try_take(fees.into()).map_err(|_| XcmError::NotHoldingFees)?;
					let unspent = trader.buy_weight(weight, max_fee)?;
					holding.subsume_assets(unspent);
				}
				Ok(())
			},
			RefundSurplus => {
				let current_surplus = total_surplus.saturating_sub(*total_refunded);
				if current_surplus > 0 {
					*total_refunded = total_refunded.saturating_add(current_surplus);
					if let Some(w) = trader.refund_weight(current_surplus) {
						holding.subsume(w);
					}
				}
				Ok(())
			},
			_ => return Err(XcmError::UnhandledEffect)?,
		};

		// The surplus weight, defined as the amount by which `max_weight` is
		// an over-estimate of the actual weight consumed. We do it this way to avoid needing the
		// execution engine to keep track of all instructions' weights (it only needs to care about
		// the weight of dynamically determined instructions such as `Transact`).
		let mut total_surplus: Weight = 0;
		let mut total_refunded: Weight = 0;
		let mut report_outcome = None;
		let mut outcome = Ok(());
		for (i, instruction) in xcm.0.into_iter().enumerate() {
			match process(
				instruction,
				holding,
				&mut origin,
				&mut report_outcome,
				&mut total_surplus,
				&mut total_refunded,
			) {
				Ok(()) => (),
				Err(e) => {
					outcome = Err((i as u32, e));
					break
				},
			}
		}

		if let Some((dest, query_id, max_weight)) = report_outcome {
			let response = Response::ExecutionResult(outcome.clone());
			let message = QueryResponse { query_id, response, max_weight };
			Config::XcmSender::send_xcm(dest, Xcm(vec![message]))?;
		}

		outcome.map(|()| total_surplus).map_err(|e| e.1)
	}
}
