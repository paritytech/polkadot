// Copyright 2021 Parity Technologies (UK) Ltd.
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

use polkadot_primitives::v1::{Hash, ValidationCodeHash};
use std::collections::{
	btree_map::{self, BTreeMap},
	HashSet,
};

#[derive(Default)]
struct PvfData {
	judgement: Option<bool>,
	seen_in: HashSet<Hash>,
}

impl PvfData {
	/// Initialize a new `PvfData` which is awaiting for the initial judgement.
	fn pending(origin: Hash) -> Self {
		// Preallocate the hashset with 5 items. This is the anticipated maximum heads we can
		// deal at the same time. In the vast majority of the cases it will have length of 1.
		let mut seen_in = HashSet::with_capacity(5);
		seen_in.insert(origin);
		Self { judgement: None, seen_in }
	}

	pub fn seen_in(&mut self, relay_hash: Hash) {
		self.seen_in.insert(relay_hash);
	}

	/// Removes the given `relay_hash` from the set of seen in, and returns if the set is now empty.
	pub fn remove_origin(&mut self, relay_hash: &Hash) -> bool {
		self.seen_in.remove(relay_hash);
		self.seen_in.is_empty()
	}
}

pub struct InterestView {
	heads: BTreeMap<Hash, HashSet<ValidationCodeHash>>,
	pvfs: BTreeMap<ValidationCodeHash, PvfData>,
}

impl InterestView {
	pub fn new() -> Self {
		Self { heads: BTreeMap::new(), pvfs: BTreeMap::new() }
	}

	pub fn on_leaves_update(
		&mut self,
		activated: Option<(Hash, Vec<ValidationCodeHash>)>,
		deactivated: &[Hash],
	) -> Vec<ValidationCodeHash> {
		let mut newcomers = Vec::new();

		// TODO: it's important to run this first.
		for (head, pending_pvfs) in activated {
			for pvf in &pending_pvfs {
				match self.pvfs.entry(*pvf) {
					btree_map::Entry::Vacant(v) => {
						v.insert(PvfData::pending(head));
						newcomers.push(*pvf);
					},
					btree_map::Entry::Occupied(mut o) => {
						o.get_mut().seen_in(head);
					},
				}
			}
			self.heads.entry(head).or_default().extend(pending_pvfs);
		}

		for head in deactivated {
			let pvfs = self.heads.remove(&head);
			for pvf in pvfs.into_iter().flatten() {
				if let btree_map::Entry::Occupied(mut o) = self.pvfs.entry(pvf) {
					let now_empty = o.get_mut().remove_origin(&head);
					if now_empty {
						o.remove();
					}
				}
			}
		}

		newcomers
	}

	/// TODO:
	/// Returns `Err` if the given PVF hash is not known.
	pub fn on_judgement(&mut self, subject: ValidationCodeHash, accept: bool) -> Result<(), ()> {
		match self.pvfs.get_mut(&subject) {
			Some(data) => {
				data.judgement = Some(accept);
				Ok(())
			},
			None => Err(()),
		}
	}

	pub fn judgements(&self) -> impl Iterator<Item = (ValidationCodeHash, bool)> + '_ {
		self.pvfs.iter().filter_map(|(code_hash, data)| match data.judgement {
			Some(accept) => Some((*code_hash, accept)),
			None => None,
		})
	}
}
