// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

//! Mocking utilities for testing.

use crate::traits::Registrar;
use frame_support::{
	dispatch::{DispatchError, DispatchResult},
	weights::Weight,
};
use parity_scale_codec::{Decode, Encode};
use primitives::v1::{HeadData, Id as ParaId, ValidationCode};
use sp_runtime::{traits::SaturatedConversion, Permill};
use std::{cell::RefCell, collections::HashMap};

thread_local! {
	static OPERATIONS: RefCell<Vec<(ParaId, u32, bool)>> = RefCell::new(Vec::new());
	static PARACHAINS: RefCell<Vec<ParaId>> = RefCell::new(Vec::new());
	static PARATHREADS: RefCell<Vec<ParaId>> = RefCell::new(Vec::new());
	static LOCKS: RefCell<HashMap<ParaId, bool>> = RefCell::new(HashMap::new());
	static MANAGERS: RefCell<HashMap<ParaId, Vec<u8>>> = RefCell::new(HashMap::new());
}

pub struct TestRegistrar<T>(sp_std::marker::PhantomData<T>);

impl<T: frame_system::Config> Registrar for TestRegistrar<T> {
	type AccountId = T::AccountId;

	fn manager_of(id: ParaId) -> Option<Self::AccountId> {
		MANAGERS.with(|x| x.borrow().get(&id).and_then(|v| T::AccountId::decode(&mut &v[..]).ok()))
	}

	fn parachains() -> Vec<ParaId> {
		PARACHAINS.with(|x| x.borrow().clone())
	}

	fn is_parathread(id: ParaId) -> bool {
		PARATHREADS.with(|x| x.borrow().binary_search(&id).is_ok())
	}

	fn apply_lock(id: ParaId) {
		LOCKS.with(|x| x.borrow_mut().insert(id, true));
	}

	fn remove_lock(id: ParaId) {
		LOCKS.with(|x| x.borrow_mut().insert(id, false));
	}

	fn register(
		manager: Self::AccountId,
		id: ParaId,
		_genesis_head: HeadData,
		_validation_code: ValidationCode,
	) -> DispatchResult {
		// Should not be parachain.
		PARACHAINS.with(|x| {
			let parachains = x.borrow_mut();
			match parachains.binary_search(&id) {
				Ok(_) => Err(DispatchError::Other("Already Parachain")),
				Err(_) => Ok(()),
			}
		})?;
		// Should not be parathread, then make it.
		PARATHREADS.with(|x| {
			let mut parathreads = x.borrow_mut();
			match parathreads.binary_search(&id) {
				Ok(_) => Err(DispatchError::Other("Already Parathread")),
				Err(i) => {
					parathreads.insert(i, id);
					Ok(())
				},
			}
		})?;
		MANAGERS.with(|x| x.borrow_mut().insert(id, manager.encode()));
		Ok(())
	}

	fn deregister(id: ParaId) -> DispatchResult {
		// Should not be parachain.
		PARACHAINS.with(|x| {
			let parachains = x.borrow_mut();
			match parachains.binary_search(&id) {
				Ok(_) => Err(DispatchError::Other("cannot deregister parachain")),
				Err(_) => Ok(()),
			}
		})?;
		// Remove from parathread.
		PARATHREADS.with(|x| {
			let mut parathreads = x.borrow_mut();
			match parathreads.binary_search(&id) {
				Ok(i) => {
					parathreads.remove(i);
					Ok(())
				},
				Err(_) => Err(DispatchError::Other("not parathread, so cannot `deregister`")),
			}
		})?;
		MANAGERS.with(|x| x.borrow_mut().remove(&id));
		Ok(())
	}

	fn make_parachain(id: ParaId) -> DispatchResult {
		PARATHREADS.with(|x| {
			let mut parathreads = x.borrow_mut();
			match parathreads.binary_search(&id) {
				Ok(i) => {
					parathreads.remove(i);
					Ok(())
				},
				Err(_) => Err(DispatchError::Other("not parathread, so cannot `make_parachain`")),
			}
		})?;
		PARACHAINS.with(|x| {
			let mut parachains = x.borrow_mut();
			match parachains.binary_search(&id) {
				Ok(_) => Err(DispatchError::Other("already parachain, so cannot `make_parachain`")),
				Err(i) => {
					parachains.insert(i, id);
					Ok(())
				},
			}
		})?;
		OPERATIONS.with(|x| {
			x.borrow_mut().push((
				id,
				frame_system::Pallet::<T>::block_number().saturated_into(),
				true,
			))
		});
		Ok(())
	}
	fn make_parathread(id: ParaId) -> DispatchResult {
		PARACHAINS.with(|x| {
			let mut parachains = x.borrow_mut();
			match parachains.binary_search(&id) {
				Ok(i) => {
					parachains.remove(i);
					Ok(())
				},
				Err(_) => Err(DispatchError::Other("not parachain, so cannot `make_parathread`")),
			}
		})?;
		PARATHREADS.with(|x| {
			let mut parathreads = x.borrow_mut();
			match parathreads.binary_search(&id) {
				Ok(_) =>
					Err(DispatchError::Other("already parathread, so cannot `make_parathread`")),
				Err(i) => {
					parathreads.insert(i, id);
					Ok(())
				},
			}
		})?;
		OPERATIONS.with(|x| {
			x.borrow_mut().push((
				id,
				frame_system::Pallet::<T>::block_number().saturated_into(),
				false,
			))
		});
		Ok(())
	}

	#[cfg(test)]
	fn worst_head_data() -> HeadData {
		vec![0u8; 1000].into()
	}

	#[cfg(test)]
	fn worst_validation_code() -> ValidationCode {
		let validation_code = vec![0u8; 1000];
		validation_code.into()
	}

	#[cfg(test)]
	fn execute_pending_transitions() {}
}

impl<T: frame_system::Config> TestRegistrar<T> {
	pub fn operations() -> Vec<(ParaId, T::BlockNumber, bool)> {
		OPERATIONS
			.with(|x| x.borrow().iter().map(|(p, b, c)| (*p, (*b).into(), *c)).collect::<Vec<_>>())
	}

	#[allow(dead_code)]
	pub fn parachains() -> Vec<ParaId> {
		PARACHAINS.with(|x| x.borrow().clone())
	}

	#[allow(dead_code)]
	pub fn parathreads() -> Vec<ParaId> {
		PARATHREADS.with(|x| x.borrow().clone())
	}

	#[allow(dead_code)]
	pub fn clear_storage() {
		OPERATIONS.with(|x| x.borrow_mut().clear());
		PARACHAINS.with(|x| x.borrow_mut().clear());
		PARATHREADS.with(|x| x.borrow_mut().clear());
		MANAGERS.with(|x| x.borrow_mut().clear());
	}
}

/// A very dumb implementation of `EstimateNextSessionRotation`. At the moment of writing, this
/// is more to satisfy type requirements rather than to test anything.
pub struct TestNextSessionRotation;

impl frame_support::traits::EstimateNextSessionRotation<u32> for TestNextSessionRotation {
	fn average_session_length() -> u32 {
		10
	}

	fn estimate_current_session_progress(_now: u32) -> (Option<Permill>, Weight) {
		(None, 0)
	}

	fn estimate_next_session_rotation(_now: u32) -> (Option<u32>, Weight) {
		(None, 0)
	}
}
