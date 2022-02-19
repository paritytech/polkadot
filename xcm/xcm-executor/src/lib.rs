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
	traits::{Contains, Get, PalletsInfoAccess},
	weights::GetDispatchInfo,
};
use parity_scale_codec::{Decode, Encode};
use sp_io::hashing::blake2_128;
use sp_runtime::traits::Saturating;
use sp_std::{marker::PhantomData, prelude::*};
use xcm::latest::prelude::*;

pub mod traits;
use traits::{
	validate_export, ClaimAssets, ConvertOrigin, DropAssets, ExportXcm, FeeManager, FeeReason,
	FilterAssetLocation, OnResponse, ShouldExecute, TransactAsset, UniversalLocation,
	VersionChangeNotifier, WeightBounds, WeightTrader,
};

mod assets;
pub use assets::Assets;
mod config;
pub use config::Config;

/// The XCM executor.
pub struct XcmExecutor<Config: config::Config> {
	pub holding: Assets,
	pub holding_limit: usize,
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
	pub error_handler: Xcm<Config::Call>,
	pub error_handler_weight: u64,
	pub appendix: Xcm<Config::Call>,
	pub appendix_weight: u64,
	pub transact_status: MaybeErrorCode,
	pub topic: Option<[u8; 32]>,
	_config: PhantomData<Config>,
}

pub struct WeighedMessage<Call>(Weight, Xcm<Call>);
impl<C> PreparedMessage for WeighedMessage<C> {
	fn weight_of(&self) -> Weight {
		self.0
	}
}

impl<Config: config::Config> ExecuteXcm<Config::Call> for XcmExecutor<Config> {
	type Prepared = WeighedMessage<Config::Call>;
	fn prepare(mut message: Xcm<Config::Call>) -> Result<Self::Prepared, Xcm<Config::Call>> {
		match Config::Weigher::weight(&mut message) {
			Ok(weight) => Ok(WeighedMessage(weight, message)),
			Err(_) => Err(message),
		}
	}
	fn execute(
		origin: impl Into<MultiLocation>,
		WeighedMessage(xcm_weight, mut message): WeighedMessage<Config::Call>,
		mut weight_credit: Weight,
	) -> Outcome {
		let origin = origin.into();
		log::trace!(
			target: "xcm::execute_xcm_in_credit",
			"origin: {:?}, message: {:?}, weight_credit: {:?}",
			origin,
			message,
			weight_credit,
		);
		if let Err(e) = Config::Barrier::should_execute(
			&origin,
			message.inner_mut(),
			xcm_weight,
			&mut weight_credit,
		) {
			log::debug!(
				target: "xcm::execute_xcm_in_credit",
				"Barrier blocked execution! Error: {:?}. (origin: {:?}, message: {:?}, weight_credit: {:?})",
				e,
				origin,
				message,
				weight_credit,
			);
			return Outcome::Error(XcmError::Barrier)
		}

		let mut vm = Self::new(origin);

		// We hash the message here instead of inside the loop, because `message` may have been
		// reassigned due to the existence of the error handler or the appendix.
		let message_hash = message.using_encoded(sp_io::hashing::blake2_256);

		while !message.0.is_empty() {
			let result = vm.execute_with_hash(message, message_hash);
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

		vm.post_execute(xcm_weight, message_hash)
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
			holding_limit: Config::MaxAssetsIntoHolding::get() as usize,
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
			transact_status: Default::default(),
			topic: None,
			_config: PhantomData,
		}
	}

	/// Execute the XCM program fragment and report back the error and which instruction caused it,
	/// or `Ok` if there was no error.
	pub fn execute(&mut self, xcm: Xcm<Config::Call>) -> Result<(), ExecutorError> {
		let message_hash = xcm.using_encoded(sp_io::hashing::blake2_256);
		self.execute_with_hash(xcm, message_hash)
	}

	fn execute_with_hash(
		&mut self,
		xcm: Xcm<Config::Call>,
		message_hash: [u8; 32],
	) -> Result<(), ExecutorError> {
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
					if let Err(e) = self.process_instruction(instr, message_hash) {
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
	pub fn post_execute(mut self, xcm_weight: Weight, message_hash: [u8; 32]) -> Outcome {
		// We silently drop any error from our attempt to refund the surplus as it's a charitable
		// thing so best-effort is all we will do.
		let _ = self.refund_surplus();
		drop(self.trader);

		let mut weight_used = xcm_weight.saturating_sub(self.total_surplus);

		if !self.holding.is_empty() {
			let context =
				XcmContext { origin: self.origin.clone(), message_hash, topic: self.topic };
			log::trace!(
				target: "xcm::execute_xcm_in_credit",
				"Trapping assets in holding register: {:?}, context: {:?} (original_origin: {:?})",
				self.holding, context, self.original_origin,
			);
			let trap_weight =
				Config::AssetTrap::drop_assets(&self.original_origin, self.holding, context);
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

	/// Send an XCM, charging fees from Holding as needed.
	fn send(
		&mut self,
		dest: MultiLocation,
		msg: Xcm<()>,
		reason: FeeReason,
	) -> Result<XcmHash, XcmError> {
		let (ticket, fee) = validate_send::<Config::XcmSender>(dest, msg)?;
		if !Config::FeeManager::is_waived(&self.origin, reason) {
			let paid = self.holding.try_take(fee.into()).map_err(|_| XcmError::NotHoldingFees)?;
			Config::FeeManager::handle_fee(paid.into());
		}
		Config::XcmSender::deliver(ticket).map_err(Into::into)
	}

	/// Remove the registered error handler and return it. Do not refund its weight.
	fn take_error_handler(&mut self) -> Xcm<Config::Call> {
		let mut r = Xcm::<Config::Call>(vec![]);
		sp_std::mem::swap(&mut self.error_handler, &mut r);
		self.error_handler_weight = 0;
		r
	}

	/// Drop the registered error handler and refund its weight.
	fn drop_error_handler(&mut self) {
		self.error_handler = Xcm::<Config::Call>(vec![]);
		self.total_surplus.saturating_accrue(self.error_handler_weight);
		self.error_handler_weight = 0;
	}

	/// Remove the registered appendix and return it.
	fn take_appendix(&mut self) -> Xcm<Config::Call> {
		let mut r = Xcm::<Config::Call>(vec![]);
		sp_std::mem::swap(&mut self.appendix, &mut r);
		self.appendix_weight = 0;
		r
	}

	fn subsume_asset(&mut self, asset: MultiAsset) -> Result<(), XcmError> {
		// worst-case, holding.len becomes 2 * holding_limit.
		ensure!(self.holding.len() + 1 <= self.holding_limit * 2, XcmError::HoldingWouldOverflow);
		self.holding.subsume(asset);
		Ok(())
	}

	fn subsume_assets(&mut self, assets: Assets) -> Result<(), XcmError> {
		// worst-case, holding.len becomes 2 * holding_limit.
		// this guarantees that if holding.len() == holding_limit and you have holding_limit more
		// items (which has a best case outcome of holding.len() == holding_limit), then you'll
		// be guaranteed of making the operation.
		let worst_case_holding_len = self.holding.len() + assets.len();
		ensure!(worst_case_holding_len <= self.holding_limit * 2, XcmError::HoldingWouldOverflow);
		self.holding.subsume_assets(assets);
		Ok(())
	}

	/// Refund any unused weight.
	fn refund_surplus(&mut self) -> Result<(), XcmError> {
		let current_surplus = self.total_surplus.saturating_sub(self.total_refunded);
		if current_surplus > 0 {
			self.total_refunded.saturating_accrue(current_surplus);
			if let Some(w) = self.trader.refund_weight(current_surplus) {
				self.subsume_asset(w)?;
			}
		}
		Ok(())
	}

	/// Process a single XCM instruction, mutating the state of the XCM virtual machine.
	fn process_instruction(
		&mut self,
		instr: Instruction<Config::Call>,
		message_hash: [u8; 32],
	) -> Result<(), XcmError> {
		match instr {
			WithdrawAsset(assets) => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?.clone();
				for asset in assets.drain().into_iter() {
					let context = XcmContext {
						origin: Some(origin.clone()),
						message_hash,
						topic: self.topic,
					};
					Config::AssetTransactor::withdraw_asset(&asset, &origin, context)?;
					self.subsume_asset(asset)?;
				}
				Ok(())
			},
			ReserveAssetDeposited(assets) => {
				// check whether we trust origin to be our reserve location for this asset.
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?.clone();
				for asset in assets.drain().into_iter() {
					// Must ensure that we recognise the asset as being managed by the origin.
					ensure!(
						Config::IsReserve::filter_asset_location(&asset, &origin),
						XcmError::UntrustedReserveLocation
					);
					self.subsume_asset(asset)?;
				}
				Ok(())
			},
			TransferAsset { assets, beneficiary } => {
				// Take `assets` from the origin account (on-chain) and place into dest account.
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.inner() {
					let context = XcmContext {
						origin: Some(origin.clone()),
						message_hash,
						topic: self.topic,
					};
					Config::AssetTransactor::beam_asset(&asset, origin, &beneficiary, context)?;
				}
				Ok(())
			},
			TransferReserveAsset { mut assets, dest, xcm } => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				// Take `assets` from the origin account (on-chain) and place into dest account.
				for asset in assets.inner() {
					let context = XcmContext {
						origin: Some(origin.clone()),
						message_hash,
						topic: self.topic,
					};
					Config::AssetTransactor::beam_asset(asset, origin, &dest, context)?;
				}
				let context = Config::LocationInverter::universal_location().into();
				assets.reanchor(&dest, &context).map_err(|()| XcmError::MultiLocationFull)?;
				let mut message = vec![ReserveAssetDeposited(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				self.send(dest, Xcm(message), FeeReason::TransferReserveAsset)?;
				Ok(())
			},
			ReceiveTeleportedAsset(assets) => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?.clone();
				let context =
					XcmContext { origin: Some(origin.clone()), message_hash, topic: self.topic };
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
					Config::AssetTransactor::can_check_in(&origin, asset, context.clone())?;
				}
				for asset in assets.drain().into_iter() {
					Config::AssetTransactor::check_in(&origin, &asset, context.clone());
					self.subsume_asset(asset)?;
				}
				Ok(())
			},
			Transact { origin_kind, require_weight_at_most, mut call } => {
				// We assume that the Relay-chain is allowed to use transact on this parachain.
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?.clone();

				// TODO: #2841 #TRANSACTFILTER allow the trait to issue filters for the relay-chain
				let message_call = call.take_decoded().map_err(|_| XcmError::FailedToDecode)?;
				let dispatch_origin = Config::OriginConverter::convert_origin(origin, origin_kind)
					.map_err(|_| XcmError::BadOrigin)?;
				let weight = message_call.get_dispatch_info().weight;
				ensure!(weight <= require_weight_at_most, XcmError::MaxWeightInvalid);
				let maybe_actual_weight = match message_call.dispatch(dispatch_origin) {
					Ok(post_info) => {
						self.transact_status = MaybeErrorCode::Success;
						post_info.actual_weight
					},
					Err(error_and_info) => {
						self.transact_status = MaybeErrorCode::Error(error_and_info.error.encode());
						error_and_info.post_info.actual_weight
					},
				};
				let actual_weight = maybe_actual_weight.unwrap_or(weight);
				let surplus = weight.saturating_sub(actual_weight);
				// We assume that the `Config::Weigher` will counts the `require_weight_at_most`
				// for the estimate of how much weight this instruction will take. Now that we know
				// that it's less, we credit it.
				//
				// We make the adjustment for the total surplus, which is used eventually
				// reported back to the caller and this ensures that they account for the total
				// weight consumed correctly (potentially allowing them to do more operations in a
				// block than they otherwise would).
				self.total_surplus.saturating_accrue(surplus);
				Ok(())
			},
			QueryResponse { query_id, response, max_weight, querier } => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				let context =
					XcmContext { origin: Some(origin.clone()), message_hash, topic: self.topic };
				Config::ResponseHandler::on_response(
					origin,
					query_id,
					querier.as_ref(),
					response,
					max_weight,
					context,
				);
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
			ReportError(response_info) => {
				// Report the given result by sending a QueryResponse XCM to a previously given outcome
				// destination if one was registered.
				self.respond(
					self.origin.clone(),
					Response::ExecutionResult(self.error),
					response_info,
					FeeReason::Report,
				)?;
				Ok(())
			},
			DepositAsset { assets, beneficiary } => {
				let deposited = self.holding.saturating_take(assets);
				for asset in deposited.into_assets_iter() {
					let context =
						XcmContext { origin: self.origin.clone(), message_hash, topic: self.topic };
					Config::AssetTransactor::deposit_asset(&asset, &beneficiary, context)?;
				}
				Ok(())
			},
			DepositReserveAsset { assets, dest, xcm } => {
				let deposited = self.holding.saturating_take(assets);
				for asset in deposited.assets_iter() {
					let context =
						XcmContext { origin: self.origin.clone(), message_hash, topic: self.topic };
					Config::AssetTransactor::deposit_asset(&asset, &dest, context)?;
				}
				// Note that we pass `None` as `maybe_failed_bin` and drop any assets which cannot
				// be reanchored  because we have already called `deposit_asset` on all assets.
				let assets = Self::reanchored(deposited, &dest, None);
				let mut message = vec![ReserveAssetDeposited(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				self.send(dest, Xcm(message), FeeReason::DepositReserveAsset)?;
				Ok(())
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
				self.send(reserve, Xcm(message), FeeReason::InitiateReserveWithdraw)?;
				Ok(())
			},
			InitiateTeleport { assets, dest, xcm } => {
				// We must do this first in order to resolve wildcards.
				let assets = self.holding.saturating_take(assets);
				for asset in assets.assets_iter() {
					let context =
						XcmContext { origin: self.origin.clone(), message_hash, topic: self.topic };
					Config::AssetTransactor::check_out(&dest, &asset, context);
				}
				// Note that we pass `None` as `maybe_failed_bin` and drop any assets which cannot
				// be reanchored  because we have already checked all assets out.
				let assets = Self::reanchored(assets, &dest, None);
				let mut message = vec![ReceiveTeleportedAsset(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				self.send(dest, Xcm(message), FeeReason::InitiateTeleport)?;
				Ok(())
			},
			ReportHolding { response_info, assets } => {
				// Note that we pass `None` as `maybe_failed_bin` since no assets were ever removed
				// from Holding.
				let assets =
					Self::reanchored(self.holding.min(&assets), &response_info.destination, None);
				self.respond(
					self.origin.clone(),
					Response::Assets(assets),
					response_info,
					FeeReason::Report,
				)?;
				Ok(())
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
					self.subsume_assets(unspent)?;
				}
				Ok(())
			},
			RefundSurplus => self.refund_surplus(),
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
				let context =
					XcmContext { origin: Some(origin.clone()), message_hash, topic: self.topic };
				let ok = Config::AssetClaims::claim_assets(origin, &ticket, &assets, context);
				ensure!(ok, XcmError::UnknownClaim);
				for asset in assets.drain().into_iter() {
					self.subsume_asset(asset)?;
				}
				Ok(())
			},
			Trap(code) => Err(XcmError::Trap(code)),
			SubscribeVersion { query_id, max_response_weight } => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				// We don't allow derivative origins to subscribe since it would otherwise pose a
				// DoS risk.
				ensure!(&self.original_origin == origin, XcmError::BadOrigin);
				let context =
					XcmContext { origin: Some(origin.clone()), message_hash, topic: self.topic };
				Config::SubscriptionService::start(origin, query_id, max_response_weight, context)
			},
			UnsubscribeVersion => {
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?;
				ensure!(&self.original_origin == origin, XcmError::BadOrigin);
				let context =
					XcmContext { origin: Some(origin.clone()), message_hash, topic: self.topic };
				Config::SubscriptionService::stop(origin, context)
			},
			BurnAsset(assets) => {
				self.holding.saturating_take(assets.into());
				Ok(())
			},
			ExpectAsset(assets) =>
				self.holding.ensure_contains(&assets).map_err(|_| XcmError::ExpectationFalse),
			ExpectOrigin(origin) => {
				ensure!(self.origin == origin, XcmError::ExpectationFalse);
				Ok(())
			},
			ExpectError(error) => {
				ensure!(self.error == error, XcmError::ExpectationFalse);
				Ok(())
			},
			QueryPallet { module_name, response_info } => {
				let pallets = Config::PalletInstancesInfo::infos()
					.into_iter()
					.filter(|x| x.module_name.as_bytes() == &module_name[..])
					.map(|x| PalletInfo {
						index: x.index as u32,
						name: x.name.as_bytes().into(),
						module_name: x.module_name.as_bytes().into(),
						major: x.crate_version.major as u32,
						minor: x.crate_version.minor as u32,
						patch: x.crate_version.patch as u32,
					})
					.collect::<Vec<_>>();
				let QueryResponseInfo { destination, query_id, max_weight } = response_info;
				let response = Response::PalletsInfo(pallets);
				let querier = Self::to_querier(self.origin.clone(), &destination)?;
				let instruction = QueryResponse { query_id, response, max_weight, querier };
				let message = Xcm(vec![instruction]);
				self.send(destination, message, FeeReason::QueryPallet)?;
				Ok(())
			},
			ExpectPallet { index, name, module_name, crate_major, min_crate_minor } => {
				let pallet = Config::PalletInstancesInfo::infos()
					.into_iter()
					.find(|x| x.index == index as usize)
					.ok_or(XcmError::PalletNotFound)?;
				ensure!(pallet.name.as_bytes() == &name[..], XcmError::NameMismatch);
				ensure!(pallet.module_name.as_bytes() == &module_name[..], XcmError::NameMismatch);
				let major = pallet.crate_version.major as u32;
				ensure!(major == crate_major, XcmError::VersionIncompatible);
				let minor = pallet.crate_version.minor as u32;
				ensure!(minor >= min_crate_minor, XcmError::VersionIncompatible);
				Ok(())
			},
			ReportTransactStatus(response_info) => {
				self.respond(
					self.origin.clone(),
					Response::DispatchResult(self.transact_status.clone()),
					response_info,
					FeeReason::Report,
				)?;
				Ok(())
			},
			ClearTransactStatus => {
				self.transact_status = Default::default();
				Ok(())
			},
			UniversalOrigin(new_global) => {
				let universal_location = Config::LocationInverter::universal_location();
				ensure!(universal_location.first() != Some(&new_global), XcmError::InvalidLocation);
				let origin = self.origin.as_ref().ok_or(XcmError::BadOrigin)?.clone();
				let origin_xform = (origin, new_global);
				let ok = Config::UniversalAliases::contains(&origin_xform);
				ensure!(ok, XcmError::InvalidLocation);
				let (_, new_global) = origin_xform;
				let new_origin = X1(new_global).relative_to(&universal_location);
				self.origin = Some(new_origin);
				Ok(())
			},
			ExportMessage { network, destination, xcm } => {
				let hash = (&self.origin, &destination).using_encoded(blake2_128);
				let channel = u32::decode(&mut hash.as_ref()).unwrap_or(0);
				// Hash identifies the lane on the exporter which we use. We use the pairwise
				// combination of the origin and destination to ensure origin/destination pairs will
				// generally have their own lanes.
				let (ticket, fee) =
					validate_export::<Config::MessageExporter>(network, channel, destination, xcm)?;
				if !Config::FeeManager::is_waived(&self.origin, FeeReason::Export(network)) {
					let paid =
						self.holding.try_take(fee.into()).map_err(|_| XcmError::NotHoldingFees)?;
					Config::FeeManager::handle_fee(paid.into());
				}
				Config::MessageExporter::deliver(ticket)?;
				Ok(())
			},
			SetTopic(topic) => {
				self.topic = Some(topic);
				Ok(())
			},
			ClearTopic => {
				self.topic = None;
				Ok(())
			},
			ExchangeAsset { .. } => Err(XcmError::Unimplemented),
			HrmpNewChannelOpenRequest { .. } => Err(XcmError::Unimplemented),
			HrmpChannelAccepted { .. } => Err(XcmError::Unimplemented),
			HrmpChannelClosing { .. } => Err(XcmError::Unimplemented),
		}
	}

	/// Calculates what `local_querier` would be from the perspective of `destination`.
	fn to_querier(
		local_querier: Option<MultiLocation>,
		destination: &MultiLocation,
	) -> Result<Option<MultiLocation>, XcmError> {
		Ok(match local_querier {
			None => None,
			Some(q) => Some(
				q.reanchored(&destination, &Config::LocationInverter::universal_location().into())
					.map_err(|_| XcmError::ReanchorFailed)?,
			),
		})
	}

	/// Send a bare `QueryResponse` message containing `response` informed by the given `info`.
	///
	/// The `local_querier` argument is the querier (if any) specified from the *local* perspective.
	fn respond(
		&mut self,
		local_querier: Option<MultiLocation>,
		response: Response,
		info: QueryResponseInfo,
		fee_reason: FeeReason,
	) -> Result<XcmHash, XcmError> {
		let querier = Self::to_querier(local_querier, &info.destination)?;
		let QueryResponseInfo { destination, query_id, max_weight } = info;
		let instruction = QueryResponse { query_id, response, max_weight, querier };
		let message = Xcm(vec![instruction]);
		let (ticket, fee) = validate_send::<Config::XcmSender>(destination, message)?;
		if !Config::FeeManager::is_waived(&self.origin, fee_reason) {
			let paid = self.holding.try_take(fee.into()).map_err(|_| XcmError::NotHoldingFees)?;
			Config::FeeManager::handle_fee(paid.into());
		}
		Config::XcmSender::deliver(ticket).map_err(Into::into)
	}

	/// NOTE: Any assets which were unable to be reanchored are introduced into `failed_bin`.
	fn reanchored(
		mut assets: Assets,
		dest: &MultiLocation,
		maybe_failed_bin: Option<&mut Assets>,
	) -> MultiAssets {
		let context = Config::LocationInverter::universal_location().into();
		assets.reanchor(dest, &context, maybe_failed_bin);
		assets.into_assets_iter().collect::<Vec<_>>().into()
	}
}
