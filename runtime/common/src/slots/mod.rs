// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Parathread and parachains leasing system. Allows para IDs to be claimed, the code and data to be
//! initialized and parachain slots (i.e. continuous scheduling) to be leased. Also allows for
//! parachains and parathreads to be swapped.
//!
//! This doesn't handle the mechanics of determining which para ID actually ends up with a parachain
//! lease. This must handled by a separately, through the trait interface that this pallet provides
//! or the root dispatchables.

pub mod migration;

use crate::traits::{LeaseError, Leaser, Registrar};
use frame_support::{
	pallet_prelude::*,
	traits::{Currency, ReservableCurrency},
	weights::Weight,
};
use frame_system::pallet_prelude::*;
pub use pallet::*;
use primitives::Id as ParaId;
use sp_runtime::traits::{CheckedConversion, CheckedSub, Saturating, Zero};
use sp_std::prelude::*;

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
type LeasePeriodOf<T> = BlockNumberFor<T>;

pub trait WeightInfo {
	fn force_lease() -> Weight;
	fn manage_lease_period_start(c: u32, t: u32) -> Weight;
	fn clear_all_leases() -> Weight;
	fn trigger_onboard() -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn force_lease() -> Weight {
		Weight::zero()
	}
	fn manage_lease_period_start(_c: u32, _t: u32) -> Weight {
		Weight::zero()
	}
	fn clear_all_leases() -> Weight {
		Weight::zero()
	}
	fn trigger_onboard() -> Weight {
		Weight::zero()
	}
}

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// The currency type used for bidding.
		type Currency: ReservableCurrency<Self::AccountId>;

		/// The parachain registrar type.
		type Registrar: Registrar<AccountId = Self::AccountId>;

		/// The number of blocks over which a single period lasts.
		#[pallet::constant]
		type LeasePeriod: Get<BlockNumberFor<Self>>;

		/// The number of blocks to offset each lease period by.
		#[pallet::constant]
		type LeaseOffset: Get<BlockNumberFor<Self>>;

		/// The origin which may forcibly create or clear leases. Root can always do this.
		type ForceOrigin: EnsureOrigin<<Self as frame_system::Config>::RuntimeOrigin>;

		/// Weight Information for the Extrinsics in the Pallet
		type WeightInfo: WeightInfo;
	}

	/// Amounts held on deposit for each (possibly future) leased parachain.
	///
	/// The actual amount locked on its behalf by any account at any time is the maximum of the
	/// second values of the items in this list whose first value is the account.
	///
	/// The first item in the list is the amount locked for the current Lease Period. Following
	/// items are for the subsequent lease periods.
	///
	/// The default value (an empty list) implies that the parachain no longer exists (or never
	/// existed) as far as this pallet is concerned.
	///
	/// If a parachain doesn't exist *yet* but is scheduled to exist in the future, then it
	/// will be left-padded with one or more `None`s to denote the fact that nothing is held on
	/// deposit for the non-existent chain currently, but is held at some point in the future.
	///
	/// It is illegal for a `None` value to trail in the list.
	#[pallet::storage]
	#[pallet::getter(fn lease)]
	pub type Leases<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, Vec<Option<(T::AccountId, BalanceOf<T>)>>, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A new `[lease_period]` is beginning.
		NewLeasePeriod { lease_period: LeasePeriodOf<T> },
		/// A para has won the right to a continuous set of lease periods as a parachain.
		/// First balance is any extra amount reserved on top of the para's existing deposit.
		/// Second balance is the total amount reserved.
		Leased {
			para_id: ParaId,
			leaser: T::AccountId,
			period_begin: LeasePeriodOf<T>,
			period_count: LeasePeriodOf<T>,
			extra_reserved: BalanceOf<T>,
			total_amount: BalanceOf<T>,
		},
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The parachain ID is not onboarding.
		ParaNotOnboarding,
		/// There was an error with the lease.
		LeaseError,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(n: BlockNumberFor<T>) -> Weight {
			if let Some((lease_period, first_block)) = Self::lease_period_index(n) {
				// If we're beginning a new lease period then handle that.
				if first_block {
					return Self::manage_lease_period_start(lease_period)
				}
			}

			// We didn't return early above, so we didn't do anything.
			Weight::zero()
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Just a connect into the `lease_out` call, in case Root wants to force some lease to
		/// happen independently of any other on-chain mechanism to use it.
		///
		/// The dispatch origin for this call must match `T::ForceOrigin`.
		#[pallet::call_index(0)]
		#[pallet::weight(T::WeightInfo::force_lease())]
		pub fn force_lease(
			origin: OriginFor<T>,
			para: ParaId,
			leaser: T::AccountId,
			amount: BalanceOf<T>,
			period_begin: LeasePeriodOf<T>,
			period_count: LeasePeriodOf<T>,
		) -> DispatchResult {
			T::ForceOrigin::ensure_origin(origin)?;
			Self::lease_out(para, &leaser, amount, period_begin, period_count)
				.map_err(|_| Error::<T>::LeaseError)?;
			Ok(())
		}

		/// Clear all leases for a Para Id, refunding any deposits back to the original owners.
		///
		/// The dispatch origin for this call must match `T::ForceOrigin`.
		#[pallet::call_index(1)]
		#[pallet::weight(T::WeightInfo::clear_all_leases())]
		pub fn clear_all_leases(origin: OriginFor<T>, para: ParaId) -> DispatchResult {
			T::ForceOrigin::ensure_origin(origin)?;
			let deposits = Self::all_deposits_held(para);

			// Refund any deposits for these leases
			for (who, deposit) in deposits {
				let err_amount = T::Currency::unreserve(&who, deposit);
				debug_assert!(err_amount.is_zero());
			}

			Leases::<T>::remove(para);
			Ok(())
		}

		/// Try to onboard a parachain that has a lease for the current lease period.
		///
		/// This function can be useful if there was some state issue with a para that should
		/// have onboarded, but was unable to. As long as they have a lease period, we can
		/// let them onboard from here.
		///
		/// Origin must be signed, but can be called by anyone.
		#[pallet::call_index(2)]
		#[pallet::weight(T::WeightInfo::trigger_onboard())]
		pub fn trigger_onboard(origin: OriginFor<T>, para: ParaId) -> DispatchResult {
			let _ = ensure_signed(origin)?;
			let leases = Leases::<T>::get(para);
			match leases.first() {
				// If the first element in leases is present, then it has a lease!
				// We can try to onboard it.
				Some(Some(_lease_info)) => T::Registrar::make_parachain(para)?,
				// Otherwise, it does not have a lease.
				Some(None) | None => return Err(Error::<T>::ParaNotOnboarding.into()),
			};
			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	/// A new lease period is beginning. We're at the start of the first block of it.
	///
	/// We need to on-board and off-board parachains as needed. We should also handle reducing/
	/// returning deposits.
	fn manage_lease_period_start(lease_period_index: LeasePeriodOf<T>) -> Weight {
		Self::deposit_event(Event::<T>::NewLeasePeriod { lease_period: lease_period_index });

		let old_parachains = T::Registrar::parachains();

		// Figure out what chains need bringing on.
		let mut parachains = Vec::new();
		for (para, mut lease_periods) in Leases::<T>::iter() {
			if lease_periods.is_empty() {
				continue
			}
			// ^^ should never be empty since we would have deleted the entry otherwise.

			if lease_periods.len() == 1 {
				// Just one entry, which corresponds to the now-ended lease period.
				//
				// `para` is now just an on-demand parachain.
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

					// If this is less than what we were holding for this leaser's now-ended lease,
					// then unreserve it.
					if let Some(rebate) = ended_lease.1.checked_sub(&now_held) {
						T::Currency::unreserve(&ended_lease.0, rebate);
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

	// Return a vector of (user, balance) for all deposits for a parachain.
	// Useful when trying to clean up a parachain leases, as this would tell
	// you all the balances you need to unreserve.
	fn all_deposits_held(para: ParaId) -> Vec<(T::AccountId, BalanceOf<T>)> {
		let mut tracker = sp_std::collections::btree_map::BTreeMap::new();
		Leases::<T>::get(para).into_iter().for_each(|lease| match lease {
			Some((who, amount)) => match tracker.get(&who) {
				Some(prev_amount) =>
					if amount > *prev_amount {
						tracker.insert(who, amount);
					},
				None => {
					tracker.insert(who, amount);
				},
			},
			None => {},
		});

		tracker.into_iter().collect()
	}
}

impl<T: Config> crate::traits::OnSwap for Pallet<T> {
	fn on_swap(one: ParaId, other: ParaId) {
		Leases::<T>::mutate(one, |x| Leases::<T>::mutate(other, |y| sp_std::mem::swap(x, y)))
	}
}

impl<T: Config> Leaser<BlockNumberFor<T>> for Pallet<T> {
	type AccountId = T::AccountId;
	type LeasePeriod = BlockNumberFor<T>;
	type Currency = T::Currency;

	fn lease_out(
		para: ParaId,
		leaser: &Self::AccountId,
		amount: <Self::Currency as Currency<Self::AccountId>>::Balance,
		period_begin: Self::LeasePeriod,
		period_count: Self::LeasePeriod,
	) -> Result<(), LeaseError> {
		let now = frame_system::Pallet::<T>::block_number();
		let (current_lease_period, _) =
			Self::lease_period_index(now).ok_or(LeaseError::NoLeasePeriod)?;
		// Finally, we update the deposit held so it is `amount` for the new lease period
		// indices that were won in the auction.
		let offset = period_begin
			.checked_sub(&current_lease_period)
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
				d.resize_with(offset, || None);
			}
			let period_count_usize =
				period_count.checked_into::<usize>().ok_or(LeaseError::AlreadyEnded)?;
			// Then place the deposit values for as long as the chain should exist.
			for i in offset..(offset + period_count_usize) {
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
						return Err(LeaseError::AlreadyLeased)
					}
				} else if d.len() == i {
					// Doesn't exist. This is usual.
					d.push(Some((leaser.clone(), amount)));
				} else {
					// earlier resize means it must be >= i; qed
					// defensive code though since we really don't want to panic here.
				}
			}

			// Figure out whether we already have some funds of `leaser` held in reserve for
			// `para_id`.  If so, then we can deduct those from the amount that we need to reserve.
			let maybe_additional = amount.checked_sub(&Self::deposit_held(para, &leaser));
			if let Some(ref additional) = maybe_additional {
				T::Currency::reserve(&leaser, *additional)
					.map_err(|_| LeaseError::ReserveFailed)?;
			}

			let reserved = maybe_additional.unwrap_or_default();

			// Check if current lease period is same as period begin, and onboard them directly.
			// This will allow us to support onboarding new parachains in the middle of a lease
			// period.
			if current_lease_period == period_begin {
				// Best effort. Not much we can do if this fails.
				let _ = T::Registrar::make_parachain(para);
			}

			Self::deposit_event(Event::<T>::Leased {
				para_id: para,
				leaser: leaser.clone(),
				period_begin,
				period_count,
				extra_reserved: reserved,
				total_amount: amount,
			});

			Ok(())
		})
	}

	fn deposit_held(
		para: ParaId,
		leaser: &Self::AccountId,
	) -> <Self::Currency as Currency<Self::AccountId>>::Balance {
		Leases::<T>::get(para)
			.into_iter()
			.map(|lease| match lease {
				Some((who, amount)) =>
					if &who == leaser {
						amount
					} else {
						Zero::zero()
					},
				None => Zero::zero(),
			})
			.max()
			.unwrap_or_else(Zero::zero)
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn lease_period_length() -> (BlockNumberFor<T>, BlockNumberFor<T>) {
		(T::LeasePeriod::get(), T::LeaseOffset::get())
	}

	fn lease_period_index(b: BlockNumberFor<T>) -> Option<(Self::LeasePeriod, bool)> {
		// Note that blocks before `LeaseOffset` do not count as any lease period.
		let offset_block_now = b.checked_sub(&T::LeaseOffset::get())?;
		let lease_period = offset_block_now / T::LeasePeriod::get();
		let first_block = (offset_block_now % T::LeasePeriod::get()).is_zero();

		Some((lease_period, first_block))
	}

	fn already_leased(
		para_id: ParaId,
		first_period: Self::LeasePeriod,
		last_period: Self::LeasePeriod,
	) -> bool {
		let now = frame_system::Pallet::<T>::block_number();
		let (current_lease_period, _) = match Self::lease_period_index(now) {
			Some(clp) => clp,
			None => return true,
		};

		// Can't look in the past, so we pick whichever is the biggest.
		let start_period = first_period.max(current_lease_period);
		// Find the offset to look into the lease period list.
		// Subtraction is safe because of max above.
		let offset = match (start_period - current_lease_period).checked_into::<usize>() {
			Some(offset) => offset,
			None => return true,
		};

		// This calculates how deep we should look in the vec for a potential lease.
		let period_count = match last_period.saturating_sub(start_period).checked_into::<usize>() {
			Some(period_count) => period_count,
			None => return true,
		};

		// Get the leases, and check each item in the vec which is part of the range we are
		// checking.
		let leases = Leases::<T>::get(para_id);
		for slot in offset..=offset + period_count {
			if let Some(Some(_)) = leases.get(slot) {
				// If there exists any lease period, we exit early and return true.
				return true
			}
		}

		// If we got here, then we did not find any overlapping leases.
		false
	}
}

/// tests for this pallet
#[cfg(test)]
mod tests {
	use super::*;

	use crate::{mock::TestRegistrar, slots};
	use ::test_helpers::{dummy_head_data, dummy_validation_code};
	use frame_support::{assert_noop, assert_ok, parameter_types};
	use frame_system::EnsureRoot;
	use pallet_balances;
	use primitives::BlockNumber;
	use sp_core::H256;
	use sp_runtime::{
		traits::{BlakeTwo256, IdentityLookup},
		BuildStorage,
	};

	type Block = frame_system::mocking::MockBlockU32<Test>;

	frame_support::construct_runtime!(
		pub enum Test
		{
			System: frame_system::{Pallet, Call, Config<T>, Storage, Event<T>},
			Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
			Slots: slots::{Pallet, Call, Storage, Event<T>},
		}
	);

	parameter_types! {
		pub const BlockHashCount: u32 = 250;
	}
	impl frame_system::Config for Test {
		type BaseCallFilter = frame_support::traits::Everything;
		type BlockWeights = ();
		type BlockLength = ();
		type RuntimeOrigin = RuntimeOrigin;
		type RuntimeCall = RuntimeCall;
		type Nonce = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Block = Block;
		type RuntimeEvent = RuntimeEvent;
		type BlockHashCount = BlockHashCount;
		type DbWeight = ();
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = pallet_balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
		type OnSetCode = ();
		type MaxConsumers = frame_support::traits::ConstU32<16>;
	}

	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
	}

	impl pallet_balances::Config for Test {
		type Balance = u64;
		type RuntimeEvent = RuntimeEvent;
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type WeightInfo = ();
		type MaxLocks = ();
		type MaxReserves = ();
		type ReserveIdentifier = [u8; 8];
		type RuntimeHoldReason = RuntimeHoldReason;
		type FreezeIdentifier = ();
		type MaxHolds = ConstU32<1>;
		type MaxFreezes = ConstU32<1>;
	}

	parameter_types! {
		pub const LeasePeriod: BlockNumber = 10;
		pub static LeaseOffset: BlockNumber = 0;
		pub const ParaDeposit: u64 = 1;
	}

	impl Config for Test {
		type RuntimeEvent = RuntimeEvent;
		type Currency = Balances;
		type Registrar = TestRegistrar<Test>;
		type LeasePeriod = LeasePeriod;
		type LeaseOffset = LeaseOffset;
		type ForceOrigin = EnsureRoot<Self::AccountId>;
		type WeightInfo = crate::slots::TestWeightInfo;
	}

	// This function basically just builds a genesis storage key/value store according to
	// our desired mock up.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
		pallet_balances::GenesisConfig::<Test> {
			balances: vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50), (6, 60)],
		}
		.assimilate_storage(&mut t)
		.unwrap();
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
			assert_eq!(Slots::lease_period_length(), (10, 0));
			let now = System::block_number();
			assert_eq!(Slots::lease_period_index(now).unwrap().0, 0);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);

			run_to_block(10);
			let now = System::block_number();
			assert_eq!(Slots::lease_period_index(now).unwrap().0, 1);
		});
	}

	#[test]
	fn lease_lifecycle_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code()
			));

			assert_ok!(Slots::lease_out(1.into(), &1, 1, 1, 1));
			assert_eq!(Slots::deposit_held(1.into(), &1), 1);
			assert_eq!(Balances::reserved_balance(1), 1);

			run_to_block(19);
			assert_eq!(Slots::deposit_held(1.into(), &1), 1);
			assert_eq!(Balances::reserved_balance(1), 1);

			run_to_block(20);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);
			assert_eq!(Balances::reserved_balance(1), 0);

			assert_eq!(
				TestRegistrar::<Test>::operations(),
				vec![(1.into(), 10, true), (1.into(), 20, false),]
			);
		});
	}

	#[test]
	fn lease_interrupted_lifecycle_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code()
			));

			assert_ok!(Slots::lease_out(1.into(), &1, 6, 1, 1));
			assert_ok!(Slots::lease_out(1.into(), &1, 4, 3, 1));

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

			assert_eq!(
				TestRegistrar::<Test>::operations(),
				vec![
					(1.into(), 10, true),
					(1.into(), 20, false),
					(1.into(), 30, true),
					(1.into(), 40, false),
				]
			);
		});
	}

	#[test]
	fn lease_relayed_lifecycle_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code()
			));

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

			assert_eq!(
				TestRegistrar::<Test>::operations(),
				vec![(1.into(), 10, true), (1.into(), 30, false),]
			);
		});
	}

	#[test]
	fn lease_deposit_increase_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code()
			));

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

			assert_eq!(
				TestRegistrar::<Test>::operations(),
				vec![(1.into(), 10, true), (1.into(), 30, false),]
			);
		});
	}

	#[test]
	fn lease_deposit_decrease_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code()
			));

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

			assert_eq!(
				TestRegistrar::<Test>::operations(),
				vec![(1.into(), 10, true), (1.into(), 30, false),]
			);
		});
	}

	#[test]
	fn clear_all_leases_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code()
			));

			let max_num = 5u32;

			// max_num different people are reserved for leases to Para ID 1
			for i in 1u32..=max_num {
				let j: u64 = i.into();
				assert_ok!(Slots::lease_out(1.into(), &j, j * 10 - 1, i * i, i));
				assert_eq!(Slots::deposit_held(1.into(), &j), j * 10 - 1);
				assert_eq!(Balances::reserved_balance(j), j * 10 - 1);
			}

			assert_ok!(Slots::clear_all_leases(RuntimeOrigin::root(), 1.into()));

			// Balances cleaned up correctly
			for i in 1u32..=max_num {
				let j: u64 = i.into();
				assert_eq!(Slots::deposit_held(1.into(), &j), 0);
				assert_eq!(Balances::reserved_balance(j), 0);
			}

			// Leases is empty.
			assert!(Leases::<Test>::get(ParaId::from(1_u32)).is_empty());
		});
	}

	#[test]
	fn lease_out_current_lease_period() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code()
			));
			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(2_u32),
				dummy_head_data(),
				dummy_validation_code()
			));

			run_to_block(20);
			let now = System::block_number();
			assert_eq!(Slots::lease_period_index(now).unwrap().0, 2);
			// Can't lease from the past
			assert!(Slots::lease_out(1.into(), &1, 1, 1, 1).is_err());
			// Lease in the current period triggers onboarding
			assert_ok!(Slots::lease_out(1.into(), &1, 1, 2, 1));
			// Lease in the future doesn't
			assert_ok!(Slots::lease_out(2.into(), &1, 1, 3, 1));

			assert_eq!(TestRegistrar::<Test>::operations(), vec![(1.into(), 20, true),]);
		});
	}

	#[test]
	fn trigger_onboard_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code()
			));
			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(2_u32),
				dummy_head_data(),
				dummy_validation_code()
			));
			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(3_u32),
				dummy_head_data(),
				dummy_validation_code()
			));

			// We will directly manipulate leases to emulate some kind of failure in the system.
			// Para 1 will have no leases
			// Para 2 will have a lease period in the current index
			Leases::<Test>::insert(ParaId::from(2_u32), vec![Some((0, 0))]);
			// Para 3 will have a lease period in a future index
			Leases::<Test>::insert(ParaId::from(3_u32), vec![None, None, Some((0, 0))]);

			// Para 1 should fail cause they don't have any leases
			assert_noop!(
				Slots::trigger_onboard(RuntimeOrigin::signed(1), 1.into()),
				Error::<Test>::ParaNotOnboarding
			);

			// Para 2 should succeed
			assert_ok!(Slots::trigger_onboard(RuntimeOrigin::signed(1), 2.into()));

			// Para 3 should fail cause their lease is in the future
			assert_noop!(
				Slots::trigger_onboard(RuntimeOrigin::signed(1), 3.into()),
				Error::<Test>::ParaNotOnboarding
			);

			// Trying Para 2 again should fail cause they are not currently an on-demand parachain
			assert!(Slots::trigger_onboard(RuntimeOrigin::signed(1), 2.into()).is_err());

			assert_eq!(TestRegistrar::<Test>::operations(), vec![(2.into(), 1, true),]);
		});
	}

	#[test]
	fn lease_period_offset_works() {
		new_test_ext().execute_with(|| {
			let (lpl, offset) = Slots::lease_period_length();
			assert_eq!(offset, 0);
			assert_eq!(Slots::lease_period_index(0), Some((0, true)));
			assert_eq!(Slots::lease_period_index(1), Some((0, false)));
			assert_eq!(Slots::lease_period_index(lpl - 1), Some((0, false)));
			assert_eq!(Slots::lease_period_index(lpl), Some((1, true)));
			assert_eq!(Slots::lease_period_index(lpl + 1), Some((1, false)));
			assert_eq!(Slots::lease_period_index(2 * lpl - 1), Some((1, false)));
			assert_eq!(Slots::lease_period_index(2 * lpl), Some((2, true)));
			assert_eq!(Slots::lease_period_index(2 * lpl + 1), Some((2, false)));

			// Lease period is 10, and we add an offset of 5.
			LeaseOffset::set(5);
			let (lpl, offset) = Slots::lease_period_length();
			assert_eq!(offset, 5);
			assert_eq!(Slots::lease_period_index(0), None);
			assert_eq!(Slots::lease_period_index(1), None);
			assert_eq!(Slots::lease_period_index(offset), Some((0, true)));
			assert_eq!(Slots::lease_period_index(lpl), Some((0, false)));
			assert_eq!(Slots::lease_period_index(lpl - 1 + offset), Some((0, false)));
			assert_eq!(Slots::lease_period_index(lpl + offset), Some((1, true)));
			assert_eq!(Slots::lease_period_index(lpl + offset + 1), Some((1, false)));
			assert_eq!(Slots::lease_period_index(2 * lpl - 1 + offset), Some((1, false)));
			assert_eq!(Slots::lease_period_index(2 * lpl + offset), Some((2, true)));
			assert_eq!(Slots::lease_period_index(2 * lpl + offset + 1), Some((2, false)));
		});
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking {
	use super::*;
	use frame_support::assert_ok;
	use frame_system::RawOrigin;
	use runtime_parachains::paras;
	use sp_runtime::traits::{Bounded, One};

	use frame_benchmarking::{account, benchmarks, whitelisted_caller, BenchmarkError};

	use crate::slots::Pallet as Slots;

	fn assert_last_event<T: Config>(generic_event: <T as Config>::RuntimeEvent) {
		let events = frame_system::Pallet::<T>::events();
		let system_event: <T as frame_system::Config>::RuntimeEvent = generic_event.into();
		// compare to the last event record
		let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
		assert_eq!(event, &system_event);
	}

	// Registers a parathread (on-demand parachain)
	fn register_a_parathread<T: Config + paras::Config>(i: u32) -> (ParaId, T::AccountId) {
		let para = ParaId::from(i);
		let leaser: T::AccountId = account("leaser", i, 0);
		T::Currency::make_free_balance_be(&leaser, BalanceOf::<T>::max_value());
		let worst_head_data = T::Registrar::worst_head_data();
		let worst_validation_code = T::Registrar::worst_validation_code();

		assert_ok!(T::Registrar::register(
			leaser.clone(),
			para,
			worst_head_data,
			worst_validation_code.clone(),
		));
		assert_ok!(paras::Pallet::<T>::add_trusted_validation_code(
			frame_system::Origin::<T>::Root.into(),
			worst_validation_code,
		));

		T::Registrar::execute_pending_transitions();

		(para, leaser)
	}

	benchmarks! {
		where_clause { where T: paras::Config }

		force_lease {
			// If there is an offset, we need to be on that block to be able to do lease things.
			frame_system::Pallet::<T>::set_block_number(T::LeaseOffset::get() + One::one());
			let para = ParaId::from(1337);
			let leaser: T::AccountId = account("leaser", 0, 0);
			T::Currency::make_free_balance_be(&leaser, BalanceOf::<T>::max_value());
			let amount = T::Currency::minimum_balance();
			let period_begin = 69u32.into();
			let period_count = 3u32.into();
			let origin =
				T::ForceOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
		}: _<T::RuntimeOrigin>(origin, para, leaser.clone(), amount, period_begin, period_count)
		verify {
			assert_last_event::<T>(Event::<T>::Leased {
				para_id: para,
				leaser, period_begin,
				period_count,
				extra_reserved: amount,
				total_amount: amount,
			}.into());
		}

		// Worst case scenario, T on-demand parachains onboard, and C lease holding parachains offboard.
		manage_lease_period_start {
			// Assume reasonable maximum of 100 paras at any time
			let c in 0 .. 100;
			let t in 0 .. 100;

			let period_begin = 1u32.into();
			let period_count = 4u32.into();

			// If there is an offset, we need to be on that block to be able to do lease things.
			frame_system::Pallet::<T>::set_block_number(T::LeaseOffset::get() + One::one());

			// Make T parathreads (on-demand parachains)
			let paras_info = (0..t).map(|i| {
				register_a_parathread::<T>(i)
			}).collect::<Vec<_>>();

			T::Registrar::execute_pending_transitions();

			// T on-demand parachains are upgrading to lease holding parachains
			for (para, leaser) in paras_info {
				let amount = T::Currency::minimum_balance();
				let origin = T::ForceOrigin::try_successful_origin()
					.expect("ForceOrigin has no successful origin required for the benchmark");
				Slots::<T>::force_lease(origin, para, leaser, amount, period_begin, period_count)?;
			}

			T::Registrar::execute_pending_transitions();

			// C lease holding parachains are downgrading to on-demand parachains
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

		// Assume that at most 8 people have deposits for leases on a parachain.
		// This would cover at least 4 years of leases in the worst case scenario.
		clear_all_leases {
			let max_people = 8;
			let (para, _) = register_a_parathread::<T>(1);

			// If there is an offset, we need to be on that block to be able to do lease things.
			frame_system::Pallet::<T>::set_block_number(T::LeaseOffset::get() + One::one());

			for i in 0 .. max_people {
				let leaser = account("lease_deposit", i, 0);
				let amount = T::Currency::minimum_balance();
				T::Currency::make_free_balance_be(&leaser, BalanceOf::<T>::max_value());

				// Average slot has 4 lease periods.
				let period_count: LeasePeriodOf<T> = 4u32.into();
				let period_begin = period_count * i.into();
				let origin = T::ForceOrigin::try_successful_origin()
					.expect("ForceOrigin has no successful origin required for the benchmark");
				Slots::<T>::force_lease(origin, para, leaser, amount, period_begin, period_count)?;
			}

			for i in 0 .. max_people {
				let leaser = account("lease_deposit", i, 0);
				assert_eq!(T::Currency::reserved_balance(&leaser), T::Currency::minimum_balance());
			}

			let origin =
				T::ForceOrigin::try_successful_origin().map_err(|_| BenchmarkError::Weightless)?;
		}: _<T::RuntimeOrigin>(origin, para)
		verify {
			for i in 0 .. max_people {
				let leaser = account("lease_deposit", i, 0);
				assert_eq!(T::Currency::reserved_balance(&leaser), 0u32.into());
			}
		}

		trigger_onboard {
			// get a parachain into a bad state where they did not onboard
			let (para, _) = register_a_parathread::<T>(1);
			Leases::<T>::insert(para, vec![Some((account::<T::AccountId>("lease_insert", 0, 0), BalanceOf::<T>::default()))]);
			assert!(T::Registrar::is_parathread(para));
			let caller = whitelisted_caller();
		}: _(RawOrigin::Signed(caller), para)
		verify {
			T::Registrar::execute_pending_transitions();
			assert!(T::Registrar::is_parachain(para));
		}

		impl_benchmark_test_suite!(
			Slots,
			crate::integration_tests::new_test_ext(),
			crate::integration_tests::Test,
		);
	}
}
