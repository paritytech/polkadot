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

//! # Simple polling module
//!
//! Note: This implementation assumes that all accounts are locked, and thus that no account balance
//! may ever reduce.

use frame_support::{
	decl_module, decl_storage, decl_event, decl_error, ensure, traits::{Currency, Get},
};
use system::{self as frame_system, ensure_signed};
use sp_runtime::traits::Saturating;

pub type BalanceOf<T> =
	<<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;

	/// The currency type used.
	type Currency: Currency<Self::AccountId>;

	/// The block number only before which voting is possible.
	type End: Get<Self::BlockNumber>;
}

/// The options someone has approved.
pub type Approvals = [bool; 4];

decl_storage! {
	trait Store for Module<T: Trait> as Poll {
		/// Votes, so far.
		pub VoteOf: map hasher(twox_64_concat) T::AccountId => (Approvals, BalanceOf<T>);

		/// The total balances voting for each option.
		pub Totals: [BalanceOf<T>; 4];
	}
}

decl_event! {
	pub enum Event<T> where
		<T as system::Trait>::AccountId,
		Balance = BalanceOf<T>,
	{
		Voted(AccountId, Balance, Approvals),
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Vote attempted after the end of voting.
		TooLate,
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		type Error = Error<T>;

		fn deposit_event() = default;

		/// The End config param.
		const End: T::BlockNumber = T::End::get();

		/// Cast a vote on the poll.
		#[weight = 100_000_000]
		fn vote(origin, approvals: Approvals) {
			let who = ensure_signed(origin)?;
			ensure!(system::Module::<T>::block_number() < T::End::get(), Error::<T>::TooLate);
			let balance = T::Currency::total_balance(&who);
			Totals::<T>::mutate(|ref mut totals| {
				VoteOf::<T>::mutate(&who, |(ref mut who_approvals, ref mut who_balance)| {
					for i in 0..approvals.len() {
						if who_approvals[i] {
							totals[i] = totals[i].saturating_sub(*who_balance);
						}
						if approvals[i] {
							totals[i] = totals[i].saturating_add(balance);
						}
					}
					*who_approvals = approvals;
					*who_balance = balance;
				});
			});
			Self::deposit_event(RawEvent::Voted(who, balance, approvals));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use frame_support::{assert_ok, assert_noop, impl_outer_origin, parameter_types, weights::Weight};
	use sp_core::H256;
	use sp_runtime::{Perbill, testing::Header, traits::{BlakeTwo256, IdentityLookup}};

	impl_outer_origin! {
		pub enum Origin for Test where system = frame_system {}
	}

	// For testing the pallet, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of pallets we want to use.
	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const MaximumBlockWeight: Weight = 1024;
		pub const MaximumBlockLength: u32 = 2 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::one();
	}
	impl frame_system::Trait for Test {
		type BaseCallFilter = ();
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Call = ();
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type DbWeight = ();
		type BlockExecutionWeight = ();
		type ExtrinsicBaseWeight = ();
		type MaximumExtrinsicWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
		type ModuleToIndex = ();
		type AccountData = balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
	}
	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
	}
	impl balances::Trait for Test {
		type Balance = u64;
		type Event = ();
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type WeightInfo = ();
	}
	parameter_types! {
		pub const End: u64 = 1;
	}
	impl Trait for Test {
		type Event = ();
		type Currency = Balances;
		type End = End;
	}
	type System = system::Module<Test>;
	type Balances = balances::Module<Test>;
	type Poll = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
		// We use default for brevity, but you can configure as desired if needed.
		balances::GenesisConfig::<Test> {
			balances: vec![
				(1, 10),
				(2, 20),
				(3, 30),
				(4, 40),
			],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(System::block_number(), 0);
		});
	}

	#[test]
	fn totaling_works() {
		new_test_ext().execute_with(|| {
			assert_ok!(Poll::vote(Origin::signed(1), [true, true, false, false]));
			assert_ok!(Poll::vote(Origin::signed(2), [false, true, true, false]));
			assert_ok!(Poll::vote(Origin::signed(3), [false, false, true, true]));
			assert_ok!(Poll::vote(Origin::signed(4), [true, false, false, true]));
			assert_eq!(Totals::<Test>::get(), [50, 30, 50, 70]);
		});
	}

	#[test]
	fn revoting_works() {
		new_test_ext().execute_with(|| {
			assert_ok!(Poll::vote(Origin::signed(1), [true, false, false, false]));
			assert_eq!(Totals::<Test>::get(), [10, 0, 0, 0]);
			assert_ok!(Poll::vote(Origin::signed(1), [false, true, false, false]));
			assert_eq!(Totals::<Test>::get(), [0, 10, 0, 0]);
			assert_ok!(Poll::vote(Origin::signed(1), [false, false, true, true]));
			assert_eq!(Totals::<Test>::get(), [0, 0, 10, 10]);
		});
	}

	#[test]
	fn vote_end_works() {
		new_test_ext().execute_with(|| {
			assert_ok!(Poll::vote(Origin::signed(1), [true, false, false, false]));
			assert_eq!(Totals::<Test>::get(), [10, 0, 0, 0]);
			system::Module::<Test>::set_block_number(1);
			assert_noop!(Poll::vote(Origin::signed(1), [false, true, false, false]), Error::<Test>::TooLate);
		});
	}
}
