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

//! The parachain on demand assignment module.

use crate::{
	configuration, paras,
	scheduler_common::{Assignment, AssignmentProvider},
};
use frame_support::{
	pallet_prelude::*,
	traits::{
		tokens::{ExistenceRequirement, WithdrawReasons},
		Currency,
	},
};
use frame_system::pallet_prelude::*;
use primitives::{CollatorId, CoreIndex, Id as ParaId, ParathreadClaim, ParathreadEntry};
use sp_runtime::{
	traits::{CheckedAdd, CheckedSub, SaturatedConversion},
	FixedPointNumber, FixedPointOperand, FixedU128, Perbill,
};

use sp_std::{collections::vec_deque::VecDeque, prelude::*};

pub use pallet::*;

#[cfg(test)]
mod tests;

// TODO enable benchmarks maybe?
// #[cfg(feature = "runtime-benchmarks")]
// mod benchmarking;

/// Keeps track of how many assignments a scheduler currently has for a specific core index.
#[derive(Encode, Decode, Default, Clone, Copy, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
struct CoreAffinityCount {
	core_idx: CoreIndex,
	count: u32,
}

/// An indicator as to which end of the `OnDemandQueue` an assignment will be placed.
pub enum QueuePushDirection {
	Back,
	Front,
}

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

#[frame_support::pallet(dev_mode)]
pub mod pallet {

	use super::*;

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + paras::Config {
		/// The runtime's definition of an event.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// The runtime's definition of a Currency.
		type Currency: Currency<Self::AccountId>;

		/// The default value for the traffic multiplier.
		#[pallet::constant]
		type TrafficDefaultValue: Get<FixedU128>;
	}

	#[pallet::type_value]
	pub fn SpotTrafficOnEmpty<T: Config>() -> FixedU128 {
		T::TrafficDefaultValue::get()
	}

	#[pallet::type_value]
	pub fn OnDemandQueueOnEmpty<T: Config>() -> VecDeque<ParathreadEntry> {
		VecDeque::new()
	}

	/// Keeps track of the multiplier used to calculate the current spot price for the on demand assigner.
	#[pallet::storage]
	pub(super) type SpotTraffic<T: Config> =
		StorageValue<_, FixedU128, ValueQuery, SpotTrafficOnEmpty<T>>;

	/// The order storage entry. Uses a VecDeque to be able to push to the front of the
	/// queue from the scheduler on session boundaries.
	#[pallet::storage]
	pub type OnDemandQueue<T: Config> =
		StorageValue<_, VecDeque<ParathreadEntry>, ValueQuery, OnDemandQueueOnEmpty<T>>;

	/// Maps a `ParaId` to `CoreIndex` and keeps track of how many assignments the scheduler has in it's
	/// lookahead. Keeping track of this affinity prevents parallel execution of two or more `ParaId`s on different
	/// `CoreIndex`es.
	#[pallet::storage]
	pub(super) type ParaIdAffinity<T: Config> =
		StorageMap<_, Twox256, ParaId, CoreAffinityCount, OptionQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		OnDemandOrderPlaced { spot_price: BalanceOf<T> },
		SpotTrafficSet { traffic: FixedU128 },
	}

	#[pallet::error]
	pub enum Error<T> {
		InsufficientFunds,
		InvalidParaId,
		QueueFull,
		SpotBaseFeeNotAvailable,
		SpotPriceHigherThanMaxAmount,
		SpotTrafficNotAvailable,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_now: T::BlockNumber) -> Weight {
			let config = <configuration::Pallet<T>>::config();
			// Calculate spot price multiplier and store it.
			let traffic = SpotTraffic::<T>::get();
			let spot_traffic = Self::calculate_spot_traffic(
				traffic,
				config.on_demand_queue_max_size,
				Self::queue_size(),
				config.on_demand_target_queue_utilization,
				config.on_demand_fee_variability,
			);

			if let Some(new_traffic) = spot_traffic {
				SpotTraffic::<T>::set(new_traffic);
				Pallet::<T>::deposit_event(Event::<T>::SpotTrafficSet { traffic: new_traffic })
			}
			//TODO this has some weight
			Weight::zero()
		}

		fn on_finalize(_now: T::BlockNumber) {}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Create a single on demand core order.
		/// Will use the spot price for the current block and can be set to reap the account if needed.
		///
		/// Parameters:
		/// - `origin`: The sender of the call, funds will be withdrawn from this account.
		/// - `max_amount`: The maximum balance to withdraw from the origin to place an order.
		/// - `para_id`: The parathread id that should be added to the queue.
		///
		/// Errors:
		/// - `InsufficientFunds`
		/// - `SpotTrafficNotAvailable`
		///
		/// Events:
		/// - `SpotOrderPlaced`
		// TODO weights
		#[pallet::call_index(0)]
		#[pallet::weight(1_000)]
		pub fn place_order(
			origin: OriginFor<T>,
			max_amount: BalanceOf<T>,
			para_id: ParaId,
			scheduled_collator: Option<CollatorId>,
			keep_alive: bool,
		) -> DispatchResult {
			// Is tx signed
			let sender = ensure_signed(origin)?;

			// Traffic always falls back to 1.0
			let traffic = SpotTraffic::<T>::get();

			let config = <configuration::Pallet<T>>::config();

			// Calculate spot price
			let spot_price: BalanceOf<T> = traffic
				.saturating_mul_int(config.on_demand_base_fee.saturated_into::<BalanceOf<T>>());

			// Is the current price higher than `max_amount`
			ensure!(spot_price.le(&max_amount), Error::<T>::SpotPriceHigherThanMaxAmount);

			let existence_requirement = match keep_alive {
				true => ExistenceRequirement::KeepAlive,
				false => ExistenceRequirement::AllowDeath,
			};

			// Charge the sending account the spot price
			T::Currency::withdraw(
				&sender,
				spot_price,
				WithdrawReasons::FEE,
				existence_requirement,
			)?;

			let entry =
				ParathreadEntry { claim: ParathreadClaim(para_id, scheduled_collator), retries: 0 };

			let res = Pallet::<T>::add_parathread_entry(entry, QueuePushDirection::Back);

			match res {
				Ok(_) => {
					Pallet::<T>::deposit_event(Event::<T>::OnDemandOrderPlaced { spot_price });
					return Ok(())
				},
				Err(err) => return Err(err),
			}
		}
	}
}

impl<T: Config> Pallet<T>
where
	BalanceOf<T>: FixedPointOperand,
{
	/// The spot price multiplier. This is based on the transaction fee calculations defined in:
	/// https://research.web3.foundation/en/latest/polkadot/overview/2-token-economics.html#setting-transaction-fees
	///
	/// Returns:
	/// - An `Option<FixedU128>` in the range of 1.0 - `FixedU128::MAX`
	/// Parameters:
	/// - `traffic`: The previously calculated multiplier, can never go below 1.0.
	/// - `queue_capacity`: The max size of the order book.
	/// - `queue_size`: How many orders are currently in the order book.
	/// - `target_queue_utilisation`: How much of the queue_capacity should be ideally occupied, expressed in percentages(perbill).
	/// - `variability`: A variability factor, i.e. how quickly the spot price adjusts. This number can be chosen by
	///                  p/(k*(1-s)) where p is the desired ratio increase in spot price over k number of blocks.
	///                  s is the target_queue_utilisation. A concrete example: v = 0.05/(20*(1-0.25)) = 0.0033.
	// TODO maybe replace with impls of Multiplier/TargetedFeeAdjustment from pallet-transaction-payment
	pub(crate) fn calculate_spot_traffic(
		traffic: FixedU128,
		queue_capacity: u32,
		queue_size: u32,
		target_queue_utilisation: Perbill,
		variability: Perbill,
	) -> Option<FixedU128> {
		if queue_capacity > 0 {
			// (queue_size / queue_capacity) - target_queue_utilisation
			let queue_util_diff =
				FixedU128::from_rational(queue_size.into(), queue_capacity.into())
					.checked_sub(&target_queue_utilisation.into())?;
			// variability * queue_util_diff
			let var_times_qud = queue_util_diff.const_checked_mul(variability.into())?;
			// variability^2 * queue_util_diff^2
			let var_times_qud_pow = var_times_qud.const_checked_mul(var_times_qud)?;
			// (variability^2 * queue_util_diff^2)/2
			let div_by_two = var_times_qud_pow.const_checked_div(2.into())?;
			// traffic * (1 + queue_util_diff) + div_by_two
			let vtq_add_one = var_times_qud.checked_add(&FixedU128::from_u32(1))?;
			let inner_add = vtq_add_one.checked_add(&div_by_two)?;
			match traffic.const_checked_mul(inner_add) {
				Some(t) => return Some(t.max(FixedU128::from_u32(1))),
				None => {},
			}
		}
		None
	}

	pub fn add_parathread_entry(
		entry: ParathreadEntry,
		location: QueuePushDirection,
	) -> Result<(), DispatchError> {
		// Only parathreads are valid paraids for on the go parachains.
		ensure!(<paras::Pallet<T>>::is_parathread(entry.claim.0), Error::<T>::InvalidParaId);

		let config = <configuration::Pallet<T>>::config();

		// Be explicit rather than implicit about the return value
		let res = OnDemandQueue::<T>::try_mutate(|queue| {
			// Abort transaction if queue is too large
			ensure!(Self::queue_size() < config.on_demand_queue_max_size, Error::<T>::QueueFull);
			match location {
				QueuePushDirection::Back => queue.push_back(entry),
				QueuePushDirection::Front => queue.push_front(entry),
			};
			Ok(())
		});
		res
	}

	/// Get the size of the on demand queue.
	/// Returns:
	/// - The size of the on demand queue, or failing to do so, the max size of the queue.
	fn queue_size() -> u32 {
		let config = <configuration::Pallet<T>>::config();
		match OnDemandQueue::<T>::get().len().try_into() {
			Ok(size) => return size,
			Err(_) => return config.on_demand_queue_max_size,
		}
	}

	/// Getter for the order queue.
	pub fn get_queue() -> Vec<ParathreadEntry> {
		OnDemandQueue::<T>::get().into()
	}

	/// Decreases the affinity of a `ParaId` to a specified `CoreIndex`.
	/// Subtracts from the count of the `CoreAffinityCount` if an entry is found and the core_idx matches.
	/// When the count reaches 0, the entry is removed.
	/// A non-existant entry is a no-op.
	fn decrease_affinity(para_id: ParaId, core_idx: CoreIndex) {
		match ParaIdAffinity::<T>::get(para_id) {
			Some(affinity) =>
				if affinity.core_idx == core_idx {
					let new_count = affinity.count.saturating_sub(1);
					if new_count > 0 {
						ParaIdAffinity::<T>::insert(
							para_id,
							CoreAffinityCount { core_idx, count: new_count },
						)
					} else {
						ParaIdAffinity::<T>::remove(para_id)
					}
				},
			None => {},
		}
	}

	/// Increases the affinity of a `ParaId` to a specified `CoreIndex`.
	/// Adds to the count of the `CoreAffinityCount` if an entry is found and the core_idx matches.
	/// A non-existant entry will be initialized with a count of 1 and uses the  supplied `CoreIndex`.
	fn increase_affinity(para_id: ParaId, core_idx: CoreIndex) -> bool {
		match ParaIdAffinity::<T>::get(para_id) {
			Some(affinity) => {
				if affinity.core_idx == core_idx {
					let new_count = affinity.count.saturating_add(1);
					ParaIdAffinity::<T>::insert(
						para_id,
						CoreAffinityCount { core_idx, count: new_count },
					);
					return true
				}
				return false
			},
			None => {
				ParaIdAffinity::<T>::insert(para_id, CoreAffinityCount { core_idx, count: 1 });
				return true
			},
		}
	}
}

impl<T: Config> AssignmentProvider<T::BlockNumber> for Pallet<T> {
	fn session_core_count() -> u32 {
		let config = <configuration::Pallet<T>>::config();
		config.parathread_cores
	}

	fn new_session() {}

	/// Take the next queued entry that is available for a given core index.
	/// Parameters:
	/// - `core_idx`: The core index
	/// - `previous_paraid`: Which paraid was previously processed on the requested core.
	///    Is None if nothing was processed on the core.
	fn pop_assignment_for_core(
		core_idx: CoreIndex,
		previous_para: Option<ParaId>,
	) -> Option<Assignment> {
		// Only decrease the affinity of the previous para if it exists.
		// A nonexistant `ParaId` indicates that the scheduler has not processed any
		// `ParaId` this session.
		if let Some(previous_para_id) = previous_para {
			Pallet::<T>::decrease_affinity(previous_para_id, core_idx)
		}

		let mut queue: VecDeque<ParathreadEntry> = OnDemandQueue::<T>::get();

		// Get the position of the next `ParaId`. Select either a `ParaId` that has an affinity
		// to the same `CoreIndex` as the scheduler asks for or a `ParaId` with no affinity at all.
		let pos = queue.iter().position(|entry| match ParaIdAffinity::<T>::get(&entry.claim.0) {
			Some(affinity) => return affinity.core_idx == core_idx,
			None => return true,
		});

		pos.and_then(|p: usize| {
			if let Some(entry) = queue.remove(p) {
				Pallet::<T>::increase_affinity(entry.claim.0, core_idx);
				// Write changes to storage.
				OnDemandQueue::<T>::set(queue);
				return Some(Assignment::ParathreadA(entry))
			};
			return None
		})
	}

	/// Push an assignment back to the queue.
	/// Typically used on session boundaries.
	/// Parameters:
	/// - `core_idx`: The core index
	/// - `assignment`: The on demand assignment.
	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment) {
		match assignment {
			Assignment::ParathreadA(entry) => {
				Pallet::<T>::decrease_affinity(entry.claim.0, core_idx);
				// Skip the queue on push backs from scheduler
				match Pallet::<T>::add_parathread_entry(entry, QueuePushDirection::Front) {
					Ok(_) => {},
					Err(_) => {},
				}
			},
			Assignment::Parachain(_para_id) => {
				// We should never end up here
			},
		}
	}

	fn get_availability_period(_core_index: CoreIndex) -> T::BlockNumber {
		let config = <configuration::Pallet<T>>::config();
		config.thread_availability_period
	}

	fn get_max_retries(_core_idx: CoreIndex) -> u32 {
		let config = <configuration::Pallet<T>>::config();
		config.parathread_retries
	}
}
