// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

use frame_support::traits::{OnUnbalanced, Imbalance, Currency};
use crate::NegativeImbalance;

/// Logic for the author to get a portion of fees.
pub struct ToAuthor<R>(sp_std::marker::PhantomData<R>);
impl<R> OnUnbalanced<NegativeImbalance<R>> for ToAuthor<R>
where
	R: pallet_balances::Trait + pallet_authorship::Trait,
	<R as frame_system::Trait>::AccountId: From<primitives::v1::AccountId>,
	<R as frame_system::Trait>::AccountId: Into<primitives::v1::AccountId>,
	<R as frame_system::Trait>::Event: From<pallet_balances::RawEvent<
		<R as frame_system::Trait>::AccountId,
		<R as pallet_balances::Trait>::Balance,
		pallet_balances::DefaultInstance>
	>,
{
	fn on_nonzero_unbalanced(amount: NegativeImbalance<R>) {
		let numeric_amount = amount.peek();
		let author = <pallet_authorship::Module<R>>::author();
		<pallet_balances::Module<R>>::resolve_creating(&<pallet_authorship::Module<R>>::author(), amount);
		<frame_system::Module<R>>::deposit_event(pallet_balances::RawEvent::Deposit(author, numeric_amount));
	}
}

pub struct DealWithFees<R>(sp_std::marker::PhantomData<R>);
impl<R> OnUnbalanced<NegativeImbalance<R>> for DealWithFees<R>
where
	R: pallet_balances::Trait + pallet_treasury::Trait + pallet_authorship::Trait,
	<R as frame_system::Trait>::AccountId: From<primitives::v1::AccountId>,
	<R as frame_system::Trait>::AccountId: Into<primitives::v1::AccountId>,
	<R as frame_system::Trait>::Event: From<pallet_balances::RawEvent<
		<R as frame_system::Trait>::AccountId,
		<R as pallet_balances::Trait>::Balance,
		pallet_balances::DefaultInstance>
	>,
	NegativeImbalance<R>: From<pallet_balances::NegativeImbalance<R>>,
{
	fn on_unbalanceds<B>(mut fees_then_tips: impl Iterator<Item=NegativeImbalance<R>>) {
		if let Some(fees) = fees_then_tips.next() {
			// for fees, 80% to treasury, 20% to author
			let mut split = fees.ration(80, 20);
			if let Some(tips) = fees_then_tips.next() {
				// for tips, if any, 100% to author
				tips.merge_into(&mut split.1);
			}
			use pallet_treasury::Module as Treasury;
			//<Treasury<R> as OnUnbalanced<_>>::on_unbalanced(split.0);
			<ToAuthor<R> as OnUnbalanced<_>>::on_unbalanced(split.1);
		}
	}
}
