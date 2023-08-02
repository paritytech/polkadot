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
use polkadot_node_network_protocol::vstaging::StatementFilter;
use polkadot_primitives::vstaging::{
	CandidateHash, CompactStatement, GroupIndex, SignedStatement, ValidatorIndex,
};
use std::collections::hash_map::{Entry as HEntry, HashMap};

use super::groups::Groups;

/// Possible origins of a statement.
pub enum StatementOrigin {
	/// The statement originated locally.
	Local,
	/// The statement originated from a remote peer.
	Remote,
}

impl StatementOrigin {
	fn is_local(&self) -> bool {
		match *self {
			StatementOrigin::Local => true,
			StatementOrigin::Remote => false,
		}
	}
}

struct StoredStatement {
	statement: SignedStatement,
	known_by_backing: bool,
}

/// Storage for statements. Intended to be used for statements signed under
/// the same relay-parent. See module docs for more details.
pub struct StatementStore {
	validator_meta: HashMap<ValidatorIndex, ValidatorMeta>,

	// we keep statements per-group because even though only one group _should_ be
	// producing statements about a candidate, until we have the candidate receipt
	// itself, we can't tell which group that is.
	group_statements: HashMap<(GroupIndex, CandidateHash), GroupStatements>,
	known_statements: HashMap<Fingerprint, StoredStatement>,
}

impl StatementStore {
	/// Create a new [`StatementStore`]
	pub fn new(groups: &Groups) -> Self {
		let mut validator_meta = HashMap::new();
		for (g, group) in groups.all().iter().enumerate() {
			for (i, v) in group.iter().enumerate() {
				validator_meta.insert(
					*v,
					ValidatorMeta {
						seconded_count: 0,
						within_group_index: i,
						group: GroupIndex(g as _),
					},
				);
			}
		}

		StatementStore {
			validator_meta,
			group_statements: HashMap::new(),
			known_statements: HashMap::new(),
		}
	}

	/// Insert a statement. Returns `true` if was not known already, `false` if it was.
	/// Ignores statements by unknown validators and returns an error.
	pub fn insert(
		&mut self,
		groups: &Groups,
		statement: SignedStatement,
		origin: StatementOrigin,
	) -> Result<bool, ValidatorUnknown> {
		let validator_index = statement.validator_index();
		let validator_meta = match self.validator_meta.get_mut(&validator_index) {
			None => return Err(ValidatorUnknown),
			Some(m) => m,
		};

		let compact = statement.payload().clone();
		let fingerprint = (validator_index, compact.clone());
		match self.known_statements.entry(fingerprint) {
			HEntry::Occupied(mut e) => {
				if let StatementOrigin::Local = origin {
					e.get_mut().known_by_backing = true;
				}

				return Ok(false)
			},
			HEntry::Vacant(e) => {
				e.insert(StoredStatement { statement, known_by_backing: origin.is_local() });
			},
		}

		let candidate_hash = *compact.candidate_hash();
		let seconded = if let CompactStatement::Seconded(_) = compact { true } else { false };

		// cross-reference updates.
		{
			let group_index = validator_meta.group;
			let group = match groups.get(group_index) {
				Some(g) => g,
				None => {
					gum::error!(
						target: crate::LOG_TARGET,
						?group_index,
						"groups passed into `insert` differ from those used at store creation"
					);

					return Err(ValidatorUnknown)
				},
			};

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

		Ok(true)
	}

	/// Fill a `StatementFilter` to be used in the grid topology with all statements
	/// we are already aware of.
	pub fn fill_statement_filter(
		&self,
		group_index: GroupIndex,
		candidate_hash: CandidateHash,
		statement_filter: &mut StatementFilter,
	) {
		if let Some(statements) = self.group_statements.get(&(group_index, candidate_hash)) {
			statement_filter.seconded_in_group |= statements.seconded.as_bitslice();
			statement_filter.validated_in_group |= statements.valid.as_bitslice();
		}
	}

	/// Get an iterator over stored signed statements by the group conforming to the
	/// given filter.
	///
	/// Seconded statements are provided first.
	pub fn group_statements<'a>(
		&'a self,
		groups: &'a Groups,
		group_index: GroupIndex,
		candidate_hash: CandidateHash,
		filter: &'a StatementFilter,
	) -> impl Iterator<Item = &'a SignedStatement> + 'a {
		let group_validators = groups.get(group_index);

		let seconded_statements = filter
			.seconded_in_group
			.iter_ones()
			.filter_map(move |i| group_validators.as_ref().and_then(|g| g.get(i)))
			.filter_map(move |v| {
				self.known_statements.get(&(*v, CompactStatement::Seconded(candidate_hash)))
			})
			.map(|s| &s.statement);

		let valid_statements = filter
			.validated_in_group
			.iter_ones()
			.filter_map(move |i| group_validators.as_ref().and_then(|g| g.get(i)))
			.filter_map(move |v| {
				self.known_statements.get(&(*v, CompactStatement::Valid(candidate_hash)))
			})
			.map(|s| &s.statement);

		seconded_statements.chain(valid_statements)
	}

	/// Get the full statement of this kind issued by this validator, if it is known.
	pub fn validator_statement(
		&self,
		validator_index: ValidatorIndex,
		statement: CompactStatement,
	) -> Option<&SignedStatement> {
		self.known_statements.get(&(validator_index, statement)).map(|s| &s.statement)
	}

	/// Get an iterator over all statements marked as being unknown by the backing subsystem.
	pub fn fresh_statements_for_backing<'a>(
		&'a self,
		validators: &'a [ValidatorIndex],
		candidate_hash: CandidateHash,
	) -> impl Iterator<Item = &SignedStatement> + 'a {
		let s_st = CompactStatement::Seconded(candidate_hash);
		let v_st = CompactStatement::Valid(candidate_hash);

		validators
			.iter()
			.flat_map(move |v| {
				let a = self.known_statements.get(&(*v, s_st.clone()));
				let b = self.known_statements.get(&(*v, v_st.clone()));

				a.into_iter().chain(b)
			})
			.filter(|stored| !stored.known_by_backing)
			.map(|stored| &stored.statement)
	}

	/// Get the amount of known `Seconded` statements by the given validator index.
	pub fn seconded_count(&self, validator_index: &ValidatorIndex) -> usize {
		self.validator_meta.get(validator_index).map_or(0, |m| m.seconded_count)
	}

	/// Note that a statement is known by the backing subsystem.
	pub fn note_known_by_backing(
		&mut self,
		validator_index: ValidatorIndex,
		statement: CompactStatement,
	) {
		if let Some(stored) = self.known_statements.get_mut(&(validator_index, statement)) {
			stored.known_by_backing = true;
		}
	}
}

/// Error indicating that the validator was unknown.
pub struct ValidatorUnknown;

type Fingerprint = (ValidatorIndex, CompactStatement);

struct ValidatorMeta {
	group: GroupIndex,
	within_group_index: usize,
	seconded_count: usize,
}

struct GroupStatements {
	seconded: BitVec<u8, BitOrderLsb0>,
	valid: BitVec<u8, BitOrderLsb0>,
}

impl GroupStatements {
	fn with_group_size(group_size: usize) -> Self {
		GroupStatements {
			seconded: BitVec::repeat(false, group_size),
			valid: BitVec::repeat(false, group_size),
		}
	}

	fn note_seconded(&mut self, within_group_index: usize) {
		self.seconded.set(within_group_index, true);
	}

	fn note_validated(&mut self, within_group_index: usize) {
		self.valid.set(within_group_index, true);
	}
}
