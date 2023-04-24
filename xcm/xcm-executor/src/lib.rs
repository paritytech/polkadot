// Copyright (C) Parity Technologies (UK) Ltd.
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
	dispatch::GetDispatchInfo,
	ensure,
	traits::{Contains, ContainsPair, Get, PalletsInfoAccess},
};
use parity_scale_codec::{Decode, Encode};
use sp_core::defer;
use sp_io::hashing::blake2_128;
use sp_std::{marker::PhantomData, prelude::*};
use sp_weights::Weight;
use xcm::latest::prelude::*;

pub mod traits;
use traits::{
	validate_export, AssetExchange, AssetLock, CallDispatcher, ClaimAssets, ConvertOrigin,
	DropAssets, Enact, ExportXcm, FeeManager, FeeReason, OnResponse, ProcessInstruction,
	ShouldExecute, TransactAsset, VersionChangeNotifier, WeightBounds, WeightTrader,
};

mod assets;
pub use assets::Assets;
mod config;
pub use config::Config;
mod vm_registers;
pub use vm_registers::XcVmRegisters;

/// A struct to specify how fees are being paid.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FeesMode {
	/// If true, then the fee assets are taken directly from the origin's on-chain account,
	/// otherwise the fee assets are taken from the holding register.
	///
	/// Defaults to false.
	pub jit_withdraw: bool,
}

const RECURSION_LIMIT: u8 = 10;

environmental::environmental!(recursion_count: u8);

/// The XCM executor.
pub struct XcmExecutor<Config: config::Config> {
	_config: PhantomData<Config>,
	processor: Config::ProcessInstruction,
	state: XcVmRegisters<Config::RuntimeCall>,
}

#[cfg(feature = "runtime-benchmarks")]
impl<Config: config::Config> XcmExecutor<Config> {
	pub fn holding(&self) -> &Assets {
		&self.state.holding
	}
	pub fn set_holding(&mut self, v: Assets) {
		self.state.holding = v
	}
	pub fn holding_limit(&self) -> &usize {
		&self.state.holding_limit()
	}
	pub fn set_holding_limit(&mut self, v: usize) {
		self.state.set_holding_limit(v)
	}
	pub fn origin(&self) -> &Option<MultiLocation> {
		&self.state.context().origin
	}
	pub fn set_origin(&mut self, v: Option<MultiLocation>) {
		self.state.set_origin(v)
	}
	pub fn original_origin(&self) -> &MultiLocation {
		&self.state.original_origin()
	}
	pub fn set_original_origin(&mut self, v: MultiLocation) {
		self.state.set_original_origin(v)
	}
	pub fn error(&self) -> Option<(u32, XcmError)> {
		self.state.get_error()
	}
	pub fn set_error(&mut self, v: Option<(u32, XcmError)>) {
		self.state.set_error(v)
	}
	pub fn total_surplus(&self) -> &Weight {
		&self.state.total_surplus
	}
	pub fn set_total_surplus(&mut self, v: Weight) {
		self.state.total_surplus = v
	}
	pub fn total_refunded(&self) -> &Weight {
		&self.state.total_refunded
	}
	pub fn set_total_refunded(&mut self, v: Weight) {
		self.state.total_refunded = v
	}
	pub fn error_handler(&self) -> &Xcm<Config::RuntimeCall> {
		&self.state.error_handler
	}
	pub fn set_error_handler(&mut self, v: Xcm<Config::RuntimeCall>) {
		self.state.error_handler = v
	}
	pub fn error_handler_weight(&self) -> &Weight {
		&self.state.error_handler_weight
	}
	pub fn set_error_handler_weight(&mut self, v: Weight) {
		self.state.error_handler_weight = v
	}
	pub fn appendix(&self) -> &Xcm<Config::RuntimeCall> {
		&self.state.appendix
	}
	pub fn set_appendix(&mut self, v: Xcm<Config::RuntimeCall>) {
		self.state.appendix = v
	}
	pub fn appendix_weight(&self) -> &Weight {
		&self.state.appendix_weight
	}
	pub fn set_appendix_weight(&mut self, v: Weight) {
		self.state.appendix_weight = v
	}
	pub fn transact_status(&self) -> &MaybeErrorCode {
		&self.state.transact_status
	}
	pub fn set_transact_status(&mut self, v: MaybeErrorCode) {
		self.state.transact_status = v
	}
	pub fn fees_mode(&self) -> &FeesMode {
		&self.state.fees_mode
	}
	pub fn set_fees_mode(&mut self, v: FeesMode) {
		self.state.fees_mode = v
	}
	pub fn topic(&self) -> &Option<[u8; 32]> {
		&self.state.context().topic
	}
	pub fn set_topic(&mut self, v: Option<[u8; 32]>) {
		self.state.set_topic(v);
	}
}

pub struct WeighedMessage<Call>(Weight, Xcm<Call>);
impl<C> PreparedMessage for WeighedMessage<C> {
	fn weight_of(&self) -> Weight {
		self.0
	}
}

impl<Config: config::Config> ExecuteXcm<Config::RuntimeCall> for XcmExecutor<Config> {
	type Prepared = WeighedMessage<Config::RuntimeCall>;
	fn prepare(
		mut message: Xcm<Config::RuntimeCall>,
	) -> Result<Self::Prepared, Xcm<Config::RuntimeCall>> {
		match Config::Weigher::weight(&mut message) {
			Ok(weight) => Ok(WeighedMessage(weight, message)),
			Err(_) => Err(message),
		}
	}
	fn execute(
		origin: impl Into<MultiLocation>,
		WeighedMessage(xcm_weight, mut message): WeighedMessage<Config::RuntimeCall>,
		message_hash: XcmHash,
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
			log::trace!(
				target: "xcm::execute_xcm_in_credit",
				"Barrier blocked execution! Error: {:?}. (origin: {:?}, message: {:?}, weight_credit: {:?})",
				e,
				origin,
				message,
				weight_credit,
			);
			return Outcome::Error(XcmError::Barrier)
		}

		let mut vm = Self::new(origin, message_hash);

		while !message.0.is_empty() {
			let result = vm.process(message);
			log::trace!(target: "xcm::execute_xcm_in_credit", "result: {:?}", result);
			message = if let Err(error) = result {
				vm.state.total_surplus.saturating_accrue(error.weight);
				vm.state.put_error(error.index, error.xcm_error);
				vm.take_error_handler().or_else(|| vm.take_appendix())
			} else {
				vm.drop_error_handler();
				vm.take_appendix()
			}
		}

		vm.post_process(xcm_weight)
	}

	fn charge_fees(origin: impl Into<MultiLocation>, fees: MultiAssets) -> XcmResult {
		let origin = origin.into();
		if !Config::FeeManager::is_waived(Some(&origin), FeeReason::ChargeFees) {
			for asset in fees.inner() {
				Config::AssetTransactor::withdraw_asset(&asset, &origin, None)?;
			}
			Config::FeeManager::handle_fee(fees);
		}
		Ok(())
	}
}

#[derive(Debug)]
pub struct ExecutorError {
	pub index: u32,
	pub xcm_error: XcmError,
	pub weight: Weight,
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
	pub fn new(origin: impl Into<MultiLocation>, message_hash: XcmHash) -> Self {
		let origin = origin.into();
		let state =
			XcVmRegisters::new(Config::MaxAssetsIntoHolding::get() as usize, origin, message_hash);
		Self { processor: Config::ProcessInstruction::new(), state, _config: PhantomData }
	}

	#[cfg(feature = "runtime-benchmarks")]
	pub fn bench_process(&mut self, xcm: Xcm<Config::RuntimeCall>) -> Result<(), ExecutorError> {
		self.process(xcm)
	}

	fn process(&mut self, xcm: Xcm<Config::RuntimeCall>) -> Result<(), ExecutorError> {
		log::trace!(
			target: "xcm::process",
			"origin: {:?}, total_surplus/refunded: {:?}/{:?}, error_handler_weight: {:?}",
			self.state.context().origin.as_ref(),
			self.state.total_surplus,
			self.state.total_refunded,
			self.state.error_handler_weight,
		);
		let mut result = Ok(());
		for (i, instr) in xcm.0.into_iter().enumerate() {
			match &mut result {
				r @ Ok(()) => {
					// Initialize the recursion count only the first time we hit this code in our
					// potential recursive execution.
					let inst_res = recursion_count::using_once(&mut 1, || {
						recursion_count::with(|count| {
							if *count > RECURSION_LIMIT {
								return Err(XcmError::ExceedsStackLimit)
							}
							*count = count.saturating_add(1);
							Ok(())
						})
						// This should always return `Some`, but let's play it safe.
						.unwrap_or(Ok(()))?;

						// Ensure that we always decrement the counter whenever we finish processing
						// the instruction.
						defer! {
							recursion_count::with(|count| {
								*count = count.saturating_sub(1);
							});
						}

						self.processor.process_instruction(&mut self.state, instr)
					});
					if let Err(e) = inst_res {
						log::trace!(target: "xcm::execute", "!!! ERROR: {:?}", e);
						*r = Err(ExecutorError {
							index: i as u32,
							xcm_error: e,
							weight: Weight::zero(),
						});
					}
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
	pub fn post_process(mut self, xcm_weight: Weight) -> Outcome {
		self.processor.post_process(&mut self.state);
		drop(self.processor);

		let vm_registers::XcVmFinalState {
			context,
			error,
			holding,
			original_origin,
			total_surplus,
		} = self.state.destruct();

		let mut weight_used = xcm_weight.saturating_sub(total_surplus);

		if !holding.is_empty() {
			log::trace!(
				target: "xcm::execute_xcm_in_credit",
				"Trapping assets in holding register: {:?}, context: {:?} (original_origin: {:?})",
				holding, context, original_origin,
			);
			let effective_origin = context.origin.unwrap_or(original_origin);
			let trap_weight = Config::AssetTrap::drop_assets(&effective_origin, holding, &context);
			weight_used.saturating_accrue(trap_weight);
		};

		match error {
			None => Outcome::Complete(weight_used),
			// TODO: #2841 #REALWEIGHT We should deduct the cost of any instructions following
			// the error which didn't end up being executed.
			Some((_i, e)) => {
				log::trace!(target: "xcm::execute_xcm_in_credit", "Execution errored at {:?}: {:?} (original_origin: {:?})", _i, e, original_origin);
				Outcome::Incomplete(weight_used, e)
			},
		}
	}

	/// Remove the registered error handler and return it. Do not refund its weight.
	fn take_error_handler(&mut self) -> Xcm<Config::RuntimeCall> {
		let mut r = Xcm::<Config::RuntimeCall>(vec![]);
		sp_std::mem::swap(&mut self.state.error_handler, &mut r);
		self.state.error_handler_weight = Weight::zero();
		r
	}

	/// Drop the registered error handler and refund its weight.
	fn drop_error_handler(&mut self) {
		self.state.error_handler = Xcm::<Config::RuntimeCall>(vec![]);
		self.state.total_surplus.saturating_accrue(self.state.error_handler_weight);
		self.state.error_handler_weight = Weight::zero();
	}

	/// Remove the registered appendix and return it.
	fn take_appendix(&mut self) -> Xcm<Config::RuntimeCall> {
		let mut r = Xcm::<Config::RuntimeCall>(vec![]);
		sp_std::mem::swap(&mut self.state.appendix, &mut r);
		self.state.appendix_weight = Weight::zero();
		r
	}
}

type StateOf<Config> = XcVmRegisters<<Config as config::Config>::RuntimeCall>;

pub struct XcmProcessor<Config: config::Config> {
	_config: PhantomData<Config>,
	trader: Config::Trader,
}

impl<Config: config::Config> ProcessInstruction<Config::RuntimeCall> for XcmProcessor<Config> {
	fn new() -> Self {
		Self { _config: PhantomData, trader: Config::Trader::new() }
	}

	fn process_instruction(
		&mut self,
		vm_state: &mut StateOf<Config>,
		instr: Instruction<Config::RuntimeCall>,
	) -> Result<(), XcmError> {
		log::trace!(
			target: "xcm::process_instruction",
			"=== {:?}",
			instr
		);
		match instr {
			WithdrawAsset(assets) => {
				// Take `assets` from the origin account (on-chain) and place in holding.
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.into_inner().into_iter() {
					Config::AssetTransactor::withdraw_asset(
						&asset,
						&origin,
						Some(&vm_state.context()),
					)?;
					vm_state.subsume_asset(asset)?;
				}
				Ok(())
			},
			ReserveAssetDeposited(assets) => {
				// check whether we trust origin to be our reserve location for this asset.
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.into_inner().into_iter() {
					// Must ensure that we recognise the asset as being managed by the origin.
					ensure!(
						Config::IsReserve::contains(&asset, &origin),
						XcmError::UntrustedReserveLocation
					);
					vm_state.subsume_asset(asset)?;
				}
				Ok(())
			},
			TransferAsset { assets, beneficiary } => {
				// Take `assets` from the origin account (on-chain) and place into dest account.
				let origin = vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				for asset in assets.inner() {
					Config::AssetTransactor::transfer_asset(
						&asset,
						origin,
						&beneficiary,
						&vm_state.context(),
					)?;
				}
				Ok(())
			},
			TransferReserveAsset { mut assets, dest, xcm } => {
				let origin = vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				// Take `assets` from the origin account (on-chain) and place into dest account.
				for asset in assets.inner() {
					Config::AssetTransactor::transfer_asset(
						asset,
						origin,
						&dest,
						&vm_state.context(),
					)?;
				}
				let reanchor_context = Config::UniversalLocation::get();
				assets.reanchor(&dest, reanchor_context).map_err(|()| XcmError::LocationFull)?;
				let mut message = vec![ReserveAssetDeposited(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				Self::send(vm_state, dest, Xcm(message), FeeReason::TransferReserveAsset)?;
				Ok(())
			},
			ReceiveTeleportedAsset(assets) => {
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				// check whether we trust origin to teleport this asset to us via config trait.
				for asset in assets.inner() {
					// We only trust the origin to send us assets that they identify as their
					// sovereign assets.
					ensure!(
						Config::IsTeleporter::contains(asset, &origin),
						XcmError::UntrustedTeleportLocation
					);
					// We should check that the asset can actually be teleported in (for this to be in error, there
					// would need to be an accounting violation by one of the trusted chains, so it's unlikely, but we
					// don't want to punish a possibly innocent chain/user).
					Config::AssetTransactor::can_check_in(&origin, asset, &vm_state.context())?;
				}
				for asset in assets.into_inner().into_iter() {
					Config::AssetTransactor::check_in(&origin, &asset, &vm_state.context());
					vm_state.subsume_asset(asset)?;
				}
				Ok(())
			},
			Transact { origin_kind, require_weight_at_most, mut call } => {
				// We assume that the Relay-chain is allowed to use transact on this parachain.
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;

				// TODO: #2841 #TRANSACTFILTER allow the trait to issue filters for the relay-chain
				let message_call = call.take_decoded().map_err(|_| XcmError::FailedToDecode)?;
				ensure!(Config::SafeCallFilter::contains(&message_call), XcmError::NoPermission);
				let dispatch_origin = Config::OriginConverter::convert_origin(origin, origin_kind)
					.map_err(|_| XcmError::BadOrigin)?;
				let weight = message_call.get_dispatch_info().weight;
				ensure!(weight.all_lte(require_weight_at_most), XcmError::MaxWeightInvalid);
				let maybe_actual_weight =
					match Config::CallDispatcher::dispatch(message_call, dispatch_origin) {
						Ok(post_info) => {
							vm_state.transact_status = MaybeErrorCode::Success;
							post_info.actual_weight
						},
						Err(error_and_info) => {
							vm_state.transact_status = error_and_info.error.encode().into();
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
				vm_state.total_surplus.saturating_accrue(surplus);
				Ok(())
			},
			QueryResponse { query_id, response, max_weight, querier } => {
				let origin = vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				Config::ResponseHandler::on_response(
					origin,
					query_id,
					querier.as_ref(),
					response,
					max_weight,
					&vm_state.context(),
				);
				Ok(())
			},
			DescendOrigin(who) => {
				let mut origin = vm_state.cloned_origin().ok_or(XcmError::BadOrigin)?;
				origin.append_with(who).map_err(|_| XcmError::LocationFull)?;
				vm_state.set_origin(Some(origin));
				Ok(())
			},
			ClearOrigin => {
				vm_state.set_origin(None);
				Ok(())
			},
			ReportError(response_info) => {
				// Report the given result by sending a QueryResponse XCM to a previously given outcome
				// destination if one was registered.
				Self::respond(
					vm_state,
					Response::ExecutionResult(vm_state.get_error()),
					response_info,
					FeeReason::Report,
				)?;
				Ok(())
			},
			DepositAsset { assets, beneficiary } => {
				let deposited = vm_state.holding.saturating_take(assets);
				for asset in deposited.into_assets_iter() {
					Config::AssetTransactor::deposit_asset(
						&asset,
						&beneficiary,
						&vm_state.context(),
					)?;
				}
				Ok(())
			},
			DepositReserveAsset { assets, dest, xcm } => {
				let deposited = vm_state.holding.saturating_take(assets);
				for asset in deposited.assets_iter() {
					Config::AssetTransactor::deposit_asset(&asset, &dest, &vm_state.context())?;
				}
				// Note that we pass `None` as `maybe_failed_bin` and drop any assets which cannot
				// be reanchored  because we have already called `deposit_asset` on all assets.
				let assets = Self::reanchored(deposited, &dest, None);
				let mut message = vec![ReserveAssetDeposited(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				Self::send(vm_state, dest, Xcm(message), FeeReason::DepositReserveAsset)?;
				Ok(())
			},
			InitiateReserveWithdraw { assets, reserve, xcm } => {
				// Note that here we are able to place any assets which could not be reanchored
				// back into Holding.
				let assets = Self::reanchored(
					vm_state.holding.saturating_take(assets),
					&reserve,
					Some(&mut vm_state.holding),
				);
				let mut message = vec![WithdrawAsset(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				Self::send(vm_state, reserve, Xcm(message), FeeReason::InitiateReserveWithdraw)?;
				Ok(())
			},
			InitiateTeleport { assets, dest, xcm } => {
				// We must do this first in order to resolve wildcards.
				let assets = vm_state.holding.saturating_take(assets);
				for asset in assets.assets_iter() {
					// We should check that the asset can actually be teleported out (for this to
					// be in error, there would need to be an accounting violation by ourselves,
					// so it's unlikely, but we don't want to allow that kind of bug to leak into
					// a trusted chain.
					Config::AssetTransactor::can_check_out(&dest, &asset, &vm_state.context())?;
				}
				for asset in assets.assets_iter() {
					Config::AssetTransactor::check_out(&dest, &asset, &vm_state.context());
				}
				// Note that we pass `None` as `maybe_failed_bin` and drop any assets which cannot
				// be reanchored  because we have already checked all assets out.
				let assets = Self::reanchored(assets, &dest, None);
				let mut message = vec![ReceiveTeleportedAsset(assets), ClearOrigin];
				message.extend(xcm.0.into_iter());
				Self::send(vm_state, dest, Xcm(message), FeeReason::InitiateTeleport)?;
				Ok(())
			},
			ReportHolding { response_info, assets } => {
				// Note that we pass `None` as `maybe_failed_bin` since no assets were ever removed
				// from Holding.
				let assets = Self::reanchored(
					vm_state.holding.min(&assets),
					&response_info.destination,
					None,
				);
				Self::respond(
					vm_state,
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
				if let Some(weight) = Option::<Weight>::from(weight_limit) {
					// pay for `weight` using up to `fees` of the holding register.
					let max_fee = vm_state
						.holding
						.try_take(fees.into())
						.map_err(|_| XcmError::NotHoldingFees)?;
					let unspent = self.trader.buy_weight(weight, max_fee)?;
					vm_state.subsume_assets(unspent)?;
				}
				Ok(())
			},
			RefundSurplus => self.refund_surplus(vm_state),
			SetErrorHandler(mut handler) => {
				let handler_weight = Config::Weigher::weight(&mut handler)
					.map_err(|()| XcmError::WeightNotComputable)?;
				vm_state.total_surplus.saturating_accrue(vm_state.error_handler_weight);
				vm_state.error_handler = handler;
				vm_state.error_handler_weight = handler_weight;
				Ok(())
			},
			SetAppendix(mut appendix) => {
				let appendix_weight = Config::Weigher::weight(&mut appendix)
					.map_err(|()| XcmError::WeightNotComputable)?;
				vm_state.total_surplus.saturating_accrue(vm_state.appendix_weight);
				vm_state.appendix = appendix;
				vm_state.appendix_weight = appendix_weight;
				Ok(())
			},
			ClearError => {
				vm_state.clear_error();
				Ok(())
			},
			ClaimAsset { assets, ticket } => {
				let origin = vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				let ok = Config::AssetClaims::claim_assets(
					origin,
					&ticket,
					&assets,
					&vm_state.context(),
				);
				ensure!(ok, XcmError::UnknownClaim);
				for asset in assets.into_inner().into_iter() {
					vm_state.subsume_asset(asset)?;
				}
				Ok(())
			},
			Trap(code) => Err(XcmError::Trap(code)),
			SubscribeVersion { query_id, max_response_weight } => {
				let origin = vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				// We don't allow derivative origins to subscribe since it would otherwise pose a
				// DoS risk.
				ensure!(vm_state.original_origin() == origin, XcmError::BadOrigin);
				Config::SubscriptionService::start(
					origin,
					query_id,
					max_response_weight,
					&vm_state.context(),
				)
			},
			UnsubscribeVersion => {
				let origin = vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				ensure!(vm_state.original_origin() == origin, XcmError::BadOrigin);
				Config::SubscriptionService::stop(origin, &vm_state.context())
			},
			BurnAsset(assets) => {
				vm_state.holding.saturating_take(assets.into());
				Ok(())
			},
			ExpectAsset(assets) => vm_state
				.holding
				.ensure_contains(&assets)
				.map_err(|_| XcmError::ExpectationFalse),
			ExpectOrigin(origin) => {
				ensure!(vm_state.context().origin == origin, XcmError::ExpectationFalse);
				Ok(())
			},
			ExpectError(error) => {
				ensure!(vm_state.get_error() == error, XcmError::ExpectationFalse);
				Ok(())
			},
			ExpectTransactStatus(transact_status) => {
				ensure!(vm_state.transact_status == transact_status, XcmError::ExpectationFalse);
				Ok(())
			},
			QueryPallet { module_name, response_info } => {
				let pallets = Config::PalletInstancesInfo::infos()
					.into_iter()
					.filter(|x| x.module_name.as_bytes() == &module_name[..])
					.map(|x| {
						PalletInfo::new(
							x.index as u32,
							x.name.as_bytes().into(),
							x.module_name.as_bytes().into(),
							x.crate_version.major as u32,
							x.crate_version.minor as u32,
							x.crate_version.patch as u32,
						)
					})
					.collect::<Result<Vec<_>, XcmError>>()?;
				let QueryResponseInfo { destination, query_id, max_weight } = response_info;
				let response =
					Response::PalletsInfo(pallets.try_into().map_err(|_| XcmError::Overflow)?);
				let querier = Self::to_querier(vm_state.cloned_origin(), &destination)?;
				let instruction = QueryResponse { query_id, response, max_weight, querier };
				let message = Xcm(vec![instruction]);
				Self::send(vm_state, destination, message, FeeReason::QueryPallet)?;
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
				Self::respond(
					vm_state,
					Response::DispatchResult(vm_state.transact_status.clone()),
					response_info,
					FeeReason::Report,
				)?;
				Ok(())
			},
			ClearTransactStatus => {
				vm_state.transact_status = Default::default();
				Ok(())
			},
			UniversalOrigin(new_global) => {
				let universal_location = Config::UniversalLocation::get();
				ensure!(universal_location.first() != Some(&new_global), XcmError::InvalidLocation);
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				let origin_xform = (origin, new_global);
				let ok = Config::UniversalAliases::contains(&origin_xform);
				ensure!(ok, XcmError::InvalidLocation);
				let (_, new_global) = origin_xform;
				let new_origin = X1(new_global).relative_to(&universal_location);
				vm_state.set_origin(Some(new_origin));
				Ok(())
			},
			ExportMessage { network, destination, xcm } => {
				// The actual message sent to the bridge for forwarding is prepended with `UniversalOrigin`
				// and `DescendOrigin` in order to ensure that the message is executed with this Origin.
				//
				// Prepend the desired message with instructions which effectively rewrite the origin.
				//
				// This only works because the remote chain empowers the bridge
				// to speak for the local network.
				let origin = vm_state.context().origin.ok_or(XcmError::BadOrigin)?;
				let universal_source = Config::UniversalLocation::get()
					.within_global(origin)
					.map_err(|()| XcmError::Unanchored)?;
				let hash = (vm_state.origin_ref(), &destination).using_encoded(blake2_128);
				let channel = u32::decode(&mut hash.as_ref()).unwrap_or(0);
				// Hash identifies the lane on the exporter which we use. We use the pairwise
				// combination of the origin and destination to ensure origin/destination pairs will
				// generally have their own lanes.
				let (ticket, fee) = validate_export::<Config::MessageExporter>(
					network,
					channel,
					universal_source,
					destination,
					xcm,
				)?;
				Self::take_fee(vm_state, fee, FeeReason::Export(network))?;
				Config::MessageExporter::deliver(ticket)?;
				Ok(())
			},
			LockAsset { asset, unlocker } => {
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				let (remote_asset, context) = Self::try_reanchor(asset.clone(), &unlocker)?;
				let lock_ticket = Config::AssetLocker::prepare_lock(unlocker, asset, origin)?;
				let owner =
					origin.reanchored(&unlocker, context).map_err(|_| XcmError::ReanchorFailed)?;
				let msg = Xcm::<()>(vec![NoteUnlockable { asset: remote_asset, owner }]);
				let (ticket, price) = validate_send::<Config::XcmSender>(unlocker, msg)?;
				Self::take_fee(vm_state, price, FeeReason::LockAsset)?;
				lock_ticket.enact()?;
				Config::XcmSender::deliver(ticket)?;
				Ok(())
			},
			UnlockAsset { asset, target } => {
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				Config::AssetLocker::prepare_unlock(origin, asset, target)?.enact()?;
				Ok(())
			},
			NoteUnlockable { asset, owner } => {
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				Config::AssetLocker::note_unlockable(origin, asset, owner)?;
				Ok(())
			},
			RequestUnlock { asset, locker } => {
				let origin = *vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
				let remote_asset = Self::try_reanchor(asset.clone(), &locker)?.0;
				let reduce_ticket =
					Config::AssetLocker::prepare_reduce_unlockable(locker, asset, origin)?;
				let msg = Xcm::<()>(vec![UnlockAsset { asset: remote_asset, target: origin }]);
				let (ticket, price) = validate_send::<Config::XcmSender>(locker, msg)?;
				Self::take_fee(vm_state, price, FeeReason::RequestUnlock)?;
				reduce_ticket.enact()?;
				Config::XcmSender::deliver(ticket)?;
				Ok(())
			},
			ExchangeAsset { give, want, maximal } => {
				let give = vm_state.holding.saturating_take(give);
				let r = Config::AssetExchanger::exchange_asset(
					vm_state.origin_ref(),
					give,
					&want,
					maximal,
				);
				let completed = r.is_ok();
				let received = r.unwrap_or_else(|a| a);
				for asset in received.into_assets_iter() {
					vm_state.holding.subsume(asset);
				}
				if completed {
					Ok(())
				} else {
					Err(XcmError::NoDeal)
				}
			},
			SetFeesMode { jit_withdraw } => {
				vm_state.fees_mode = FeesMode { jit_withdraw };
				Ok(())
			},
			SetTopic(topic) => {
				vm_state.set_topic(Some(topic));
				Ok(())
			},
			ClearTopic => {
				vm_state.set_topic(None);
				Ok(())
			},
			AliasOrigin(_) => Err(XcmError::NoPermission),
			UnpaidExecution { check_origin, .. } => {
				ensure!(
					check_origin.is_none() || vm_state.context().origin == check_origin,
					XcmError::BadOrigin
				);
				Ok(())
			},
			HrmpNewChannelOpenRequest { .. } => Err(XcmError::Unimplemented),
			HrmpChannelAccepted { .. } => Err(XcmError::Unimplemented),
			HrmpChannelClosing { .. } => Err(XcmError::Unimplemented),
		}
	}

	fn post_process(&mut self, vm_state: &mut XcVmRegisters<Config::RuntimeCall>) {
		// We silently drop any error from our attempt to refund the surplus as it's a charitable
		// thing so best-effort is all we will do.
		let _ = self.refund_surplus(vm_state);
	}
}

impl<Config: config::Config> XcmProcessor<Config> {
	/// Refund any unused weight.
	fn refund_surplus(&mut self, vm_state: &mut StateOf<Config>) -> Result<(), XcmError> {
		let current_surplus = vm_state.total_surplus.saturating_sub(vm_state.total_refunded);
		if current_surplus.any_gt(Weight::zero()) {
			vm_state.total_refunded.saturating_accrue(current_surplus);
			if let Some(w) = self.trader.refund_weight(current_surplus) {
				vm_state.subsume_asset(w)?;
			}
		}
		Ok(())
	}
	/// Send an XCM, charging fees from Holding as needed.
	fn send(
		vm_state: &mut StateOf<Config>,
		dest: MultiLocation,
		msg: Xcm<()>,
		reason: FeeReason,
	) -> Result<XcmHash, XcmError> {
		let (ticket, fee) = validate_send::<Config::XcmSender>(dest, msg)?;
		if !Config::FeeManager::is_waived(vm_state.origin_ref(), reason) {
			let paid =
				vm_state.holding.try_take(fee.into()).map_err(|_| XcmError::NotHoldingFees)?;
			Config::FeeManager::handle_fee(paid.into());
		}
		Config::XcmSender::deliver(ticket).map_err(Into::into)
	}

	fn take_fee(vm_state: &mut StateOf<Config>, fee: MultiAssets, reason: FeeReason) -> XcmResult {
		if Config::FeeManager::is_waived(vm_state.origin_ref(), reason) {
			return Ok(())
		}
		let paid = if vm_state.fees_mode.jit_withdraw {
			let origin = vm_state.origin_ref().ok_or(XcmError::BadOrigin)?;
			for asset in fee.inner() {
				Config::AssetTransactor::withdraw_asset(&asset, origin, Some(&vm_state.context()))?;
			}
			fee
		} else {
			vm_state
				.holding
				.try_take(fee.into())
				.map_err(|_| XcmError::NotHoldingFees)?
				.into()
		};
		Config::FeeManager::handle_fee(paid);
		Ok(())
	}

	/// Calculates what `local_querier` would be from the perspective of `destination`.
	fn to_querier(
		local_querier: Option<MultiLocation>,
		destination: &MultiLocation,
	) -> Result<Option<MultiLocation>, XcmError> {
		Ok(match local_querier {
			None => None,
			Some(q) => Some(
				q.reanchored(&destination, Config::UniversalLocation::get())
					.map_err(|_| XcmError::ReanchorFailed)?,
			),
		})
	}

	/// Send a bare `QueryResponse` message containing `response` informed by the given `info`.
	///
	/// The `local_querier` argument is the querier (if any) specified from the *local* perspective.
	fn respond(
		vm_state: &mut StateOf<Config>,
		response: Response,
		info: QueryResponseInfo,
		fee_reason: FeeReason,
	) -> Result<XcmHash, XcmError> {
		let local_querier = vm_state.cloned_origin();
		let querier = Self::to_querier(local_querier, &info.destination)?;
		let QueryResponseInfo { destination, query_id, max_weight } = info;
		let instruction = QueryResponse { query_id, response, max_weight, querier };
		let message = Xcm(vec![instruction]);
		let (ticket, fee) = validate_send::<Config::XcmSender>(destination, message)?;
		if !Config::FeeManager::is_waived(vm_state.origin_ref(), fee_reason) {
			let paid =
				vm_state.holding.try_take(fee.into()).map_err(|_| XcmError::NotHoldingFees)?;
			Config::FeeManager::handle_fee(paid.into());
		}
		Config::XcmSender::deliver(ticket).map_err(Into::into)
	}

	fn try_reanchor(
		asset: MultiAsset,
		destination: &MultiLocation,
	) -> Result<(MultiAsset, InteriorMultiLocation), XcmError> {
		let reanchor_context = Config::UniversalLocation::get();
		let asset = asset
			.reanchored(&destination, reanchor_context)
			.map_err(|()| XcmError::ReanchorFailed)?;
		Ok((asset, reanchor_context))
	}

	/// NOTE: Any assets which were unable to be reanchored are introduced into `failed_bin`.
	fn reanchored(
		mut assets: Assets,
		dest: &MultiLocation,
		maybe_failed_bin: Option<&mut Assets>,
	) -> MultiAssets {
		let reanchor_context = Config::UniversalLocation::get();
		assets.reanchor(dest, reanchor_context, maybe_failed_bin);
		assets.into_assets_iter().collect::<Vec<_>>().into()
	}
}
