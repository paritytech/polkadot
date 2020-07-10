// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

use frame_support::{
	decl_module, decl_storage, decl_event, decl_error, storage::child, ensure,
	traits::{
		Currency, Get, OnUnbalanced, WithdrawReason, ExistenceRequirement::AllowDeath
	},
};
use system::ensure_signed;
use sp_runtime::{ModuleId,
	traits::{AccountIdConversion, Hash, Saturating, Zero, CheckedAdd}
};
use crate::slots;
use codec::{Encode, Decode};
use sp_std::vec::Vec;
use primitives::v0::{Id as ParaId, HeadData};

pub type BalanceOf<T> =
<<T as slots::Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;
#[allow(dead_code)]
pub type NegativeImbalanceOf<T> =
<<T as slots::Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::NegativeImbalance;

pub trait Trait: slots::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;

	/// The currency type used.
	type Currency: Currency<Self::AccountId>;

	/// The block number only before which voting is possible.
	type End: Get<T::BlockNumber>;
}

/// The number of options.
pub const OPTIONS: usize = 4;

/// The options someone has approved.
pub type Approvals = [bool; OPTIONS];

decl_storage! {
	trait Store for Module<T: Trait> as Poll {
		/// Votes, so far.
		pub VoteOf map hasher(twox_64_concat) T::AccountId => (Approvals, BalanceOf<T>);

		/// The total balances voting for each option.
		pub Totals: [BalanceOf<T>, OPTIONS];
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
	pub struct Module<T: Trait> for enum Call where origin: T::Origin, system = system {
		type Error = Error<T>;

		const Options: u32 = OPTIONS as u32;

		fn deposit_event() = default;

		/// Create a new crowdfunding campaign for a parachain slot deposit for the current auction.
		#[weight = 100_000_000]
		fn vote(origin, approvals: Approvals) {
			let who = ensure_signed(origin)?;
			ensure!(system::Module::<T>::block_number() < T::End::get(), Error::<T>::TooLate);
			let balance = T::Currency::total_balance(&who);
			Totals::mutate(|ref mut totals| {
				VoteOf::<T>::mutate(&who, |(ref mut who_approvals, ref mut who_balance)| {
					for i in 0..OPTIONS {
						if who_approvals[i] {
							totals[i] -= who_balance;
						}
						*who_balance = balance;
						if approvals[i] {
							totals[i] -= who_balance;
						}
					}
				});
			})
			Self::deposit_event(RawEvent::Voted(who, balance, approvals));
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use sp_runtime::traits::BadOrigin;
	use frame_support::{
		assert_ok, assert_noop, impl_outer_origin, parameter_types, weights::Weight,
		ord_parameter_types,
	};
	use sp_core::H256;
	use frame_system::{EnsureSignedBy, EnsureOneOf, EnsureRoot};
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are required.
	use sp_runtime::{
		Perbill, testing::Header, traits::{BlakeTwo256, IdentityLookup},
	};

	impl_outer_origin! {
		pub enum Origin for Test  where system = frame_system {}
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
		type AccountData = pallet_balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
	}
	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
	}
	impl pallet_balances::Trait for Test {
		type Balance = u64;
		type Event = ();
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
	}
	parameter_types! {
		pub const End: u64 = 10;
	}
	impl Trait for Test {
		type Event = ();
		type Currency = Balances;
		type End = End;
	}
	type System = frame_system::Module<Test>;
	type Balances = pallet_balances::Module<Test>;
	type Poll = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		// We use default for brevity, but you can configure as desired if needed.
		pallet_balances::GenesisConfig::<Test> {
			balances: vec![
				(1, 10),
				(2, 10),
				(3, 10),
				(10, 100),
				(20, 100),
				(30, 100),
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
}
