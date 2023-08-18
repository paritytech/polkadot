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
//!
//! Implements a mechanism for taking in orders for pay as you go (PAYG) or on demand
//! parachain (previously parathreads) assignments. This module is not handled by the
//! initializer but is instead instantiated in the `construct_runtime` macro.
//!
//! The module currently limits parallel execution of blocks from the same `ParaId` via
//! a core affinity mechanism. As long as there exists an affinity for a `CoreIndex` for
//! a specific `ParaId`, orders for blockspace for that `ParaId` will only be assigned to
//! that `CoreIndex`. This affinity mechanism can be removed if it can be shown that parallel
//! execution is valid.

mod benchmarking;
mod mock_helpers;

#[cfg(test)]
mod tests;

use crate::{
	configuration, paras,
	scheduler::common::{AssignmentProvider, AssignmentProviderConfig},
};

use frame_support::{
	pallet_prelude::*,
	traits::{
		Currency,
		ExistenceRequirement::{self, AllowDeath, KeepAlive},
		WithdrawReasons,
	},
};
use frame_system::pallet_prelude::*;
use primitives::{v5::Assignment, CoreIndex, Id as ParaId};
use sp_runtime::{
	traits::{One, SaturatedConversion},
	FixedPointNumber, FixedPointOperand, FixedU128, Perbill, Saturating,
};

use sp_std::{collections::vec_deque::VecDeque, prelude::*};

const LOG_TARGET: &str = "runtime::parachains::assigner-on-demand";

pub use pallet::*;

pub trait WeightInfo {
	fn place_order_allow_death(s: u32) -> Weight;
	fn place_order_keep_alive(s: u32) -> Weight;
}

/// A weight info that is only suitable for testing.
pub struct TestWeightInfo;

impl WeightInfo for TestWeightInfo {
	fn place_order_allow_death(_: u32) -> Weight {
		Weight::MAX
	}

	fn place_order_keep_alive(_: u32) -> Weight {
		Weight::MAX
	}
}

/// Keeps track of how many assignments a scheduler currently has at a specific `CoreIndex` for a
/// specific `ParaId`.
#[derive(Encode, Decode, Default, Clone, Copy, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct CoreAffinityCount {
	core_idx: CoreIndex,
	count: u32,
}

/// An indicator as to which end of the `OnDemandQueue` an assignment will be placed.
pub enum QueuePushDirection {
	Back,
	Front,
}

/// Shorthand for the Balance type the runtime is using.
type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

/// Errors that can happen during spot traffic calculation.
#[derive(PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum SpotTrafficCalculationErr {
	/// The order queue capacity is at 0.
	QueueCapacityIsZero,
	/// The queue size is larger than the queue capacity.
	QueueSizeLargerThanCapacity,
	/// Arithmetic error during division, either division by 0 or over/underflow.
	Division,
}

#[frame_support::pallet]
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

		/// Something that provides the weight of this pallet.
		type WeightInfo: WeightInfo;

		/// The default value for the spot traffic multiplier.
		#[pallet::constant]
		type TrafficDefaultValue: Get<FixedU128>;
	}

	/// Creates an empty spot traffic value if one isn't present in storage already.
	#[pallet::type_value]
	pub fn SpotTrafficOnEmpty<T: Config>() -> FixedU128 {
		T::TrafficDefaultValue::get()
	}

	/// Creates an empty on demand queue if one isn't present in storage already.
	#[pallet::type_value]
	pub fn OnDemandQueueOnEmpty<T: Config>() -> VecDeque<Assignment> {
		VecDeque::new()
	}

	/// Keeps track of the multiplier used to calculate the current spot price for the on demand
	/// assigner.
	#[pallet::storage]
	pub(super) type SpotTraffic<T: Config> =
		StorageValue<_, FixedU128, ValueQuery, SpotTrafficOnEmpty<T>>;

	/// The order storage entry. Uses a VecDeque to be able to push to the front of the
	/// queue from the scheduler on session boundaries.
	#[pallet::storage]
	pub type OnDemandQueue<T: Config> =
		StorageValue<_, VecDeque<Assignment>, ValueQuery, OnDemandQueueOnEmpty<T>>;

	/// Maps a `ParaId` to `CoreIndex` and keeps track of how many assignments the scheduler has in
	/// it's lookahead. Keeping track of this affinity prevents parallel execution of the same
	/// `ParaId` on two or more `CoreIndex`es.
	#[pallet::storage]
	pub(super) type ParaIdAffinity<T: Config> =
		StorageMap<_, Twox256, ParaId, CoreAffinityCount, OptionQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// An order was placed at some spot price amount.
		OnDemandOrderPlaced { para_id: ParaId, spot_price: BalanceOf<T> },
		/// The value of the spot traffic multiplier changed.
		SpotTrafficSet { traffic: FixedU128 },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The `ParaId` supplied to the `place_order` call is not a valid `ParaThread`, making the
		/// call is invalid.
		InvalidParaId,
		/// The order queue is full, `place_order` will not continue.
		QueueFull,
		/// The current spot price is higher than the max amount specified in the `place_order`
		/// call, making it invalid.
		SpotPriceHigherThanMaxAmount,
		/// There are no on demand cores available. `place_order` will not add anything to the
		/// queue.
		NoOnDemandCores,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_now: BlockNumberFor<T>) -> Weight {
			let config = <configuration::Pallet<T>>::config();
			// Calculate spot price multiplier and store it.
			let old_traffic = SpotTraffic::<T>::get();
			match Self::calculate_spot_traffic(
				old_traffic,
				config.on_demand_queue_max_size,
				Self::queue_size(),
				config.on_demand_target_queue_utilization,
				config.on_demand_fee_variability,
			) {
				Ok(new_traffic) => {
					// Only update storage on change
					if new_traffic != old_traffic {
						SpotTraffic::<T>::set(new_traffic);
						Pallet::<T>::deposit_event(Event::<T>::SpotTrafficSet {
							traffic: new_traffic,
						});
						return T::DbWeight::get().reads_writes(2, 1)
					}
				},
				Err(SpotTrafficCalculationErr::QueueCapacityIsZero) => {
					log::debug!(
						target: LOG_TARGET,
						"Error calculating spot traffic: The order queue capacity is at 0."
					);
				},
				Err(SpotTrafficCalculationErr::QueueSizeLargerThanCapacity) => {
					log::debug!(
						target: LOG_TARGET,
						"Error calculating spot traffic: The queue size is larger than the queue capacity."
					);
				},
				Err(SpotTrafficCalculationErr::Division) => {
					log::debug!(
						target: LOG_TARGET,
						"Error calculating spot traffic: Arithmetic error during division, either division by 0 or over/underflow."
					);
				},
			};
			T::DbWeight::get().reads_writes(2, 0)
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Create a single on demand core order.
		/// Will use the spot price for the current block and will reap the account if needed.
		///
		/// Parameters:
		/// - `origin`: The sender of the call, funds will be withdrawn from this account.
		/// - `max_amount`: The maximum balance to withdraw from the origin to place an order.
		/// - `para_id`: A `ParaId` the origin wants to provide blockspace for.
		///
		/// Errors:
		/// - `InsufficientBalance`: from the Currency implementation
		/// - `InvalidParaId`
		/// - `QueueFull`
		/// - `SpotPriceHigherThanMaxAmount`
		/// - `NoOnDemandCores`
		///
		/// Events:
		/// - `SpotOrderPlaced`
		#[pallet::call_index(0)]
		#[pallet::weight(<T as Config>::WeightInfo::place_order_allow_death(OnDemandQueue::<T>::get().len() as u32))]
		pub fn place_order_allow_death(
			origin: OriginFor<T>,
			max_amount: BalanceOf<T>,
			para_id: ParaId,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			Pallet::<T>::do_place_order(sender, max_amount, para_id, AllowDeath)
		}

		/// Same as the [`place_order_allow_death`] call , but with a check that placing the order
		/// will not reap the account.
		///
		/// Parameters:
		/// - `origin`: The sender of the call, funds will be withdrawn from this account.
		/// - `max_amount`: The maximum balance to withdraw from the origin to place an order.
		/// - `para_id`: A `ParaId` the origin wants to provide blockspace for.
		///
		/// Errors:
		/// - `InsufficientBalance`: from the Currency implementation
		/// - `InvalidParaId`
		/// - `QueueFull`
		/// - `SpotPriceHigherThanMaxAmount`
		/// - `NoOnDemandCores`
		///
		/// Events:
		/// - `SpotOrderPlaced`
		#[pallet::call_index(1)]
		#[pallet::weight(<T as Config>::WeightInfo::place_order_keep_alive(OnDemandQueue::<T>::get().len() as u32))]
		pub fn place_order_keep_alive(
			origin: OriginFor<T>,
			max_amount: BalanceOf<T>,
			para_id: ParaId,
		) -> DispatchResult {
			let sender = ensure_signed(origin)?;
			Pallet::<T>::do_place_order(sender, max_amount, para_id, KeepAlive)
		}
	}
}

impl<T: Config> Pallet<T>
where
	BalanceOf<T>: FixedPointOperand,
{
	/// Helper function for `place_order_*` calls. Used to differentiate between placing orders
	/// with a keep alive check or to allow the account to be reaped.
	///
	/// Parameters:
	/// - `sender`: The sender of the call, funds will be withdrawn from this account.
	/// - `max_amount`: The maximum balance to withdraw from the origin to place an order.
	/// - `para_id`: A `ParaId` the origin wants to provide blockspace for.
	/// - `existence_requirement`: Whether or not to ensure that the account will not be reaped.
	///
	/// Errors:
	/// - `InsufficientBalance`: from the Currency implementation
	/// - `InvalidParaId`
	/// - `QueueFull`
	/// - `SpotPriceHigherThanMaxAmount`
	/// - `NoOnDemandCores`
	///
	/// Events:
	/// - `SpotOrderPlaced`
	fn do_place_order(
		sender: <T as frame_system::Config>::AccountId,
		max_amount: BalanceOf<T>,
		para_id: ParaId,
		existence_requirement: ExistenceRequirement,
	) -> DispatchResult {
		let config = <configuration::Pallet<T>>::config();

		// Are there any schedulable cores in this session
		ensure!(config.on_demand_cores > 0, Error::<T>::NoOnDemandCores);

		// Traffic always falls back to 1.0
		let traffic = SpotTraffic::<T>::get();

		// Calculate spot price
		let spot_price: BalanceOf<T> =
			traffic.saturating_mul_int(config.on_demand_base_fee.saturated_into::<BalanceOf<T>>());

		// Is the current price higher than `max_amount`
		ensure!(spot_price.le(&max_amount), Error::<T>::SpotPriceHigherThanMaxAmount);

		// Charge the sending account the spot price
		T::Currency::withdraw(&sender, spot_price, WithdrawReasons::FEE, existence_requirement)?;

		let assignment = Assignment::new(para_id);

		let res = Pallet::<T>::add_on_demand_assignment(assignment, QueuePushDirection::Back);

		match res {
			Ok(_) => {
				Pallet::<T>::deposit_event(Event::<T>::OnDemandOrderPlaced { para_id, spot_price });
				return Ok(())
			},
			Err(err) => return Err(err),
		}
	}

	/// The spot price multiplier. This is based on the transaction fee calculations defined in:
	/// https://research.web3.foundation/Polkadot/overview/token-economics#setting-transaction-fees
	///
	/// Parameters:
	/// - `traffic`: The previously calculated multiplier, can never go below 1.0.
	/// - `queue_capacity`: The max size of the order book.
	/// - `queue_size`: How many orders are currently in the order book.
	/// - `target_queue_utilisation`: How much of the queue_capacity should be ideally occupied,
	///   expressed in percentages(perbill).
	/// - `variability`: A variability factor, i.e. how quickly the spot price adjusts. This number
	///   can be chosen by p/(k*(1-s)) where p is the desired ratio increase in spot price over k
	///   number of blocks. s is the target_queue_utilisation. A concrete example: v =
	///   0.05/(20*(1-0.25)) = 0.0033.
	///
	/// Returns:
	/// - A `FixedU128` in the range of  `Config::TrafficDefaultValue` - `FixedU128::MAX` on
	///   success.
	///
	/// Errors:
	/// - `SpotTrafficCalculationErr::QueueCapacityIsZero`
	/// - `SpotTrafficCalculationErr::QueueSizeLargerThanCapacity`
	/// - `SpotTrafficCalculationErr::Division`
	pub(crate) fn calculate_spot_traffic(
		traffic: FixedU128,
		queue_capacity: u32,
		queue_size: u32,
		target_queue_utilisation: Perbill,
		variability: Perbill,
	) -> Result<FixedU128, SpotTrafficCalculationErr> {
		// Return early if queue has no capacity.
		if queue_capacity == 0 {
			return Err(SpotTrafficCalculationErr::QueueCapacityIsZero)
		}

		// Return early if queue size is greater than capacity.
		if queue_size > queue_capacity {
			return Err(SpotTrafficCalculationErr::QueueSizeLargerThanCapacity)
		}

		// (queue_size / queue_capacity) - target_queue_utilisation
		let queue_util_ratio = FixedU128::from_rational(queue_size.into(), queue_capacity.into());
		let positive = queue_util_ratio >= target_queue_utilisation.into();
		let queue_util_diff = queue_util_ratio.max(target_queue_utilisation.into()) -
			queue_util_ratio.min(target_queue_utilisation.into());

		// variability * queue_util_diff
		let var_times_qud = queue_util_diff.saturating_mul(variability.into());

		// variability^2 * queue_util_diff^2
		let var_times_qud_pow = var_times_qud.saturating_mul(var_times_qud);

		// (variability^2 * queue_util_diff^2)/2
		let div_by_two: FixedU128;
		match var_times_qud_pow.const_checked_div(2.into()) {
			Some(dbt) => div_by_two = dbt,
			None => return Err(SpotTrafficCalculationErr::Division),
		}

		// traffic * (1 + queue_util_diff) + div_by_two
		if positive {
			let new_traffic = queue_util_diff
				.saturating_add(div_by_two)
				.saturating_add(One::one())
				.saturating_mul(traffic);
			Ok(new_traffic.max(<T as Config>::TrafficDefaultValue::get()))
		} else {
			let new_traffic = queue_util_diff.saturating_sub(div_by_two).saturating_mul(traffic);
			Ok(new_traffic.max(<T as Config>::TrafficDefaultValue::get()))
		}
	}

	/// Adds an assignment to the on demand queue.
	///
	/// Paramenters:
	/// - `assignment`: The on demand assignment to add to the queue.
	/// - `location`: Whether to push this entry to the back or the front of the queue. Pushing an
	///   entry to the front of the queue is only used when the scheduler wants to push back an
	///   entry it has already popped.
	/// Returns:
	/// - The unit type on success.
	///
	/// Errors:
	/// - `InvalidParaId`
	/// - `QueueFull`
	pub fn add_on_demand_assignment(
		assignment: Assignment,
		location: QueuePushDirection,
	) -> Result<(), DispatchError> {
		// Only parathreads are valid paraids for on the go parachains.
		ensure!(<paras::Pallet<T>>::is_parathread(assignment.para_id), Error::<T>::InvalidParaId);

		let config = <configuration::Pallet<T>>::config();

		OnDemandQueue::<T>::try_mutate(|queue| {
			// Abort transaction if queue is too large
			ensure!(Self::queue_size() < config.on_demand_queue_max_size, Error::<T>::QueueFull);
			match location {
				QueuePushDirection::Back => queue.push_back(assignment),
				QueuePushDirection::Front => queue.push_front(assignment),
			};
			Ok(())
		})
	}

	/// Get the size of the on demand queue.
	///
	/// Returns:
	/// - The size of the on demand queue.
	fn queue_size() -> u32 {
		let config = <configuration::Pallet<T>>::config();
		match OnDemandQueue::<T>::get().len().try_into() {
			Ok(size) => return size,
			Err(_) => {
				log::debug!(
					target: LOG_TARGET,
					"Failed to fetch the on demand queue size, returning the max size."
				);
				return config.on_demand_queue_max_size
			},
		}
	}

	/// Getter for the order queue.
	pub fn get_queue() -> VecDeque<Assignment> {
		OnDemandQueue::<T>::get()
	}

	/// Getter for the affinity tracker.
	pub fn get_affinity_map(para_id: ParaId) -> Option<CoreAffinityCount> {
		ParaIdAffinity::<T>::get(para_id)
	}

	/// Decreases the affinity of a `ParaId` to a specified `CoreIndex`.
	/// Subtracts from the count of the `CoreAffinityCount` if an entry is found and the core_idx
	/// matches. When the count reaches 0, the entry is removed.
	/// A non-existant entry is a no-op.
	fn decrease_affinity(para_id: ParaId, core_idx: CoreIndex) {
		ParaIdAffinity::<T>::mutate(para_id, |maybe_affinity| {
			if let Some(affinity) = maybe_affinity {
				if affinity.core_idx == core_idx {
					let new_count = affinity.count.saturating_sub(1);
					if new_count > 0 {
						*maybe_affinity = Some(CoreAffinityCount { core_idx, count: new_count });
					} else {
						*maybe_affinity = None;
					}
				}
			}
		});
	}

	/// Increases the affinity of a `ParaId` to a specified `CoreIndex`.
	/// Adds to the count of the `CoreAffinityCount` if an entry is found and the core_idx matches.
	/// A non-existant entry will be initialized with a count of 1 and uses the  supplied
	/// `CoreIndex`.
	fn increase_affinity(para_id: ParaId, core_idx: CoreIndex) {
		ParaIdAffinity::<T>::mutate(para_id, |maybe_affinity| match maybe_affinity {
			Some(affinity) =>
				if affinity.core_idx == core_idx {
					*maybe_affinity = Some(CoreAffinityCount {
						core_idx,
						count: affinity.count.saturating_add(1),
					});
				},
			None => {
				*maybe_affinity = Some(CoreAffinityCount { core_idx, count: 1 });
			},
		})
	}
}

impl<T: Config> AssignmentProvider<BlockNumberFor<T>> for Pallet<T> {
	fn session_core_count() -> u32 {
		let config = <configuration::Pallet<T>>::config();
		config.on_demand_cores
	}

	/// Take the next queued entry that is available for a given core index.
	/// Invalidates and removes orders with a `para_id` that is not `ParaLifecycle::Parathread`
	/// but only in [0..P] range slice of the order queue, where P is the element that is
	/// removed from the order queue.
	///
	/// Parameters:
	/// - `core_idx`: The core index
	/// - `previous_paraid`: Which paraid was previously processed on the requested core. Is None if
	///   nothing was processed on the core.
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

		let mut queue: VecDeque<Assignment> = OnDemandQueue::<T>::get();

		let mut invalidated_para_id_indexes: Vec<usize> = vec![];

		// Get the position of the next `ParaId`. Select either a valid `ParaId` that has an
		// affinity to the same `CoreIndex` as the scheduler asks for or a valid `ParaId` with no
		// affinity at all.
		let pos = queue.iter().enumerate().position(|(index, assignment)| {
			if <paras::Pallet<T>>::is_parathread(assignment.para_id) {
				match ParaIdAffinity::<T>::get(&assignment.para_id) {
					Some(affinity) => return affinity.core_idx == core_idx,
					None => return true,
				}
			}
			// Record no longer valid para_ids.
			invalidated_para_id_indexes.push(index);
			return false
		});

		// Collect the popped value.
		let popped = pos.and_then(|p: usize| {
			if let Some(assignment) = queue.remove(p) {
				Pallet::<T>::increase_affinity(assignment.para_id, core_idx);
				return Some(assignment)
			};
			None
		});

		// Only remove the invalid indexes *after* using the index.
		// Removed in reverse order so that the indexes don't shift.
		invalidated_para_id_indexes.iter().rev().for_each(|idx| {
			queue.remove(*idx);
		});

		// Write changes to storage.
		OnDemandQueue::<T>::set(queue);

		popped
	}

	/// Push an assignment back to the queue.
	/// Typically used on session boundaries.
	/// Parameters:
	/// - `core_idx`: The core index
	/// - `assignment`: The on demand assignment.
	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment) {
		Pallet::<T>::decrease_affinity(assignment.para_id, core_idx);
		// Skip the queue on push backs from scheduler
		match Pallet::<T>::add_on_demand_assignment(assignment, QueuePushDirection::Front) {
			Ok(_) => {},
			Err(_) => {},
		}
	}

	fn get_provider_config(_core_idx: CoreIndex) -> AssignmentProviderConfig<BlockNumberFor<T>> {
		let config = <configuration::Pallet<T>>::config();
		AssignmentProviderConfig {
			availability_period: config.paras_availability_period,
			max_availability_timeouts: config.on_demand_retries,
			ttl: config.on_demand_ttl,
		}
	}
}
