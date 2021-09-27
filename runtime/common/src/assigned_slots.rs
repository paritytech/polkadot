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
	traits::{Leaser, Registrar},
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
				PermanentSlotCount::<T>::get() + 1 < T::MaxPermanentSlots::get(),
				Error::<T>::MaxPermanentSlotsExceeded
			);

			<PermanentSlotCount<T>>::try_mutate(|count| -> DispatchResult {
				Self::configure_slot_lease(
					id,
					manager,
					current_lease_period,
					T::PermanentSlotLeasePeriodLength::get().into(),
				)?;
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
					Some(last_lease) => {
						// Slot w/ past lease, only consider it every other slot lease period (times period_count)
						if last_lease <=
							lease_period_index - (slot.period_count.saturating_mul(2u32.into()))
						{
							pending_temp_slots.push((para, slot));
						}
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
			pending_temp_slots.sort_unstable_by_key(|(_id, s)| s.lease_count);

			let slots_to_be_upgraded = pending_temp_slots
				.iter()
				.take((T::MaxTemporarySlotPerLeasePeriod::get() - active_temp_slots) as usize)
				.collect::<Vec<_>>();

			for (id, temp_slot) in slots_to_be_upgraded.iter() {
				TemporarySlots::<T>::try_mutate::<_, _, Error<T>, _>(id, |s| {
					// Configure temp slot lease
					T::Leaser::lease_out(
						*id,
						&temp_slot.manager,
						BalanceOf::<T>::zero(),
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
	) -> DispatchResult {
		T::Leaser::lease_out(para, &manager, BalanceOf::<T>::zero(), lease_period, lease_duration)
			.map_err(|_| Error::<T>::CannotUpgrade)?;

		Ok(())
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
