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

use std::{cell::RefCell, collections::HashMap};
use parity_scale_codec::{Encode, Decode};
use sp_runtime::traits::SaturatedConversion;
use frame_support::dispatch::DispatchResult;
use primitives::v1::Id as ParaId;
use crate::traits::Registrar;

thread_local! {
	static OPERATIONS: RefCell<Vec<(ParaId, u32, bool)>> = RefCell::new(Vec::new());
	static PARACHAINS: RefCell<Vec<ParaId>> = RefCell::new(Vec::new());
	static PARATHREADS: RefCell<Vec<ParaId>> = RefCell::new(Vec::new());
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

	fn make_parachain(id: ParaId) -> DispatchResult {
		OPERATIONS.with(|x| x.borrow_mut().push((id, frame_system::Module::<T>::block_number().saturated_into(), true)));
		PARACHAINS.with(|x| {
			let mut parachains = x.borrow_mut();
			match parachains.binary_search(&id) {
				Ok(_) => {},
				Err(i) => parachains.insert(i, id),
			}
		});
		PARATHREADS.with(|x| {
			let mut parathreads = x.borrow_mut();
			match parathreads.binary_search(&id) {
				Ok(i) => {
					parathreads.remove(i);
				},
				Err(_) => {},
			}
		});
		Ok(())
	}
	fn make_parathread(id: ParaId) -> DispatchResult {
		OPERATIONS.with(|x| x.borrow_mut().push((id, frame_system::Module::<T>::block_number().saturated_into(), false)));
		PARACHAINS.with(|x| {
			let mut parachains = x.borrow_mut();
			match parachains.binary_search(&id) {
				Ok(i) => {
					parachains.remove(i);
				},
				Err(_) =>{},
			}
		});
		PARATHREADS.with(|x| {
			let mut parathreads = x.borrow_mut();
			match parathreads.binary_search(&id) {
				Ok(_) => {},
				Err(i) => parathreads.insert(i, id),
			}
		});
		Ok(())
	}
}

impl<T: frame_system::Config> TestRegistrar<T> {
	pub fn operations() -> Vec<(ParaId, T::BlockNumber, bool)> {
		OPERATIONS.with(|x| x.borrow().iter().map(|(p, b, c)| (*p, (*b).into(), *c)).collect::<Vec<_>>())
	}

	pub fn parachains() -> Vec<ParaId> {
		PARACHAINS.with(|x| x.borrow().clone())
	}

	pub fn parathreads() -> Vec<ParaId> {
		PARATHREADS.with(|x| x.borrow().clone())
	}

	pub fn set_manager(id: ParaId, manager: T::AccountId) {
		MANAGERS.with(|x| x.borrow_mut().insert(id, manager.encode()));
	}
}
