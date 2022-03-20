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

use polkadot_primitives::v2::{Hash, ValidationCodeHash};
use std::collections::{
	btree_map::{self, BTreeMap},
	HashSet,
};

/// Whether the PVF passed pre-checking or not.
#[derive(Copy, Clone, Debug)]
pub enum Judgement {
	Valid,
	Invalid,
}

impl Judgement {
	/// Whether the PVF is valid or not.
	pub fn is_valid(&self) -> bool {
		match self {
			Judgement::Valid => true,
			Judgement::Invalid => false,
		}
	}
}

/// Data about a particular validation code.
#[derive(Default, Debug)]
struct PvfData {
	/// If `Some` then the PVF pre-checking was run for this PVF. If `None` we are either waiting
	/// for the judgement to come in or the PVF pre-checking failed.
	judgement: Option<Judgement>,

	/// The set of block hashes where this PVF was seen.
	seen_in: HashSet<Hash>,
}

impl PvfData {
	/// Initialize a new `PvfData` which is awaiting for the initial judgement.
	fn pending(origin: Hash) -> Self {
		// Preallocate the hashset with 5 items. This is the anticipated maximum leaves we can
		// deal at the same time. In the vast majority of the cases it will have length of 1.
		let mut seen_in = HashSet::with_capacity(5);
		seen_in.insert(origin);
		Self { judgement: None, seen_in }
	}

	/// Mark a the `PvfData` as seen in the provided relay-chain block referenced by `relay_hash`.
	pub fn seen_in(&mut self, relay_hash: Hash) {
		self.seen_in.insert(relay_hash);
	}

	/// Removes the given `relay_hash` from the set of seen in, and returns if the set is now empty.
	pub fn remove_origin(&mut self, relay_hash: &Hash) -> bool {
		self.seen_in.remove(relay_hash);
		self.seen_in.is_empty()
	}
}

/// The result of [`InterestView::on_leaves_update`].
pub struct OnLeavesUpdateOutcome {
	/// The list of PVFs that we first seen in the activated block.
	pub newcomers: Vec<ValidationCodeHash>,
	/// The number of PVFs that were removed from the view.
	pub left_num: usize,
}

/// A structure that keeps track of relevant PVFs and judgements about them. A relevant PVF is one
/// that resides in at least a single active leaf.
#[derive(Debug)]
pub struct InterestView {
	active_leaves: BTreeMap<Hash, HashSet<ValidationCodeHash>>,
	pvfs: BTreeMap<ValidationCodeHash, PvfData>,
}

impl InterestView {
	pub fn new() -> Self {
		Self { active_leaves: BTreeMap::new(), pvfs: BTreeMap::new() }
	}

	pub fn on_leaves_update(
		&mut self,
		activated: Option<(Hash, Vec<ValidationCodeHash>)>,
		deactivated: &[Hash],
	) -> OnLeavesUpdateOutcome {
		let mut newcomers = Vec::new();

		if let Some((leaf, pending_pvfs)) = activated {
			for pvf in &pending_pvfs {
				match self.pvfs.entry(*pvf) {
					btree_map::Entry::Vacant(v) => {
						v.insert(PvfData::pending(leaf));
						newcomers.push(*pvf);
					},
					btree_map::Entry::Occupied(mut o) => {
						o.get_mut().seen_in(leaf);
					},
				}
			}
			self.active_leaves.entry(leaf).or_default().extend(pending_pvfs);
		}

		let mut left_num = 0;
		for leaf in deactivated {
			let pvfs = self.active_leaves.remove(leaf);
			for pvf in pvfs.into_iter().flatten() {
				if let btree_map::Entry::Occupied(mut o) = self.pvfs.entry(pvf) {
					let now_empty = o.get_mut().remove_origin(leaf);
					if now_empty {
						left_num += 1;
						o.remove();
					}
				}
			}
		}

		OnLeavesUpdateOutcome { newcomers, left_num }
	}

	/// Handles a new judgement for the given `pvf`.
	///
	/// Returns `Err` if the given PVF hash is not known.
	pub fn on_judgement(
		&mut self,
		subject: ValidationCodeHash,
		judgement: Judgement,
	) -> Result<(), ()> {
		match self.pvfs.get_mut(&subject) {
			Some(data) => {
				data.judgement = Some(judgement);
				Ok(())
			},
			None => Err(()),
		}
	}

	/// Returns all PVFs that previously received a judgement.
	pub fn judgements(&self) -> impl Iterator<Item = (ValidationCodeHash, Judgement)> + '_ {
		self.pvfs
			.iter()
			.filter_map(|(code_hash, data)| data.judgement.map(|judgement| (*code_hash, judgement)))
	}
}
