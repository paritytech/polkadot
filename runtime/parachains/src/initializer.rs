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
use primitives::v1::ValidatorId;
use frame_support::{
	decl_storage, decl_module, decl_error, traits::Randomness,
};
use sp_runtime::traits::One;
use codec::{Encode, Decode};
use crate::{
	configuration::{self, HostConfiguration},
	paras, scheduler, inclusion, dmp, ump, hrmp,
};

/// Information about a session change that has just occurred.
#[derive(Default, Clone)]
pub struct SessionChangeNotification<BlockNumber> {
	/// The new validators in the session.
	pub validators: Vec<ValidatorId>,
	/// The qeueud validators for the following session.
	pub queued: Vec<ValidatorId>,
	/// The configuration before handling the session change
	pub prev_config: HostConfiguration<BlockNumber>,
	/// The configuration after handling the session change.
	pub new_config: HostConfiguration<BlockNumber>,
	/// A secure random seed for the session, gathered from BABE.
	pub random_seed: [u8; 32],
	/// New session index.
	pub session_index: sp_staking::SessionIndex,
}

#[derive(Encode, Decode)]
struct BufferedSessionChange<N> {
	apply_at: N,
	validators: Vec<ValidatorId>,
	queued: Vec<ValidatorId>,
	session_index: sp_staking::SessionIndex,
}

pub trait Trait:
	frame_system::Trait
	+ configuration::Trait
	+ paras::Trait
	+ scheduler::Trait
	+ inclusion::Trait
	+ dmp::Trait
	+ ump::Trait
	+ hrmp::Trait
{
	/// A randomness beacon.
	type Randomness: Randomness<Self::Hash>;
}

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
		/// Buffered session changes along with the block number at which they should be applied.
		///
		/// Typically this will be empty or one element long, with the single element having a block
		/// number of the next block.
		///
		/// However this is a `Vec` regardless to handle various edge cases that may occur at runtime
		/// upgrade boundaries or if governance intervenes.
		BufferedSessionChanges: Vec<BufferedSessionChange<T::BlockNumber>>;
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> { }
}

decl_module! {
	/// The initializer module.
	pub struct Module<T: Trait> for enum Call where origin: <T as frame_system::Trait>::Origin {
		type Error = Error<T>;

		fn on_initialize(now: T::BlockNumber) -> Weight {
			// Apply buffered session changes before initializing modules, so they
			// can be initialized with respect to the current validator set.
			<BufferedSessionChanges<T>>::mutate(|v| {
				let drain_up_to = v.iter().take_while(|b| b.apply_at <= now).count();

				// apply only the last session as all others lasted less than a block (weirdly).
				if let Some(buffered) = v.drain(..drain_up_to).last() {
					Self::apply_new_session(
						buffered.session_index,
						buffered.validators,
						buffered.queued,
					);
				}
			});

			// The other modules are initialized in this order:
			// - Configuration
			// - Paras
			// - Scheduler
			// - Inclusion
			// - Validity
			// - DMP
			// - UMP
			// - HRMP
			let total_weight = configuration::Module::<T>::initializer_initialize(now) +
				paras::Module::<T>::initializer_initialize(now) +
				scheduler::Module::<T>::initializer_initialize(now) +
				inclusion::Module::<T>::initializer_initialize(now) +
				dmp::Module::<T>::initializer_initialize(now) +
				ump::Module::<T>::initializer_initialize(now) +
				hrmp::Module::<T>::initializer_initialize(now);

			HasInitialized::set(Some(()));

			total_weight
		}

		fn on_finalize() {
			// reverse initialization order.

			hrmp::Module::<T>::initializer_finalize();
			ump::Module::<T>::initializer_finalize();
			dmp::Module::<T>::initializer_finalize();
			inclusion::Module::<T>::initializer_finalize();
			scheduler::Module::<T>::initializer_finalize();
			paras::Module::<T>::initializer_finalize();
			configuration::Module::<T>::initializer_finalize();
			HasInitialized::take();
		}
	}
}

impl<T: Trait> Module<T> {
	fn apply_new_session(
		session_index: sp_staking::SessionIndex,
		validators: Vec<ValidatorId>,
		queued: Vec<ValidatorId>,
	) {
		let prev_config = <configuration::Module<T>>::config();

		let random_seed = {
			let mut buf = [0u8; 32];
			let random_hash = T::Randomness::random(&b"paras"[..]);
			let len = sp_std::cmp::min(32, random_hash.as_ref().len());
			buf[..len].copy_from_slice(&random_hash.as_ref()[..len]);
			buf
		};

		// We can't pass the new config into the thing that determines the new config,
		// so we don't pass the `SessionChangeNotification` into this module.
		configuration::Module::<T>::initializer_on_new_session(&validators, &queued);

		let new_config = <configuration::Module<T>>::config();

		let notification = SessionChangeNotification {
			validators,
			queued,
			prev_config,
			new_config,
			random_seed,
			session_index,
		};

		paras::Module::<T>::initializer_on_new_session(&notification);
		scheduler::Module::<T>::initializer_on_new_session(&notification);
		inclusion::Module::<T>::initializer_on_new_session(&notification);
		dmp::Module::<T>::initializer_on_new_session(&notification);
		ump::Module::<T>::initializer_on_new_session(&notification);
		hrmp::Module::<T>::initializer_on_new_session(&notification);
	}

	/// Should be called when a new session occurs. Buffers the session notification to be applied
	/// at the next block. If `queued` is `None`, the `validators` are considered queued.
	fn on_new_session<'a, I: 'a>(
		_changed: bool,
		session_index: sp_staking::SessionIndex,
		validators: I,
		queued: Option<I>,
	)
		where I: Iterator<Item=(&'a T::AccountId, ValidatorId)>
	{
		let validators: Vec<_> = validators.map(|(_, v)| v).collect();
		let queued: Vec<_> = if let Some(queued) = queued {
			queued.map(|(_, v)| v).collect()
		} else {
			validators.clone()
		};

		<BufferedSessionChanges<T>>::mutate(|v| v.push(BufferedSessionChange {
			apply_at: <frame_system::Module<T>>::block_number() + One::one(),
			validators,
			queued,
			session_index,
		}));
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

	fn on_new_session<'a, I: 'a>(changed: bool, validators: I, queued: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		let session_index = <pallet_session::Module<T>>::current_index();
		<Module<T>>::on_new_session(changed, session_index, validators, Some(queued));
	}

	fn on_disabled(_i: usize) { }
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, Initializer, Test, System};

	use frame_support::traits::{OnFinalize, OnInitialize};

	#[test]
	fn session_change_before_initialize_is_still_buffered_after() {
		new_test_ext(Default::default()).execute_with(|| {
			Initializer::on_new_session(
				false,
				1,
				Vec::new().into_iter(),
				Some(Vec::new().into_iter()),
			);

			let now = System::block_number();
			Initializer::on_initialize(now);

			let v = <BufferedSessionChanges<Test>>::get();
			assert_eq!(v.len(), 1);

			let apply_at = now + 1;
			assert_eq!(v[0].apply_at, apply_at);
		});
	}

	#[test]
	fn session_change_applied_on_initialize() {
		new_test_ext(Default::default()).execute_with(|| {
			Initializer::on_initialize(1);

			let now = System::block_number();
			Initializer::on_new_session(
				false,
				1,
				Vec::new().into_iter(),
				Some(Vec::new().into_iter()),
			);

			Initializer::on_initialize(now + 1);

			assert!(<BufferedSessionChanges<Test>>::get().is_empty());
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
