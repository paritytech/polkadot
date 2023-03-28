// Copyright 2023 Parity Technologies (UK) Ltd.
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
	configuration,
	initializer::SessionChangeNotification,
	paras,
	scheduler_common::{Assignment, AssignmentProvider},
};
use frame_support::{
	fail,
	pallet_prelude::*,
	traits::{
		tokens::{ExistenceRequirement, WithdrawReasons},
		Currency,
	},
};
use frame_system::pallet_prelude::*;
use primitives::{CollatorId, CoreIndex, Id as ParaId, ParathreadClaim};
use sp_runtime::{
	traits::{CheckedAdd, CheckedSub, SaturatedConversion},
	FixedPointNumber, FixedPointOperand, FixedU128, Perbill,
};

use sp_std::prelude::*;

pub use pallet::*;

#[cfg(test)]
mod tests;

// TODO enable benchmarks maybe?
// #[cfg(feature = "runtime-benchmarks")]
// mod benchmarking;

#[derive(Encode, Decode, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
pub struct SpotClaim {
	para_id: ParaId,
	collator_id: CollatorId,
}

impl From<SpotClaim> for ParathreadClaim {
	fn from(se: SpotClaim) -> Self {
		Self(se.para_id, se.collator_id)
	}
}

impl From<ParathreadClaim> for SpotClaim {
	fn from(se: ParathreadClaim) -> Self {
		Self { para_id: se.0, collator_id: se.1 }
	}
}

/// The spot price multiplier. This is based on the transaction fee calculations defined in:
/// https://research.web3.foundation/en/latest/polkadot/overview/2-token-economics.html#setting-transaction-fees
///
/// Returns:
/// - An `Option<FixedU128>` in the range of `1.0`- `FixedU128::MAX`
/// Parameters:
/// - `traffic`: The previously calculated multiplier, can never go below 1.0.
/// - `queue_capacity`: The max size of the order book.
/// - `queue_size`: How many orders are currently in the order book.
/// - `target_queue_utilisation`: How much of the queue_capacity should be ideally occupied, expressed in percentages(perbill).
/// - `variability`: A variability factor, i.e. how quickly the spot price adjusts. This number can be chosen by
///                  p/(k*(1-s)) where p is the desired ratio increase in spot price over k number of blocks.
///                  s is the target_queue_utilisation. A concrete example: v = 0.05/(20*(1-0.25)) = 0.0033.
// TODO maybe replace with impls of Multiplier/TargetedFeeAdjustment from pallet-transaction-payment
fn calculate_spot_traffic(
	traffic: FixedU128,
	queue_capacity: u32,
	queue_size: u32,
	target_queue_utilisation: Perbill,
	variability: Perbill,
) -> Option<FixedU128> {
	if queue_capacity > 0 {
		// (queue_size / queue_capacity) - target_queue_utilisation
		let queue_util_diff = FixedU128::from_rational(queue_size.into(), queue_capacity.into())
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


#[derive(Encode, Decode, Default, Clone, Copy, TypeInfo)]
#[cfg_attr(test, derive(PartialEq, Debug))]
struct CoreAffinityCount {
	core_idx: CoreIndex,
	count: u32,
}

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

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

		/// The number of paraids in the map is bounded by the number of
		/// `config.parathread_cores` * `config.scheduling_lookahead` in the worst case.
		#[pallet::constant]
		type MaxParaIdsInAffinityMap: Get<u32>;

		/// The upper limit of how many claims can be entered into storage
		#[pallet::constant]
		type MaxClaims: Get<u32>;

		#[pallet::constant]
		type MaxUpperBoundLookahead: Get<u32>;

		/// The default value for the traffic multiplier.
		#[pallet::constant]
		type TrafficDefaultValue: Get<FixedU128>;
	}

	#[pallet::type_value]
	pub fn SpotTrafficOnEmpty<T: Config>() -> FixedU128 {
		T::TrafficDefaultValue::get()
	}

	#[pallet::type_value]
	pub fn OnDemandQueueOnEmpty<T: Config>() -> BoundedVec<SpotClaim, T::MaxClaims> {
		BoundedVec::truncate_from(Vec::new())
	}

	// TODO getters are on the way out, remove this one preemptively
	#[pallet::storage]
	pub(super) type SpotTraffic<T: Config> =
		StorageValue<_, FixedU128, ValueQuery, SpotTrafficOnEmpty<T>>;

	/// The order storage. A simple bounded vec will do for now but this can be changed to a
	/// message queue at a later point if needed.
	// TODO naming is hard
	#[pallet::storage]
	pub type OnDemandQueue<T: Config> =
		StorageValue<_, BoundedVec<SpotClaim, T::MaxClaims>, ValueQuery, OnDemandQueueOnEmpty<T>>;

	#[pallet::storage]
	pub(super) type ParaIdAffinity<T: Config> =
		StorageMap<_, Twox256, ParaId, CoreAffinityCount, OptionQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		OnDemandOrderPlaced,
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

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Create a single parathread core order.
		/// Will use the current spot price for the block.
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
			scheduled_collator: CollatorId,
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

			// TODO session change para lifecycle throw away upgraded parathread

			// Charge the sending account the spot price
			// TODO should liveness be exposed to the user. i.e. should a spot price bid be able to reap the account?
			T::Currency::withdraw(
				&sender,
				spot_price,
				WithdrawReasons::FEE,
				ExistenceRequirement::KeepAlive,
			)?;

			let claim = SpotClaim { para_id, collator_id: scheduled_collator };

			let res = Pallet::<T>::add_parathread_claim(claim);

			match res {
				Ok(_) => {
					Pallet::<T>::deposit_event(Event::<T>::OnDemandOrderPlaced);
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
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		let config = <configuration::Pallet<T>>::config();
		// Calculate spot price multiplier and store it.
		let traffic = SpotTraffic::<T>::get();
		let spot_traffic = calculate_spot_traffic(
			traffic,
			config.on_demand_queue_max_size,
			Self::queue_size(),
			config.on_demand_target_queue_utilization,
			config.on_demand_fee_variability,
		);

		if let Some(new_traffic) = spot_traffic {
			SpotTraffic::<T>::set(new_traffic);
		}
		Weight::zero()
	}

	pub(crate) fn initializer_finalize() {}

	pub(crate) fn initializer_on_new_session(
		_notification: &SessionChangeNotification<T::BlockNumber>,
	) {
	}

	pub fn add_parathread_claim(claim: SpotClaim) -> Result<(), DispatchError> {
		// Only parathreads are valid paraids for on the go parachains.
		ensure!(<paras::Pallet<T>>::is_parathread(claim.para_id), Error::<T>::InvalidParaId);

		let config = <configuration::Pallet<T>>::config();

		// Be explicit rather than implicit about the return value
		let res = OnDemandQueue::<T>::try_mutate(|queue| {
			// Abort transaction if queue is too large
			ensure!(Self::queue_size() < config.on_demand_queue_max_size, Error::<T>::QueueFull);
			match queue.try_push(claim) {
				Ok(_) => return Ok(()),
				Err(_) => fail!(Error::<T>::QueueFull),
			}
		});

		res
	}

	/// Get the size of the on demand queue. Returns 0 on error.
	fn queue_size() -> u32 {
		match OnDemandQueue::<T>::get().len().try_into() {
			Ok(size) => return size,
			Err(_) => return 0,
		}
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

	/// Take the next queued claim that is available for a given core index.
	/// Parameters:
	/// - `core_idx`: The core index
	/// - `previous_paraid`: Which paraid was previously processed on the requested core. Is None if nothing was processed
	///    on the core.
	fn pop_assignment_for_core(
		core_idx: CoreIndex,
		previous_para: Option<ParaId>,
	) -> Option<Assignment> {
		// If there's no previous `ParaId` coming from the scheduler we can grab the next `ParaId` in the queue with no affinity.
		// The assumption being that this only happens when the core has never popped a claim in this session.
		match previous_para {
			Some(prev_para_id) => {
				if let Some(prev_para_affinity) = ParaIdAffinity::<T>::get(prev_para_id) {
					let mut queue: BoundedVec<SpotClaim, T::MaxClaims> = OnDemandQueue::<T>::get();
					let pos = queue.iter().position(|elem| {
						let queue_elem = ParaIdAffinity::<T>::get(&elem.para_id);
						match queue_elem {
							Some(a) => return a.core_idx == prev_para_affinity.core_idx,
							None => return true,
						}
					});

					if let Some(i) = pos {
						let claim = queue.remove(i);

						// Equivalent to decreasing the affinity if the claim `ParaId` matches the previous `ParaId`
						if claim.para_id != prev_para_id {
							Pallet::<T>::increase_affinity(claim.para_id, core_idx);
						}
						return Some(Assignment::ParathreadA(claim.into()))
					}
				}

				return None
			},
			None => {
				let mut queue: BoundedVec<SpotClaim, T::MaxClaims> = OnDemandQueue::<T>::get();
				let pos =
					queue.iter().position(|elem| ParaIdAffinity::<T>::get(&elem.para_id).is_none());
				// Add claim to affinity maps and remove from queue.
				if let Some(i) = pos {
					let claim = queue.remove(i);
					Pallet::<T>::increase_affinity(claim.para_id, core_idx);
					// Write changes to storage.
					OnDemandQueue::<T>::set(queue);

					return Some(Assignment::ParathreadA(claim.into()))
				}

				return None
			},
		}
	}

	/// Pushes assignment back to the queue
	fn push_assignment_for_core(core_idx: CoreIndex, assignment: Assignment) {
		match assignment {
			Assignment::ParathreadA(pushback_claim) => {
				let claim: SpotClaim = pushback_claim.into();
				Pallet::<T>::decrease_affinity(claim.para_id, core_idx);
				match Pallet::<T>::add_parathread_claim(claim) {
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
}
