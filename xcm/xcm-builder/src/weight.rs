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

use frame_support::{
	traits::{tokens::currency::Currency as CurrencyT, Get, OnUnbalanced as OnUnbalancedT},
	weights::{constants::WEIGHT_PER_SECOND, GetDispatchInfo, Weight, WeightToFeePolynomial},
};
use parity_scale_codec::Decode;
use sp_runtime::traits::{SaturatedConversion, Saturating, Zero};
use sp_std::{convert::TryInto, marker::PhantomData, result::Result};
use xcm::latest::prelude::*;
use xcm_executor::{
	traits::{WeightBounds, WeightTrader},
	Assets,
};

pub struct FixedWeightBounds<T, C>(PhantomData<(T, C)>);
impl<T: Get<Weight>, C: Decode + GetDispatchInfo> WeightBounds<C> for FixedWeightBounds<T, C> {
	fn weight(message: &mut Xcm<C>) -> Result<Weight, ()> {
		let mut r = 0;
		for m in message.0.iter_mut() {
			r += Self::instr_weight(m)?;
		}
		Ok(r)
	}
}

impl<T: Get<Weight>, C: Decode + GetDispatchInfo> FixedWeightBounds<T, C> {
	fn instr_weight(message: &mut Instruction<C>) -> Result<Weight, ()> {
		Ok(T::get().saturating_add(match message {
			Transact { require_weight_at_most, .. } => *require_weight_at_most,
			_ => 0,
		}))
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
#[deprecated = "Use `FixedRateOfFungible` instead"]
pub struct FixedRateOfConcreteFungible<T: Get<(MultiLocation, u128)>, R: TakeRevenue>(
	Weight,
	u128,
	PhantomData<(T, R)>,
);
#[allow(deprecated)]
impl<T: Get<(MultiLocation, u128)>, R: TakeRevenue> WeightTrader
	for FixedRateOfConcreteFungible<T, R>
{
	fn new() -> Self {
		Self(0, 0, PhantomData)
	}

	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, XcmError> {
		let (id, units_per_second) = T::get();
		let amount = units_per_second * (weight as u128) / (WEIGHT_PER_SECOND as u128);
		let unused =
			payment.checked_sub((id, amount).into()).map_err(|_| XcmError::TooExpensive)?;
		self.0 = self.0.saturating_add(weight);
		self.1 = self.1.saturating_add(amount);
		Ok(unused)
	}

	fn refund_weight(&mut self, weight: Weight) -> Option<MultiAsset> {
		let (id, units_per_second) = T::get();
		let weight = weight.min(self.0);
		let amount = units_per_second * (weight as u128) / 1_000_000_000_000u128;
		self.0 -= weight;
		self.1 = self.1.saturating_sub(amount);
		if amount > 0 {
			Some((Concrete(id), amount).into())
		} else {
			None
		}
	}
}
#[allow(deprecated)]
impl<T: Get<(MultiLocation, u128)>, R: TakeRevenue> Drop for FixedRateOfConcreteFungible<T, R> {
	fn drop(&mut self) {
		if self.1 > 0 {
			R::take_revenue((Concrete(T::get().0), self.1).into());
		}
	}
}

/// Simple fee calculator that requires payment in a single fungible at a fixed rate.
///
/// The constant `Get` type parameter should be the fungible ID and the amount of it required for
/// one second of weight.
pub struct FixedRateOfFungible<T: Get<(AssetId, u128)>, R: TakeRevenue>(
	Weight,
	u128,
	PhantomData<(T, R)>,
);
impl<T: Get<(AssetId, u128)>, R: TakeRevenue> WeightTrader for FixedRateOfFungible<T, R> {
	fn new() -> Self {
		Self(0, 0, PhantomData)
	}

	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, XcmError> {
		let (id, units_per_second) = T::get();
		let amount = units_per_second * (weight as u128) / (WEIGHT_PER_SECOND as u128);
		if amount == 0 {
			return Ok(payment)
		}
		let unused =
			payment.checked_sub((id, amount).into()).map_err(|_| XcmError::TooExpensive)?;
		self.0 = self.0.saturating_add(weight);
		self.1 = self.1.saturating_add(amount);
		Ok(unused)
	}

	fn refund_weight(&mut self, weight: Weight) -> Option<MultiAsset> {
		let (id, units_per_second) = T::get();
		let weight = weight.min(self.0);
		let amount = units_per_second * (weight as u128) / (WEIGHT_PER_SECOND as u128);
		self.0 -= weight;
		self.1 = self.1.saturating_sub(amount);
		if amount > 0 {
			Some((id, amount).into())
		} else {
			None
		}
	}
}

impl<T: Get<(AssetId, u128)>, R: TakeRevenue> Drop for FixedRateOfFungible<T, R> {
	fn drop(&mut self) {
		if self.1 > 0 {
			R::take_revenue((T::get().0, self.1).into());
		}
	}
}

/// Weight trader which uses the `TransactionPayment` pallet to set the right price for weight and then
/// places any weight bought into the right account.
pub struct UsingComponents<
	WeightToFee: WeightToFeePolynomial<Balance = Currency::Balance>,
	AssetId: Get<MultiLocation>,
	AccountId,
	Currency: CurrencyT<AccountId>,
	OnUnbalanced: OnUnbalancedT<Currency::NegativeImbalance>,
>(
	Weight,
	Currency::Balance,
	PhantomData<(WeightToFee, AssetId, AccountId, Currency, OnUnbalanced)>,
);
impl<
		WeightToFee: WeightToFeePolynomial<Balance = Currency::Balance>,
		AssetId: Get<MultiLocation>,
		AccountId,
		Currency: CurrencyT<AccountId>,
		OnUnbalanced: OnUnbalancedT<Currency::NegativeImbalance>,
	> WeightTrader for UsingComponents<WeightToFee, AssetId, AccountId, Currency, OnUnbalanced>
{
	fn new() -> Self {
		Self(0, Zero::zero(), PhantomData)
	}

	fn buy_weight(&mut self, weight: Weight, payment: Assets) -> Result<Assets, XcmError> {
		let amount = WeightToFee::calc(&weight);
		let u128_amount: u128 = amount.try_into().map_err(|_| XcmError::Overflow)?;
		let required = (Concrete(AssetId::get()), u128_amount).into();
		let unused = payment.checked_sub(required).map_err(|_| XcmError::TooExpensive)?;
		self.0 = self.0.saturating_add(weight);
		self.1 = self.1.saturating_add(amount);
		Ok(unused)
	}

	fn refund_weight(&mut self, weight: Weight) -> Option<MultiAsset> {
		let weight = weight.min(self.0);
		let amount = WeightToFee::calc(&weight);
		self.0 -= weight;
		self.1 = self.1.saturating_sub(amount);
		let amount: u128 = amount.saturated_into();
		if amount > 0 {
			Some((AssetId::get(), amount).into())
		} else {
			None
		}
	}
}
impl<
		WeightToFee: WeightToFeePolynomial<Balance = Currency::Balance>,
		AssetId: Get<MultiLocation>,
		AccountId,
		Currency: CurrencyT<AccountId>,
		OnUnbalanced: OnUnbalancedT<Currency::NegativeImbalance>,
	> Drop for UsingComponents<WeightToFee, AssetId, AccountId, Currency, OnUnbalanced>
{
	fn drop(&mut self) {
		OnUnbalanced::on_unbalanced(Currency::issue(self.1));
	}
}
