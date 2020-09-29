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

use frame_support::{
	decl_error, decl_module,
	dispatch::DispatchResult,
	weights::DispatchClass,
};
use frame_system::ensure_root;
use runtime_parachains::{
	router,
	paras::{self, ParaGenesisArgs},
};
use primitives::v1::Id as ParaId;

/// The module's configuration trait.
pub trait Trait: paras::Trait + router::Trait { }

decl_error! {
	pub enum Error for Module<T: Trait> { }
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
			paras::Module::<T>::schedule_para_initialize(id, genesis);
			Ok(())
		}

		/// Schedule a para to be cleaned up at the start of the next session.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn sudo_schedule_para_cleanup(origin, id: ParaId) -> DispatchResult {
			ensure_root(origin)?;
			paras::Module::<T>::schedule_para_cleanup(id);
			router::Module::<T>::schedule_para_cleanup(id);
			Ok(())
		}
	}
}
