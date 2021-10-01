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

//! This pallet allows to assign permanent (long-lived) or temporary
//! (short-lived) parachain slots to paras, leveraging the existing
//! parachain slot lease mechanism. Temporary slots are given turns
//! in a fair (though best-effort) manner.
//! The dispatchables must be called from the configured origin
//! (typically `Sudo` or a governance origin).
//! This pallet is mostly to be used on test relay chain (e.g. Rococo).

use crate::{
	slots::{self, Pallet as Slots},
	traits::{LeaseError, Leaser, Registrar},
};
use frame_support::{pallet_prelude::*, traits::Currency};
use frame_system::pallet_prelude::*;
pub use pallet::*;
use parity_scale_codec::Encode;
use primitives::v1::Id as ParaId;
use runtime_parachains::{
	configuration,
	paras::{self},
	ParaLifecycle,
};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{One, Saturating, Zero},
	ArithmeticError,
};
use sp_std::prelude::*;

#[derive(Encode, Decode, Clone, Copy, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub enum SlotLeasePeriodStart {
	Current,
	Next,
}

#[derive(Encode, Decode, Clone, PartialEq, Eq, Default, RuntimeDebug, TypeInfo)]
pub struct ParachainTemporarySlot<AccountId, LeasePeriod> {
	manager: AccountId,
	period_begin: LeasePeriod,
	period_count: LeasePeriod,
	last_lease: Option<LeasePeriod>,
	lease_count: u32,
}

type BalanceOf<T> = <<<T as Config>::Leaser as Leaser>::Currency as Currency<
	<T as frame_system::Config>::AccountId,
>>::Balance;
type LeasePeriodOf<T> = <<T as Config>::Leaser as Leaser>::LeasePeriod;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	#[pallet::disable_frame_system_supertrait_check]
	pub trait Config: configuration::Config + paras::Config + slots::Config {
		/// The overarching event type.
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

		/// Origin for assigning slots
		type AssignSlotOrigin: EnsureOrigin<<Self as frame_system::Config>::Origin>;

		/// The type representing the leasing system.
		type Leaser: Leaser<AccountId = Self::AccountId, LeasePeriod = Self::BlockNumber>;

		/// The number of lease periods a permanent parachain slot lasts
		#[pallet::constant]
		type PermanentSlotLeasePeriodLength: Get<u32>;

		/// The number of lease periods a temporary parachain slot lasts
		#[pallet::constant]
		type TemporarySlotLeasePeriodLength: Get<u32>;

		/// The max number of permanent slots that can be assigned
		#[pallet::constant]
		type MaxPermanentSlots: Get<u32>;

		/// The max number of temporary slots to be scheduled per lease periods
		#[pallet::constant]
		type MaxTemporarySlotPerLeasePeriod: Get<u32>;
	}

	#[pallet::storage]
	#[pallet::getter(fn permanent_slots)]
	pub type PermanentSlots<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, Option<LeasePeriodOf<T>>, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn permanent_slot_count)]
	pub type PermanentSlotCount<T: Config> = StorageValue<_, u32, ValueQuery>;

	#[pallet::storage]
	#[pallet::getter(fn temporary_slots)]
	pub type TemporarySlots<T: Config> = StorageMap<
		_,
		Twox64Concat,
		ParaId,
		Option<ParachainTemporarySlot<T::AccountId, LeasePeriodOf<T>>>,
		ValueQuery,
	>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// A para was assigned a permanent parachain slot
		PermanentSlotAssigned(ParaId),
		/// A para was assigned a temporary parachain slot
		TemporarySlotAssigned(ParaId),
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The specified parachain or parathread is not registered.
		ParaDoesntExist,
		/// Not a parathread.
		NotParathread,
		/// Cannot upgrade parathread.
		CannotUpgrade,
		/// Cannot downgrade parachain.
		CannotDowngrade,
		/// Permanent or Temporary slot already assigned.
		SlotAlreadyAssigned,
		/// Permanent or Temporary slot has not been assigned.
		SlotNotAssigned,
		/// An ongoing lease already exists.
		OngoingLeaseExists,
		// Maximum number of permanent slots exceeded
		MaxPermanentSlotsExceeded,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(n: T::BlockNumber) -> Weight {
			let lease_period = Self::lease_period();
			let lease_period_index = Self::lease_period_index();
			if (n % lease_period).is_zero() {
				Self::manage_lease_period_start(lease_period_index)
			} else {
				0
			}
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Assign a permanent parachain slot
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn assign_perm_parachain_slot(origin: OriginFor<T>, id: ParaId) -> DispatchResult {
			T::AssignSlotOrigin::ensure_origin(origin)?;

			let manager = T::Registrar::manager_of(id).ok_or(Error::<T>::ParaDoesntExist)?;

			ensure!(T::Registrar::is_parathread(id), Error::<T>::NotParathread,);

			ensure!(
				!Self::has_permanent_slot(id) && !Self::has_temporary_slot(id),
				Error::<T>::SlotAlreadyAssigned
			);

			let current_lease_period: T::BlockNumber = Self::lease_period_index();
			ensure!(
				!T::Leaser::already_leased(
					id,
					current_lease_period,
					// Check current lease & next one
					current_lease_period.saturating_add(
						T::BlockNumber::from(2u32)
							.saturating_mul(T::PermanentSlotLeasePeriodLength::get().into())
					)
				),
				Error::<T>::OngoingLeaseExists
			);

			ensure!(
				PermanentSlotCount::<T>::get() + 1 <= T::MaxPermanentSlots::get(),
				Error::<T>::MaxPermanentSlotsExceeded
			);

			<PermanentSlotCount<T>>::try_mutate(|count| -> DispatchResult {
				Self::configure_slot_lease(
					id,
					manager,
					current_lease_period,
					T::PermanentSlotLeasePeriodLength::get().into(),
				)
				.map_err(|_| Error::<T>::CannotUpgrade)?;
				PermanentSlots::<T>::insert(id, Some(current_lease_period));
				*count = count.checked_add(One::one()).ok_or(ArithmeticError::Overflow)?;
				Ok(())
			})?;

			Self::deposit_event(Event::<T>::PermanentSlotAssigned(id));
			Ok(())
		}

		/// Assign a temporary parachain slot
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn assign_temp_parachain_slot(
			origin: OriginFor<T>,
			id: ParaId,
			lease_period_start: SlotLeasePeriodStart,
		) -> DispatchResult {
			T::AssignSlotOrigin::ensure_origin(origin)?;

			let manager = T::Registrar::manager_of(id).ok_or(Error::<T>::ParaDoesntExist)?;

			ensure!(T::Registrar::is_parathread(id), Error::<T>::NotParathread,);

			ensure!(
				!Self::has_permanent_slot(id) && !Self::has_temporary_slot(id),
				Error::<T>::SlotAlreadyAssigned
			);

			let current_lease_period: T::BlockNumber = Self::lease_period_index();
			ensure!(
				!T::Leaser::already_leased(
					id,
					current_lease_period,
					// Check current lease & next one
					current_lease_period.saturating_add(
						T::BlockNumber::from(2u32)
							.saturating_mul(T::TemporarySlotLeasePeriodLength::get().into())
					)
				),
				Error::<T>::OngoingLeaseExists
			);

			TemporarySlots::<T>::insert(
				id,
				Some(ParachainTemporarySlot {
					manager,
					period_begin: match lease_period_start {
						SlotLeasePeriodStart::Current => current_lease_period,
						SlotLeasePeriodStart::Next => current_lease_period + One::one(),
					},
					period_count: T::TemporarySlotLeasePeriodLength::get().into(),
					last_lease: None,
					lease_count: 0,
				}),
			);

			if lease_period_start == SlotLeasePeriodStart::Current {
				Self::allocate_temporary_slot_leases(current_lease_period)?;
			}

			Self::deposit_event(Event::<T>::TemporarySlotAssigned(id));

			Ok(())
		}

		/// Unassign a permanent or temporary parachain slot
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn unassign_parachain_slot(origin: OriginFor<T>, id: ParaId) -> DispatchResult {
			T::AssignSlotOrigin::ensure_origin(origin.clone())?;

			ensure!(
				Self::has_permanent_slot(id) || Self::has_temporary_slot(id),
				Error::<T>::SlotNotAssigned
			);

			if PermanentSlots::<T>::contains_key(id) {
				<PermanentSlotCount<T>>::try_mutate(|count| -> DispatchResult {
					*count = count.checked_sub(1).ok_or(ArithmeticError::Underflow)?;
					Ok(())
				})?;
				PermanentSlots::<T>::remove(id);
			} else if TemporarySlots::<T>::contains_key(id) {
				TemporarySlots::<T>::remove(id);
			}

			// Clean any para lease
			Self::clear_slot_leases(origin.clone(), id)?;

			// Force downgrade to parathread (if needed) before end of lease period
			if paras::Pallet::<T>::lifecycle(id) == Some(ParaLifecycle::Parachain) {
				runtime_parachains::schedule_parachain_downgrade::<T>(id)
					.map_err(|_| Error::<T>::CannotDowngrade)?;
			}

			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	fn allocate_temporary_slot_leases(lease_period_index: LeasePeriodOf<T>) -> DispatchResult {
		let mut active_temp_slots = 0u32;
		let mut pending_temp_slots = Vec::new();
		TemporarySlots::<T>::iter().for_each(|(para, maybe_slot)| {
			if let Some(slot) = maybe_slot {
				match slot.last_lease {
					Some(last_lease)
						if last_lease <= lease_period_index &&
							lease_period_index <
								(last_lease.saturating_add(slot.period_count)) =>
					{
						// Active slot lease
						active_temp_slots += 1;
					}
					Some(last_lease)
						// Slot w/ past lease, only consider it every other slot lease period (times period_count)
						if last_lease.saturating_add(slot.period_count.saturating_mul(2u32.into())) <= lease_period_index => {
							pending_temp_slots.push((para, slot));
					},
					None if slot.period_begin <= lease_period_index => {
						// Slot hasn't had a lease yet
						pending_temp_slots.insert(0, (para, slot));
					},
					_ => {
						// Slot not being considered for this lease period (will be for a subsequent one)
					},
				}
			}
		});

		if active_temp_slots < T::MaxTemporarySlotPerLeasePeriod::get() &&
			!pending_temp_slots.is_empty()
		{
			// Sort by lease_count, favoring slots that had no or less turns first
			// (then by last_lease index, and then Para ID)
			pending_temp_slots.sort_by(|a, b| {
				a.1.lease_count
					.cmp(&b.1.lease_count)
					.then_with(|| a.1.last_lease.cmp(&b.1.last_lease))
					.then_with(|| a.0.cmp(&b.0))
			});

			let slots_to_be_upgraded = pending_temp_slots
				.iter()
				.take((T::MaxTemporarySlotPerLeasePeriod::get() - active_temp_slots) as usize)
				.collect::<Vec<_>>();

			for (id, temp_slot) in slots_to_be_upgraded.iter() {
				TemporarySlots::<T>::try_mutate::<_, _, Error<T>, _>(id, |s| {
					// Configure temp slot lease
					Self::configure_slot_lease(
						*id,
						temp_slot.manager.clone(),
						lease_period_index,
						temp_slot.period_count,
					)
					.map_err(|_| Error::<T>::CannotUpgrade)?;

					// Update temp slot lease info in storage
					*s = Some(ParachainTemporarySlot {
						manager: temp_slot.manager.clone(),
						period_begin: temp_slot.period_begin,
						period_count: temp_slot.period_count,
						last_lease: Some(lease_period_index),
						lease_count: temp_slot.lease_count + 1,
					});

					Ok(())
				})?;
			}
		}
		Ok(())
	}

	fn clear_slot_leases(origin: OriginFor<T>, id: ParaId) -> DispatchResult {
		Slots::<T>::clear_all_leases(origin, id)
	}

	fn configure_slot_lease(
		para: ParaId,
		manager: T::AccountId,
		lease_period: LeasePeriodOf<T>,
		lease_duration: LeasePeriodOf<T>,
	) -> Result<(), LeaseError> {
		T::Leaser::lease_out(para, &manager, BalanceOf::<T>::zero(), lease_period, lease_duration)
	}

	fn has_permanent_slot(id: ParaId) -> bool {
		PermanentSlots::<T>::contains_key(id)
	}

	fn has_temporary_slot(id: ParaId) -> bool {
		TemporarySlots::<T>::contains_key(id)
	}

	fn lease_period_index() -> LeasePeriodOf<T> {
		T::Leaser::lease_period_index()
	}

	fn lease_period() -> LeasePeriodOf<T> {
		T::Leaser::lease_period()
	}

	fn manage_lease_period_start(lease_period_index: LeasePeriodOf<T>) -> Weight {
		// Note: leases that have ended in previous lease period,
		// should have been cleaned in slots pallet.
		let _ = Self::allocate_temporary_slot_leases(lease_period_index);
		0
	}
}

/// tests for this pallet
#[cfg(test)]
mod tests {
	use super::*;

	use crate::{assigned_slots, mock::TestRegistrar, slots};
	use frame_support::{assert_noop, assert_ok, parameter_types};
	use frame_system::EnsureRoot;
	use pallet_balances;
	use primitives::v1::{BlockNumber, Header};
	use runtime_parachains::{
		configuration as parachains_configuration, paras as parachains_paras,
		shared as parachains_shared,
	};
	use sp_core::H256;
	use sp_runtime::{
		traits::{BlakeTwo256, IdentityLookup},
		DispatchError::BadOrigin,
	};

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
			Configuration: parachains_configuration::{Pallet, Call, Storage, Config<T>},
			ParasShared: parachains_shared::{Pallet, Call, Storage},
			Parachains: parachains_paras::{Pallet, Origin, Call, Storage, Config, Event},
			Slots: slots::{Pallet, Call, Storage, Event<T>},
			AssignedSlots: assigned_slots::{Pallet, Call, Storage, Event<T>},
		}
	);

	parameter_types! {
		pub const BlockHashCount: u32 = 250;
	}
	impl frame_system::Config for Test {
		type BaseCallFilter = frame_support::traits::Everything;
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
		type OnSetCode = ();
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
		type MaxReserves = ();
		type ReserveIdentifier = [u8; 8];
	}

	impl parachains_configuration::Config for Test {
		type WeightInfo = parachains_configuration::weights::WeightInfo<Test>;
	}

	impl parachains_paras::Config for Test {
		type Origin = Origin;
		type Event = Event;
		type WeightInfo = parachains_paras::weights::WeightInfo<Test>;
	}

	impl parachains_shared::Config for Test {}

	parameter_types! {
		pub const LeasePeriod: BlockNumber = 3;
		pub const ParaDeposit: u64 = 1;
	}

	impl slots::Config for Test {
		type Event = Event;
		type Currency = Balances;
		type Registrar = TestRegistrar<Test>;
		type LeasePeriod = LeasePeriod;
		type WeightInfo = crate::slots::TestWeightInfo;
	}

	parameter_types! {
		pub const PermanentSlotLeasePeriodLength: u32 = 3;
		pub const TemporarySlotLeasePeriodLength: u32 = 2;
		pub const MaxPermanentSlots: u32 = 2;
		pub const MaxTemporarySlotPerLeasePeriod: u32 = 2;
	}

	impl assigned_slots::Config for Test {
		type Event = Event;
		type AssignSlotOrigin = EnsureRoot<Self::AccountId>;
		type Leaser = Slots;
		type PermanentSlotLeasePeriodLength = PermanentSlotLeasePeriodLength;
		type TemporarySlotLeasePeriodLength = TemporarySlotLeasePeriodLength;
		type MaxPermanentSlots = MaxPermanentSlots;
		type MaxTemporarySlotPerLeasePeriod = MaxTemporarySlotPerLeasePeriod;
	}

	// This function basically just builds a genesis storage key/value store according to
	// our desired mock up.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		pallet_balances::GenesisConfig::<Test> {
			balances: vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50), (6, 60)],
		}
		.assimilate_storage(&mut t)
		.unwrap();
		t.into()
	}

	fn run_to_block(n: BlockNumber) {
		while System::block_number() < n {
			let mut block = System::block_number();
			// on_finalize hooks
			AssignedSlots::on_finalize(block);
			Slots::on_finalize(block);
			Parachains::on_finalize(block);
			ParasShared::on_finalize(block);
			Configuration::on_finalize(block);
			Balances::on_finalize(block);
			System::on_finalize(block);
			// Set next block
			System::set_block_number(block + 1);
			block = System::block_number();
			// on_initialize hooks
			System::on_initialize(block);
			Balances::on_initialize(block);
			Configuration::on_initialize(block);
			ParasShared::on_initialize(block);
			Parachains::on_initialize(block);
			Slots::on_initialize(block);
			AssignedSlots::on_initialize(block);
		}
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_eq!(Slots::lease_period(), 3);
			assert_eq!(Slots::lease_period_index(), 0);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);

			run_to_block(3);
			assert_eq!(Slots::lease_period_index(), 1);
		});
	}

	#[test]
	fn assign_perm_slot_fails_for_unknown_para() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(1),),
				Error::<Test>::ParaDoesntExist
			);
		});
	}

	#[test]
	fn assign_perm_slot_fails_for_invalid_origin() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(Origin::signed(1), ParaId::from(1),),
				BadOrigin
			);
		});
	}

	#[test]
	fn assign_perm_slot_fails_when_not_parathread() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));
			assert_ok!(TestRegistrar::<Test>::make_parachain(ParaId::from(1)));

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(1),),
				Error::<Test>::NotParathread
			);
		});
	}

	#[test]
	fn assign_perm_slot_fails_when_existing_lease() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));

			// Register lease in current lease period
			assert_ok!(Slots::lease_out(ParaId::from(1), &1, 1, 1, 1));
			// Try to assign a perm slot in current period fails
			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(1),),
				Error::<Test>::OngoingLeaseExists
			);

			// Cleanup
			assert_ok!(Slots::clear_all_leases(Origin::root(), 1.into()));

			// Register lease for next lease period
			assert_ok!(Slots::lease_out(ParaId::from(1), &1, 1, 2, 1));
			// Should be detected and also fail
			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(1),),
				Error::<Test>::OngoingLeaseExists
			);
		});
	}

	#[test]
	fn assign_perm_slot_fails_when_max_perm_slots_exceeded() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));

			assert_ok!(TestRegistrar::<Test>::register(
				2,
				ParaId::from(2),
				Default::default(),
				Default::default()
			));

			assert_ok!(TestRegistrar::<Test>::register(
				3,
				ParaId::from(3),
				Default::default(),
				Default::default()
			));

			assert_ok!(AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(1),));
			assert_ok!(AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(2),));
			assert_eq!(AssignedSlots::permanent_slot_count(), 2);

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(3),),
				Error::<Test>::MaxPermanentSlotsExceeded
			);
		});
	}

	#[test]
	fn assign_perm_slot_succeeds_for_parathread() {
		new_test_ext().execute_with(|| {
			let mut block = 1;
			run_to_block(block);
			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));

			assert_eq!(AssignedSlots::permanent_slot_count(), 0);
			assert_eq!(AssignedSlots::permanent_slots(ParaId::from(1)), None);

			assert_ok!(AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(1),));

			// Para is a parachain for PermanentSlotLeasePeriodLength * LeasePeriod blocks
			while block < 9 {
				println!("block #{}", block);

				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), true);

				assert_eq!(AssignedSlots::permanent_slot_count(), 1);
				assert_eq!(AssignedSlots::has_permanent_slot(ParaId::from(1)), true);
				assert_eq!(AssignedSlots::permanent_slots(ParaId::from(1)), Some(0));

				assert_eq!(Slots::already_leased(ParaId::from(1), 0, 2), true);

				block += 1;
				run_to_block(block);
			}

			// Para lease ended, downgraded back to parathread
			assert_eq!(TestRegistrar::<Test>::is_parathread(ParaId::from(1)), true);
			assert_eq!(Slots::already_leased(ParaId::from(1), 0, 5), false);
		});
	}

	#[test]
	fn assign_temp_slot_fails_for_unknown_para() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					Origin::root(),
					ParaId::from(1),
					SlotLeasePeriodStart::Current
				),
				Error::<Test>::ParaDoesntExist
			);
		});
	}

	#[test]
	fn assign_temp_slot_fails_for_invalid_origin() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					Origin::signed(1),
					ParaId::from(1),
					SlotLeasePeriodStart::Current
				),
				BadOrigin
			);
		});
	}

	#[test]
	fn assign_temp_slot_fails_when_not_parathread() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));
			assert_ok!(TestRegistrar::<Test>::make_parachain(ParaId::from(1)));

			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					Origin::root(),
					ParaId::from(1),
					SlotLeasePeriodStart::Current
				),
				Error::<Test>::NotParathread
			);
		});
	}

	#[test]
	fn assign_temp_slot_fails_when_existing_lease() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));

			// Register lease in current lease period
			assert_ok!(Slots::lease_out(ParaId::from(1), &1, 1, 1, 1));
			// Try to assign a perm slot in current period fails
			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					Origin::root(),
					ParaId::from(1),
					SlotLeasePeriodStart::Current
				),
				Error::<Test>::OngoingLeaseExists
			);

			// Cleanup
			assert_ok!(Slots::clear_all_leases(Origin::root(), 1.into()));

			// Register lease for next lease period
			assert_ok!(Slots::lease_out(ParaId::from(1), &1, 1, 2, 1));
			// Should be detected and also fail
			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					Origin::root(),
					ParaId::from(1),
					SlotLeasePeriodStart::Current
				),
				Error::<Test>::OngoingLeaseExists
			);
		});
	}

	#[test]
	fn assign_temp_slot_succeeds_for_single_parathread() {
		new_test_ext().execute_with(|| {
			let mut block = 1;
			run_to_block(block);
			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));

			assert_eq!(AssignedSlots::temporary_slots(ParaId::from(1)), None);

			assert_ok!(AssignedSlots::assign_temp_parachain_slot(
				Origin::root(),
				ParaId::from(1),
				SlotLeasePeriodStart::Current
			));

			// Block 1-5
			// Para is a parachain for TemporarySlotLeasePeriodLength * LeasePeriod blocks
			while block < 6 {
				println!("block #{}", block);
				println!("lease period #{}", Slots::lease_period_index());
				println!("lease {:?}", Slots::lease(ParaId::from(1)));

				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), true);

				assert_eq!(AssignedSlots::has_temporary_slot(ParaId::from(1)), true);
				assert_eq!(
					AssignedSlots::temporary_slots(ParaId::from(1)),
					Some(ParachainTemporarySlot {
						manager: 1,
						period_begin: 0,
						period_count: 2, // TemporarySlotLeasePeriodLength
						last_lease: Some(0),
						lease_count: 1
					})
				);

				assert_eq!(Slots::already_leased(ParaId::from(1), 0, 1), true);

				block += 1;
				run_to_block(block);
			}

			// Block 6
			println!("block #{}", block);
			println!("lease period #{}", Slots::lease_period_index());
			println!("lease {:?}", Slots::lease(ParaId::from(1)));

			// Para lease ended, downgraded back to parathread
			assert_eq!(TestRegistrar::<Test>::is_parathread(ParaId::from(1)), true);
			assert_eq!(Slots::already_leased(ParaId::from(1), 0, 3), false);

			// Block 12
			// Para should get a turn after TemporarySlotLeasePeriodLength * LeasePeriod blocks
			run_to_block(12);
			println!("block #{}", block);
			println!("lease period #{}", Slots::lease_period_index());
			println!("lease {:?}", Slots::lease(ParaId::from(1)));

			assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), true);
			assert_eq!(Slots::already_leased(ParaId::from(1), 4, 5), true);
		});
	}

	#[test]
	fn assign_temp_slot_succeeds_for_multiple_parathreads() {
		new_test_ext().execute_with(|| {
			// Block 1, Period 0
			run_to_block(1);

			// Register 6 paras & a temp slot for each
			// (3 slots in current lease period, 3 in the next one)
			for n in 0..=5 {
				assert_ok!(TestRegistrar::<Test>::register(
					n,
					ParaId::from(n as u32),
					Default::default(),
					Default::default()
				));

				assert_ok!(AssignedSlots::assign_temp_parachain_slot(
					Origin::root(),
					ParaId::from(n as u32),
					if (n % 2).is_zero() {
						SlotLeasePeriodStart::Current
					} else {
						SlotLeasePeriodStart::Next
					}
				));
			}

			// Block 1-5, Period 0-1
			for n in 1..=5 {
				if n > 1 {
					run_to_block(n);
				}
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5)), false);
			}

			// Block 6-11, Period 2-3
			for n in 6..=11 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5)), false);
			}

			// Block 12-17, Period 4-5
			for n in 12..=17 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5)), true);
			}

			// Block 18-23, Period 6-7
			for n in 18..=23 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5)), false);
			}

			// Block 24-29, Period 8-9
			for n in 24..=29 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5)), false);
			}

			// Block 30-35, Period 10-11
			for n in 30..=35 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5)), true);
			}
		});
	}

	#[test]
	fn unassign_slot_fails_for_unknown_para() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::unassign_parachain_slot(Origin::root(), ParaId::from(1),),
				Error::<Test>::SlotNotAssigned
			);
		});
	}

	#[test]
	fn unassign_slot_fails_for_invalid_origin() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(Origin::signed(1), ParaId::from(1),),
				BadOrigin
			);
		});
	}

	#[test]
	fn unassign_perm_slot_succeeds() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));

			assert_ok!(AssignedSlots::assign_perm_parachain_slot(Origin::root(), ParaId::from(1),));

			assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), true);

			assert_ok!(AssignedSlots::unassign_parachain_slot(Origin::root(), ParaId::from(1),));

			assert_eq!(AssignedSlots::permanent_slot_count(), 0);
			assert_eq!(AssignedSlots::has_permanent_slot(ParaId::from(1)), false);
			assert_eq!(AssignedSlots::permanent_slots(ParaId::from(1)), None);

			assert_eq!(Slots::already_leased(ParaId::from(1), 0, 2), false);
		});
	}

	#[test]
	fn unassign_temp_slot_succeeds() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1),
				Default::default(),
				Default::default()
			));

			assert_ok!(AssignedSlots::assign_temp_parachain_slot(
				Origin::root(),
				ParaId::from(1),
				SlotLeasePeriodStart::Current
			));

			assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1)), true);

			assert_ok!(AssignedSlots::unassign_parachain_slot(Origin::root(), ParaId::from(1),));

			assert_eq!(AssignedSlots::has_temporary_slot(ParaId::from(1)), false);
			assert_eq!(AssignedSlots::temporary_slots(ParaId::from(1)), None);

			assert_eq!(Slots::already_leased(ParaId::from(1), 0, 1), false);
		});
	}
}
