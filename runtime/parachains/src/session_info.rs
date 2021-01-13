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

//! The session info module provides information about validator sets
//! from prior sessions needed for approvals and disputes.
//!
//! See https://w3f.github.io/parachain-implementers-guide/runtime/session_info.html.

use primitives::v1::{AssignmentId, AuthorityDiscoveryId, SessionIndex, SessionInfo};
use frame_support::{
	decl_storage, decl_module, decl_error,
	weights::Weight,
};
use crate::{configuration, paras, scheduler};
use sp_std::vec::Vec;

pub trait Config:
	frame_system::Config
	+ configuration::Config
	+ paras::Config
	+ scheduler::Config
	+ AuthorityDiscoveryConfig
{
}

decl_storage! {
	trait Store for Module<T: Config> as ParaSessionInfo {
		/// Assignment keys for the current session.
		/// Note that this API is private due to it being prone to 'off-by-one' at session boundaries.
		/// When in doubt, use `Sessions` API instead.
		AssignmentKeysUnsafe: Vec<AssignmentId>;
		/// The earliest session for which previous session info is stored.
		EarliestStoredSession get(fn earliest_stored_session): SessionIndex;
		/// Session information in a rolling window.
		/// Should have an entry in range `EarliestStoredSession..=CurrentSessionIndex`.
		/// Does not have any entries before the session index in the first session change notification.
		Sessions get(fn session_info): map hasher(identity) SessionIndex => Option<SessionInfo>;
	}
}

decl_error! {
	pub enum Error for Module<T: Config> { }
}

decl_module! {
	/// The session info module.
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;
	}
}

/// An abstraction for the authority discovery pallet
/// to help with mock testing.
pub trait AuthorityDiscoveryConfig {
	/// Retrieve authority identifiers of the current and next authority set.
	fn authorities() -> Vec<AuthorityDiscoveryId>;
}

impl<T: pallet_authority_discovery::Config> AuthorityDiscoveryConfig for T {
	fn authorities() -> Vec<AuthorityDiscoveryId> {
		<pallet_authority_discovery::Module<T>>::authorities()
	}
}

impl<T: Config> Module<T> {
	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		notification: &crate::initializer::SessionChangeNotification<T::BlockNumber>
	) {
		let config = <configuration::Module<T>>::config();

		let dispute_period = config.dispute_period;
		let n_parachains = <paras::Module<T>>::parachains().len() as u32;

		let validators = notification.validators.clone();
		let discovery_keys = <T as AuthorityDiscoveryConfig>::authorities();
		let assignment_keys = AssignmentKeysUnsafe::get();
		let validator_groups = <scheduler::Module<T>>::validator_groups();
		let n_cores = n_parachains + config.parathread_cores;
		let zeroth_delay_tranche_width = config.zeroth_delay_tranche_width;
		let relay_vrf_modulo_samples = config.relay_vrf_modulo_samples;
		let n_delay_tranches = config.n_delay_tranches;
		let no_show_slots = config.no_show_slots;
		let needed_approvals = config.needed_approvals;

		let new_session_index = notification.session_index;
		let old_earliest_stored_session = EarliestStoredSession::get();
		let new_earliest_stored_session = new_session_index.saturating_sub(dispute_period);
		let new_earliest_stored_session = core::cmp::max(new_earliest_stored_session, old_earliest_stored_session);
		// remove all entries from `Sessions` from the previous value up to the new value
		// avoid a potentially heavy loop when introduced on a live chain
		if old_earliest_stored_session != 0 || Sessions::get(0).is_some() {
			for idx in old_earliest_stored_session..new_earliest_stored_session {
				Sessions::remove(&idx);
			}
			// update `EarliestStoredSession` based on `config.dispute_period`
			EarliestStoredSession::set(new_earliest_stored_session);
		} else {
			// just introduced on a live chain
			EarliestStoredSession::set(new_session_index);
		}
		// create a new entry in `Sessions` with information about the current session
		let new_session_info = SessionInfo {
			validators,
			discovery_keys,
			assignment_keys,
			validator_groups,
			n_cores,
			zeroth_delay_tranche_width,
			relay_vrf_modulo_samples,
			n_delay_tranches,
			no_show_slots,
			needed_approvals,
		};
		Sessions::insert(&new_session_index, &new_session_info);
	}

	/// Called by the initializer to initialize the session info module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the session info module.
	pub(crate) fn initializer_finalize() {}
}

impl<T: Config> sp_runtime::BoundToRuntimeAppPublic for Module<T> {
	type Public = AssignmentId;
}

impl<T: pallet_session::Config + Config> pallet_session::OneSessionHandler<T::AccountId> for Module<T> {
	type Key = AssignmentId;

	fn on_genesis_session<'a, I: 'a>(_validators: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{

	}

	fn on_new_session<'a, I: 'a>(_changed: bool, validators: I, _queued: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		let assignment_keys: Vec<_> = validators.map(|(_, v)| v).collect();
		AssignmentKeysUnsafe::set(assignment_keys);
	}

	fn on_disabled(_i: usize) { }
}


#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{
		new_test_ext, Configuration, SessionInfo, System, GenesisConfig as MockGenesisConfig,
		Origin,
	};
	use crate::initializer::SessionChangeNotification;
	use crate::configuration::HostConfiguration;
	use frame_support::traits::{OnFinalize, OnInitialize};
	use primitives::v1::BlockNumber;

	fn run_to_block(
		to: BlockNumber,
		new_session: impl Fn(BlockNumber) -> Option<SessionChangeNotification<BlockNumber>>,
	) {
		while System::block_number() < to {
			let b = System::block_number();

			SessionInfo::initializer_finalize();
			Configuration::initializer_finalize();

			if let Some(notification) = new_session(b + 1) {
				Configuration::initializer_on_new_session(&notification.validators, &notification.queued);
				SessionInfo::initializer_on_new_session(&notification);
			}

			System::on_finalize(b);

			System::on_initialize(b + 1);
			System::set_block_number(b + 1);

			Configuration::initializer_initialize(b + 1);
			SessionInfo::initializer_initialize(b + 1);
		}
	}

	fn default_config() -> HostConfiguration<BlockNumber> {
		HostConfiguration {
			parathread_cores: 1,
			dispute_period: 2,
			needed_approvals: 3,
			..Default::default()
		}
	}

	fn genesis_config() -> MockGenesisConfig {
		MockGenesisConfig {
			configuration: configuration::GenesisConfig {
				config: default_config(),
				..Default::default()
			},
			..Default::default()
		}
	}

	fn session_changes(n: BlockNumber) -> Option<SessionChangeNotification<BlockNumber>> {
		match n {
			100 => Some(SessionChangeNotification {
				session_index: 10,
				..Default::default()
			}),
			200 => Some(SessionChangeNotification {
				session_index: 20,
				..Default::default()
			}),
			300 => Some(SessionChangeNotification {
				session_index: 30,
				..Default::default()
			}),
			400 => Some(SessionChangeNotification {
				session_index: 40,
				..Default::default()
			}),
			_ => None,
		}
	}

	fn new_session_every_block(n: BlockNumber) -> Option<SessionChangeNotification<BlockNumber>> {
		Some(SessionChangeNotification{
			session_index: n,
			..Default::default()
		})
	}

	#[test]
	fn session_pruning_is_based_on_dispute_period() {
		new_test_ext(genesis_config()).execute_with(|| {
			let default_info = primitives::v1::SessionInfo::default();
			Sessions::insert(9, default_info);
			run_to_block(100, session_changes);
			// but the first session change is not based on dispute_period
			assert_eq!(EarliestStoredSession::get(), 10);
			// and we didn't prune the last changes
			assert!(Sessions::get(9).is_some());

			// changing dispute_period works
			let dispute_period = 5;
			Configuration::set_dispute_period(Origin::root(), dispute_period).unwrap();
			run_to_block(200, session_changes);
			assert_eq!(EarliestStoredSession::get(), 20 - dispute_period);

			// we don't have that many sessions stored
			let new_dispute_period = 16;
			Configuration::set_dispute_period(Origin::root(), new_dispute_period).unwrap();
			run_to_block(300, session_changes);
			assert_eq!(EarliestStoredSession::get(), 20 - dispute_period);

			// now we do
			run_to_block(400, session_changes);
			assert_eq!(EarliestStoredSession::get(), 40 - new_dispute_period);
		})
	}

	#[test]
	fn session_info_is_based_on_config() {
		new_test_ext(genesis_config()).execute_with(|| {
			run_to_block(1, new_session_every_block);
			let session = Sessions::get(&1).unwrap();
			assert_eq!(session.needed_approvals, 3);

			// change some param
			Configuration::set_needed_approvals(Origin::root(), 42).unwrap();
			run_to_block(2, new_session_every_block);
			let session = Sessions::get(&2).unwrap();
			assert_eq!(session.needed_approvals, 42);
		})
	}
}
