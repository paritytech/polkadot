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

//! The router module is responsible for handling messaging.
//!
//! The core of the messaging is checking and processing messages sent out by the candidates,
//! routing the messages at their destinations and informing the parachains about the incoming
//! messages.

use crate::{configuration, initializer};
use sp_std::prelude::*;
use frame_support::{decl_error, decl_module, decl_storage, weights::Weight};
use primitives::v1::{Id as ParaId};

pub trait Trait: frame_system::Trait + configuration::Trait {}

decl_storage! {
	trait Store for Module<T: Trait> as Router {
		/// Paras that are to be cleaned up at the end of the session.
		/// The entries are sorted ascending by the para id.
		OutgoingParas: Vec<ParaId>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The router module.
	pub struct Module<T: Trait> for enum Call where origin: <T as frame_system::Trait>::Origin {
		type Error = Error<T>;
	}
}

impl<T: Trait> Module<T> {
	/// Block initialization logic, called by initializer.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Block finalization logic, called by initializer.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note that a new session has started.
	pub(crate) fn initializer_on_new_session(
		_notification: &initializer::SessionChangeNotification<T::BlockNumber>,
	) {
		let outgoing = OutgoingParas::take();
		for _outgoing_para in outgoing {

		}
	}

	/// Schedule a para to be cleaned up at the start of the next session.
	pub fn schedule_para_cleanup(id: ParaId) {
		OutgoingParas::mutate(|v| {
			if let Err(i) = v.binary_search(&id) {
				v.insert(i, id);
			}
		});
	}
}

#[cfg(test)]
mod tests {
}
