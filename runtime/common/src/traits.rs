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

//! Traits used across pallets for Polkadot.

use sp_std::vec::*;
use primitives::v1::{HeadData, ValidationCode, Id as ParaId};
use frame_support::{
	dispatch::DispatchResult,
	traits::{Currency, ReservableCurrency},
};

/// Parachain registration API.
pub trait Registrar {
	/// The account ID type that encodes a parachain manager ID.
	type AccountId;

	/// Report the manager (permissioned owner) of a parachain, if there is one.
	fn manager_of(id: ParaId) -> Option<Self::AccountId>;

	/// All parachains. Ordered ascending by ParaId. Parathreads are not included.
	fn parachains() -> Vec<ParaId>;

	/// Return if a ParaId is a Parachain.
	fn is_parachain(id: ParaId) -> bool {
		Self::parachains().binary_search(&id).is_ok()
	}

	/// Return if a ParaId is a Parathread.
	fn is_parathread(id: ParaId) -> bool;

	/// Return if a ParaId is registered in the system.
	fn is_registered(id: ParaId) -> bool {
		Self::is_parathread(id) || Self::is_parachain(id)
	}

	/// Apply a lock to the para registration so that it cannot be modified by
	/// the manager directly. Instead the para must use its sovereign governance
	/// or the governance of the relay chain.
	fn apply_lock(id: ParaId);

	/// Remove any lock on the para registration.
	fn remove_lock(id: ParaId);

	/// Register a Para ID under control of `who`. Registration may be be
	/// delayed by session rotation.
	fn register(
		who: Self::AccountId,
		id: ParaId,
		genesis_head: HeadData,
		validation_code: ValidationCode,
	) -> DispatchResult;

	/// Deregister a Para ID, free any data, and return any deposits.
	fn deregister(id: ParaId) -> DispatchResult;

	/// Elevate a para to parachain status.
	fn make_parachain(id: ParaId) -> DispatchResult;

	/// Lower a para back to normal from parachain status.
	fn make_parathread(id: ParaId) -> DispatchResult;

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn worst_head_data() -> HeadData;

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn worst_validation_code() -> ValidationCode;

	/// Execute any pending state transitions for paras.
	/// For example onboarding to parathread, or parathread to parachain.
	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn execute_pending_transitions();
}

/// Error type for something that went wrong with leasing.
#[derive(Debug)]
pub enum LeaseError {
	/// Unable to reserve the funds in the leaser's account.
	ReserveFailed,
	/// There is already a lease on at least one period for the given para.
	AlreadyLeased,
	/// The period to be leased has already ended.
	AlreadyEnded,
}

/// Lease manager. Used by the auction module to handle parachain slot leases.
pub trait Leaser {
	/// An account identifier for a leaser.
	type AccountId;

	/// The measurement type for counting lease periods (generally just a `BlockNumber`).
	type LeasePeriod;

	/// The currency type in which the lease is taken.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// Lease a new parachain slot for `para`.
	///
	/// `leaser` shall have a total of `amount` balance reserved by the implementor of this trait.
	///
	/// Note: The implementor of the trait (the leasing system) is expected to do all reserve/unreserve calls. The
	/// caller of this trait *SHOULD NOT* pre-reserve the deposit (though should ensure that it is reservable).
	///
	/// The lease will last from `period_begin` for `period_count` lease periods. It is undefined if the `para`
	/// already has a slot leased during those periods.
	///
	/// Returns `Err` in the case of an error, and in which case nothing is changed.
	fn lease_out(
		para: ParaId,
		leaser: &Self::AccountId,
		amount: <Self::Currency as Currency<Self::AccountId>>::Balance,
		period_begin: Self::LeasePeriod,
		period_count: Self::LeasePeriod,
	) -> Result<(), LeaseError>;

	/// Return the amount of balance currently held in reserve on `leaser`'s account for leasing `para`. This won't
	/// go down outside of a lease period.
	fn deposit_held(para: ParaId, leaser: &Self::AccountId) -> <Self::Currency as Currency<Self::AccountId>>::Balance;

	/// The lease period. This is constant, but can't be a `const` due to it being a runtime configurable quantity.
	fn lease_period() -> Self::LeasePeriod;

	/// Returns the current lease period.
	fn lease_period_index() -> Self::LeasePeriod;
}

pub trait Auctioneer {
	/// An account identifier for a leaser.
	type AccountId;

	/// The measurement type for counting blocks.
	type BlockNumber;

	/// The measurement type for counting lease periods (generally the same as `BlockNumber`).
	type LeasePeriod;

	/// The currency type in which the lease is taken.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// Create a new auction.
	///
	/// This can only happen when there isn't already an auction in progress. Accepts the `duration`
	/// of this auction and the `lease_period_index` of the initial lease period of the four that
	/// are to be auctioned.
	fn new_auction(duration: Self::BlockNumber, lease_period_index: Self::LeasePeriod) -> DispatchResult;

	/// Returns `Some(n)` if the `now` block is part of the ending period of an auction, where `n`
	/// represents how far into the ending period this block is. Otherwise, returns `None`.
	fn is_ending(now: Self::BlockNumber) -> Option<Self::BlockNumber>;

	/// Place a bid in the current auction.
	///
	/// - `bidder`: The account that will be funding this bid.
	/// - `para`: The para to bid for.
	/// - `first_slot`: The first lease period index of the range to be bid on.
	/// - `last_slot`: The last lease period index of the range to be bid on (inclusive).
	/// - `amount`: The total amount to be the bid for deposit over the range.
	///
	/// The account `Bidder` must have at least `amount` available as a free balance in `Currency`. The
	/// implementation *MUST* remove or reserve `amount` funds from `bidder` and those funds should be returned
	/// or freed once the bid is rejected or lease has ended.
	fn place_bid(
		bidder: Self::AccountId,
		para: ParaId,
		first_slot: Self::LeasePeriod,
		last_slot: Self::LeasePeriod,
		amount: <Self::Currency as Currency<Self::AccountId>>::Balance,
	) -> DispatchResult;

	/// Returns the current lease period.
	fn lease_period_index() -> Self::LeasePeriod;

	/// Returns the length of a lease period.
	fn lease_period() -> Self::LeasePeriod;

	/// Check if the para and user combination has won an auction in the past.
	fn has_won_an_auction(para: ParaId, bidder: &Self::AccountId) -> bool;
}

/// Runtime hook for when we swap a parachain and parathread.
#[impl_trait_for_tuples::impl_for_tuples(30)]
pub trait OnSwap {
	/// Updates any needed state/references to enact a logical swap of two parachains. Identity,
	/// code and `head_data` remain equivalent for all parachains/threads, however other properties
	/// such as leases, deposits held and thread/chain nature are swapped.
	fn on_swap(one: ParaId, other: ParaId);
}
