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
use xcm::v0::{Xcm, Order, MultiAsset, MultiLocation, Error};
use sp_runtime::traits::{Zero, Saturating, SaturatedConversion};
use frame_support::traits::{Get, OnUnbalanced as OnUnbalancedT, tokens::currency::Currency as CurrencyT};
use frame_support::weights::{Weight, GetDispatchInfo, WeightToFeePolynomial};
use xcm_executor::{Assets, traits::{WeightBounds, WeightTrader}};

pub struct FixedWeightBounds<T, C>(PhantomData<(T, C)>);
impl<T: Get<Weight>, C: Decode + GetDispatchInfo> WeightBounds<C> for FixedWeightBounds<T, C> {
	fn shallow(message: &mut Xcm<C>) -> Result<Weight, ()> {
		Ok(match message {
			Xcm::Transact { call, .. } => {
				call.ensure_decoded()?.get_dispatch_info().weight.saturating_add(T::get())
			}
			Xcm::RelayedFrom { ref mut message, .. } => T::get().saturating_add(Self::shallow(message.as_mut())?),
			Xcm::WithdrawAsset { effects, .. }
			| Xcm::ReserveAssetDeposit { effects, .. }
			| Xcm::TeleportAsset { effects, .. }
			=> {
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
				T::get().saturating_add(inner)
			}
			_ => T::get(),
		})
	}
	fn deep(message: &mut Xcm<C>) -> Result<Weight, ()> {
		Ok(match message {
			Xcm::RelayedFrom { ref mut message, .. } => Self::deep(message.as_mut())?,
			Xcm::WithdrawAsset { effects, .. }
			| Xcm::ReserveAssetDeposit { effects, .. }
			| Xcm::TeleportAsset { effects, .. }
			=> {
				let mut extra = 0;
				for effect in effects.iter_mut() {
					match effect {
						Order::BuyExecution { xcm, .. } => {
							for message in xcm.iter_mut() {
								extra.saturating_accrue(Self::shallow(message)?.saturating_add(Self::deep(message)?));
							}
						},
						_ => {}
					}
				}
				extra
			},
			_ => 0,
		})
	}
}

/// Function trait for handling some revenue. Similar to a negative imbalance (credit) handler, but for a
/// `MultiAsset`. Sensible implementations will deposit the asset in some known treasury or block-author account.
pub trait TakeRevenue {
	/// Do something with the given `revenue`, which is a single non-wildcard `MultiAsset`.
	fn take_revenue(revenue: MultiAsset);
}

/// Null implementation just burns the revenue.
impl TakeRevenue for () {
	fn take_revenue(_revenue: MultiAsset) {}
}

/// Simple fee calculator that requires payment in a single concrete fungible at a fixed rate.
///
/// The constant `Get` type parameter should be the concrete fungible ID and the amount of it required for
/// one second of weight.
pub struct FixedRateOfConcreteFungible<
	T: Get<(MultiLocation, u128)>,
	R: TakeRevenue,
>(Weight, u128, PhantomData<(T, R)>);
impl<T: Get<(MultiLocation, u128)>, R: TakeRevenue> WeightTrader for FixedRateOfConcreteFungible<T, R> {
	fn new() -> Self { Self(0, 0, PhantomData) }

	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, Error> {
		let (id, units_per_second) = T::get();
		use frame_support::weights::constants::WEIGHT_PER_SECOND;
		let amount = units_per_second * (weight as u128) / (WEIGHT_PER_SECOND as u128);
		let required = MultiAsset::ConcreteFungible { amount, id };
		let (unused, _) = payment.less(required).map_err(|_| Error::TooExpensive)?;
		self.0 = self.0.saturating_add(weight);
		self.1 = self.1.saturating_add(amount);
		Ok(unused)
	}

	fn refund_weight(&mut self, weight: Weight) -> MultiAsset {
		let (id, units_per_second) = T::get();
		let weight = weight.min(self.0);
		let amount = units_per_second * (weight as u128) / 1_000_000_000_000u128;
		self.0 -= weight;
		self.1 = self.1.saturating_sub(amount);
		let result = MultiAsset::ConcreteFungible { amount, id };
		result
	}
}

impl<T: Get<(MultiLocation, u128)>, R: TakeRevenue> Drop for FixedRateOfConcreteFungible<T, R> {
	fn drop(&mut self) {
		let revenue = MultiAsset::ConcreteFungible { amount: self.1, id: T::get().0 };
		R::take_revenue(revenue);
	}
}

/// Weight trader which uses the TransactionPayment pallet to set the right price for weight and then
/// places any weight bought into the right account.
pub struct UsingComponents<
	WeightToFee: WeightToFeePolynomial<Balance=Currency::Balance>,
	AssetId: Get<MultiLocation>,
	AccountId,
	Currency: CurrencyT<AccountId>,
	OnUnbalanced: OnUnbalancedT<Currency::NegativeImbalance>,
>(Weight, Currency::Balance, PhantomData<(WeightToFee, AssetId, AccountId, Currency, OnUnbalanced)>);
impl<
	WeightToFee: WeightToFeePolynomial<Balance=Currency::Balance>,
	AssetId: Get<MultiLocation>,
	AccountId,
	Currency: CurrencyT<AccountId>,
	OnUnbalanced: OnUnbalancedT<Currency::NegativeImbalance>,
> WeightTrader for UsingComponents<WeightToFee, AssetId, AccountId, Currency, OnUnbalanced> {
	fn new() -> Self { Self(0, Zero::zero(), PhantomData) }

	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, Error> {
		let amount = WeightToFee::calc(&weight);
		let required = MultiAsset::ConcreteFungible {
			amount: amount.try_into().map_err(|_| Error::Overflow)?,
			id: AssetId::get(),
		};
		let (unused, _) = payment.less(required).map_err(|_| Error::TooExpensive)?;
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

}
impl<
	WeightToFee: WeightToFeePolynomial<Balance=Currency::Balance>,
	AssetId: Get<MultiLocation>,
	AccountId,
	Currency: CurrencyT<AccountId>,
	OnUnbalanced: OnUnbalancedT<Currency::NegativeImbalance>,
> Drop for UsingComponents<WeightToFee, AssetId, AccountId, Currency, OnUnbalanced> {
	fn drop(&mut self) {
		OnUnbalanced::on_unbalanced(Currency::issue(self.1));
	}
}
