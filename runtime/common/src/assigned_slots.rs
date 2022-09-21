// Copyright 2021 Parity Technologies (UK) Ltd.
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
//! This pallet should not be used on a production relay chain,
//! only on a test relay chain (e.g. Rococo).

use crate::{
	slots::{self, Pallet as Slots, WeightInfo},
	traits::{LeaseError, Leaser, Registrar},
	MAXIMUM_BLOCK_WEIGHT,
};
use frame_support::{pallet_prelude::*, traits::Currency};
use frame_system::pallet_prelude::*;
pub use pallet::*;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use primitives::v2::Id as ParaId;
use runtime_parachains::{
	configuration,
	paras::{self},
};
use scale_info::TypeInfo;
use sp_runtime::traits::{One, Saturating, Zero};
use sp_std::prelude::*;

/// Lease period an assigned slot should start from (current, or next one).
#[derive(Encode, Decode, Clone, Copy, Eq, PartialEq, RuntimeDebug, TypeInfo)]
pub enum SlotLeasePeriodStart {
	Current,
	Next,
}

/// Information about a temporary parachain slot.
#[derive(Encode, Decode, Clone, PartialEq, Eq, Default, MaxEncodedLen, RuntimeDebug, TypeInfo)]
pub struct ParachainTemporarySlot<AccountId, LeasePeriod> {
	/// Manager account of the para.
	pub manager: AccountId,
	/// Lease period the parachain slot should ideally start from,
	/// As slot are allocated in a best-effort manner, this could be later,
	/// but not earlier than the specified period.
	pub period_begin: LeasePeriod,
	/// Number of lease period the slot lease will last.
	/// This is set to the value configured in `TemporarySlotLeasePeriodLength`.
	pub period_count: LeasePeriod,
	/// Last lease period this slot had a turn in (incl. current).
	/// This is set to the beginning period of a slot.
	pub last_lease: Option<LeasePeriod>,
	/// Number of leases this temporary slot had (incl. current).
	pub lease_count: u32,
}

type BalanceOf<T> = <<<T as Config>::Leaser as Leaser<<T as frame_system::Config>::BlockNumber>>::Currency as Currency<
	<T as frame_system::Config>::AccountId,
>>::Balance;
type LeasePeriodOf<T> =
	<<T as Config>::Leaser as Leaser<<T as frame_system::Config>::BlockNumber>>::LeasePeriod;

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
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// Origin for assigning slots.
		type AssignSlotOrigin: EnsureOrigin<<Self as frame_system::Config>::RuntimeOrigin>;

		/// The type representing the leasing system.
		type Leaser: Leaser<
			Self::BlockNumber,
			AccountId = Self::AccountId,
			LeasePeriod = Self::BlockNumber,
		>;

		/// The number of lease periods a permanent parachain slot lasts.
		#[pallet::constant]
		type PermanentSlotLeasePeriodLength: Get<u32>;

		/// The number of lease periods a temporary parachain slot lasts.
		#[pallet::constant]
		type TemporarySlotLeasePeriodLength: Get<u32>;

		/// The max number of permanent slots that can be assigned.
		#[pallet::constant]
		type MaxPermanentSlots: Get<u32>;

		/// The max number of temporary slots that can be assigned.
		#[pallet::constant]
		type MaxTemporarySlots: Get<u32>;

		/// The max number of temporary slots to be scheduled per lease periods.
		#[pallet::constant]
		type MaxTemporarySlotPerLeasePeriod: Get<u32>;
	}

	/// Assigned permanent slots, with their start lease period, and duration.
	#[pallet::storage]
	#[pallet::getter(fn permanent_slots)]
	pub type PermanentSlots<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, (LeasePeriodOf<T>, LeasePeriodOf<T>), OptionQuery>;

	/// Number of assigned (and active) permanent slots.
	#[pallet::storage]
	#[pallet::getter(fn permanent_slot_count)]
	pub type PermanentSlotCount<T: Config> = StorageValue<_, u32, ValueQuery>;

	/// Assigned temporary slots.
	#[pallet::storage]
	#[pallet::getter(fn temporary_slots)]
	pub type TemporarySlots<T: Config> = StorageMap<
		_,
		Twox64Concat,
		ParaId,
		ParachainTemporarySlot<T::AccountId, LeasePeriodOf<T>>,
		OptionQuery,
	>;

	/// Number of assigned temporary slots.
	#[pallet::storage]
	#[pallet::getter(fn temporary_slot_count)]
	pub type TemporarySlotCount<T: Config> = StorageValue<_, u32, ValueQuery>;

	/// Number of active temporary slots in current slot lease period.
	#[pallet::storage]
	#[pallet::getter(fn active_temporary_slot_count)]
	pub type ActiveTemporarySlotCount<T: Config> = StorageValue<_, u32, ValueQuery>;

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
		// Maximum number of temporary slots exceeded
		MaxTemporarySlotsExceeded,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(n: T::BlockNumber) -> Weight {
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
		// TODO: Benchmark this
		/// Assign a permanent parachain slot and immediately create a lease for it.
		#[pallet::weight(((MAXIMUM_BLOCK_WEIGHT / 10) as Weight, DispatchClass::Operational))]
		pub fn assign_perm_parachain_slot(origin: OriginFor<T>, id: ParaId) -> DispatchResult {
			T::AssignSlotOrigin::ensure_origin(origin)?;

			let manager = T::Registrar::manager_of(id).ok_or(Error::<T>::ParaDoesntExist)?;

			ensure!(T::Registrar::is_parathread(id), Error::<T>::NotParathread,);

			ensure!(
				!Self::has_permanent_slot(id) && !Self::has_temporary_slot(id),
				Error::<T>::SlotAlreadyAssigned
			);

			let current_lease_period: T::BlockNumber = Self::current_lease_period_index();
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
				PermanentSlotCount::<T>::get() < T::MaxPermanentSlots::get(),
				Error::<T>::MaxPermanentSlotsExceeded
			);

			// Permanent slot assignment fails if a lease cannot be created
			Self::configure_slot_lease(
				id,
				manager,
				current_lease_period,
				T::PermanentSlotLeasePeriodLength::get().into(),
			)
			.map_err(|_| Error::<T>::CannotUpgrade)?;

			PermanentSlots::<T>::insert(
				id,
				(
					current_lease_period,
					LeasePeriodOf::<T>::from(T::PermanentSlotLeasePeriodLength::get()),
				),
			);
			<PermanentSlotCount<T>>::mutate(|count| count.saturating_inc());

			Self::deposit_event(Event::<T>::PermanentSlotAssigned(id));
			Ok(())
		}

		// TODO: Benchmark this
		/// Assign a temporary parachain slot. The function tries to create a lease for it
		/// immediately if `SlotLeasePeriodStart::Current` is specified, and if the number
		/// of currently active temporary slots is below `MaxTemporarySlotPerLeasePeriod`.
		#[pallet::weight(((MAXIMUM_BLOCK_WEIGHT / 10) as Weight, DispatchClass::Operational))]
		pub fn assign_temp_parachain_slot(
			origin: OriginFor<T>,
			id: ParaId,
			lease_period_start: SlotLeasePeriodStart,
		) -> DispatchResult {
			T::AssignSlotOrigin::ensure_origin(origin)?;

			let manager = T::Registrar::manager_of(id).ok_or(Error::<T>::ParaDoesntExist)?;

			ensure!(T::Registrar::is_parathread(id), Error::<T>::NotParathread);

			ensure!(
				!Self::has_permanent_slot(id) && !Self::has_temporary_slot(id),
				Error::<T>::SlotAlreadyAssigned
			);

			let current_lease_period: T::BlockNumber = Self::current_lease_period_index();
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

			ensure!(
				TemporarySlotCount::<T>::get() < T::MaxTemporarySlots::get(),
				Error::<T>::MaxTemporarySlotsExceeded
			);

			let mut temp_slot = ParachainTemporarySlot {
				manager: manager.clone(),
				period_begin: match lease_period_start {
					SlotLeasePeriodStart::Current => current_lease_period,
					SlotLeasePeriodStart::Next => current_lease_period + One::one(),
				},
				period_count: T::TemporarySlotLeasePeriodLength::get().into(),
				last_lease: None,
				lease_count: 0,
			};

			if lease_period_start == SlotLeasePeriodStart::Current &&
				Self::active_temporary_slot_count() < T::MaxTemporarySlotPerLeasePeriod::get()
			{
				// Try to allocate slot directly
				match Self::configure_slot_lease(
					id,
					manager,
					temp_slot.period_begin,
					temp_slot.period_count,
				) {
					Ok(_) => {
						ActiveTemporarySlotCount::<T>::mutate(|count| count.saturating_inc());
						temp_slot.last_lease = Some(temp_slot.period_begin);
						temp_slot.lease_count += 1;
					},
					Err(err) => {
						// Treat failed lease creation as warning .. slot will be allocated a lease
						// in a subsequent lease period by the `allocate_temporary_slot_leases` function.
						log::warn!(target: "assigned_slots",
							"Failed to allocate a temp slot for para {:?} at period {:?}: {:?}",
							id, current_lease_period, err
						);
					},
				}
			}

			TemporarySlots::<T>::insert(id, temp_slot);
			<TemporarySlotCount<T>>::mutate(|count| count.saturating_inc());

			Self::deposit_event(Event::<T>::TemporarySlotAssigned(id));

			Ok(())
		}

		// TODO: Benchmark this
		/// Unassign a permanent or temporary parachain slot
		#[pallet::weight(((MAXIMUM_BLOCK_WEIGHT / 10) as Weight, DispatchClass::Operational))]
		pub fn unassign_parachain_slot(origin: OriginFor<T>, id: ParaId) -> DispatchResult {
			T::AssignSlotOrigin::ensure_origin(origin.clone())?;

			ensure!(
				Self::has_permanent_slot(id) || Self::has_temporary_slot(id),
				Error::<T>::SlotNotAssigned
			);

			// Check & cache para status before we clear the lease
			let is_parachain = Self::is_parachain(id);

			// Remove perm or temp slot
			Self::clear_slot_leases(origin.clone(), id)?;

			if PermanentSlots::<T>::contains_key(id) {
				PermanentSlots::<T>::remove(id);
				<PermanentSlotCount<T>>::mutate(|count| *count = count.saturating_sub(One::one()));
			} else if TemporarySlots::<T>::contains_key(id) {
				TemporarySlots::<T>::remove(id);
				<TemporarySlotCount<T>>::mutate(|count| *count = count.saturating_sub(One::one()));
				if is_parachain {
					<ActiveTemporarySlotCount<T>>::mutate(|active_count| {
						*active_count = active_count.saturating_sub(One::one())
					});
				}
			}

			// Force downgrade to parathread (if needed) before end of lease period
			if is_parachain {
				if let Err(err) = runtime_parachains::schedule_parachain_downgrade::<T>(id) {
					// Treat failed downgrade as warning .. slot lease has been cleared,
					// so the parachain will be downgraded anyway by the slots pallet
					// at the end of the lease period .
					log::warn!(target: "assigned_slots",
						"Failed to downgrade parachain {:?} at period {:?}: {:?}",
						id, Self::current_lease_period_index(), err
					);
				}
			}

			Ok(())
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Allocate temporary slot leases up to `MaxTemporarySlotPerLeasePeriod` per lease period.
	/// Beyond the already active temporary slot leases, this function will activate more leases
	/// in the following order of preference:
	/// - Assigned slots that didn't have a turn yet, though their `period_begin` has passed.
	/// - Assigned slots that already had one (or more) turn(s): they will be considered for the
	/// current slot lease if they weren't active in the preceding one, and will be ranked by
	/// total number of lease (lower first), and then when they last a turn (older ones first).
	/// If any remaining ex-aequo, we just take the para ID in ascending order as discriminator.
	///
	/// Assigned slots with a `period_begin` bigger than current lease period are not considered (yet).
	///
	/// The function will call out to `Leaser::lease_out` to create the appropriate slot leases.
	fn allocate_temporary_slot_leases(lease_period_index: LeasePeriodOf<T>) -> DispatchResult {
		let mut active_temp_slots = 0u32;
		let mut pending_temp_slots = Vec::new();
		TemporarySlots::<T>::iter().for_each(|(para, slot)| {
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
		});

		let mut newly_created_lease = 0u32;
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

			let slots_to_be_upgraded = pending_temp_slots.iter().take(
				(T::MaxTemporarySlotPerLeasePeriod::get().saturating_sub(active_temp_slots))
					as usize,
			);

			for (id, temp_slot) in slots_to_be_upgraded {
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

					newly_created_lease += 1;

					Ok(())
				})?;
			}
		}

		ActiveTemporarySlotCount::<T>::set(active_temp_slots + newly_created_lease);

		Ok(())
	}

	/// Clear out all slot leases for both permanent & temporary slots.
	/// The function merely calls out to `Slots::clear_all_leases`.
	fn clear_slot_leases(origin: OriginFor<T>, id: ParaId) -> DispatchResult {
		Slots::<T>::clear_all_leases(origin, id)
	}

	/// Create a parachain slot lease based on given params.
	/// The function merely calls out to `Leaser::lease_out`.
	fn configure_slot_lease(
		para: ParaId,
		manager: T::AccountId,
		lease_period: LeasePeriodOf<T>,
		lease_duration: LeasePeriodOf<T>,
	) -> Result<(), LeaseError> {
		T::Leaser::lease_out(para, &manager, BalanceOf::<T>::zero(), lease_period, lease_duration)
	}

	/// Returns whether a para has been assigned a permanent slot.
	fn has_permanent_slot(id: ParaId) -> bool {
		PermanentSlots::<T>::contains_key(id)
	}

	/// Returns whether a para has been assigned temporary slot.
	fn has_temporary_slot(id: ParaId) -> bool {
		TemporarySlots::<T>::contains_key(id)
	}

	/// Returns whether a para is currently a parachain.
	fn is_parachain(id: ParaId) -> bool {
		T::Registrar::is_parachain(id)
	}

	/// Returns current lease period index.
	fn current_lease_period_index() -> LeasePeriodOf<T> {
		T::Leaser::lease_period_index(frame_system::Pallet::<T>::block_number())
			.and_then(|x| Some(x.0))
			.unwrap()
	}

	/// Returns lease period index for block
	fn lease_period_index(block: BlockNumberFor<T>) -> Option<(LeasePeriodOf<T>, bool)> {
		T::Leaser::lease_period_index(block)
	}

	/// Handles start of a lease period.
	fn manage_lease_period_start(lease_period_index: LeasePeriodOf<T>) -> Weight {
		// Note: leases that have ended in previous lease period, should have been cleaned in slots pallet.
		if let Err(err) = Self::allocate_temporary_slot_leases(lease_period_index) {
			log::error!(target: "assigned_slots",
				"Allocating slots failed for lease period {:?}, with: {:?}",
				lease_period_index, err
			);
		}
		<T as slots::Config>::WeightInfo::force_lease() *
			(T::MaxTemporarySlotPerLeasePeriod::get() as u64)
	}
}

/// tests for this pallet
#[cfg(test)]
mod tests {
	use super::*;

	use crate::{assigned_slots, mock::TestRegistrar, slots};
	use ::test_helpers::{dummy_head_data, dummy_validation_code};
	use frame_support::{assert_noop, assert_ok, parameter_types};
	use frame_system::EnsureRoot;
	use pallet_balances;
	use primitives::v2::{BlockNumber, Header};
	use runtime_parachains::{
		configuration as parachains_configuration, paras as parachains_paras,
		shared as parachains_shared,
	};
	use sp_core::H256;
	use sp_runtime::{
		traits::{BlakeTwo256, IdentityLookup},
		transaction_validity::TransactionPriority,
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
			Parachains: parachains_paras::{Pallet, Call, Storage, Config, Event},
			Slots: slots::{Pallet, Call, Storage, Event<T>},
			AssignedSlots: assigned_slots::{Pallet, Call, Storage, Event<T>},
		}
	);

	impl<C> frame_system::offchain::SendTransactionTypes<C> for Test
	where
		RuntimeCall: From<C>,
	{
		type Extrinsic = UncheckedExtrinsic;
		type OverarchingCall = RuntimeCall;
	}

	parameter_types! {
		pub const BlockHashCount: u32 = 250;
	}
	impl frame_system::Config for Test {
		type BaseCallFilter = frame_support::traits::Everything;
		type BlockWeights = ();
		type BlockLength = ();
		type RuntimeOrigin = RuntimeOrigin;
		type RuntimeCall = RuntimeCall;
		type Index = u64;
		type BlockNumber = BlockNumber;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
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
	}

	impl parachains_configuration::Config for Test {
		type WeightInfo = parachains_configuration::TestWeightInfo;
	}

	parameter_types! {
		pub const ParasUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
	}

	impl parachains_paras::Config for Test {
		type RuntimeEvent = RuntimeEvent;
		type WeightInfo = parachains_paras::TestWeightInfo;
		type UnsignedPriority = ParasUnsignedPriority;
		type NextSessionRotation = crate::mock::TestNextSessionRotation;
	}

	impl parachains_shared::Config for Test {}

	parameter_types! {
		pub const LeasePeriod: BlockNumber = 3;
		pub static LeaseOffset: BlockNumber = 0;
		pub const ParaDeposit: u64 = 1;
	}

	impl slots::Config for Test {
		type RuntimeEvent = RuntimeEvent;
		type Currency = Balances;
		type Registrar = TestRegistrar<Test>;
		type LeasePeriod = LeasePeriod;
		type LeaseOffset = LeaseOffset;
		type ForceOrigin = EnsureRoot<Self::AccountId>;
		type WeightInfo = crate::slots::TestWeightInfo;
	}

	parameter_types! {
		pub const PermanentSlotLeasePeriodLength: u32 = 3;
		pub const TemporarySlotLeasePeriodLength: u32 = 2;
		pub const MaxPermanentSlots: u32 = 2;
		pub const MaxTemporarySlots: u32 = 6;
		pub const MaxTemporarySlotPerLeasePeriod: u32 = 2;
	}

	impl assigned_slots::Config for Test {
		type RuntimeEvent = RuntimeEvent;
		type AssignSlotOrigin = EnsureRoot<Self::AccountId>;
		type Leaser = Slots;
		type PermanentSlotLeasePeriodLength = PermanentSlotLeasePeriodLength;
		type TemporarySlotLeasePeriodLength = TemporarySlotLeasePeriodLength;
		type MaxPermanentSlots = MaxPermanentSlots;
		type MaxTemporarySlots = MaxTemporarySlots;
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
			assert_eq!(AssignedSlots::current_lease_period_index(), 0);
			assert_eq!(Slots::deposit_held(1.into(), &1), 0);

			run_to_block(3);
			assert_eq!(AssignedSlots::current_lease_period_index(), 1);
		});
	}

	#[test]
	fn assign_perm_slot_fails_for_unknown_para() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(1_u32),
				),
				Error::<Test>::ParaDoesntExist
			);
		});
	}

	#[test]
	fn assign_perm_slot_fails_for_invalid_origin() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(
					RuntimeOrigin::signed(1),
					ParaId::from(1_u32),
				),
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
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));
			assert_ok!(TestRegistrar::<Test>::make_parachain(ParaId::from(1_u32)));

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(1_u32),
				),
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
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			// Register lease in current lease period
			assert_ok!(Slots::lease_out(ParaId::from(1_u32), &1, 1, 1, 1));
			// Try to assign a perm slot in current period fails
			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(1_u32),
				),
				Error::<Test>::OngoingLeaseExists
			);

			// Cleanup
			assert_ok!(Slots::clear_all_leases(RuntimeOrigin::root(), 1.into()));

			// Register lease for next lease period
			assert_ok!(Slots::lease_out(ParaId::from(1_u32), &1, 1, 2, 1));
			// Should be detected and also fail
			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(1_u32),
				),
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
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			assert_ok!(TestRegistrar::<Test>::register(
				2,
				ParaId::from(2_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			assert_ok!(TestRegistrar::<Test>::register(
				3,
				ParaId::from(3_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			assert_ok!(AssignedSlots::assign_perm_parachain_slot(
				RuntimeOrigin::root(),
				ParaId::from(1_u32),
			));
			assert_ok!(AssignedSlots::assign_perm_parachain_slot(
				RuntimeOrigin::root(),
				ParaId::from(2_u32),
			));
			assert_eq!(AssignedSlots::permanent_slot_count(), 2);

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(3_u32),
				),
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
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			assert_eq!(AssignedSlots::permanent_slot_count(), 0);
			assert_eq!(AssignedSlots::permanent_slots(ParaId::from(1_u32)), None);

			assert_ok!(AssignedSlots::assign_perm_parachain_slot(
				RuntimeOrigin::root(),
				ParaId::from(1_u32),
			));

			// Para is a parachain for PermanentSlotLeasePeriodLength * LeasePeriod blocks
			while block < 9 {
				println!("block #{}", block);

				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), true);

				assert_eq!(AssignedSlots::permanent_slot_count(), 1);
				assert_eq!(AssignedSlots::has_permanent_slot(ParaId::from(1_u32)), true);
				assert_eq!(AssignedSlots::permanent_slots(ParaId::from(1_u32)), Some((0, 3)));

				assert_eq!(Slots::already_leased(ParaId::from(1_u32), 0, 2), true);

				block += 1;
				run_to_block(block);
			}

			// Para lease ended, downgraded back to parathread
			assert_eq!(TestRegistrar::<Test>::is_parathread(ParaId::from(1_u32)), true);
			assert_eq!(Slots::already_leased(ParaId::from(1_u32), 0, 5), false);
		});
	}

	#[test]
	fn assign_temp_slot_fails_for_unknown_para() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(1_u32),
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
					RuntimeOrigin::signed(1),
					ParaId::from(1_u32),
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
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));
			assert_ok!(TestRegistrar::<Test>::make_parachain(ParaId::from(1_u32)));

			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(1_u32),
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
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			// Register lease in current lease period
			assert_ok!(Slots::lease_out(ParaId::from(1_u32), &1, 1, 1, 1));
			// Try to assign a perm slot in current period fails
			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(1_u32),
					SlotLeasePeriodStart::Current
				),
				Error::<Test>::OngoingLeaseExists
			);

			// Cleanup
			assert_ok!(Slots::clear_all_leases(RuntimeOrigin::root(), 1.into()));

			// Register lease for next lease period
			assert_ok!(Slots::lease_out(ParaId::from(1_u32), &1, 1, 2, 1));
			// Should be detected and also fail
			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(1_u32),
					SlotLeasePeriodStart::Current
				),
				Error::<Test>::OngoingLeaseExists
			);
		});
	}

	#[test]
	fn assign_temp_slot_fails_when_max_temp_slots_exceeded() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			// Register 6 paras & a temp slot for each
			for n in 0..=5 {
				assert_ok!(TestRegistrar::<Test>::register(
					n,
					ParaId::from(n as u32),
					dummy_head_data(),
					dummy_validation_code()
				));

				assert_ok!(AssignedSlots::assign_temp_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(n as u32),
					SlotLeasePeriodStart::Current
				));
			}

			assert_eq!(AssignedSlots::temporary_slot_count(), 6);

			// Attempt to assign one more temp slot
			assert_ok!(TestRegistrar::<Test>::register(
				7,
				ParaId::from(7_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));
			assert_noop!(
				AssignedSlots::assign_temp_parachain_slot(
					RuntimeOrigin::root(),
					ParaId::from(7_u32),
					SlotLeasePeriodStart::Current
				),
				Error::<Test>::MaxTemporarySlotsExceeded
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
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			assert_eq!(AssignedSlots::temporary_slots(ParaId::from(1_u32)), None);

			assert_ok!(AssignedSlots::assign_temp_parachain_slot(
				RuntimeOrigin::root(),
				ParaId::from(1_u32),
				SlotLeasePeriodStart::Current
			));
			assert_eq!(AssignedSlots::temporary_slot_count(), 1);
			assert_eq!(AssignedSlots::active_temporary_slot_count(), 1);

			// Block 1-5
			// Para is a parachain for TemporarySlotLeasePeriodLength * LeasePeriod blocks
			while block < 6 {
				println!("block #{}", block);
				println!("lease period #{}", AssignedSlots::current_lease_period_index());
				println!("lease {:?}", Slots::lease(ParaId::from(1_u32)));

				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), true);

				assert_eq!(AssignedSlots::has_temporary_slot(ParaId::from(1_u32)), true);
				assert_eq!(AssignedSlots::active_temporary_slot_count(), 1);
				assert_eq!(
					AssignedSlots::temporary_slots(ParaId::from(1_u32)),
					Some(ParachainTemporarySlot {
						manager: 1,
						period_begin: 0,
						period_count: 2, // TemporarySlotLeasePeriodLength
						last_lease: Some(0),
						lease_count: 1
					})
				);

				assert_eq!(Slots::already_leased(ParaId::from(1_u32), 0, 1), true);

				block += 1;
				run_to_block(block);
			}

			// Block 6
			println!("block #{}", block);
			println!("lease period #{}", AssignedSlots::current_lease_period_index());
			println!("lease {:?}", Slots::lease(ParaId::from(1_u32)));

			// Para lease ended, downgraded back to parathread
			assert_eq!(TestRegistrar::<Test>::is_parathread(ParaId::from(1_u32)), true);
			assert_eq!(Slots::already_leased(ParaId::from(1_u32), 0, 3), false);
			assert_eq!(AssignedSlots::active_temporary_slot_count(), 0);

			// Block 12
			// Para should get a turn after TemporarySlotLeasePeriodLength * LeasePeriod blocks
			run_to_block(12);
			println!("block #{}", block);
			println!("lease period #{}", AssignedSlots::current_lease_period_index());
			println!("lease {:?}", Slots::lease(ParaId::from(1_u32)));

			assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), true);
			assert_eq!(Slots::already_leased(ParaId::from(1_u32), 4, 5), true);
			assert_eq!(AssignedSlots::active_temporary_slot_count(), 1);
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
					dummy_head_data(),
					dummy_validation_code()
				));

				assert_ok!(AssignedSlots::assign_temp_parachain_slot(
					RuntimeOrigin::root(),
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
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2_u32)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5_u32)), false);
				assert_eq!(AssignedSlots::active_temporary_slot_count(), 2);
			}

			// Block 6-11, Period 2-3
			for n in 6..=11 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3_u32)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5_u32)), false);
				assert_eq!(AssignedSlots::active_temporary_slot_count(), 2);
			}

			// Block 12-17, Period 4-5
			for n in 12..=17 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4_u32)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5_u32)), true);
				assert_eq!(AssignedSlots::active_temporary_slot_count(), 2);
			}

			// Block 18-23, Period 6-7
			for n in 18..=23 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2_u32)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5_u32)), false);
				assert_eq!(AssignedSlots::active_temporary_slot_count(), 2);
			}

			// Block 24-29, Period 8-9
			for n in 24..=29 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3_u32)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5_u32)), false);
				assert_eq!(AssignedSlots::active_temporary_slot_count(), 2);
			}

			// Block 30-35, Period 10-11
			for n in 30..=35 {
				run_to_block(n);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(0)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(2_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(3_u32)), false);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(4_u32)), true);
				assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(5_u32)), true);
				assert_eq!(AssignedSlots::active_temporary_slot_count(), 2);
			}
		});
	}

	#[test]
	fn unassign_slot_fails_for_unknown_para() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::unassign_parachain_slot(RuntimeOrigin::root(), ParaId::from(1_u32),),
				Error::<Test>::SlotNotAssigned
			);
		});
	}

	#[test]
	fn unassign_slot_fails_for_invalid_origin() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(
				AssignedSlots::assign_perm_parachain_slot(
					RuntimeOrigin::signed(1),
					ParaId::from(1_u32),
				),
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
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			assert_ok!(AssignedSlots::assign_perm_parachain_slot(
				RuntimeOrigin::root(),
				ParaId::from(1_u32),
			));

			assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), true);

			assert_ok!(AssignedSlots::unassign_parachain_slot(
				RuntimeOrigin::root(),
				ParaId::from(1_u32),
			));

			assert_eq!(AssignedSlots::permanent_slot_count(), 0);
			assert_eq!(AssignedSlots::has_permanent_slot(ParaId::from(1_u32)), false);
			assert_eq!(AssignedSlots::permanent_slots(ParaId::from(1_u32)), None);

			assert_eq!(Slots::already_leased(ParaId::from(1_u32), 0, 2), false);
		});
	}

	#[test]
	fn unassign_temp_slot_succeeds() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(TestRegistrar::<Test>::register(
				1,
				ParaId::from(1_u32),
				dummy_head_data(),
				dummy_validation_code(),
			));

			assert_ok!(AssignedSlots::assign_temp_parachain_slot(
				RuntimeOrigin::root(),
				ParaId::from(1_u32),
				SlotLeasePeriodStart::Current
			));

			assert_eq!(TestRegistrar::<Test>::is_parachain(ParaId::from(1_u32)), true);

			assert_ok!(AssignedSlots::unassign_parachain_slot(
				RuntimeOrigin::root(),
				ParaId::from(1_u32),
			));

			assert_eq!(AssignedSlots::temporary_slot_count(), 0);
			assert_eq!(AssignedSlots::active_temporary_slot_count(), 0);
			assert_eq!(AssignedSlots::has_temporary_slot(ParaId::from(1_u32)), false);
			assert_eq!(AssignedSlots::temporary_slots(ParaId::from(1_u32)), None);

			assert_eq!(Slots::already_leased(ParaId::from(1_u32), 0, 1), false);
		});
	}
}
