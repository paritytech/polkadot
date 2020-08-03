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

use sp_runtime::traits::Convert;
use frame_support::traits::{OnUnbalanced, Imbalance, Currency};
use crate::NegativeImbalance;

/// Logic for the author to get a portion of fees.
pub struct ToAuthor<R>(sp_std::marker::PhantomData<R>);

impl<R> OnUnbalanced<NegativeImbalance<R>> for ToAuthor<R>
where
	R: pallet_balances::Trait + pallet_authorship::Trait,
	<R as frame_system::Trait>::AccountId: From<primitives::v0::AccountId>,
	<R as frame_system::Trait>::AccountId: Into<primitives::v0::AccountId>,
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

/// Converter for currencies to votes.
pub struct CurrencyToVoteHandler<R>(sp_std::marker::PhantomData<R>);

impl<R> CurrencyToVoteHandler<R>
where
	R: pallet_balances::Trait,
	R::Balance: Into<u128>,
{
	fn factor() -> u128 {
		let issuance: u128 = <pallet_balances::Module<R>>::total_issuance().into();
		(issuance / u64::max_value() as u128).max(1)
	}
}

impl<R> Convert<u128, u64> for CurrencyToVoteHandler<R>
where
	R: pallet_balances::Trait,
	R::Balance: Into<u128>,
{
	fn convert(x: u128) -> u64 { (x / Self::factor()) as u64 }
}

impl<R> Convert<u128, u128> for CurrencyToVoteHandler<R>
where
	R: pallet_balances::Trait,
	R::Balance: Into<u128>,
{
	fn convert(x: u128) -> u128 { x * Self::factor() }
}
