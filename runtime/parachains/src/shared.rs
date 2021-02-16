// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! A module for any shared state that other pallets may want access to.
//!
//! To avoid cyclic dependencies, it is important that this module is only
//! dependent on the `configuration` module.

use primitives::v1::SessionIndex;
use frame_support::{
	decl_storage, decl_module, decl_error,
	weights::Weight,
};
use crate::{
	configuration,
	initializer::SessionChangeNotification,
};

pub trait Config: frame_system::Config + configuration::Config { }

decl_storage! {
	trait Store for Module<T: Config> as ParaSessionInfo {
		/// The current session index.
		CurrentSessionIndex get(fn session_index): SessionIndex;
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

impl<T: Config> Module<T> {
	/// Called by the initializer to initialize the configuration module.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the configuration module.
	pub(crate) fn initializer_finalize() { }

	/// Called by the initializer to note that a new session has started.
	///
	/// Returns the list of outgoing paras from the actions queue.
	pub(crate) fn initializer_on_new_session(notification: &SessionChangeNotification<T::BlockNumber>) {
		CurrentSessionIndex::set(notification.session_index);
	}

	#[cfg(test)]
	pub(crate) fn set_session_index(index: SessionIndex) {
		CurrentSessionIndex::set(index);
	}
}
