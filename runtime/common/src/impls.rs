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
	R: pallet_balances::Config + pallet_authorship::Config,
	<R as frame_system::Config>::AccountId: From<primitives::v1::AccountId>,
	<R as frame_system::Config>::AccountId: Into<primitives::v1::AccountId>,
	<R as frame_system::Config>::Event: From<pallet_balances::RawEvent<
		<R as frame_system::Config>::AccountId,
		<R as pallet_balances::Config>::Balance,
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
	R: pallet_balances::Config + pallet_treasury::Config + pallet_authorship::Config,
	pallet_treasury::Module<R>: OnUnbalanced<NegativeImbalance<R>>,
	<R as frame_system::Config>::AccountId: From<primitives::v1::AccountId>,
	<R as frame_system::Config>::AccountId: Into<primitives::v1::AccountId>,
	<R as frame_system::Config>::Event: From<pallet_balances::RawEvent<
		<R as frame_system::Config>::AccountId,
		<R as pallet_balances::Config>::Balance,
		pallet_balances::DefaultInstance>
	>,
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
			<Treasury<R> as OnUnbalanced<_>>::on_unbalanced(split.0);
			<ToAuthor<R> as OnUnbalanced<_>>::on_unbalanced(split.1);
		}
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use frame_support::{impl_outer_origin, parameter_types, weights::Weight};
	use frame_support::traits::FindAuthor;
	use sp_core::H256;
	use sp_runtime::{
		testing::Header, ModuleId,
		traits::{BlakeTwo256, IdentityLookup},
		Perbill,
	};
	use primitives::v1::AccountId;

	#[derive(Clone, PartialEq, Eq, Debug)]
	pub struct Test;

	impl_outer_origin!{
		pub enum Origin for Test {}
	}

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const ExtrinsicBaseWeight: u64 = 100;
		pub const MaximumBlockWeight: Weight = 1024;
		pub const MaximumBlockLength: u32 = 2 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::one();
	}

	impl frame_system::Config for Test {
		type BaseCallFilter = ();
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Call = ();
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = AccountId;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type DbWeight = ();
		type BlockExecutionWeight = ();
		type ExtrinsicBaseWeight = ExtrinsicBaseWeight;
		type MaximumExtrinsicWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
		type PalletInfo = ();
		type AccountData = pallet_balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
	}

	impl pallet_balances::Config for Test {
		type Balance = u64;
		type Event = ();
		type DustRemoval = ();
		type ExistentialDeposit = ();
		type AccountStore = System;
		type MaxLocks = ();
		type WeightInfo = ();
	}

	pub struct Nobody;
	impl frame_support::traits::Contains<AccountId> for Nobody {
		fn contains(_: &AccountId) -> bool { false }
		fn sorted_members() -> Vec<AccountId> { vec![] }
		#[cfg(feature = "runtime-benchmarks")]
		fn add(_: &AccountId) { unimplemented!() }
	}
	impl frame_support::traits::ContainsLengthBound for Nobody {
		fn min_len() -> usize { 0 }
		fn max_len() -> usize { 0 }
	}

	parameter_types! {
		pub const TreasuryModuleId: ModuleId = ModuleId(*b"py/trsry");
	}

	impl pallet_treasury::Config for Test {
		type Currency = pallet_balances::Module<Test>;
		type ApproveOrigin = frame_system::EnsureRoot<AccountId>;
		type RejectOrigin = frame_system::EnsureRoot<AccountId>;
		type Event = ();
		type OnSlash = ();
		type ProposalBond = ();
		type ProposalBondMinimum = ();
		type SpendPeriod = ();
		type Burn = ();
		type BurnDestination = ();
		type Tippers = Nobody;
		type TipCountdown = ();
		type TipFindersFee = ();
		type TipReportDepositBase = ();
		type DataDepositPerByte = ();
		type BountyDepositBase = ();
		type BountyDepositPayoutDelay = ();
		type BountyUpdatePeriod = ();
		type MaximumReasonLength = ();
		type BountyCuratorDeposit = ();
		type BountyValueMinimum = ();
		type ModuleId = TreasuryModuleId;
		type WeightInfo = ();
	}

	pub struct OneAuthor;
	impl FindAuthor<AccountId> for OneAuthor {
		fn find_author<'a, I>(_: I) -> Option<AccountId>
			where I: 'a,
		{
			Some(Default::default())
		}
	}
	impl pallet_authorship::Config for Test {
		type FindAuthor = OneAuthor;
		type UncleGenerations = ();
		type FilterUncle = ();
		type EventHandler = ();
	}

	type Treasury = pallet_treasury::Module<Test>;
	type Balances = pallet_balances::Module<Test>;
	type System = frame_system::Module<Test>;

	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		// We use default for brevity, but you can configure as desired if needed.
		pallet_balances::GenesisConfig::<Test>::default().assimilate_storage(&mut t).unwrap();
		t.into()
	}

	#[test]
	fn test_fees_and_tip_split() {
		new_test_ext().execute_with(|| {
			let fee = Balances::issue(10);
			let tip = Balances::issue(20);

			assert_eq!(Balances::free_balance(Treasury::account_id()), 0);
			assert_eq!(Balances::free_balance(AccountId::default()), 0);

			DealWithFees::on_unbalanceds(vec![fee, tip].into_iter());

			// Author gets 100% of tip and 20% of fee = 22
			assert_eq!(Balances::free_balance(AccountId::default()), 22);
			// Treasury gets 80% of fee
			assert_eq!(Balances::free_balance(Treasury::account_id()), 8);
		});
	}
}
