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

//! A simple wrapper allowing `Sudo` to call into `paras` routines.

use sp_std::prelude::*;
use frame_support::{
	decl_error, decl_module, ensure,
	dispatch::DispatchResult,
	weights::DispatchClass,
};
use frame_system::ensure_root;
use runtime_parachains::{
	configuration, dmp, ump, hrmp, paras::{self, ParaGenesisArgs},
};
use primitives::v1::Id as ParaId;

/// The module's configuration trait.
pub trait Trait:
	configuration::Trait + paras::Trait + dmp::Trait + ump::Trait + hrmp::Trait
{
}

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// The specified parachain or parathread is not registered.
		ParaDoesntExist,
		/// A DMP message couldn't be sent because it exceeds the maximum size allowed for a downward
		/// message.
		ExceedsMaxMessageSize,
	}
}

decl_module! {
	/// A sudo wrapper to call into v1 paras module.
	pub struct Module<T: Trait> for enum Call where origin: <T as frame_system::Trait>::Origin {
		type Error = Error<T>;

		/// Schedule a para to be initialized at the start of the next session.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn sudo_schedule_para_initialize(
			origin,
			id: ParaId,
			genesis: ParaGenesisArgs,
		) -> DispatchResult {
			ensure_root(origin)?;
			runtime_parachains::schedule_para_initialize::<T>(id, genesis);
			Ok(())
		}

		/// Schedule a para to be cleaned up at the start of the next session.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn sudo_schedule_para_cleanup(origin, id: ParaId) -> DispatchResult {
			ensure_root(origin)?;
			runtime_parachains::schedule_para_cleanup::<T>(id);
			Ok(())
		}

		/// Send a downward message to the given para.
		///
		/// The given parachain should exist and the payload should not exceed the preconfigured size
		/// `config.max_downward_message_size`.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn sudo_queue_downward_message(origin, id: ParaId, payload: Vec<u8>) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(<paras::Module<T>>::is_valid_para(id), Error::<T>::ParaDoesntExist);
			let config = <configuration::Module<T>>::config();
			<dmp::Module<T>>::queue_downward_message(&config, id, payload)
				.map_err(|e| match e {
					dmp::QueueDownwardMessageError::ExceedsMaxMessageSize =>
						Error::<T>::ExceedsMaxMessageSize.into(),
				})
		}
	}
}
