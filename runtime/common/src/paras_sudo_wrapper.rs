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

use crate::WASM_MAGIC;
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
use parity_scale_codec::Encode;

/// The module's configuration trait.
pub trait Config:
	configuration::Config + paras::Config + dmp::Config + ump::Config + hrmp::Config
{
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// The specified parachain or parathread is not registered.
		ParaDoesntExist,
		/// A DMP message couldn't be sent because it exceeds the maximum size allowed for a downward
		/// message.
		ExceedsMaxMessageSize,
		/// The validation code provided doesn't start with the Wasm file magic string.
		DefinitelyNotWasm,
	}
}

decl_module! {
	/// A sudo wrapper to call into v1 paras module.
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;

		/// Schedule a para to be initialized at the start of the next session.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn sudo_schedule_para_initialize(
			origin,
			id: ParaId,
			genesis: ParaGenesisArgs,
		) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(genesis.validation_code.0.starts_with(WASM_MAGIC), Error::<T>::DefinitelyNotWasm);
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

		/// Send a downward XCM to the given para.
		///
		/// The given parachain should exist and the payload should not exceed the preconfigured size
		/// `config.max_downward_message_size`.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn sudo_queue_downward_xcm(origin, id: ParaId, xcm: xcm::VersionedXcm) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(<paras::Module<T>>::is_valid_para(id), Error::<T>::ParaDoesntExist);
			let config = <configuration::Module<T>>::config();
			<dmp::Module<T>>::queue_downward_message(&config, id, xcm.encode())
				.map_err(|e| match e {
					dmp::QueueDownwardMessageError::ExceedsMaxMessageSize =>
						Error::<T>::ExceedsMaxMessageSize.into(),
				})
		}

		/// Forcefully establish a channel from the sender to the recipient.
		///
		/// This is equivalent to sending an `Hrmp::hrmp_init_open_channel` extrinsic followed by
		/// `Hrmp::hrmp_accept_open_channel`.
		#[weight = (1_000, DispatchClass::Operational)]
		pub fn sudo_establish_hrmp_channel(
			origin,
			sender: ParaId,
			recipient: ParaId,
			max_capacity: u32,
			max_message_size: u32,
		) -> DispatchResult {
			ensure_root(origin)?;

			<hrmp::Module<T>>::init_open_channel(
				sender,
				recipient,
				max_capacity,
				max_message_size,
			)?;
			<hrmp::Module<T>>::accept_open_channel(recipient, sender)?;
			Ok(())
		}
	}
}
