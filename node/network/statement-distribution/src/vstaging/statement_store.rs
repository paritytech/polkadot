// Copyright 2022 Parity Technologies (UK) Ltd.
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

//! A store of all statements under a given relay-parent.
//!
//! This structure doesn't attempt to do any spam protection, which must
//! be provided at a higher level.
//!
//! This keeps track of statements submitted with a number of different of
//! views into this data: views based on the candidate, views based on the validator
//! groups, and views based on the validators themselves.

use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use polkadot_primitives::vstaging::{
	CandidateHash, CompactStatement, GroupIndex, SignedStatement, ValidatorIndex,
};
use std::collections::hash_map::{Entry as HEntry, HashMap};

/// Storage for statements. Intended to be used for statements signed under
/// the same relay-parent. See module docs for more details.
pub struct StatementStore {
	groups: Vec<Vec<ValidatorIndex>>,
	validator_meta: HashMap<ValidatorIndex, ValidatorMeta>,
	group_statements: HashMap<(GroupIndex, CandidateHash), GroupStatements>,
	known_statements: HashMap<Fingerprint, SignedStatement>,
}

impl StatementStore {
	/// Create a new [`StatementStore`]
	pub fn new(groups: Vec<Vec<ValidatorIndex>>) -> Self {
		let mut validator_meta = HashMap::new();
		for (g, group) in groups.iter().enumerate() {
			for (i, v) in group.iter().enumerate() {
				validator_meta.insert(
					v,
					ValidatorMeta {
						seconded_count: 0,
						within_group_index: i,
						group: GroupIndex(g as _),
					},
				);
			}
		}

		StatementStore {
			groups,
			validator_meta: HashMap::new(),
			group_statements: HashMap::new(),
			known_statements: HashMap::new(),
		}
	}

	/// Insert a statement. Returns `true` if was not known already, `false` if it was.
	/// Ignores statements by unknown validators and returns `false`.
	pub fn insert(&mut self, statement: SignedStatement) -> bool {
		let validator_index = statement.validator_index();

		let validator_meta = match self.validator_meta.get_mut(&validator_index) {
			None => return false,
			Some(m) => m,
		};

		let compact = statement.payload().clone();
		let fingerprint = (validator_index, compact.clone());
		match self.known_statements.entry(fingerprint) {
			HEntry::Occupied(_) => return false,
			HEntry::Vacant(mut e) => {
				e.insert(statement);
			},
		}

		let candidate_hash = *compact.candidate_hash();
		let seconded = if let CompactStatement::Seconded(_) = compact { true } else { false };

		// cross-reference updates.
		{
			let group_index = validator_meta.group;
			let group = self.groups.get(group_index.0 as usize).expect(
				"we only have meta info on validators confirmed to be \
					in groups at construction; qed",
			);

			let group_statements = self
				.group_statements
				.entry((group_index, candidate_hash))
				.or_insert_with(|| GroupStatements::with_group_size(group.len()));

			if seconded {
				validator_meta.seconded_count += 1;
				group_statements.note_seconded(validator_meta.within_group_index);
			} else {
				group_statements.note_validated(validator_meta.within_group_index);
			}
		}

		true
	}
}

type Fingerprint = (ValidatorIndex, CompactStatement);

struct ValidatorMeta {
	group: GroupIndex,
	within_group_index: usize,
	seconded_count: usize,
}

struct GroupStatements {
	seconded_statements: BitVec<u8, BitOrderLsb0>,
	valid_statements: BitVec<u8, BitOrderLsb0>,
}

impl GroupStatements {
	fn with_group_size(group_size: usize) -> Self {
		GroupStatements {
			seconded_statements: BitVec::repeat(false, group_size),
			valid_statements: BitVec::repeat(false, group_size),
		}
	}

	fn note_seconded(&mut self, within_group_index: usize) {
		self.seconded_statements.set(within_group_index, true);
	}

	fn note_validated(&mut self, within_group_index: usize) {
		self.valid_statements.set(within_group_index, true);
	}
}
