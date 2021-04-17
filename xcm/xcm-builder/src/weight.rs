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

use sp_std::{result::Result, marker::PhantomData, convert::TryInto};
use parity_scale_codec::Decode;
use xcm::v0::{Xcm, Order, MultiAsset, MultiLocation};
use sp_runtime::traits::{Zero, Saturating, SaturatedConversion};
use frame_support::traits::{Get, OnUnbalanced as OnUnbalancedT, tokens::currency::Currency as CurrencyT};
use frame_support::weights::{Weight, GetDispatchInfo, WeightToFeePolynomial};
use xcm_executor::{Assets, traits::{WeightBounds, WeightTrader}};

pub struct FixedWeightBounds<T, C>(PhantomData<(T, C)>);
impl<T: Get<Weight>, C: Decode + GetDispatchInfo> WeightBounds<C> for FixedWeightBounds<T, C> {
	fn shallow(message: &mut Xcm<C>) -> Result<Weight, ()> {
		let min = match message {
			Xcm::Transact { call, .. } => {
				call.ensure_decoded()?.get_dispatch_info().weight + T::get()
			}
			Xcm::WithdrawAsset { effects, .. }
			| Xcm::ReserveAssetDeposit { effects, .. }
			| Xcm::TeleportAsset { effects, .. } => {
				let inner: Weight = effects.iter_mut()
					.map(|effect| match effect {
						Order::BuyExecution { .. } => {
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
	fn deep(message: &mut Xcm<C>) -> Result<Weight, ()> {
		let mut extra = 0;
		match message {
			Xcm::Transact { .. } => {}
			Xcm::WithdrawAsset { effects, .. }
			| Xcm::ReserveAssetDeposit { effects, .. }
			| Xcm::TeleportAsset { effects, .. } => {
				for effect in effects.iter_mut() {
					match effect {
						Order::BuyExecution { xcm, .. } => {
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

/// Weight trader which uses the TransactionPayment pallet to set the right price for weight and then
/// places any weight bought into the right account.
pub struct UsingComponents<W, A, AI, C: CurrencyT<AI>, OU>(Weight, C::Balance, PhantomData<(W, A, AI, C, OU)>);
impl<
	WeightToFee: WeightToFeePolynomial<Balance=Currency::Balance>,
	AssetId: Get<MultiLocation>,
	AccountId,
	Currency: CurrencyT<AccountId>,
	OnUnbalanced: OnUnbalancedT<Currency::NegativeImbalance>,
> WeightTrader for UsingComponents<WeightToFee, AssetId, AccountId, Currency, OnUnbalanced> {
	fn new() -> Self { Self(0, Zero::zero(), PhantomData) }
	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, ()> {
		let amount = WeightToFee::calc(&weight);
		let required = MultiAsset::ConcreteFungible {
			amount: amount.try_into().map_err(|_| ())?,
			id: AssetId::get(),
		};
		let (unused, _) = payment.less(required).map_err(|_| ())?;
		self.0 = self.0.saturating_add(weight);
		self.1 = self.1.saturating_add(amount);
		Ok(unused)
	}
	fn refund_weight(&mut self, weight: Weight) -> MultiAsset {
		let weight = weight.min(self.0);
		let amount = WeightToFee::calc(&weight);
		self.0 -= weight;
		self.1 = self.1.saturating_sub(amount);
		let result = MultiAsset::ConcreteFungible {
			amount: amount.saturated_into(),
			id: AssetId::get(),
		};
		result
	}
	fn finalize(self) {
		OnUnbalanced::on_unbalanced(Currency::issue(self.1));
	}
}
