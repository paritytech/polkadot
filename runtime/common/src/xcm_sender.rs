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

//! XCM sender for relay chain.

use frame_support::traits::Get;
use frame_system::pallet_prelude::BlockNumberFor;
use parity_scale_codec::Encode;
use primitives::Id as ParaId;
use runtime_parachains::{
	configuration::{self, HostConfiguration},
	dmp, FeeTracker,
};
use sp_runtime::FixedPointNumber;
use sp_std::{marker::PhantomData, prelude::*};
use xcm::prelude::*;
use SendError::*;

/// Simple value-bearing trait for determining/expressing the assets required to be paid for a
/// messages to be delivered to a parachain.
pub trait PriceForParachainDelivery {
	/// Return the assets required to deliver `message` to the given `para` destination.
	fn price_for_parachain_delivery(para: ParaId, message: &Xcm<()>) -> MultiAssets;
}
impl PriceForParachainDelivery for () {
	fn price_for_parachain_delivery(_: ParaId, _: &Xcm<()>) -> MultiAssets {
		MultiAssets::new()
	}
}

/// Implementation of `PriceForParachainDelivery` which returns a fixed price.
pub struct ConstantPrice<T>(sp_std::marker::PhantomData<T>);
impl<T: Get<MultiAssets>> PriceForParachainDelivery for ConstantPrice<T> {
	fn price_for_parachain_delivery(_: ParaId, _: &Xcm<()>) -> MultiAssets {
		T::get()
	}
}

/// Implementation of `PriceForParachainDelivery` which returns an exponentially increasing price.
/// The `A` type parameter is used to denote the asset ID that will be used for paying the delivery
/// fee.
///
/// The formula for the fee is based on the sum of a base fee plus a message length fee, multiplied
/// by a specified factor. In mathematical form, it is `F * (B + encoded_msg_len * M)`.
pub struct ExponentialPrice<A, B, M, F>(sp_std::marker::PhantomData<(A, B, M, F)>);
impl<A: Get<AssetId>, B: Get<u128>, M: Get<u128>, F: FeeTracker> PriceForParachainDelivery
	for ExponentialPrice<A, B, M, F>
{
	fn price_for_parachain_delivery(para: ParaId, msg: &Xcm<()>) -> MultiAssets {
		let msg_fee = (msg.encoded_size() as u128).saturating_mul(M::get());
		let fee_sum = B::get().saturating_add(msg_fee);
		let amount = F::get_fee_factor(para).saturating_mul_int(fee_sum);
		(A::get(), amount).into()
	}
}

/// XCM sender for relay chain. It only sends downward message.
pub struct ChildParachainRouter<T, W, P>(PhantomData<(T, W, P)>);

impl<T: configuration::Config + dmp::Config, W: xcm::WrapVersion, P: PriceForParachainDelivery>
	SendXcm for ChildParachainRouter<T, W, P>
{
	type Ticket = (HostConfiguration<BlockNumberFor<T>>, ParaId, Vec<u8>);

	fn validate(
		dest: &mut Option<MultiLocation>,
		msg: &mut Option<Xcm<()>>,
	) -> SendResult<(HostConfiguration<BlockNumberFor<T>>, ParaId, Vec<u8>)> {
		let d = dest.take().ok_or(MissingArgument)?;
		let id = if let MultiLocation { parents: 0, interior: X1(Parachain(id)) } = &d {
			*id
		} else {
			*dest = Some(d);
			return Err(NotApplicable)
		};

		// Downward message passing.
		let xcm = msg.take().ok_or(MissingArgument)?;
		let config = <configuration::Pallet<T>>::config();
		let para = id.into();
		let price = P::price_for_parachain_delivery(para, &xcm);
		let blob = W::wrap_version(&d, xcm).map_err(|()| DestinationUnsupported)?.encode();
		<dmp::Pallet<T>>::can_queue_downward_message(&config, &para, &blob)
			.map_err(Into::<SendError>::into)?;

		Ok(((config, para, blob), price))
	}

	fn deliver(
		(config, para, blob): (HostConfiguration<BlockNumberFor<T>>, ParaId, Vec<u8>),
	) -> Result<XcmHash, SendError> {
		let hash = sp_io::hashing::blake2_256(&blob[..]);
		<dmp::Pallet<T>>::queue_downward_message(&config, para, blob)
			.map(|()| hash)
			.map_err(|_| SendError::Transport(&"Error placing into DMP queue"))
	}
}

/// Implementation of `pallet_xcm_benchmarks::EnsureDelivery` which helps to ensure delivery to the
/// `ParaId` parachain (sibling or child). Deposits existential deposit for origin (if needed).
/// Deposits estimated fee to the origin account (if needed).
/// Allows to trigger additional logic for specific `ParaId` (e.g. open HRMP channel) (if neeeded).
#[cfg(feature = "runtime-benchmarks")]
pub struct ToParachainDeliveryHelper<
	XcmConfig,
	ExistentialDeposit,
	PriceForDelivery,
	ParaId,
	ToParaIdHelper,
>(
	sp_std::marker::PhantomData<(
		XcmConfig,
		ExistentialDeposit,
		PriceForDelivery,
		ParaId,
		ToParaIdHelper,
	)>,
);

#[cfg(feature = "runtime-benchmarks")]
impl<
		XcmConfig: xcm_executor::Config,
		ExistentialDeposit: Get<Option<MultiAsset>>,
		PriceForDelivery: PriceForParachainDelivery,
		Parachain: Get<ParaId>,
		ToParachainHelper: EnsureForParachain,
	> pallet_xcm_benchmarks::EnsureDelivery
	for ToParachainDeliveryHelper<
		XcmConfig,
		ExistentialDeposit,
		PriceForDelivery,
		Parachain,
		ToParachainHelper,
	>
{
	fn ensure_successful_delivery(
		origin_ref: &MultiLocation,
		_dest: &MultiLocation,
		fee_reason: xcm_executor::traits::FeeReason,
	) -> (Option<xcm_executor::FeesMode>, Option<MultiAssets>) {
		use xcm_executor::{
			traits::{FeeManager, TransactAsset},
			FeesMode,
		};

		let mut fees_mode = None;
		if !XcmConfig::FeeManager::is_waived(Some(origin_ref), fee_reason) {
			// if not waived, we need to set up accounts for paying and receiving fees

			// mint ED to origin if needed
			if let Some(ed) = ExistentialDeposit::get() {
				XcmConfig::AssetTransactor::deposit_asset(&ed, &origin_ref, None).unwrap();
			}

			// overestimate delivery fee
			let overestimated_xcm = vec![ClearOrigin; 128].into();
			let overestimated_fees = PriceForDelivery::price_for_parachain_delivery(
				Parachain::get(),
				&overestimated_xcm,
			);

			// mint overestimated fee to origin
			for fee in overestimated_fees.inner() {
				XcmConfig::AssetTransactor::deposit_asset(&fee, &origin_ref, None).unwrap();
			}

			// allow more initialization for target parachain
			ToParachainHelper::ensure(Parachain::get());

			// expected worst case - direct withdraw
			fees_mode = Some(FeesMode { jit_withdraw: true });
		}
		(fees_mode, None)
	}
}

/// Ensure more initialization for `ParaId`. (e.g. open HRMP channels, ...)
#[cfg(feature = "runtime-benchmarks")]
pub trait EnsureForParachain {
	fn ensure(para_id: ParaId);
}
#[cfg(feature = "runtime-benchmarks")]
impl EnsureForParachain for () {
	fn ensure(_para_id: ParaId) {
		// doing nothing
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use frame_support::parameter_types;
	use runtime_parachains::FeeTracker;
	use sp_runtime::FixedU128;

	parameter_types! {
		pub const BaseDeliveryFee: u128 = 300_000_000;
		pub const TransactionByteFee: u128 = 1_000_000;
		pub FeeAssetId: AssetId = Concrete(Here.into());
	}

	struct TestFeeTracker;
	impl FeeTracker for TestFeeTracker {
		fn get_fee_factor(_: ParaId) -> FixedU128 {
			FixedU128::from_rational(101, 100)
		}
	}

	type TestExponentialPrice =
		ExponentialPrice<FeeAssetId, BaseDeliveryFee, TransactionByteFee, TestFeeTracker>;

	#[test]
	fn exponential_price_correct_price_calculation() {
		let id: ParaId = 123.into();
		let b: u128 = BaseDeliveryFee::get();
		let m: u128 = TransactionByteFee::get();

		// F * (B + msg_length * M)
		// message_length = 1
		let result: u128 = TestFeeTracker::get_fee_factor(id).saturating_mul_int(b + m);
		assert_eq!(
			TestExponentialPrice::price_for_parachain_delivery(id, &Xcm(vec![])),
			(FeeAssetId::get(), result).into()
		);

		// message size = 2
		let result: u128 = TestFeeTracker::get_fee_factor(id).saturating_mul_int(b + (2 * m));
		assert_eq!(
			TestExponentialPrice::price_for_parachain_delivery(id, &Xcm(vec![ClearOrigin])),
			(FeeAssetId::get(), result).into()
		);

		// message size = 4
		let result: u128 = TestFeeTracker::get_fee_factor(id).saturating_mul_int(b + (4 * m));
		assert_eq!(
			TestExponentialPrice::price_for_parachain_delivery(
				id,
				&Xcm(vec![SetAppendix(Xcm(vec![ClearOrigin]))])
			),
			(FeeAssetId::get(), result).into()
		);
	}
}
