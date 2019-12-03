// Copyright 2019 Parity Technologies (UK) Ltd.
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

//! Auxillary struct/enums for polkadot runtime.

use primitives::Balance;
use sp_runtime::traits::{Convert, Saturating};
use sp_runtime::{Fixed64, Perbill};
use frame_support::weights::Weight;
use frame_support::traits::{OnUnbalanced, Currency, Get};
use crate::{Balances, System, Authorship, MaximumBlockWeight, NegativeImbalance};

/// Logic for the author to get a portion of fees.
pub struct ToAuthor;

impl OnUnbalanced<NegativeImbalance> for ToAuthor {
	fn on_nonzero_unbalanced(amount: NegativeImbalance) {
		Balances::resolve_creating(&Authorship::author(), amount);
	}
}

/// Converter for currencies to votes.
pub struct CurrencyToVoteHandler;

impl CurrencyToVoteHandler {
	fn factor() -> u128 { (Balances::total_issuance() / u64::max_value() as u128).max(1) }
}

impl Convert<u128, u64> for CurrencyToVoteHandler {
	fn convert(x: u128) -> u64 { (x / Self::factor()) as u64 }
}

impl Convert<u128, u128> for CurrencyToVoteHandler {
	fn convert(x: u128) -> u128 { x * Self::factor() }
}

/// Handles converting a weight scalar to a fee value, based on the scale and granularity of the
/// node's balance type.
///
/// This should typically create a mapping between the following ranges:
///   - [0, system::MaximumBlockWeight]
///   - [Balance::min, Balance::max]
///
/// Yet, it can be used for any other sort of change to weight-fee. Some examples being:
///   - Setting it to `0` will essentially disable the weight fee.
///   - Setting it to `1` will cause the literal `#[weight = x]` values to be charged.
pub struct WeightToFee;
impl Convert<Weight, Balance> for WeightToFee {
	fn convert(x: Weight) -> Balance {
		// in Polkadot a weight of 10_000 (smallest non-zero weight) to be mapped to 10^7 units of
		// fees (1/10 CENT), hence:
		Balance::from(x).saturating_mul(1_000)
	}
}

/// Update the given multiplier based on the following formula
///
///   diff = (target_weight - previous_block_weight)
///   v = 0.00004
///   next_weight = weight * (1 + (v . diff) + (v . diff)^2 / 2)
///
/// Where `target_weight` must be given as the `Get` implementation of the `T` generic type.
/// https://research.web3.foundation/en/latest/polkadot/Token%20Economics/#relay-chain-transaction-fees
pub struct TargetedFeeAdjustment<T>(rstd::marker::PhantomData<T>);

impl<T: Get<Perbill>> Convert<Fixed64, Fixed64> for TargetedFeeAdjustment<T> {
	fn convert(multiplier: Fixed64) -> Fixed64 {
		let block_weight = System::all_extrinsics_weight();
		let max_weight = MaximumBlockWeight::get();
		let target_weight = (T::get() * max_weight) as u128;
		let block_weight = block_weight as u128;

		// determines if the first_term is positive
		let positive = block_weight >= target_weight;
		let diff_abs = block_weight.max(target_weight) - block_weight.min(target_weight);
		// diff is within u32, safe.
		let diff = Fixed64::from_rational(diff_abs as i64, max_weight as u64);
		let diff_squared = diff.saturating_mul(diff);

		// 0.00004 = 4/100_000 = 40_000/10^9
		let v = Fixed64::from_rational(4, 100_000);
		// 0.00004^2 = 16/10^10 ~= 2/10^9. Taking the future /2 into account, then it is just 1
		// parts from a billionth.
		let v_squared_2 = Fixed64::from_rational(1, 1_000_000_000);

		let first_term = v.saturating_mul(diff);
		// It is very unlikely that this will exist (in our poor perbill estimate) but we are giving
		// it a shot.
		let second_term = v_squared_2.saturating_mul(diff_squared);

		if positive {
			// Note: this is merely bounded by how big the multiplier and the inner value can go,
			// not by any economical reasoning.
			let excess = first_term.saturating_add(second_term);
			multiplier.saturating_add(excess)
		} else {
			// Proof: first_term > second_term. Safe subtraction.
			let negative = first_term - second_term;
			multiplier.saturating_sub(negative)
				// despite the fact that apply_to saturates weight (final fee cannot go below 0)
				// it is crucially important to stop here and don't further reduce the weight fee
				// multiplier. While at -1, it means that the network is so un-congested that all
				// transactions have no weight fee. We stop here and only increase if the network
				// became more busy.
				.max(Fixed64::from_rational(-1, 1))
		}
	}
}
