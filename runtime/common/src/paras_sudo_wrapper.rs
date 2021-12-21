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

use frame_support::pallet_prelude::*;
use frame_system::pallet_prelude::*;
pub use pallet::*;
use parity_scale_codec::Encode;
use primitives::v1::Id as ParaId;
use runtime_parachains::{
	configuration, dmp, hrmp,
	paras::{self, ParaGenesisArgs},
	ump, ParaLifecycle,
};
use sp_std::boxed::Box;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	#[pallet::disable_frame_system_supertrait_check]
	pub trait Config:
		configuration::Config + paras::Config + dmp::Config + ump::Config + hrmp::Config
	{
	}

	#[pallet::error]
	pub enum Error<T> {
		/// The specified parachain or parathread is not registered.
		ParaDoesntExist,
		/// The specified parachain or parathread is already registered.
		ParaAlreadyExists,
		/// A DMP message couldn't be sent because it exceeds the maximum size allowed for a downward
		/// message.
		ExceedsMaxMessageSize,
		/// Could not schedule para cleanup.
		CouldntCleanup,
		/// Not a parathread.
		NotParathread,
		/// Not a parachain.
		NotParachain,
		/// Cannot upgrade parathread.
		CannotUpgrade,
		/// Cannot downgrade parachain.
		CannotDowngrade,
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Schedule a para to be initialized at the start of the next session.
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn sudo_schedule_para_initialize(
			origin: OriginFor<T>,
			id: ParaId,
			genesis: ParaGenesisArgs,
		) -> DispatchResult {
			ensure_root(origin)?;
			runtime_parachains::schedule_para_initialize::<T>(id, genesis)
				.map_err(|_| Error::<T>::ParaAlreadyExists)?;
			Ok(())
		}

		/// Schedule a para to be cleaned up at the start of the next session.
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn sudo_schedule_para_cleanup(origin: OriginFor<T>, id: ParaId) -> DispatchResult {
			ensure_root(origin)?;
			runtime_parachains::schedule_para_cleanup::<T>(id)
				.map_err(|_| Error::<T>::CouldntCleanup)?;
			Ok(())
		}

		/// Upgrade a parathread to a parachain
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn sudo_schedule_parathread_upgrade(
			origin: OriginFor<T>,
			id: ParaId,
		) -> DispatchResult {
			ensure_root(origin)?;
			// Para backend should think this is a parathread...
			ensure!(
				paras::Pallet::<T>::lifecycle(id) == Some(ParaLifecycle::Parathread),
				Error::<T>::NotParathread,
			);
			runtime_parachains::schedule_parathread_upgrade::<T>(id)
				.map_err(|_| Error::<T>::CannotUpgrade)?;
			Ok(())
		}

		/// Downgrade a parachain to a parathread
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn sudo_schedule_parachain_downgrade(
			origin: OriginFor<T>,
			id: ParaId,
		) -> DispatchResult {
			ensure_root(origin)?;
			// Para backend should think this is a parachain...
			ensure!(
				paras::Pallet::<T>::lifecycle(id) == Some(ParaLifecycle::Parachain),
				Error::<T>::NotParachain,
			);
			runtime_parachains::schedule_parachain_downgrade::<T>(id)
				.map_err(|_| Error::<T>::CannotDowngrade)?;
			Ok(())
		}

		/// Send a downward XCM to the given para.
		///
		/// The given parachain should exist and the payload should not exceed the preconfigured size
		/// `config.max_downward_message_size`.
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn sudo_queue_downward_xcm(
			origin: OriginFor<T>,
			id: ParaId,
			xcm: Box<xcm::opaque::VersionedXcm>,
		) -> DispatchResult {
			ensure_root(origin)?;
			ensure!(<paras::Pallet<T>>::is_valid_para(id), Error::<T>::ParaDoesntExist);
			let config = <configuration::Pallet<T>>::config();
			<dmp::Pallet<T>>::queue_downward_message(&config, id, xcm.encode()).map_err(|e| match e
			{
				dmp::QueueDownwardMessageError::ExceedsMaxMessageSize =>
					Error::<T>::ExceedsMaxMessageSize.into(),
			})
		}

		/// Forcefully establish a channel from the sender to the recipient.
		///
		/// This is equivalent to sending an `Hrmp::hrmp_init_open_channel` extrinsic followed by
		/// `Hrmp::hrmp_accept_open_channel`.
		#[pallet::weight((1_000, DispatchClass::Operational))]
		pub fn sudo_establish_hrmp_channel(
			origin: OriginFor<T>,
			sender: ParaId,
			recipient: ParaId,
			max_capacity: u32,
			max_message_size: u32,
		) -> DispatchResult {
			ensure_root(origin)?;

			<hrmp::Pallet<T>>::init_open_channel(
				sender,
				recipient,
				max_capacity,
				max_message_size,
			)?;
			<hrmp::Pallet<T>>::accept_open_channel(recipient, sender)?;
			Ok(())
		}
	}
}
