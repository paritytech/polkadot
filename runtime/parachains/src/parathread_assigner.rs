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
	pub fn calculate_spot_price(
		traffic: FixedI64,
		queue_capacity: u32,
		queue_size: u32,
		target_queue_utilization: Perbill,
		variability: Perbill,
	) -> Option<FixedI64> {
		let queue_utilization = FixedI64::from_rational(queue_size.into(), queue_capacity.into());
		let queue_util_diff = queue_utilization.sub(target_queue_utilization.into());
		let util_diff_pow = queue_util_diff.const_checked_mul(queue_util_diff).expect("The queue_util_diff is bounded by the ratio of two u32 numbers subtracted by a percentage and will therefore never overflow QED");
		let vpow = FixedI64::from(variability.square());

		traffic.const_checked_mul(
			(FixedI64::from_float(1.0) + queue_util_diff) +
				((vpow * util_diff_pow) / FixedI64::from_float(2.0)),
		)
	}
}
