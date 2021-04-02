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
use parity_scale_codec::Decode;
use xcm::v0::{Xcm, SendXcm, MultiLocation, MultiAsset, Order, XcmGeneric, OrderGeneric};
use frame_support::{
	ensure, dispatch::{Dispatchable, Parameter, Weight}, weights::{PostDispatchInfo, GetDispatchInfo}
};
use frame_support::traits::{Get, Contains};
use crate::traits::{TransactAsset, ConvertOrigin, FilterAssetLocation, InvertLocation};
use crate::assets::Assets;

/// Determine the weight of an XCM message.
pub trait WeightOf<Call> {
	/// Return the weight of `message`.
	fn weight_of(message: &mut XcmGeneric<Call>) -> Result<Weight, ()>;
}

pub struct FixedWeightOf<T, C>(PhantomData<(T, C)>);
impl<T: Get<Weight>, C: Decode + GetDispatchInfo> WeightOf<C> for FixedWeightOf<T, C> {
	fn weight_of(message: &mut XcmGeneric<C>) -> Result<Weight, ()> {
		Ok(match message {
			XcmGeneric::Transact { call, .. } => {
				call.ensure_decoded()?.get_dispatch_info().weight + T::get()
			}
			XcmGeneric::WithdrawAsset { effects, .. }
			| XcmGeneric::ReserveAssetDeposit { effects, .. }
			| XcmGeneric::TeleportAsset { effects, .. } => {
				effects.iter()
					.map(|effect| match effect {
						_ => T::get(),
					}).sum()
			}
			_ => T::get(),
		})
	}
}

/// Charge the for a given XCM message.
pub trait BuyWeight {
	/// Purchase execution weight credit in return for up to a given `fee`. If less of the fee is required
	/// then the surplus is returned. If the `fee` cannot be used to pay for the `weight`, then an error is
	/// returned.
	fn buy_weight(weight: Weight, payment: Assets) -> Result<Assets, ()>;
}

/// Simple fee calculator that requires payment in a single concrete fungible at a fixed rate.
///
/// The constant `Get` type parameter should be the concrete fungible ID and the amount of it required for
/// one second of weight.
pub struct FixedRateOfConcreteFungible<T>(PhantomData<T>);
impl<T: Get<(MultiLocation, u128)>> BuyWeight for FixedRateOfConcreteFungible<T> {
	fn buy_weight(weight: Weight, payment: Assets) -> Result<Assets, ()> {
		let (required_id, units_per_second) = T::get();
		let required_amount = units_per_second * (weight as u128) / 1_000_000_000_000u128;
		let required = MultiAsset::ConcreteFungible { amount: required_amount, id: required_id };
		Ok(payment.checked_sub(required).map_err(|_| ())?.0)
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
	/// - `message_weight`: The weight of the message; the actual weight of the message is guaranteed to be
	///   no greater than this.
	/// - `weight_credit`: The pre-established amount of weight that the system has determined this message
	///   may utilise in its execution. Typically non-zero only because of prior fee payment, but could
	///   in principle be due to other factors.
	fn should_execute<Call>(
		origin: &MultiLocation,
		top_level: bool,
		message: &XcmGeneric<Call>,
		message_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl ShouldExecute for Tuple {
	fn should_execute<Call>(
		origin: &MultiLocation,
		top_level: bool,
		message: &XcmGeneric<Call>,
		message_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		for_tuples!( #(
			match Tuple::should_execute(origin, top_level, message, message_weight, weight_credit) {
				o @ Ok(()) => return o,
				_ => (),
			}
		)* );
		Err(())
	}
}

pub struct TakeWeightCredit;
impl ShouldExecute for TakeWeightCredit {
	fn should_execute<Call>(
		_origin: &MultiLocation,
		_top_level: bool,
		_message: &XcmGeneric<Call>,
		message_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		*weight_credit = weight_credit.checked_sub(message_weight).ok_or(())?;
		Ok(())
	}
}

pub struct AllowTopLevelPaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowTopLevelPaidExecutionFrom<T> {
	fn should_execute<Call>(
		origin: &MultiLocation,
		top_level: bool,
		message: &XcmGeneric<Call>,
		message_weight: Weight,
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
					Some(OrderGeneric::BuyExecution { debt, ..}) if *debt >= message_weight
				)
				=> Ok(()),
			_ => Err(()),
		}
	}
}

pub struct AllowUnpaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowUnpaidExecutionFrom<T> {
	fn should_execute<Call>(
		origin: &MultiLocation,
		_top_level: bool,
		_message: &XcmGeneric<Call>,
		_message_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		ensure!(T::contains(origin), ());
		Ok(())
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
	type Weigher: WeightOf<Self::Call>;

	/// The means of purchasing weight credit for XCM execution.
	type Sale: BuyWeight;
}
