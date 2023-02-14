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

//! The parathread assignment pallet
//!
use crate::initializer::SessionChangeNotification;
use frame_support::{
	pallet_prelude::*,
	traits::{Currency, ReservableCurrency},
};
use frame_system::pallet_prelude::*;
use primitives::v2::{CollatorId, Id as ParaId};
use sp_runtime::{FixedI64, Perbill};

#[cfg(test)]
mod tests;

// TODO enable benchmarks maybe?
// #[cfg(feature = "runtime-benchmarks")]
// mod benchmarking;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	// TODO Using libp2p `PeerId` or it's public key implementation instead of `CollatorId` could be
	// beneficial for node side execution/filtering of peers.
	type ScheduledCollator = CollatorId;

	type BalanceOf<T> =
		<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The runtime's definition of an event.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// The runtime's definition of a Currency.
		type Currency: ReservableCurrency<Self::AccountId>;

		/// An origin which is allowed to set the base price of parathreads.
		type SetPriceOrigin: EnsureOrigin<<Self as frame_system::Config>::RuntimeOrigin>;

		/// The maximum number of orders stored by the price controller. This should be bounded by the
		/// ~atoms in the universe~ some sensible number
		#[pallet::constant]
		type MaxOrders: Get<u32>;
	}

	#[derive(Encode, Decode, Debug, TypeInfo, MaxEncodedLen, Eq, PartialEq, Clone)]
	pub struct CoreOrder {
		exotic_id: ParaId,
		scheduled_collator: ScheduledCollator,
	}

	#[pallet::storage]
	#[pallet::getter(fn get_base_spot_price)]
	pub(super) type BaseSpotPrice<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	// The order storage. A simple bounded vec will do for now but this can be changed to a
	// message queue at a later point.
	#[pallet::storage]
	pub type OrderBook<T> =
		StorageValue<_, BoundedVec<CoreOrder, <T as Config>::MaxOrders>, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		BaseSpotPriceSet { amount: BalanceOf<T> },
	}

	#[pallet::error]
	pub enum Error<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Create a single parathread core order.
		///
		/// Parameters:
		/// - `origin`:
		///
		/// Errors:
		/// Events:
		/// TODO weights
		#[pallet::call_index(0)]
		#[pallet::weight(1_000)]
		pub fn place_order(origin: OriginFor<T>) -> DispatchResult {
			let _res = ensure_signed(origin);
			Ok(())
		}

		/// Set the base spot price. Should only be available to the runtime's SetPriceOrigin.
		///
		/// Parameters:
		/// - `origin`: Must be root
		/// - `amount`: The price to set as the base
		///
		/// Errors:
		/// - `BadOrigin`
		/// Events:
		/// - `BaseSpotPriceSet(amount)`
		/// TODO weights
		#[pallet::call_index(1)]
		#[pallet::weight(1_000)]
		pub fn set_base_spot_price(origin: OriginFor<T>, amount: BalanceOf<T>) -> DispatchResult {
			// Ensure the proper origin authority
			T::SetPriceOrigin::ensure_origin(origin)?;

			BaseSpotPrice::<T>::set(amount);
			Self::deposit_event(Event::BaseSpotPriceSet { amount });
			Ok(())
		}
	}

	impl<T: Config> Pallet<T> {
		/// Called by the initializer to initialize the pallet.
		pub(crate) fn initializer_initialize(now: T::BlockNumber) -> Weight {
			Weight::zero()
		}

		/// Called by the initializer to finalize the pallet.
		pub(crate) fn initializer_finalize() {}

		/// Called by the initializer to note that a new session has started.
		pub(crate) fn initializer_on_new_session(
			notification: &SessionChangeNotification<T::BlockNumber>,
		) {
		}

		/// The spot price multiplier. This is based on the transaction fee calculations defined in:
		/// https://research.web3.foundation/en/latest/polkadot/overview/2-token-economics.html#setting-transaction-fees
		///
		/// Returns:
		/// - A `FixedI64` in the range of 1.0 - `FixedI64::MAX`
		/// Parameters:
		/// - `traffic`: The previously calculated multiplier, can never go below 1.0.
		/// - `queue_capacity`: The max size of the order book.
		/// - `queue_size`: How many orders are currently in the order book.
		/// - `target_queue_utilization`: How much of the queue_capacity should be ideally occupied, expressed in percentages(perbill).
		/// - `variability`: A variability factor, i.e. how quickly the spot price adjusts. This number can be chosen by
		///                  p/(k*(1-s)) where p is the desired ratio increase in spot price over k number of blocks.
		///                  s is the target_queue_utilization. A concrete example: v = 0.05/(20*(1-0.25)) = 0.0033.
		pub(crate) fn calculate_spot_price(
			traffic: FixedI64,
			queue_capacity: u32,
			queue_size: u32,
			target_queue_utilization: Perbill,
			variability: Perbill,
		) -> Option<FixedI64> {
			if queue_capacity > 0 {
				// queue_size / queue_capacity
				let queue_utilization =
					FixedI64::from_rational(queue_size.into(), queue_capacity.into());
				// queue_utilization - target_queue_utilization
				let queue_util_diff = queue_utilization.sub(target_queue_utilization.into());
				// variability * queue_util_diff
				if let Some(var_times_util_diff) =
					queue_util_diff.const_checked_mul(variability.into())
				{
					// queue_util_diff^2
					if let Some(util_diff_pow) = queue_util_diff.const_checked_mul(queue_util_diff)
					{
						// queue_util_diff^2
						// variability^2
						let vpow = FixedI64::from(variability.square());
						// variability^2 * queue_util_diff^2
						if let Some(vpow_times_utildiffpow) = util_diff_pow.const_checked_mul(vpow)
						{
							// (variability^2 * queue_util_diff^2)/2
							if let Some(div_by_two) =
								vpow_times_utildiffpow.const_checked_div(2.into())
							{
								// traffic * (1 + queue_util_diff) + div_by_two
								match traffic.const_checked_mul(
									var_times_util_diff.add(1.into()) + div_by_two,
								) {
									Some(t) => {
										if t >= FixedI64::from_inner(1_000_000_000i64) {
											return Some(t)
										}
										// Never return a multiplier below 1.0.
										return Some(FixedI64::from_inner(1_000_000_000i64))
									},
									None => {},
								}
							}
						}
					}
				}
			}
			None
			//FixedI64::from_inner(i64::MAX)
		}
	}
}
