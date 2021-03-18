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

//! Parathread and parachains leasing system. Allows para IDs to be claimed, the code and data to be initialized and
//! parachain slots (i.e. continuous scheduling) to be leased. Also allows for parachains and parathreads to be
//! swapped.
//!
//! This doesn't handle the mechanics of determining which para ID actually ends up with a parachain lease. This
//! must handled by a separately, through the trait interface that this pallet provides or the root dispatchables.

use sp_std::prelude::*;
use sp_runtime::traits::{CheckedSub, Zero, CheckedConversion};
use frame_support::{
	decl_module, decl_storage, decl_event, decl_error, dispatch::DispatchResult,
	traits::{Currency, ReservableCurrency, Get}, weights::Weight,
};
use primitives::v1::Id as ParaId;
use frame_system::ensure_root;
use crate::traits::{Leaser, LeaseError, Registrar};

type BalanceOf<T> = <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type LeasePeriodOf<T> = <T as frame_system::Config>::BlockNumber;

pub trait WeightInfo {
	fn force_lease() -> Weight;
	fn manage_lease_period_start(c: u32, t: u32) -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn force_lease() -> Weight { 0 }
	fn manage_lease_period_start(_c: u32, _t:u32) -> Weight { 0 }
}

/// The module's configuration trait.
pub trait Config: frame_system::Config {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;

	/// The currency type used for bidding.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// The parachain registrar type.
	type Registrar: Registrar<AccountId=Self::AccountId>;

	/// The number of blocks over which a single period lasts.
	type LeasePeriod: Get<Self::BlockNumber>;

	/// Weight Information for the Extrinsics in the Pallet
	type WeightInfo: WeightInfo;
}

// This module's storage items.
decl_storage! {
	trait Store for Module<T: Config> as Slots {
		/// Amounts held on deposit for each (possibly future) leased parachain.
		///
		/// The actual amount locked on its behalf by any account at any time is the maximum of the second values
		/// of the items in this list whose first value is the account.
		///
		/// The first item in the list is the amount locked for the current Lease Period. Following
		/// items are for the subsequent lease periods.
		///
		/// The default value (an empty list) implies that the parachain no longer exists (or never
		/// existed) as far as this module is concerned.
		///
		/// If a parachain doesn't exist *yet* but is scheduled to exist in the future, then it
		/// will be left-padded with one or more `None`s to denote the fact that nothing is held on
		/// deposit for the non-existent chain currently, but is held at some point in the future.
		///
		/// It is illegal for a `None` value to trail in the list.
		pub Leases get(fn lease): map hasher(twox_64_concat) ParaId => Vec<Option<(T::AccountId, BalanceOf<T>)>>;
	}
}

decl_event!(
	pub enum Event<T> where
		AccountId = <T as frame_system::Config>::AccountId,
		LeasePeriod = LeasePeriodOf<T>,
		ParaId = ParaId,
		Balance = BalanceOf<T>,
	{
		/// A new [lease_period] is beginning.
		NewLeasePeriod(LeasePeriod),
		/// An existing parachain won the right to continue.
		/// First balance is the extra amount reseved. Second is the total amount reserved.
		/// \[parachain_id, leaser, period_begin, period_count, extra_reseved, total_amount\]
		Leased(ParaId, AccountId, LeasePeriod, LeasePeriod, Balance, Balance),
		/// A para ID value has been claimed.
		Claimed(ParaId),
	}
);

decl_error! {
	pub enum Error for Module<T: Config> {
		/// The lease period is in the past.
		LeasePeriodInPast,
		/// The origin for this call must be a parachain.
		NotParaOrigin,
		/// The parachain ID is not onboarding.
		ParaNotOnboarding,
		/// The origin for this call must be the origin who registered the parachain.
		InvalidOrigin,
		/// Parachain is already registered.
		AlreadyRegistered,
		/// The code must correspond to the hash.
		InvalidCode,
		/// Deployment data has not been set for this parachain.
		UnsetDeployData,
		/// The bid must overlap all intersecting ranges.
		NonIntersectingRange,
		/// Given code size is too large.
		CodeTooLarge,
		/// Given initial head data is too large.
		HeadDataTooLarge,
		/// The Id given is already in use.
		InUse,
		/// There was an error with the lease.
		LeaseError,
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: T::Origin {
		type Error = Error<T>;

		const LeasePeriod: T::BlockNumber = T::LeasePeriod::get();

		fn deposit_event() = default;

		fn on_initialize(n: T::BlockNumber) -> Weight {
			// If we're beginning a new lease period then handle that.
			let lease_period = T::LeasePeriod::get();
			if (n % lease_period).is_zero() {
				let lease_period_index = n / lease_period;
				Self::manage_lease_period_start(lease_period_index)
			} else {
				0
			}
		}

		/// Just a hotwire into the `lease_out` call, in case Root wants to force some lease to happen
		/// independently of any other on-chain mechanism to use it.
		#[weight = T::WeightInfo::force_lease()]
		fn force_lease(origin,
			para: ParaId,
			leaser: T::AccountId,
			amount: BalanceOf<T>,
			period_begin: LeasePeriodOf<T>,
			period_count: LeasePeriodOf<T>,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::lease_out(para, &leaser, amount, period_begin, period_count)
				.map_err(|_| Error::<T>::LeaseError)?;
			Ok(())
		}
	}
}

impl<T: Config> Module<T> {
	/// A new lease period is beginning. We're at the start of the first block of it.
	///
	/// We need to on-board and off-board parachains as needed. We should also handle reducing/
	/// returning deposits.
	fn manage_lease_period_start(lease_period_index: LeasePeriodOf<T>) -> Weight {
		Self::deposit_event(RawEvent::NewLeasePeriod(lease_period_index));

		let old_parachains = T::Registrar::parachains();

		// Figure out what chains need bringing on.
		let mut parachains = Vec::new();
		for (para, mut lease_periods) in Leases::<T>::iter() {
			if lease_periods.is_empty() { continue }
			// ^^ should never be empty since we would have deleted the entry otherwise.

			if lease_periods.len() == 1 {
				// Just one entry, which corresponds to the now-ended lease period.
				//
				// `para` is now just a parathread.
				//
				// Unreserve whatever is left.
				if let Some((who, value)) = &lease_periods[0] {
					T::Currency::unreserve(&who, *value);
				}

				// Remove the now-empty lease list.
				Leases::<T>::remove(para);
			} else {
				// The parachain entry has leased future periods.

				// We need to pop the first deposit entry, which corresponds to the now-
				// ended lease period.
				let maybe_ended_lease = lease_periods.remove(0);

				Leases::<T>::insert(para, &lease_periods);

				// If we *were* active in the last period and so have ended a lease...
				if let Some(ended_lease) = maybe_ended_lease {
					// Then we need to get the new amount that should continue to be held on
					// deposit for the parachain.
					let now_held = Self::deposit_held(para, &ended_lease.0);

					// If this is less than what we were holding for this leaser's now-ended lease, then
					// unreserve it.
					if let Some(rebate) = ended_lease.1.checked_sub(&now_held) {
						T::Currency::unreserve( &ended_lease.0, rebate);
					}
				}

				// If we have an active lease in the new period, then add to the current parachains
				if lease_periods[0].is_some() {
					parachains.push(para);
				}
			}
		}
		parachains.sort();

		for para in parachains.iter() {
			if old_parachains.binary_search(para).is_err() {
				// incoming.
				let res = T::Registrar::make_parachain(*para);
				debug_assert!(res.is_ok());
			}
		}

		for para in old_parachains.iter() {
			if parachains.binary_search(para).is_err() {
				// outgoing.
				let res = T::Registrar::make_parathread(*para);
				debug_assert!(res.is_ok());
			}
		}

		T::WeightInfo::manage_lease_period_start(
			old_parachains.len() as u32,
			parachains.len() as u32,
		)
	}
}

impl<T: Config> crate::traits::OnSwap for Module<T> {
	fn on_swap(one: ParaId, other: ParaId) {
		Leases::<T>::mutate(one, |x|
			Leases::<T>::mutate(other, |y|
				sp_std::mem::swap(x, y)
			)
		)
	}
}

impl<T: Config> Leaser for Module<T> {
	type AccountId = T::AccountId;
	type LeasePeriod = T::BlockNumber;
	type Currency = T::Currency;

	fn lease_out(
		para: ParaId,
		leaser: &Self::AccountId,
		amount: <Self::Currency as Currency<Self::AccountId>>::Balance,
		period_begin: Self::LeasePeriod,
		period_count: Self::LeasePeriod,
	) -> Result<(), LeaseError> {
		// Finally, we update the deposit held so it is `amount` for the new lease period
		// indices that were won in the auction.
		let offset = period_begin
			.checked_sub(&Self::lease_period_index())
			.and_then(|x| x.checked_into::<usize>())
			.ok_or(LeaseError::AlreadyEnded)?;

		// offset is the amount into the `Deposits` items list that our lease begins. `period_count`
		// is the number of items that it lasts for.

		// The lease period index range (begin, end) that newly belongs to this parachain
		// ID. We need to ensure that it features in `Deposits` to prevent it from being
		// reaped too early (any managed parachain whose `Deposits` set runs low will be
		// removed).
		Leases::<T>::try_mutate(para, |d| {
			// Left-pad with `None`s as necessary.
			if d.len() < offset {
				d.resize_with(offset, || { None });
			}
			let period_count_usize = period_count.checked_into::<usize>()
				.ok_or(LeaseError::AlreadyEnded)?;
			// Then place the deposit values for as long as the chain should exist.
			for i in offset .. (offset + period_count_usize) {
				if d.len() > i {
					// Already exists but it's `None`. That means a later slot was already leased.
					// No problem.
					if d[i] == None {
						d[i] = Some((leaser.clone(), amount));
					} else {
						// The chain tried to lease the same period twice. This might be a griefing
						// attempt.
						//
						// We bail, not giving any lease and leave it for governance to sort out.
						return Err(LeaseError::AlreadyLeased);
					}
				} else if d.len() == i {
					// Doesn't exist. This is usual.
					d.push(Some((leaser.clone(), amount)));
				} else {
					// earlier resize means it must be >= i; qed
					// defensive code though since we really don't want to panic here.
				}
			}

			// Figure out whether we already have some funds of `leaser` held in reserve for `para_id`.
			//  If so, then we can deduct those from the amount that we need to reserve.
			let maybe_additional = amount.checked_sub(&Self::deposit_held(para, &leaser));
			if let Some(ref additional) = maybe_additional {
				T::Currency::reserve(&leaser, *additional)
					.map_err(|_| LeaseError::ReserveFailed)?;
			}

			let reserved = maybe_additional.unwrap_or_default();
			Self::deposit_event(
				RawEvent::Leased(para, leaser.clone(), period_begin, period_count, reserved, amount)
			);

			Ok(())
		})
	}

	fn deposit_held(para: ParaId, leaser: &Self::AccountId) -> <Self::Currency as Currency<Self::AccountId>>::Balance {
		Leases::<T>::get(para)
			.into_iter()
			.map(|lease| {
				match lease {
					Some((who, amount)) => {
						if &who == leaser { amount } else { Zero::zero() }
					},
					None => Zero::zero(),
				}
			})
			.max()
			.unwrap_or_else(Zero::zero)
	}

	fn lease_period() -> Self::LeasePeriod {
		T::LeasePeriod::get()
	}

	fn lease_period_index() -> Self::LeasePeriod {
		<frame_system::Pallet<T>>::block_number() / T::LeasePeriod::get()
	}
}


/// tests for this module
#[cfg(test)]
mod tests {
	use super::*;

	use sp_core::H256;
	use sp_runtime::traits::{BlakeTwo256, IdentityLookup};
	use frame_support::{
		parameter_types, assert_ok,
		traits::{OnInitialize, OnFinalize}
	};
	use pallet_balances;
	use primitives::v1::{BlockNumber, Header};
	use crate::{slots, mock::TestRegistrar};

	type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
	type Block = frame_system::mocking::MockBlock<Test>;

	frame_support::construct_runtime!(
		pub enum Test where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
			Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
			Slots: slots::{Pallet, Call, Storage, Event<T>},
	 		RandomnessCollectiveFlip: pallet_randomness_collective_flip::{Pallet, Call, Storage},
		}
	);

	parameter_types! {
		pub const BlockHashCount: u32 = 250;
	}
	impl frame_system::Config for Test {
		type BaseCallFilter = ();
		type BlockWeights = ();
		type BlockLength = ();
		type Origin = Origin;
		type Call = Call;
		type Index = u64;
		type BlockNumber = BlockNumber;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type DbWeight = ();
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = pallet_balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
	}

	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
	}

	impl pallet_balances::Config for Test {
		type Balance = u64;
		type Event = Event;
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type WeightInfo = ();
		type MaxLocks = ();
	}

	parameter_types! {
		pub const LeasePeriod: BlockNumber = 10;
		pub const ParaDeposit: u64 = 1;
	}

	impl Config for Test {
		type Event = Event;
		type Currency = Balances;
		type Registrar = TestRegistrar<Test>;
		type LeasePeriod = LeasePeriod;
		type WeightInfo = crate::slots::TestWeightInfo;
	}

	// This function basically just builds a genesis storage key/value store according to
	// our desired mock up.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		pallet_balances::GenesisConfig::<Test> {
			balances: vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50), (6, 60)],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	fn run_to_block(n: BlockNumber) {
		while System::block_number() < n {
			Slots::on_finalize(System::block_number());
			Balances::on_finalize(System::block_number());
			System::on_finalize(System::block_number());
			System::set_block_number(System::block_number() + 1);
			System::on_initialize(System::block_number());
			Balances::on_initialize(System::block_number());
			Slots::on_initialize(System::block_number());
		}
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_eq!(Slots::lease_period(), 10);
			assert_eq!(Slots::lease_period_index(), 0);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);

			run_to_block(10);
			assert_eq!(Slots::lease_period_index(), 1);
		});
	}

	#[test]
	fn lease_lifecycle_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(1, ParaId::from(1), Default::default(), Default::default()));

			assert!(Slots::lease_out(1.into(), &1, 1, 1, 1).is_ok());
			assert_eq!(Slots::deposit_held(1.into(), &1), 1);
			assert_eq!(Balances::reserved_balance(1), 1);

			run_to_block(19);
			assert_eq!(Slots::deposit_held(1.into(), &1), 1);
			assert_eq!(Balances::reserved_balance(1), 1);

			run_to_block(20);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);
			assert_eq!(Balances::reserved_balance(1), 0);

			assert_eq!(TestRegistrar::<Test>::operations(), vec![
				(1.into(), 10, true),
				(1.into(), 20, false),
			]);
		});
	}

	#[test]
	fn lease_interrupted_lifecycle_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(1, ParaId::from(1), Default::default(), Default::default()));

			assert!(Slots::lease_out(1.into(), &1, 6, 1, 1).is_ok());
			assert!(Slots::lease_out(1.into(), &1, 4, 3, 1).is_ok());

			run_to_block(19);
			assert_eq!(Slots::deposit_held(1.into(), &1), 6);
			assert_eq!(Balances::reserved_balance(1), 6);

			run_to_block(20);
			assert_eq!(Slots::deposit_held(1.into(), &1), 4);
			assert_eq!(Balances::reserved_balance(1), 4);

			run_to_block(39);
			assert_eq!(Slots::deposit_held(1.into(), &1), 4);
			assert_eq!(Balances::reserved_balance(1), 4);

			run_to_block(40);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);
			assert_eq!(Balances::reserved_balance(1), 0);

			assert_eq!(TestRegistrar::<Test>::operations(), vec![
				(1.into(), 10, true),
				(1.into(), 20, false),
				(1.into(), 30, true),
				(1.into(), 40, false),
			]);
		});
	}

	#[test]
	fn lease_relayed_lifecycle_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(1, ParaId::from(1), Default::default(), Default::default()));

			assert!(Slots::lease_out(1.into(), &1, 6, 1, 1).is_ok());
			assert!(Slots::lease_out(1.into(), &2, 4, 2, 1).is_ok());
			assert_eq!(Slots::deposit_held(1.into(), &1), 6);
			assert_eq!(Balances::reserved_balance(1), 6);
			assert_eq!(Slots::deposit_held(1.into(), &2), 4);
			assert_eq!(Balances::reserved_balance(2), 4);

			run_to_block(19);
			assert_eq!(Slots::deposit_held(1.into(), &1), 6);
			assert_eq!(Balances::reserved_balance(1), 6);
			assert_eq!(Slots::deposit_held(1.into(), &2), 4);
			assert_eq!(Balances::reserved_balance(2), 4);

			run_to_block(20);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);
			assert_eq!(Balances::reserved_balance(1), 0);
			assert_eq!(Slots::deposit_held(1.into(), &2), 4);
			assert_eq!(Balances::reserved_balance(2), 4);

			run_to_block(29);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);
			assert_eq!(Balances::reserved_balance(1), 0);
			assert_eq!(Slots::deposit_held(1.into(), &2), 4);
			assert_eq!(Balances::reserved_balance(2), 4);

			run_to_block(30);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);
			assert_eq!(Balances::reserved_balance(1), 0);
			assert_eq!(Slots::deposit_held(1.into(), &2), 0);
			assert_eq!(Balances::reserved_balance(2), 0);

			assert_eq!(TestRegistrar::<Test>::operations(), vec![
				(1.into(), 10, true),
				(1.into(), 30, false),
			]);
		});
	}

	#[test]
	fn lease_deposit_increase_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(1, ParaId::from(1), Default::default(), Default::default()));

			assert!(Slots::lease_out(1.into(), &1, 4, 1, 1).is_ok());
			assert_eq!(Slots::deposit_held(1.into(), &1), 4);
			assert_eq!(Balances::reserved_balance(1), 4);

			assert!(Slots::lease_out(1.into(), &1, 6, 2, 1).is_ok());
			assert_eq!(Slots::deposit_held(1.into(), &1), 6);
			assert_eq!(Balances::reserved_balance(1), 6);

			run_to_block(29);
			assert_eq!(Slots::deposit_held(1.into(), &1), 6);
			assert_eq!(Balances::reserved_balance(1), 6);

			run_to_block(30);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);
			assert_eq!(Balances::reserved_balance(1), 0);

			assert_eq!(TestRegistrar::<Test>::operations(), vec![
				(1.into(), 10, true),
				(1.into(), 30, false),
			]);
		});
	}

	#[test]
	fn lease_deposit_decrease_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(1, ParaId::from(1), Default::default(), Default::default()));

			assert!(Slots::lease_out(1.into(), &1, 6, 1, 1).is_ok());
			assert_eq!(Slots::deposit_held(1.into(), &1), 6);
			assert_eq!(Balances::reserved_balance(1), 6);

			assert!(Slots::lease_out(1.into(), &1, 4, 2, 1).is_ok());
			assert_eq!(Slots::deposit_held(1.into(), &1), 6);
			assert_eq!(Balances::reserved_balance(1), 6);

			run_to_block(19);
			assert_eq!(Slots::deposit_held(1.into(), &1), 6);
			assert_eq!(Balances::reserved_balance(1), 6);

			run_to_block(20);
			assert_eq!(Slots::deposit_held(1.into(), &1), 4);
			assert_eq!(Balances::reserved_balance(1), 4);

			run_to_block(29);
			assert_eq!(Slots::deposit_held(1.into(), &1), 4);
			assert_eq!(Balances::reserved_balance(1), 4);

			run_to_block(30);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);
			assert_eq!(Balances::reserved_balance(1), 0);

			assert_eq!(TestRegistrar::<Test>::operations(), vec![
				(1.into(), 10, true),
				(1.into(), 30, false),
			]);
		});
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking {
	use super::*;
	use frame_system::RawOrigin;
	use frame_support::assert_ok;
	use sp_runtime::traits::Bounded;

	use frame_benchmarking::{benchmarks, account};

	use crate::slots::Module as Slots;

	fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
		let events = frame_system::Pallet::<T>::events();
		let system_event: <T as frame_system::Config>::Event = generic_event.into();
		// compare to the last event record
		let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
		assert_eq!(event, &system_event);
	}

	fn register_a_parathread<T: Config>(i: u32) -> (ParaId, T::AccountId) {
		let para = ParaId::from(i);
		let leaser: T::AccountId = account("leaser", i, 0);
		T::Currency::make_free_balance_be(&leaser, BalanceOf::<T>::max_value());
		let worst_head_data = T::Registrar::worst_head_data();
		let worst_validation_code = T::Registrar::worst_validation_code();

		assert_ok!(T::Registrar::register(leaser.clone(), para, worst_head_data, worst_validation_code));
		T::Registrar::execute_pending_transitions();

		(para, leaser)
	}

	benchmarks! {
		force_lease {
			let para = ParaId::from(1337);
			let leaser: T::AccountId = account("leaser", 0, 0);
			T::Currency::make_free_balance_be(&leaser, BalanceOf::<T>::max_value());
			let amount = T::Currency::minimum_balance();
			let period_begin = 0u32.into();
			let period_count = 3u32.into();
		}: _(RawOrigin::Root, para, leaser.clone(), amount, period_begin, period_count)
		verify {
			assert_last_event::<T>(RawEvent::Leased(para, leaser, period_begin, period_count, amount, amount).into());
		}

		// Worst case scenario, T parathreads onboard, and C parachains offboard.
		manage_lease_period_start {
			// Assume reasonable maximum of 100 paras at any time
			let c in 1 .. 100;
			let t in 1 .. 100;

			let period_begin = 0u32.into();
			let period_count = 3u32.into();

			// T parathread are upgrading to parachains
			for i in 0 .. t {
				let (para, leaser) = register_a_parathread::<T>(i);
				let amount = T::Currency::minimum_balance();

				Slots::<T>::force_lease(RawOrigin::Root.into(), para, leaser, amount, period_begin, period_count)?;
			}

			T::Registrar::execute_pending_transitions();

			// C parachains are downgrading to parathreads
			for i in 200 .. 200 + c {
				let (para, leaser) = register_a_parathread::<T>(i);
				T::Registrar::make_parachain(para)?;
			}

			T::Registrar::execute_pending_transitions();

			for i in 0 .. t {
				assert!(T::Registrar::is_parathread(ParaId::from(i)));
			}

			for i in 200 .. 200 + c {
				assert!(T::Registrar::is_parachain(ParaId::from(i)));
			}
		}: {
				Slots::<T>::manage_lease_period_start(period_begin);
		} verify {
			// All paras should have switched.
			T::Registrar::execute_pending_transitions();
			for i in 0 .. t {
				assert!(T::Registrar::is_parachain(ParaId::from(i)));
			}
			for i in 200 .. 200 + c {
				assert!(T::Registrar::is_parathread(ParaId::from(i)));
			}
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use crate::integration_tests::{new_test_ext, Test};
		use frame_support::assert_ok;

		#[test]
		fn force_lease_benchmark() {
			new_test_ext().execute_with(|| {
				assert_ok!(test_benchmark_force_lease::<Test>());
			});
		}

		#[test]
		fn manage_lease_period_start_benchmark() {
			new_test_ext().execute_with(|| {
				assert_ok!(test_benchmark_manage_lease_period_start::<Test>());
			});
		}
	}
}
