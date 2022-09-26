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
	dispatch::{Dispatchable, GetDispatchInfo},
	ensure,
};
use sp_runtime::traits::Saturating;
use sp_std::{marker::PhantomData, prelude::*};
use xcm::latest::{
	Error as XcmError, ExecuteXcm,
	Instruction::{self, *},
	MultiAssets, MultiLocation, Outcome, Response, SendXcm, Weight, Xcm,
};

pub mod traits;
use traits::{
	ClaimAssets, ConvertOrigin, DropAssets, FilterAssetLocation, InvertLocation, OnResponse,
	ShouldExecute, TransactAsset, VersionChangeNotifier, WeightBounds, WeightTrader,
};

mod assets;
pub use assets::Assets;
mod config;
pub use config::Config;

/// The XCM executor.
pub struct XcmExecutor<Config: config::Config> {
	pub holding: Assets,
	pub origin: Option<MultiLocation>,
	pub original_origin: MultiLocation,
	pub trader: Config::Trader,
	/// The most recent error result and instruction index into the fragment in which it occurred,
	/// if any.
	pub error: Option<(u32, XcmError)>,
	/// The surplus weight, defined as the amount by which `max_weight` is
	/// an over-estimate of the actual weight consumed. We do it this way to avoid needing the
	/// execution engine to keep track of all instructions' weights (it only needs to care about
	/// the weight of dynamically determined instructions such as `Transact`).
	pub total_surplus: u64,
	pub total_refunded: u64,
	pub error_handler: Xcm<Config::RuntimeCall>,
	pub error_handler_weight: u64,
	pub appendix: Xcm<Config::RuntimeCall>,
	pub appendix_weight: u64,
	_config: PhantomData<Config>,
}

/// The maximum recursion limit for `execute_xcm` and `execute_effects`.
pub const MAX_RECURSION_LIMIT: u32 = 8;

impl<Config: config::Config> ExecuteXcm<Config::RuntimeCall> for XcmExecutor<Config> {
	fn execute_xcm_in_credit(
		origin: impl Into<MultiLocation>,
		mut message: Xcm<Config::RuntimeCall>,
		weight_limit: Weight,
		mut weight_credit: Weight,
	) -> Outcome {
		let origin = origin.into();
		log::trace!(
			target: "xcm::execute_xcm_in_credit",
			"origin: {:?}, message: {:?}, weight_limit: {:?}, weight_credit: {:?}",
			origin,
			message,
			weight_limit,
			weight_credit,
		);
		let xcm_weight = match Config::Weigher::weight(&mut message) {
			Ok(x) => x,
			Err(()) => {
				log::debug!(
					target: "xcm::execute_xcm_in_credit",
					"Weight not computable! (origin: {:?}, message: {:?}, weight_limit: {:?}, weight_credit: {:?})",
					origin,
					message,
					weight_limit,
					weight_credit,
				);
				return Outcome::Error(XcmError::WeightNotComputable)
			},
		};
		if xcm_weight > weight_limit {
			log::debug!(
				target: "xcm::execute_xcm_in_credit",
				"Weight limit reached! weight > weight_limit: {:?} > {:?}. (origin: {:?}, message: {:?}, weight_limit: {:?}, weight_credit: {:?})",
				xcm_weight,
				weight_limit,
				origin,
				message,
				weight_limit,
				weight_credit,
			);
			return Outcome::Error(XcmError::WeightLimitReached(xcm_weight))
		}

		if let Err(e) =
			Config::Barrier::should_execute(&origin, &mut message, xcm_weight, &mut weight_credit)
		{
			log::debug!(
				target: "xcm::execute_xcm_in_credit",
				"Barrier blocked execution! Error: {:?}. (origin: {:?}, message: {:?}, weight_limit: {:?}, weight_credit: {:?})",
				e,
				origin,
				message,
				weight_limit,
				weight_credit,
			);
			return Outcome::Error(XcmError::Barrier)
		}

		let mut vm = Self::new(origin);

		while !message.0.is_empty() {
			let result = vm.execute(message);
			log::trace!(target: "xcm::execute_xcm_in_credit", "result: {:?}", result);
			message = if let Err(error) = result {
				vm.total_surplus.saturating_accrue(error.weight);
				vm.error = Some((error.index, error.xcm_error));
				vm.take_error_handler().or_else(|| vm.take_appendix())
			} else {
				vm.drop_error_handler();
				vm.take_appendix()
			}
		}

		vm.post_execute(xcm_weight)
	}
}

#[derive(Debug)]
pub struct ExecutorError {
	pub index: u32,
	pub xcm_error: XcmError,
	pub weight: u64,
}

#[cfg(feature = "runtime-benchmarks")]
impl From<ExecutorError> for frame_benchmarking::BenchmarkError {
	fn from(error: ExecutorError) -> Self {
		log::error!(
			"XCM ERROR >> Index: {:?}, Error: {:?}, Weight: {:?}",
			error.index,
			error.xcm_error,
			error.weight
		);
		Self::Stop("xcm executor error: see error logs")
	}
}

impl<Config: config::Config> XcmExecutor<Config> {
	pub fn new(origin: impl Into<MultiLocation>) -> Self {
		let origin = origin.into();
		Self {
			holding: Assets::new(),
			origin: Some(origin.clone()),
			original_origin: origin,
			trader: Config::Trader::new(),
			error: None,
			total_surplus: 0,
			total_refunded: 0,
			error_handler: Xcm(vec![]),
			error_handler_weight: 0,
			appendix: Xcm(vec![]),
			appendix_weight: 0,
			_config: PhantomData,
		}
	}

	/// Execute the XCM program fragment and report back the error and which instruction caused it,
	/// or `Ok` if there was no error.
	pub fn execute(&mut self, xcm: Xcm<Config::RuntimeCall>) -> Result<(), ExecutorError> {
		log::trace!(
			target: "xcm::execute",
			"origin: {:?}, total_surplus/refunded: {:?}/{:?}, error_handler_weight: {:?}",
			self.origin,
			self.total_surplus,
			self.total_refunded,
			self.error_handler_weight,
		);
		let mut result = Ok(());
		for (i, instr) in xcm.0.into_iter().enumerate() {
			match &mut result {
				r @ Ok(()) =>
					if let Err(e) = self.process_instruction(instr) {
						*r = Err(ExecutorError { index: i as u32, xcm_error: e, weight: 0 });
					},
				Err(ref mut error) =>
					if let Ok(x) = Config::Weigher::instr_weight(&instr) {
						error.weight.saturating_accrue(x)
					},
			}
		}
		result
	}

	/// Execute any final operations after having executed the XCM message.
	/// This includes refunding surplus weight, trapping extra holding funds, and returning any errors during execution.
	pub fn post_execute(mut self, xcm_weight: Weight) -> Outcome {
		self.refund_surplus();
		drop(self.trader);

		let mut weight_used = xcm_weight.saturating_sub(self.total_surplus);

		if !self.holding.is_empty() {
			log::trace!(target: "xcm::execute_xcm_in_credit", "Trapping assets in holding register: {:?} (original_origin: {:?})", self.holding, self.original_origin);
			let trap_weight = Config::AssetTrap::drop_assets(&self.original_origin, self.holding);
			weight_used.saturating_accrue(trap_weight);
		};

		match self.error {
			None => Outcome::Complete(weight_used),
			// TODO: #2841 #REALWEIGHT We should deduct the cost of any instructions following
			// the error which didn't end up being executed.
			Some((_i, e)) => {
				log::debug!(target: "xcm::execute_xcm_in_credit", "Execution errored at {:?}: {:?} (original_origin: {:?})", _i, e, self.original_origin);
				Outcome::Incomplete(weight_used, e)
			},
		}
	}

	/// Remove the registered error handler and return it. Do not refund its weight.
	fn take_error_handler(&mut self) -> Xcm<Config::RuntimeCall> {
		let mut r = Xcm::<Config::RuntimeCall>(vec![]);
		sp_std::mem::swap(&mut self.error_handler, &mut r);
		self.error_handler_weight = 0;
		r
	}

	/// Drop the registered error handler and refund its weight.
	fn drop_error_handler(&mut self) {
		self.error_handler = Xcm::<Config::RuntimeCall>(vec![]);
		self.total_surplus.saturating_accrue(self.error_handler_weight);
		self.error_handler_weight = 0;
	}

	/// Remove the registered appendix and return it.
	fn take_appendix(&mut self) -> Xcm<Config::RuntimeCall> {
		let mut r = Xcm::<Config::RuntimeCall>(vec![]);
		sp_std::mem::swap(&mut self.appendix, &mut r);
		self.appendix_weight = 0;
		r
	}

	/// Refund any unused weight.
	fn refund_surplus(&mut self) {
		let current_surplus = self.total_surplus.saturating_sub(self.total_refunded);
		if current_surplus > 0 {
			self.total_refunded.saturating_accrue(current_surplus);
			if let Some(w) = self.trader.refund_weight(current_surplus) {
				self.holding.subsume(w);
			}
		}
	}

	/// Process a single XCM instruction, mutating the state of the XCM virtual machine.
	fn process_instruction(
		&mut self,
		instr: Instruction<Config::RuntimeCall>,
	) -> Result<(), XcmError> {
		match instr {
			WithdrawAsset(assets) => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.drain().into_iter() {
					Config::AssetTransactor::withdraw_asset(&asset, origin)?;
					self.holding.subsume(asset);
				}
				Ok(())
			},
			ReserveAssetDeposited(assets) => {
				// check whether we trust origin to be our reserve location for this asset.
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.drain().into_iter() {
					// Must ensure that we recognise the asset as being managed by the origin.
					ensure!(
						Config::IsReserve::filter_asset_location(&asset, origin),
						XcmError::UntrustedReserveLocation
					);
					self.holding.subsume(asset);
				}
				Ok(())
			},
			TransferAsset { assets, beneficiary } => {
				// Take `assets` from the origin account (on-chain) and place into dest account.
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.inner() {
					Config::AssetTransactor::transfer_asset(&asset, origin, &beneficiary)?;
				}
				Ok(())
			},
			TransferReserveAsset { mut assets, dest, xcm } => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				// Take `assets` from the origin account (on-chain) and place into dest account.
				for asset in assets.inner() {
					Config::AssetTransactor::transfer_asset(asset, origin, &dest)?;
				}
				let ancestry = Config::LocationInverter::ancestry();
				assets.reanchor(&dest, &ancestry).map_err(|()| XcmError::MultiLocationFull)?;
				let mut message = vec![ReserveAssetDeposited(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				Config::XcmSender::send_xcm(dest, Xcm(message)).map_err(Into::into)
			},
			ReceiveTeleportedAsset(assets) => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
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
					self.holding.subsume(asset);
				}
				Ok(())
			},
			Transact { origin_type, require_weight_at_most, mut call } => {
				// We assume that the Relay-chain is allowed to use transact on this parachain.
				let origin = self.origin.clone().ok_or(XcmError::BadOrigin)?;

				// TODO: #2841 #TRANSACTFILTER allow the trait to issue filters for the relay-chain
				let message_call = call.take_decoded().map_err(|_| XcmError::FailedToDecode)?;
				let dispatch_origin = Config::OriginConverter::convert_origin(origin, origin_type)
					.map_err(|_| XcmError::BadOrigin)?;
				let weight = message_call.get_dispatch_info().weight;
				ensure!(weight.ref_time() <= require_weight_at_most, XcmError::MaxWeightInvalid);
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
				// We assume that the `Config::Weigher` will counts the `require_weight_at_most`
				// for the estimate of how much weight this instruction will take. Now that we know
				// that it's less, we credit it.
				//
				// We make the adjustment for the total surplus, which is used eventually
				// reported back to the caller and this ensures that they account for the total
				// weight consumed correctly (potentially allowing them to do more operations in a
				// block than they otherwise would).
				self.total_surplus.saturating_accrue(surplus.ref_time());
				Ok(())
			},
			QueryResponse { query_id, response, max_weight } => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				Config::ResponseHandler::on_response(origin, query_id, response, max_weight);
				Ok(())
			},
			DescendOrigin(who) => self
				.origin
				.as_mut()
				.ok_or(XcmError::BadOrigin)?
				.append_with(who)
				.map_err(|_| XcmError::MultiLocationFull),
			ClearOrigin => {
				self.origin = None;
				Ok(())
			},
			ReportError { query_id, dest, max_response_weight: max_weight } => {
				// Report the given result by sending a QueryResponse XCM to a previously given outcome
				// destination if one was registered.
				let response = Response::ExecutionResult(self.error);
				let message = QueryResponse { query_id, response, max_weight };
				Config::XcmSender::send_xcm(dest, Xcm(vec![message]))?;
				Ok(())
			},
			DepositAsset { assets, max_assets, beneficiary } => {
				let deposited = self.holding.limited_saturating_take(assets, max_assets as usize);
				for asset in deposited.into_assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &beneficiary)?;
				}
				Ok(())
			},
			DepositReserveAsset { assets, max_assets, dest, xcm } => {
				let deposited = self.holding.limited_saturating_take(assets, max_assets as usize);
				for asset in deposited.assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest)?;
				}
				// Note that we pass `None` as `maybe_failed_bin` and drop any assets which cannot
				// be reanchored  because we have already called `deposit_asset` on all assets.
				let assets = Self::reanchored(deposited, &dest, None);
				let mut message = vec![ReserveAssetDeposited(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				Config::XcmSender::send_xcm(dest, Xcm(message)).map_err(Into::into)
			},
			InitiateReserveWithdraw { assets, reserve, xcm } => {
				// Note that here we are able to place any assets which could not be reanchored
				// back into Holding.
				let assets = Self::reanchored(
					self.holding.saturating_take(assets),
					&reserve,
					Some(&mut self.holding),
				);
				let mut message = vec![WithdrawAsset(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				Config::XcmSender::send_xcm(reserve, Xcm(message)).map_err(Into::into)
			},
			InitiateTeleport { assets, dest, xcm } => {
				// We must do this first in order to resolve wildcards.
				let assets = self.holding.saturating_take(assets);
				for asset in assets.assets_iter() {
					Config::AssetTransactor::check_out(&dest, &asset);
				}
				// Note that we pass `None` as `maybe_failed_bin` and drop any assets which cannot
				// be reanchored  because we have already checked all assets out.
				let assets = Self::reanchored(assets, &dest, None);
				let mut message = vec![ReceiveTeleportedAsset(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				Config::XcmSender::send_xcm(dest, Xcm(message)).map_err(Into::into)
			},
			QueryHolding { query_id, dest, assets, max_response_weight } => {
				// Note that we pass `None` as `maybe_failed_bin` since no assets were ever removed
				// from Holding.
				let assets = Self::reanchored(self.holding.min(&assets), &dest, None);
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
						self.holding.try_take(fees.into()).map_err(|_| XcmError::NotHoldingFees)?;
					let unspent = self.trader.buy_weight(weight, max_fee)?;
					self.holding.subsume_assets(unspent);
				}
				Ok(())
			},
			RefundSurplus => {
				self.refund_surplus();
				Ok(())
			},
			SetErrorHandler(mut handler) => {
				let handler_weight = Config::Weigher::weight(&mut handler)
					.map_err(|()| XcmError::WeightNotComputable)?;
				self.total_surplus.saturating_accrue(self.error_handler_weight);
				self.error_handler = handler;
				self.error_handler_weight = handler_weight;
				Ok(())
			},
			SetAppendix(mut appendix) => {
				let appendix_weight = Config::Weigher::weight(&mut appendix)
					.map_err(|()| XcmError::WeightNotComputable)?;
				self.total_surplus.saturating_accrue(self.appendix_weight);
				self.appendix = appendix;
				self.appendix_weight = appendix_weight;
				Ok(())
			},
			ClearError => {
				self.error = None;
				Ok(())
			},
			ClaimAsset { assets, ticket } => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				let ok = Config::AssetClaims::claim_assets(origin, &ticket, &assets);
				ensure!(ok, XcmError::UnknownClaim);
				for asset in assets.drain().into_iter() {
					self.holding.subsume(asset);
				}
				Ok(())
			},
			Trap(code) => Err(XcmError::Trap(code)),
			SubscribeVersion { query_id, max_response_weight } => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?.clone();
				// We don't allow derivative origins to subscribe since it would otherwise pose a
				// DoS risk.
				ensure!(self.original_origin == origin, XcmError::BadOrigin);
				Config::SubscriptionService::start(&origin, query_id, max_response_weight)
			},
			UnsubscribeVersion => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				ensure!(&self.original_origin == origin, XcmError::BadOrigin);
				Config::SubscriptionService::stop(origin)
			},
			ExchangeAsset { .. } => Err(XcmError::Unimplemented),
			HrmpNewChannelOpenRequest { .. } => Err(XcmError::Unimplemented),
			HrmpChannelAccepted { .. } => Err(XcmError::Unimplemented),
			HrmpChannelClosing { .. } => Err(XcmError::Unimplemented),
		}
	}

	/// NOTE: Any assets which were unable to be reanchored are introduced into `failed_bin`.
	fn reanchored(
		mut assets: Assets,
		dest: &MultiLocation,
		maybe_failed_bin: Option<&mut Assets>,
	) -> MultiAssets {
		assets.reanchor(dest, &Config::LocationInverter::ancestry(), maybe_failed_bin);
		assets.into_assets_iter().collect::<Vec<_>>().into()
	}
}
