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

//! This module is responsible for maintaining a consistent initialization order for all other
//! parachains modules. It's also responsible for finalization and session change notifications.
//!
//! This module can throw fatal errors if session-change notifications are received after initialization.

use sp_std::prelude::*;
use frame_support::weights::Weight;
use primitives::{
	parachain::{ValidatorId},
};
use frame_support::{
	decl_storage, decl_module, decl_error,
};
use crate::{configuration, paras};

pub trait Trait: system::Trait + configuration::Trait + paras::Trait { }

decl_storage! {
	trait Store for Module<T: Trait> as Initializer {
		/// Whether the parachains modules have been initialized within this block.
		///
		/// Semantically a bool, but this guarantees it should never hit the trie,
		/// as this is cleared in `on_finalize` and Frame optimizes `None` values to be empty values.
		///
		/// As a bool, `set(false)` and `remove()` both lead to the next `get()` being false, but one of
		/// them writes to the trie and one does not. This confusion makes `Option<()>` more suitable for
		/// the semantics of this variable.
		HasInitialized: Option<()>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The initializer module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;

		fn on_initialize(now: T::BlockNumber) -> Weight {
			// The other modules are initialized in this order:
			// - Configuration
			// - Paras
			// - Scheduler
			// - Inclusion
			// - Validity
			let total_weight = configuration::Module::<T>::initializer_initialize(now) +
				paras::Module::<T>::initializer_initialize(now);

			HasInitialized::set(Some(()));

			total_weight
		}

		fn on_finalize() {
			paras::Module::<T>::initializer_finalize();
			configuration::Module::<T>::initializer_finalize();
			HasInitialized::take();
		}
	}
}

impl<T: Trait> Module<T> {
	/// Should be called when a new session occurs. Forwards the session notification to all
	/// wrapped modules.
	///
	/// Panics if the modules have already been initialized.
	fn on_new_session<'a, I: 'a>(_changed: bool, validators: I, queued: I)
		where I: Iterator<Item=(&'a T::AccountId, ValidatorId)>
	{
		assert!(HasInitialized::get().is_none());

		let validators: Vec<_> = validators.map(|(_, v)| v).collect();
		let queued: Vec<_> = queued.map(|(_, v)| v).collect();

		configuration::Module::<T>::initializer_on_new_session(&validators, &queued);
		paras::Module::<T>::initializer_on_new_session(&validators, &queued);
	}
}

impl<T: Trait> sp_runtime::BoundToRuntimeAppPublic for Module<T> {
	type Public = ValidatorId;
}

impl<T: Trait> session::OneSessionHandler<T::AccountId> for Module<T> {
	type Key = ValidatorId;

	fn on_genesis_session<'a, I: 'a>(_validators: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{

	}

	fn on_new_session<'a, I: 'a>(changed: bool, validators: I, queued: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		<Module<T>>::on_new_session(changed, validators, queued);
	}

	fn on_disabled(_i: usize) { }
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, Initializer};

	use frame_support::traits::{OnFinalize, OnInitialize};

	#[test]
	#[should_panic]
	fn panics_if_session_changes_after_on_initialize() {
		new_test_ext(Default::default()).execute_with(|| {
			Initializer::on_initialize(1);
			Initializer::on_new_session(false, Vec::new().into_iter(), Vec::new().into_iter());
		});
	}

	#[test]
	fn sets_flag_on_initialize() {
		new_test_ext(Default::default()).execute_with(|| {
			Initializer::on_initialize(1);

			assert!(HasInitialized::get().is_some());
		})
	}

	#[test]
	fn clears_flag_on_finalize() {
		new_test_ext(Default::default()).execute_with(|| {
			Initializer::on_initialize(1);
			Initializer::on_finalize(1);

			assert!(HasInitialized::get().is_none());
		})
	}
}
