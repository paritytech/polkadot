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

use sp_std::marker::PhantomData;
use parity_scale_codec::{Encode, Decode};
use xcm::v0::{SendXcm, MultiLocation, MultiAsset, XcmGeneric, OrderGeneric, Response};
use frame_support::{
	ensure, dispatch::{Dispatchable, Parameter, Weight}, weights::{PostDispatchInfo, GetDispatchInfo}
};
use frame_support::traits::{Get, Contains};
use crate::traits::{TransactAsset, ConvertOrigin, FilterAssetLocation, InvertLocation};
use crate::assets::Assets;

/// Determine the weight of an XCM message.
pub trait WeightBounds<Call: Encode> {
	/// Return the minimum amount of weight that an attempted execution of this message would definitely
	/// consume.
	///
	/// This is useful to gauge how many fees should be paid up front to begin execution of the message.
	/// It is not useful for determining whether execution should begin lest it result in surpassing weight
	/// limits - in that case `deep` is the function to use.
	fn shallow(message: &mut XcmGeneric<Call>) -> Result<Weight, ()>;

	/// Return the deep amount of weight, over `shallow` that complete, successful and worst-case execution of
	/// `message` would incur.
	///
	/// This is perhaps overly pessimistic for determining how many fees should be paid for up-front since
	/// fee payment (or any other way of offsetting the execution costs such as an voucher-style NFT) may
	/// happen in stages throughout execution of the XCM.
	///
	/// A reminder: if it is possible that `message` may have alternative means of successful completion
	/// (perhaps a conditional path), then the *worst case* weight must be reported.
	///
	/// This is guaranteed equal to the eventual sum of all `shallow` XCM messages that get executed through
	/// any internal effects. Inner XCM messages may be executed by:
	/// - Order::BuyExecution
	fn deep(message: &mut XcmGeneric<Call>) -> Result<Weight, ()>;
}

pub struct FixedWeightBounds<T, C>(PhantomData<(T, C)>);
impl<T: Get<Weight>, C: Encode + Decode + GetDispatchInfo> WeightBounds<C> for FixedWeightBounds<T, C> {
	fn shallow(message: &mut XcmGeneric<C>) -> Result<Weight, ()> {
		let min = match message {
			XcmGeneric::Transact { call, .. } => {
				call.ensure_decoded()?.get_dispatch_info().weight + T::get()
			}
			XcmGeneric::WithdrawAsset { effects, .. }
			| XcmGeneric::ReserveAssetDeposit { effects, .. }
			| XcmGeneric::TeleportAsset { effects, .. } => {
				let inner: Weight = effects.iter_mut()
					.map(|effect| match effect {
						OrderGeneric::BuyExecution { .. } => {
							// On success, execution of this will result in more weight being consumed but
							// we don't count it here since this is only the *shallow*, non-negotiable weight
							// spend and doesn't count weight placed behind a `BuyExecution` since it will not
							// be definitely consumed from any existing weight credit if execution of the message
							// is attempted.
							T::get()
						},
						_ => T::get(),
					}).sum();
				T::get() + inner
			}
			_ => T::get(),
		};
		Ok(min)
	}
	fn deep(message: &mut XcmGeneric<C>) -> Result<Weight, ()> {
		let mut extra = 0;
		match message {
			XcmGeneric::Transact { .. } => {}
			XcmGeneric::WithdrawAsset { effects, .. }
			| XcmGeneric::ReserveAssetDeposit { effects, .. }
			| XcmGeneric::TeleportAsset { effects, .. } => {
				for effect in effects.iter_mut() {
					match effect {
						OrderGeneric::BuyExecution { xcm, .. } => {
							for message in xcm.iter_mut() {
								extra += Self::shallow(message)? + Self::deep(message)?;
							}
						},
						_ => {}
					}
				}
			}
			_ => {}
		};
		Ok(extra)
	}
}

/// Charge for weight in order to execute XCM.
pub trait WeightTrader {
	/// Create a new trader instance.
	fn new() -> Self;

	/// Purchase execution weight credit in return for up to a given `fee`. If less of the fee is required
	/// then the surplus is returned. If the `fee` cannot be used to pay for the `weight`, then an error is
	/// returned.
	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, ()>;

	/// Attempt a refund of `weight` into some asset. The caller does not guarantee that the weight was
	/// purchased using `buy_weight`.
	///
	/// Default implementation refunds nothing.
	fn refund_weight(&mut self, _weight: Weight) -> MultiAsset { MultiAsset::None }
}

/// Simple fee calculator that requires payment in a single concrete fungible at a fixed rate.
///
/// The constant `Get` type parameter should be the concrete fungible ID and the amount of it required for
/// one second of weight.
pub struct FixedRateOfConcreteFungible<T>(Weight, PhantomData<T>);
impl<T: Get<(MultiLocation, u128)>> WeightTrader for FixedRateOfConcreteFungible<T> {
	fn new() -> Self { Self(0, PhantomData) }
	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, ()> {
		let (id, units_per_second) = T::get();
		let amount = units_per_second * (weight as u128) / 1_000_000_000_000u128;
		let required = MultiAsset::ConcreteFungible { amount, id };
		let (used, _) = payment.less(required).map_err(|_| ())?;
		self.0 = self.0.saturating_add(weight);
		Ok(used)
	}
	fn refund_weight(&mut self, weight: Weight) -> MultiAsset {
		let weight = weight.min(self.0);
		self.0 -= weight;
		let (id, units_per_second) = T::get();
		let amount = units_per_second * (weight as u128) / 1_000_000_000_000u128;
		let result = MultiAsset::ConcreteFungible { amount, id };
		result
	}
}

/// Trait to determine whether the execution engine should actually execute a given XCM.
pub trait ShouldExecute {
	/// Returns `true` if the given `message` may be executed.
	///
	/// - `origin`: The origin (sender) of the message.
	/// - `top_level`: `true`` indicates the initial XCM coming from the `origin`, `false` indicates an embedded
	///   XCM executed internally as part of another message or an `Order`.
	/// - `message`: The message itself.
	/// - `shallow_weight`: The weight of the non-negotiable execution of the message. This does not include any
	///   embedded XCMs sat behind mechanisms like `BuyExecution` which would need to answer for their own weight.
	/// - `weight_credit`: The pre-established amount of weight that the system has determined this message
	///   may utilise in its execution. Typically non-zero only because of prior fee payment, but could
	///   in principle be due to other factors.
	fn should_execute<Call: Encode>(
		origin: &MultiLocation,
		top_level: bool,
		message: &XcmGeneric<Call>,
		shallow_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl ShouldExecute for Tuple {
	fn should_execute<Call: Encode>(
		origin: &MultiLocation,
		top_level: bool,
		message: &XcmGeneric<Call>,
		shallow_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		for_tuples!( #(
			match Tuple::should_execute(origin, top_level, message, shallow_weight, weight_credit) {
				o @ Ok(()) => return o,
				_ => (),
			}
		)* );
		Err(())
	}
}

pub struct TakeWeightCredit;
impl ShouldExecute for TakeWeightCredit {
	fn should_execute<Call: Encode>(
		_origin: &MultiLocation,
		_top_level: bool,
		_message: &XcmGeneric<Call>,
		shallow_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		*weight_credit = weight_credit.checked_sub(shallow_weight).ok_or(())?;
		Ok(())
	}
}

pub struct AllowTopLevelPaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowTopLevelPaidExecutionFrom<T> {
	fn should_execute<Call: Encode>(
		origin: &MultiLocation,
		top_level: bool,
		message: &XcmGeneric<Call>,
		shallow_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		ensure!(T::contains(origin), ());
		ensure!(top_level, ());
		match message {
			XcmGeneric::TeleportAsset { effects, .. }
			| XcmGeneric::WithdrawAsset { effects, ..}
			| XcmGeneric::ReserveAssetDeposit { effects, ..}
				if matches!(
					effects.first(),
					Some(OrderGeneric::BuyExecution { debt, ..}) if *debt >= shallow_weight
				)
				=> Ok(()),
			_ => Err(()),
		}
	}
}

pub struct AllowUnpaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowUnpaidExecutionFrom<T> {
	fn should_execute<Call: Encode>(
		origin: &MultiLocation,
		_top_level: bool,
		_message: &XcmGeneric<Call>,
		_shallow_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		ensure!(T::contains(origin), ());
		Ok(())
	}
}

pub trait OnResponse {
	fn expecting_response(origin: &MultiLocation, query_id: u64) -> bool;
	fn on_response(origin: MultiLocation, query_id: u64, response: Response) -> Weight;
}
impl OnResponse for () {
	fn expecting_response(_origin: &MultiLocation, _query_id: u64) -> bool { false }
	fn on_response(_origin: MultiLocation, _query_id: u64, _response: Response) -> Weight { 0 }
}

pub struct AllowKnownQueryResponses<ResponseHandler>(PhantomData<ResponseHandler>);
impl<ResponseHandler: OnResponse> ShouldExecute for AllowKnownQueryResponses<ResponseHandler> {
	fn should_execute<Call: Encode>(
		origin: &MultiLocation,
		_top_level: bool,
		message: &XcmGeneric<Call>,
		_shallow_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		match message {
			XcmGeneric::QueryResponse { query_id, .. } if ResponseHandler::expecting_response(origin, *query_id)
			=> Ok(()),
			_ => Err(()),
		}
	}
}

/// The trait to parametrize the `XcmExecutor`.
pub trait Config {
	/// The outer call dispatch type.
	type Call: Parameter + Dispatchable<PostInfo=PostDispatchInfo> + GetDispatchInfo;

	/// How to send an onward XCM message.
	type XcmSender: SendXcm;

	/// How to withdraw and deposit an asset.
	type AssetTransactor: TransactAsset;

	/// How to get a call origin from a `OriginKind` value.
	type OriginConverter: ConvertOrigin<<Self::Call as Dispatchable>::Origin>;

	/// Combinations of (Location, Asset) pairs which we unilateral trust as reserves.
	type IsReserve: FilterAssetLocation;

	/// Combinations of (Location, Asset) pairs which we bilateral trust as teleporters.
	type IsTeleporter: FilterAssetLocation;

	/// Means of inverting a location.
	type LocationInverter: InvertLocation;

	/// Whether we should execute the given XCM at all.
	type Barrier: ShouldExecute;

	/// The means of determining an XCM message's weight.
	type Weigher: WeightBounds<Self::Call>;

	/// The means of purchasing weight credit for XCM execution.
	type Trader: WeightTrader;

	/// What to do when a response of a query is found.
	type ResponseHandler: OnResponse;
}
