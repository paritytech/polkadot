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

use primitives::v1::{SessionIndex, SessionInfo, ValidatorId};
use frame_support::{
	decl_storage, decl_module, decl_error,
};
use crate::configuration;

pub trait Trait:
	frame_system::Trait
	+ configuration::Trait
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

impl<T: Trait> sp_runtime::BoundToRuntimeAppPublic for Module<T> {
	type Public = ValidatorId;
}

impl<T: pallet_session::Trait + Trait> pallet_session::OneSessionHandler<T::AccountId> for Module<T> {
	type Key = ValidatorId;

	fn on_genesis_session<'a, I: 'a>(_validators: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{

	}

	fn on_new_session<'a, I: 'a>(_changed: bool, _validators: I, _queued: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		// TODO:
		// fetch dispute_period from config
		let new_session_index = <pallet_session::Module<T>>::current_index();
		todo!()
	}

	fn on_disabled(_i: usize) { }
}


#[cfg(test)]
mod tests {
	use super::*;
	fn it_works() {
		todo!()
	}
}
