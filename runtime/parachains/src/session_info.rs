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

use primitives::v1::{AuthorityDiscoveryId, SessionIndex, SessionInfo};
use frame_support::{
	decl_storage, decl_module, decl_error,
	weights::Weight,
};
use crate::{configuration, paras, scheduler};

pub trait Trait:
	frame_system::Trait
	+ configuration::Trait
	+ paras::Trait
	+ scheduler::Trait
	+ AuthorityDiscoveryTrait
{
}

decl_storage! {
	trait Store for Module<T: Trait> as ParaSessionInfo {
		/// The earliest session for which previous session info is stored.
		EarliestStoredSession get(fn earliest_stored_session): SessionIndex;
		/// Session information in a rolling window.
		/// Should have an entry in range `EarliestStoredSession..=CurrentSessionIndex`.
		Sessions get(fn session_info): map hasher(identity) SessionIndex => Option<SessionInfo>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The session info module.
	pub struct Module<T: Trait> for enum Call where origin: <T as frame_system::Trait>::Origin {
		type Error = Error<T>;
	}
}

/// An abstraction for the authority discovery pallet
/// to help with mock testing.
pub trait AuthorityDiscoveryTrait {
	/// Retrieve authority identifiers of the current and next authority set.
	fn authorities() -> Vec<AuthorityDiscoveryId>;
}

impl<T: pallet_authority_discovery::Trait> AuthorityDiscoveryTrait for T {
	fn authorities() -> Vec<AuthorityDiscoveryId> {
		<pallet_authority_discovery::Module<T>>::authorities()
	}
}

impl<T: Trait> Module<T> {
	/// Handle an incoming session change.
	pub(crate) fn initializer_on_new_session(
		notification: &crate::initializer::SessionChangeNotification<T::BlockNumber>
	) {
		let config = <configuration::Module<T>>::config();

		let dispute_period = config.dispute_period;
		let n_parachains = <paras::Module<T>>::parachains().len() as u32;

		let validators: Vec<_> = notification.validators.clone();
		let discovery_keys = <T as AuthorityDiscoveryTrait>::authorities();
		// FIXME:???
		let approval_keys = Vec::new();
		let validator_groups =  <scheduler::Module<T>>::validator_groups();
		let n_cores = n_parachains + config.parathread_cores;
		let zeroth_delay_tranche_width = config.zeroth_delay_tranche_width;
		let relay_vrf_modulo_samples = config.relay_vrf_modulo_samples;
		let n_delay_tranches = config.n_delay_tranches;
		let no_show_slots = config.no_show_slots;
		let needed_approvals = config.needed_approvals;

		let new_session_index = notification.session_index;
		let old_earliest_stored_session = EarliestStoredSession::get();
		let new_earliest_stored_session = new_session_index.checked_sub(dispute_period).unwrap_or(0);
		// update `EarliestStoredSession` based on `config.dispute_period`
		EarliestStoredSession::set(new_earliest_stored_session);
		// remove all entries from `Sessions` from the previous value up to the new value
		for idx in old_earliest_stored_session..new_earliest_stored_session {
			Sessions::remove(&idx);
		}
		// create a new entry in `Sessions` with information about the current session
		let new_session_info = SessionInfo {
			validators,
			discovery_keys,
			approval_keys,
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


#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{
		new_test_ext, Configuration, SessionInfo, System, GenesisConfig as MockGenesisConfig,
		Test,
	};
	use crate::initializer::SessionChangeNotification;
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

			System::on_finalize(b);

			System::on_initialize(b + 1);
			System::set_block_number(b + 1);

			if let Some(notification) = new_session(b + 1) {
				Configuration::initializer_on_new_session(&notification.validators, &notification.queued);
				SessionInfo::initializer_on_new_session(&notification);
			}

			Configuration::initializer_initialize(b + 1);
			SessionInfo::initializer_initialize(b + 1);
		}
	}

	#[test]
	fn it_works() {
		// TODO:
		assert_eq!(2 + 2, 4);
	}
}
