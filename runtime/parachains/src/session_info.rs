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

//! The session info pallet provides information about validator sets
//! from prior sessions needed for approvals and disputes.
//!
//! See https://w3f.github.io/parachain-implementers-guide/runtime/session_info.html.

use crate::{
	configuration, paras, scheduler, shared,
	util::{take_active_subset, take_active_subset_and_inactive},
};
use frame_support::{pallet_prelude::*, traits::OneSessionHandler};
use primitives::{
	v1::{AssignmentId, AuthorityDiscoveryId, SessionIndex},
	v2::SessionInfo,
};
use sp_std::vec::Vec;

pub use pallet::*;

pub mod migration;

#[cfg(test)]
mod tests;

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_system::pallet_prelude::BlockNumberFor;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::storage_version(migration::STORAGE_VERSION)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config:
		frame_system::Config
		+ configuration::Config
		+ shared::Config
		+ paras::Config
		+ scheduler::Config
		+ AuthorityDiscoveryConfig
	{
	}

	/// Assignment keys for the current session.
	/// Note that this API is private due to it being prone to 'off-by-one' at session boundaries.
	/// When in doubt, use `Sessions` API instead.
	#[pallet::storage]
	pub(super) type AssignmentKeysUnsafe<T: Config> =
		StorageValue<_, Vec<AssignmentId>, ValueQuery>;

	/// The earliest session for which previous session info is stored.
	#[pallet::storage]
	#[pallet::getter(fn earliest_stored_session)]
	pub(crate) type EarliestStoredSession<T: Config> = StorageValue<_, SessionIndex, ValueQuery>;

	/// Session information in a rolling window.
	/// Should have an entry in range `EarliestStoredSession..=CurrentSessionIndex`.
	/// Does not have any entries before the session index in the first session change notification.
	#[pallet::storage]
	#[pallet::getter(fn session_info)]
	pub(crate) type Sessions<T: Config> = StorageMap<_, Identity, SessionIndex, SessionInfo>;

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_runtime_upgrade() -> Weight {
			migration::migrate_to_latest::<T>()
		}
	}
}

/// An abstraction for the authority discovery pallet
/// to help with mock testing.
pub trait AuthorityDiscoveryConfig {
	/// Retrieve authority identifiers of the current authority set in canonical ordering.
	fn authorities() -> Vec<AuthorityDiscoveryId>;
}

impl<T: pallet_authority_discovery::Config> AuthorityDiscoveryConfig for T {
	fn authorities() -> Vec<AuthorityDiscoveryId> {
		<pallet_authority_discovery::Pallet<T>>::current_authorities().to_vec()
	}
}

impl<T: Config> Pallet<T> {
	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		notification: &crate::initializer::SessionChangeNotification<T::BlockNumber>,
	) {
		let config = <configuration::Pallet<T>>::config();

		let dispute_period = config.dispute_period;

		let validators = notification.validators.clone();
		let discovery_keys = <T as AuthorityDiscoveryConfig>::authorities();
		let assignment_keys = AssignmentKeysUnsafe::<T>::get();
		let active_set = <shared::Pallet<T>>::active_validator_indices();

		let validator_groups = <scheduler::Pallet<T>>::validator_groups();
		let n_cores = <scheduler::Pallet<T>>::availability_cores().len() as u32;
		let zeroth_delay_tranche_width = config.zeroth_delay_tranche_width;
		let relay_vrf_modulo_samples = config.relay_vrf_modulo_samples;
		let n_delay_tranches = config.n_delay_tranches;
		let no_show_slots = config.no_show_slots;
		let needed_approvals = config.needed_approvals;

		let new_session_index = notification.session_index;
		let random_seed = notification.random_seed;
		let old_earliest_stored_session = EarliestStoredSession::<T>::get();
		let new_earliest_stored_session = new_session_index.saturating_sub(dispute_period);
		let new_earliest_stored_session =
			core::cmp::max(new_earliest_stored_session, old_earliest_stored_session);
		// remove all entries from `Sessions` from the previous value up to the new value
		// avoid a potentially heavy loop when introduced on a live chain
		if old_earliest_stored_session != 0 || Sessions::<T>::get(0).is_some() {
			for idx in old_earliest_stored_session..new_earliest_stored_session {
				Sessions::<T>::remove(&idx);
			}
			// update `EarliestStoredSession` based on `config.dispute_period`
			EarliestStoredSession::<T>::set(new_earliest_stored_session);
		} else {
			// just introduced on a live chain
			EarliestStoredSession::<T>::set(new_session_index);
		}
		// create a new entry in `Sessions` with information about the current session
		let new_session_info = SessionInfo {
			validators, // these are from the notification and are thus already correct.
			discovery_keys: take_active_subset_and_inactive(&active_set, &discovery_keys),
			assignment_keys: take_active_subset(&active_set, &assignment_keys),
			validator_groups,
			n_cores,
			zeroth_delay_tranche_width,
			relay_vrf_modulo_samples,
			n_delay_tranches,
			no_show_slots,
			needed_approvals,
			active_validator_indices: active_set,
			random_seed,
			dispute_period,
		};
		Sessions::<T>::insert(&new_session_index, &new_session_info);
	}

	/// Called by the initializer to initialize the session info pallet.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the session info pallet.
	pub(crate) fn initializer_finalize() {}
}

impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Pallet<T> {
	type Public = AssignmentId;
}

impl<T: pallet_session::Config + Config> OneSessionHandler<T::AccountId> for Pallet<T> {
	type Key = AssignmentId;

	fn on_genesis_session<'a, I: 'a>(_validators: I)
	where
		I: Iterator<Item = (&'a T::AccountId, Self::Key)>,
	{
	}

	fn on_new_session<'a, I: 'a>(_changed: bool, validators: I, _queued: I)
	where
		I: Iterator<Item = (&'a T::AccountId, Self::Key)>,
	{
		let assignment_keys: Vec<_> = validators.map(|(_, v)| v).collect();
		AssignmentKeysUnsafe::<T>::set(assignment_keys);
	}

	fn on_disabled(_i: u32) {}
}
