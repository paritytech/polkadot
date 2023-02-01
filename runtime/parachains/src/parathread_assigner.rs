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

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

// TODO shim for now.
pub trait ClaimQueue {
	fn dimensions() -> (u32, u32);
	fn clear();
}


/// Interfaces we might want to support:
///
/// # Collators placing orders directly (via their CollatorId).
///
/// For this we need two extrinsics:
///
/// 1. A signed extrinsic for setting the current `CollatorId` for an account.
/// 2. An unsigned extrinisc, with a collator signature provided as the payload for actually
///    placing an order. Only valid if the collator id has been registered by an account first,
///    which has enough funds.
///
/// # Placing bids via a proxy account
///
/// There we need to define a new kind of proxy which is only allowed to place orders for a
/// particular `ParaId`, then a collator can use signed extrinsics (signed by that proxy account)
/// to order a core.
///
/// # Combination of both
///
/// The limited proxy account could be the one getting associated with the `CollatorId`.

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

		/// The exotic scheduling module that is associated with this instance of the auctioneer.
		type ClaimQueueProvider: ClaimQueue;

		/// The maximum number of orders stored by the price controller. This should be bounded by the
		/// ~atoms in the universe~ some sensible number
		#[pallet::constant]
		type MaxOrders: Get<u32>;
	}

	#[derive(Encode, Decode, Debug, TypeInfo, MaxEncodedLen, Eq, PartialEq, Clone)]
	pub struct CoreOrder<T: Config> {
		lifetime: u32,
		account_id: T::AccountId,
		exotic_id: ParaId,
		scheduled_collator: ScheduledCollator,
		max_amount: BalanceOf<T>,
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {}

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
        /// Associate a `CollatorId` with an account.
		///
		/// The registered `CollatorId` will then be allowed to place orders on behalf of that
		/// account via `collator_place_bid`.
        /// TODO: 
		/// - [ ] take a deposit 
		/// - [ ] make it restricted to a given para id
		/// - [ ] weights
		///
		#[pallet::call_index(1)]
		#[pallet::weight(1_000)]
        pub fn register_collator_id(origin: OriginFor<T>, para_id: ParaId, collator_id: CollatorId) -> DispatchResult {
			let account_id = ensure_signed(origin);
			Ok(())
        }
        /// Disassociate a `CollatorId` from an account.
		///
		/// The registered `CollatorId` will then be allowed to place orders on behalv of that
		/// account via `collator_place_bid`.
        /// TODO: weights
		#[pallet::call_index(1)]
		#[pallet::weight(1_000)]
        pub fn deregister_collator_id(origin: OriginFor<T>, collator_id: CollatorId) -> DispatchResult {
			let account_id = ensure_signed(origin);
			Ok(())
        }

		/// Accept an order by a collator that has been registered previously.
        /// TODO: weights
		#[pallet::call_index(2)]
		#[pallet::weight(1_000)]
        pub fn collator_place_order(collator_id: CollatorId, order: CoreOrder) -> DispatchResult {
			Ok(())
		}

		/// Place the actual order, charging the given account without authorization.
		///
		/// Authorization has to be handled by the caller. 
		fn place_order_unchecked(account: AccountId, order: CoreOrder) -> DispatchResult {
		}
	}

	/**
	*  impl<T: Config> Pallet<T> {
	*		/// Called by the initializer to initialize the auctioneer pallet.
	*		pub(crate) fn initializer_initialize(now: T::BlockNumber) -> Weight {
	*			Weight::zero()
	*		}

	*		/// Called by the initializer to finalize the auctioneer pallet.
	*		pub(crate) fn initializer_finalize() {}

	*		/// Called by the initializer to note that a new session has started.
	*		pub(crate) fn initializer_on_new_session(
	*			notification: &SessionChangeNotification<T::BlockNumber>,
	*		) {
	*		}
	*  }
	*/

	/// https://research.web3.foundation/en/latest/polkadot/overview/2-token-economics.html#setting-transaction-fees
	/// Returns `None` if any one of the checked arithmetic fails
	pub fn calculate_spot_price(
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
			// queue_util_diff^2
			if let Some(util_diff_pow) = queue_util_diff.const_checked_mul(queue_util_diff) {
				// variability^2
				let vpow = FixedI64::from(variability.square());
				// variability^2 * queue_util_diff^2
				if let Some(vpow_times_utildiffpow) = util_diff_pow.const_checked_mul(vpow) {
					// (variability^2 * queue_util_diff^2)/2
					if let Some(div_by_two) = vpow_times_utildiffpow.const_checked_div(2.into()) {
						// traffic * (1 + queue_util_diff) + div_by_two
						return traffic.const_checked_mul(queue_util_diff.add(1.into()) + div_by_two)
					}
				}
			}
		}
		None
	}
}
