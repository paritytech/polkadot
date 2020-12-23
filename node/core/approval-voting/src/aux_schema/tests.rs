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

//! Tests for the aux-schema of approval voting.

use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct TestStore {
	inner: RefCell<HashMap<Vec<u8>, Vec<u8>>>,
}

impl AuxStore for TestStore {
	fn insert_aux<'a, 'b: 'a, 'c: 'a, I, D>(&self, insertions: I, deletions: D) -> sp_blockchain::Result<()>
		where I: IntoIterator<Item = &'a (&'c [u8], &'c [u8])>, D: IntoIterator<Item = &'a &'b [u8]>
	{
		let mut store = self.inner.borrow_mut();

		// insertions before deletions.
		for (k, v) in insertions {
			store.insert(k.to_vec(), v.to_vec());
		}

		for k in deletions {
			store.remove(&k[..]);
		}

		Ok(())
	}

	fn get_aux(&self, key: &[u8]) -> sp_blockchain::Result<Option<Vec<u8>>> {
		Ok(self.inner.borrow().get(key).map(|v| v.clone()))
	}
}
