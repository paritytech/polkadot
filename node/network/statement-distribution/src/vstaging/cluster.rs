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

//! Direct distribution of statements within a cluster,
//! even those concerning candidates which are not yet backed.
//!
//! Members of a validation group assigned to a para at a given relay-parent
//! always distribute statements directly to each other.
//!
//! The main way we limit the amount of candidates that have to be handled by
//! the system is to limit the amount of `Seconded` messages that we allow
//! each validator to issue at each relay-parent. Since the amount of relay-parents
//! that we have to deal with at any time is itself bounded, this lets us bound
//! the memory and work that we have here. Bounding `Seconded` statements is enough
//! because they imply a bounded amount of `Valid` statements about the same candidate
//! which may follow.
//!
//! The motivation for this piece of code is that the statements that each validator
//! sees may differ. i.e. even though a validator is allowed to issue X `Seconded`
//! statements at a relay-parent, they may in fact issue X*2 and issue one set to
//! one partition of the backing group and one set to another. Of course, in practice
//! these types of partitions will not exist, but in the worst case each validator in the
//! group would see an entirely different set of X `Seconded` statements from some validator
//! and each validator is in its own partition. After that partition resolves, we'd have to
//! deal with up to `limit*group_size` `Seconded` statements from that validator. And then
//! if every validator in the group does the same thing, we're dealing with something like
//! `limit*group_size^2` `Seconded` statements in total.
//!
//! Given that both our group sizes and our limits per relay-parent are small, this is
//! quite manageable, and the utility here lets us deal with it in only a few kilobytes
//! of memory.
//!
//! It's also worth noting that any case where a validator issues more than the legal limit
//! of `Seconded` statements at a relay parent is trivially slashable on-chain, which means
//! the 'worst case' adversary that this code defends against is effectively lighting money
//! on fire. Nevertheless, we handle the case here to ensure that the behavior of the
//! system is well-defined even if an adversary is willing to be slashed.
//!
//! More concretely, this module exposes a [`ClusterTracker`] utility which allows us to determine
//! whether to accept or reject messages from other validators in the same group as we
//! are in, based on _the most charitable possible interpretation of our protocol rules_,
//! and to keep track of what we have sent to other validators in the group and what we may
//! continue to send them.

use polkadot_primitives::vstaging::{CandidateHash, CompactStatement, ValidatorIndex};

use std::collections::{HashMap, HashSet};

#[derive(Hash, PartialEq, Eq)]
struct ValidStatementManifest {
	remote: ValidatorIndex,
	originator: ValidatorIndex,
	candidate_hash: CandidateHash,
}

// A piece of knowledge about a candidate
#[derive(Hash, Clone, PartialEq, Eq)]
enum Knowledge {
	// General knowledge.
	General(CandidateHash),
	// Specific knowledge of a given statement (with its originator)
	Specific(CompactStatement, ValidatorIndex),
}

// Knowledge paired with its source.
#[derive(Hash, Clone, PartialEq, Eq)]
enum TaggedKnowledge {
	// Knowledge we have received from the validator on the p2p layer.
	IncomingP2P(Knowledge),
	// Knowledge we have sent to the validator on the p2p layer.
	OutgoingP2P(Knowledge),
	// Knowledge of candidates the validator has seconded.
	// This is limited only to `Seconded` statements we have accepted
	// _without prejudice_.
	Seconded(CandidateHash),
}

/// Utility for keeping track of limits on direct statements within a group.
///
/// See module docs for more details.
pub struct ClusterTracker {
	validators: Vec<ValidatorIndex>,
	seconding_limit: usize,
	knowledge: HashMap<ValidatorIndex, HashSet<TaggedKnowledge>>,
	// Statements known locally which haven't been sent to particular validators.
	// maps target validator to (originator, statement) pairs.
	pending: HashMap<ValidatorIndex, HashSet<(ValidatorIndex, CompactStatement)>>,
}

impl ClusterTracker {
	/// Instantiate a new `ClusterTracker` tracker. Fails if `cluster_validators` is empty
	pub fn new(cluster_validators: Vec<ValidatorIndex>, seconding_limit: usize) -> Option<Self> {
		if cluster_validators.is_empty() {
			return None
		}
		Some(ClusterTracker {
			validators: cluster_validators,
			seconding_limit,
			knowledge: HashMap::new(),
			pending: HashMap::new(),
		})
	}

	/// Query whether we can receive some statement from the given validator.
	///
	/// This does no deduplication of `Valid` statements.
	pub fn can_receive(
		&self,
		sender: ValidatorIndex,
		originator: ValidatorIndex,
		statement: CompactStatement,
	) -> Result<Accept, RejectIncoming> {
		if !self.is_in_group(sender) || !self.is_in_group(originator) {
			return Err(RejectIncoming::NotInGroup)
		}

		if self.they_sent(sender, Knowledge::Specific(statement.clone(), originator)) {
			return Err(RejectIncoming::Duplicate)
		}

		match statement {
			CompactStatement::Seconded(candidate_hash) => {
				// check whether the sender has not sent too many seconded statements for the
				// originator. we know by the duplicate check above that this iterator doesn't
				// include the statement itself.
				let other_seconded_for_orig_from_remote = self
					.knowledge
					.get(&sender)
					.into_iter()
					.flat_map(|v_knowledge| v_knowledge.iter())
					.filter(|k| match k {
						TaggedKnowledge::IncomingP2P(Knowledge::Specific(
							CompactStatement::Seconded(_),
							orig,
						)) if orig == &originator => true,
						_ => false,
					})
					.count();

				if other_seconded_for_orig_from_remote == self.seconding_limit {
					return Err(RejectIncoming::ExcessiveSeconded)
				}

				// at this point, it doesn't seem like the remote has done anything wrong.
				if self.seconded_already_or_within_limit(originator, candidate_hash) {
					Ok(Accept::Ok)
				} else {
					Ok(Accept::WithPrejudice)
				}
			},
			CompactStatement::Valid(candidate_hash) => {
				if !self.knows_candidate(sender, candidate_hash) {
					return Err(RejectIncoming::CandidateUnknown)
				}

				Ok(Accept::Ok)
			},
		}
	}

	/// Note that we issued a statement. This updates internal structures.
	pub fn note_issued(&mut self, originator: ValidatorIndex, statement: CompactStatement) {
		for cluster_member in &self.validators {
			if !self.they_know_statement(*cluster_member, originator, statement.clone()) {
				// add the statement to pending knowledge for all peers
				// which don't know the statement.
				self.pending
					.entry(*cluster_member)
					.or_default()
					.insert((originator, statement.clone()));
			}
		}
	}

	/// Note that we accepted an incoming statement. This updates internal structures.
	///
	/// Should only be called after a successful `can_receive` call.
	pub fn note_received(
		&mut self,
		sender: ValidatorIndex,
		originator: ValidatorIndex,
		statement: CompactStatement,
	) {
		for cluster_member in &self.validators {
			if cluster_member == &sender {
				if let Some(pending) = self.pending.get_mut(&sender) {
					pending.remove(&(originator, statement.clone()));
				}
			} else if !self.they_know_statement(*cluster_member, originator, statement.clone()) {
				// add the statement to pending knowledge for all peers
				// which don't know the statement.
				self.pending
					.entry(*cluster_member)
					.or_default()
					.insert((originator, statement.clone()));
			}
		}

		{
			let sender_knowledge = self.knowledge.entry(sender).or_default();
			sender_knowledge.insert(TaggedKnowledge::IncomingP2P(Knowledge::Specific(
				statement.clone(),
				originator,
			)));

			if let CompactStatement::Seconded(candidate_hash) = statement.clone() {
				sender_knowledge
					.insert(TaggedKnowledge::IncomingP2P(Knowledge::General(candidate_hash)));
			}
		}

		if let CompactStatement::Seconded(candidate_hash) = statement {
			// since we accept additional `Seconded` statements beyond the limits
			// 'with prejudice', we must respect the limit here.
			if self.seconded_already_or_within_limit(originator, candidate_hash) {
				let originator_knowledge = self.knowledge.entry(originator).or_default();
				originator_knowledge.insert(TaggedKnowledge::Seconded(candidate_hash));
			}
		}
	}

	/// Query whether we can send a statement to a given validator.
	pub fn can_send(
		&self,
		target: ValidatorIndex,
		originator: ValidatorIndex,
		statement: CompactStatement,
	) -> Result<(), RejectOutgoing> {
		if !self.is_in_group(target) || !self.is_in_group(originator) {
			return Err(RejectOutgoing::NotInGroup)
		}

		if self.they_know_statement(target, originator, statement.clone()) {
			return Err(RejectOutgoing::Known)
		}

		match statement {
			CompactStatement::Seconded(candidate_hash) => {
				// we send the same `Seconded` statements to all our peers, and only the first `k`
				// from each originator.
				if !self.seconded_already_or_within_limit(originator, candidate_hash) {
					return Err(RejectOutgoing::ExcessiveSeconded)
				}

				Ok(())
			},
			CompactStatement::Valid(candidate_hash) => {
				if !self.knows_candidate(target, candidate_hash) {
					return Err(RejectOutgoing::CandidateUnknown)
				}

				Ok(())
			},
		}
	}

	/// Note that we sent an outgoing statement to a peer in the group.
	/// This must be preceded by a successful `can_send` call.
	pub fn note_sent(
		&mut self,
		target: ValidatorIndex,
		originator: ValidatorIndex,
		statement: CompactStatement,
	) {
		{
			let target_knowledge = self.knowledge.entry(target).or_default();
			target_knowledge.insert(TaggedKnowledge::OutgoingP2P(Knowledge::Specific(
				statement.clone(),
				originator,
			)));

			if let CompactStatement::Seconded(candidate_hash) = statement.clone() {
				target_knowledge
					.insert(TaggedKnowledge::OutgoingP2P(Knowledge::General(candidate_hash)));
			}
		}

		if let CompactStatement::Seconded(candidate_hash) = statement {
			let originator_knowledge = self.knowledge.entry(originator).or_default();
			originator_knowledge.insert(TaggedKnowledge::Seconded(candidate_hash));
		}

		if let Some(pending) = self.pending.get_mut(&target) {
			pending.remove(&(originator, statement));
		}
	}

	/// Get all targets as validator-indices. This doesn't attempt to filter
	/// out the local validator index.
	pub fn targets(&self) -> &[ValidatorIndex] {
		&self.validators
	}

	/// Get all possible senders for the given originator.
	/// Returns the empty slice in the case that the originator
	/// is not part of the cluster.
	// note: this API is future-proofing for a case where we may
	// extend clusters beyond just the assigned group, for optimization
	// purposes.
	pub fn senders_for_originator(&self, originator: ValidatorIndex) -> &[ValidatorIndex] {
		if self.validators.contains(&originator) {
			&self.validators[..]
		} else {
			&[]
		}
	}

	/// Whether a validator knows the candidate is `Seconded`.
	pub fn knows_candidate(
		&self,
		validator: ValidatorIndex,
		candidate_hash: CandidateHash,
	) -> bool {
		// we sent, they sent, or they signed and we received from someone else.

		self.we_sent_seconded(validator, candidate_hash) ||
			self.they_sent_seconded(validator, candidate_hash) ||
			self.validator_seconded(validator, candidate_hash)
	}

	/// Returns a Vec of pending statements to be sent to a particular validator
	/// index. `Seconded` statements are sorted to the front of the vector.
	///
	/// Pending statements have the form (originator, compact statement).
	pub fn pending_statements_for(
		&self,
		target: ValidatorIndex,
	) -> Vec<(ValidatorIndex, CompactStatement)> {
		let mut v = self
			.pending
			.get(&target)
			.map(|x| x.iter().cloned().collect::<Vec<_>>())
			.unwrap_or_default();

		v.sort_by_key(|(_, s)| match s {
			CompactStatement::Seconded(_) => 0u8,
			CompactStatement::Valid(_) => 1u8,
		});

		v
	}

	// returns true if it's legal to accept a new `Seconded` message from this validator.
	// This is either
	//   1. because we've already accepted it.
	//   2. because there's space for more seconding.
	fn seconded_already_or_within_limit(
		&self,
		validator: ValidatorIndex,
		candidate_hash: CandidateHash,
	) -> bool {
		let seconded_other_candidates = self
			.knowledge
			.get(&validator)
			.into_iter()
			.flat_map(|v_knowledge| v_knowledge.iter())
			.filter(|k| match k {
				TaggedKnowledge::Seconded(c) if c != &candidate_hash => true,
				_ => false,
			})
			.count();

		// This fulfills both properties by under-counting when the validator is at the limit
		// but _has_ seconded the candidate already.
		seconded_other_candidates < self.seconding_limit
	}

	fn they_know_statement(
		&self,
		validator: ValidatorIndex,
		originator: ValidatorIndex,
		statement: CompactStatement,
	) -> bool {
		let knowledge = Knowledge::Specific(statement, originator);
		self.we_sent(validator, knowledge.clone()) || self.they_sent(validator, knowledge)
	}

	fn they_sent(&self, validator: ValidatorIndex, knowledge: Knowledge) -> bool {
		self.knowledge
			.get(&validator)
			.map_or(false, |k| k.contains(&TaggedKnowledge::IncomingP2P(knowledge)))
	}

	fn we_sent(&self, validator: ValidatorIndex, knowledge: Knowledge) -> bool {
		self.knowledge
			.get(&validator)
			.map_or(false, |k| k.contains(&TaggedKnowledge::OutgoingP2P(knowledge)))
	}

	fn we_sent_seconded(&self, validator: ValidatorIndex, candidate_hash: CandidateHash) -> bool {
		self.we_sent(validator, Knowledge::General(candidate_hash))
	}

	fn they_sent_seconded(&self, validator: ValidatorIndex, candidate_hash: CandidateHash) -> bool {
		self.they_sent(validator, Knowledge::General(candidate_hash))
	}

	fn validator_seconded(&self, validator: ValidatorIndex, candidate_hash: CandidateHash) -> bool {
		self.knowledge
			.get(&validator)
			.map_or(false, |k| k.contains(&TaggedKnowledge::Seconded(candidate_hash)))
	}

	fn is_in_group(&self, validator: ValidatorIndex) -> bool {
		self.validators.contains(&validator)
	}
}

/// Incoming statement was accepted.
#[derive(Debug, PartialEq)]
pub enum Accept {
	/// Neither the peer nor the originator have apparently exceeded limits.
	/// Candidate or statement may already be known.
	Ok,
	/// Accept the message; the peer hasn't exceeded limits but the originator has.
	WithPrejudice,
}

/// Incoming statement was rejected.
#[derive(Debug, PartialEq)]
pub enum RejectIncoming {
	/// Peer sent excessive `Seconded` statements.
	ExcessiveSeconded,
	/// Sender or originator is not in the group.
	NotInGroup,
	/// Candidate is unknown to us. Only applies to `Valid` statements.
	CandidateUnknown,
	/// Statement is duplicate.
	Duplicate,
}

/// Outgoing statement was rejected.
#[derive(Debug, PartialEq)]
pub enum RejectOutgoing {
	/// Candidate was unknown. Only applies to `Valid` statements.
	CandidateUnknown,
	/// We attempted to send excessive `Seconded` statements.
	/// indicates a bug on the local node's code.
	ExcessiveSeconded,
	/// The statement was already known to the peer.
	Known,
	/// Target or originator not in the group.
	NotInGroup,
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::vstaging::Hash;

	#[test]
	fn rejects_incoming_outside_of_group() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;

		let tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(100),
				ValidatorIndex(5),
				CompactStatement::Seconded(CandidateHash(Hash::repeat_byte(1))),
			),
			Err(RejectIncoming::NotInGroup),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(100),
				CompactStatement::Seconded(CandidateHash(Hash::repeat_byte(1))),
			),
			Err(RejectIncoming::NotInGroup),
		);
	}

	#[test]
	fn begrudgingly_accepts_too_many_seconded_from_multiple_peers() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;
		let hash_a = CandidateHash(Hash::repeat_byte(1));
		let hash_b = CandidateHash(Hash::repeat_byte(2));
		let hash_c = CandidateHash(Hash::repeat_byte(3));

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			),
			Ok(Accept::Ok),
		);
		tracker.note_received(
			ValidatorIndex(5),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_a),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_b),
			),
			Ok(Accept::Ok),
		);
		tracker.note_received(
			ValidatorIndex(5),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_b),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_c),
			),
			Err(RejectIncoming::ExcessiveSeconded),
		);
	}

	#[test]
	fn rejects_too_many_seconded_from_sender() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;
		let hash_a = CandidateHash(Hash::repeat_byte(1));
		let hash_b = CandidateHash(Hash::repeat_byte(2));
		let hash_c = CandidateHash(Hash::repeat_byte(3));

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			),
			Ok(Accept::Ok),
		);
		tracker.note_received(
			ValidatorIndex(5),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_a),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_b),
			),
			Ok(Accept::Ok),
		);
		tracker.note_received(
			ValidatorIndex(5),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_b),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(200),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_c),
			),
			Ok(Accept::WithPrejudice),
		);
	}

	#[test]
	fn rejects_duplicates() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;
		let hash_a = CandidateHash(Hash::repeat_byte(1));

		let mut tracker = ClusterTracker::new(group, seconding_limit).expect("not empty");

		tracker.note_received(
			ValidatorIndex(5),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_a),
		);

		tracker.note_received(
			ValidatorIndex(5),
			ValidatorIndex(200),
			CompactStatement::Valid(hash_a),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			),
			Err(RejectIncoming::Duplicate),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(200),
				CompactStatement::Valid(hash_a),
			),
			Err(RejectIncoming::Duplicate),
		);
	}

	#[test]
	fn rejects_incoming_valid_without_seconded() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;

		let tracker = ClusterTracker::new(group, seconding_limit).expect("not empty");

		let hash_a = CandidateHash(Hash::repeat_byte(1));

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Valid(hash_a),
			),
			Err(RejectIncoming::CandidateUnknown),
		);
	}

	#[test]
	fn accepts_incoming_valid_after_receiving_seconded() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");
		let hash_a = CandidateHash(Hash::repeat_byte(1));

		tracker.note_received(
			ValidatorIndex(5),
			ValidatorIndex(200),
			CompactStatement::Seconded(hash_a),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Valid(hash_a),
			),
			Ok(Accept::Ok)
		);
	}

	#[test]
	fn accepts_incoming_valid_after_outgoing_seconded() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");
		let hash_a = CandidateHash(Hash::repeat_byte(1));

		tracker.note_sent(
			ValidatorIndex(5),
			ValidatorIndex(200),
			CompactStatement::Seconded(hash_a),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Valid(hash_a),
			),
			Ok(Accept::Ok)
		);
	}

	#[test]
	fn cannot_send_too_many_seconded_even_to_multiple_peers() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");
		let hash_a = CandidateHash(Hash::repeat_byte(1));
		let hash_b = CandidateHash(Hash::repeat_byte(2));
		let hash_c = CandidateHash(Hash::repeat_byte(3));

		tracker.note_sent(
			ValidatorIndex(200),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_a),
		);

		tracker.note_sent(
			ValidatorIndex(200),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_b),
		);

		assert_eq!(
			tracker.can_send(
				ValidatorIndex(200),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_c),
			),
			Err(RejectOutgoing::ExcessiveSeconded),
		);

		assert_eq!(
			tracker.can_send(
				ValidatorIndex(24),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_c),
			),
			Err(RejectOutgoing::ExcessiveSeconded),
		);
	}

	#[test]
	fn cannot_send_duplicate() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");
		let hash_a = CandidateHash(Hash::repeat_byte(1));

		tracker.note_sent(
			ValidatorIndex(200),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_a),
		);

		assert_eq!(
			tracker.can_send(
				ValidatorIndex(200),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			),
			Err(RejectOutgoing::Known),
		);
	}

	#[test]
	fn cannot_send_what_was_received() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 2;

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");
		let hash_a = CandidateHash(Hash::repeat_byte(1));

		tracker.note_received(
			ValidatorIndex(200),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_a),
		);

		assert_eq!(
			tracker.can_send(
				ValidatorIndex(200),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			),
			Err(RejectOutgoing::Known),
		);
	}

	// Ensure statements received with prejudice don't prevent sending later.
	#[test]
	fn can_send_statements_received_with_prejudice() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 1;

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");
		let hash_a = CandidateHash(Hash::repeat_byte(1));
		let hash_b = CandidateHash(Hash::repeat_byte(2));

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(200),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			),
			Ok(Accept::Ok),
		);

		tracker.note_received(
			ValidatorIndex(200),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_a),
		);

		assert_eq!(
			tracker.can_receive(
				ValidatorIndex(24),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_b),
			),
			Ok(Accept::WithPrejudice),
		);

		tracker.note_received(
			ValidatorIndex(24),
			ValidatorIndex(5),
			CompactStatement::Seconded(hash_b),
		);

		assert_eq!(
			tracker.can_send(
				ValidatorIndex(24),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			),
			Ok(()),
		);
	}

	// Test that the `pending_statements` are set whenever we receive a fresh statement.
	//
	// Also test that pending statements are sorted, with `Seconded` statements in the front.
	#[test]
	fn pending_statements_set_when_receiving_fresh_statements() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 1;

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");
		let hash_a = CandidateHash(Hash::repeat_byte(1));
		let hash_b = CandidateHash(Hash::repeat_byte(2));

		// Receive a 'Seconded' statement for candidate A.
		{
			assert_eq!(
				tracker.can_receive(
					ValidatorIndex(200),
					ValidatorIndex(5),
					CompactStatement::Seconded(hash_a),
				),
				Ok(Accept::Ok),
			);
			tracker.note_received(
				ValidatorIndex(200),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			);

			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(5)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
			assert_eq!(tracker.pending_statements_for(ValidatorIndex(200)), vec![]);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(24)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(146)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
		}

		// Receive a 'Valid' statement for candidate A.
		{
			// First, send a `Seconded` statement for the candidate.
			assert_eq!(
				tracker.can_send(
					ValidatorIndex(24),
					ValidatorIndex(200),
					CompactStatement::Seconded(hash_a)
				),
				Ok(())
			);
			tracker.note_sent(
				ValidatorIndex(24),
				ValidatorIndex(200),
				CompactStatement::Seconded(hash_a),
			);

			// We have to see that the candidate is known by the sender, e.g. we sent them
			// 'Seconded' above.
			assert_eq!(
				tracker.can_receive(
					ValidatorIndex(24),
					ValidatorIndex(200),
					CompactStatement::Valid(hash_a),
				),
				Ok(Accept::Ok),
			);
			tracker.note_received(
				ValidatorIndex(24),
				ValidatorIndex(200),
				CompactStatement::Valid(hash_a),
			);

			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(5)),
				vec![
					(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
					(ValidatorIndex(200), CompactStatement::Valid(hash_a))
				]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(200)),
				vec![(ValidatorIndex(200), CompactStatement::Valid(hash_a))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(24)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(146)),
				vec![
					(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
					(ValidatorIndex(200), CompactStatement::Valid(hash_a))
				]
			);
		}

		// Receive a 'Seconded' statement for candidate B.
		{
			assert_eq!(
				tracker.can_receive(
					ValidatorIndex(5),
					ValidatorIndex(146),
					CompactStatement::Seconded(hash_b),
				),
				Ok(Accept::Ok),
			);
			tracker.note_received(
				ValidatorIndex(5),
				ValidatorIndex(146),
				CompactStatement::Seconded(hash_b),
			);

			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(5)),
				vec![
					(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
					(ValidatorIndex(200), CompactStatement::Valid(hash_a))
				]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(200)),
				vec![
					(ValidatorIndex(146), CompactStatement::Seconded(hash_b)),
					(ValidatorIndex(200), CompactStatement::Valid(hash_a)),
				]
			);
			{
				let mut pending_statements = tracker.pending_statements_for(ValidatorIndex(24));
				pending_statements.sort();
				assert_eq!(
					pending_statements,
					vec![
						(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
						(ValidatorIndex(146), CompactStatement::Seconded(hash_b))
					],
				);
			}
			{
				let mut pending_statements = tracker.pending_statements_for(ValidatorIndex(146));
				pending_statements.sort();
				assert_eq!(
					pending_statements,
					vec![
						(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
						(ValidatorIndex(146), CompactStatement::Seconded(hash_b)),
						(ValidatorIndex(200), CompactStatement::Valid(hash_a)),
					]
				);
			}
		}
	}

	// Test that the `pending_statements` are updated when we send or receive statements from others
	// in the cluster.
	#[test]
	fn pending_statements_updated_when_sending_statements() {
		let group =
			vec![ValidatorIndex(5), ValidatorIndex(200), ValidatorIndex(24), ValidatorIndex(146)];

		let seconding_limit = 1;

		let mut tracker = ClusterTracker::new(group.clone(), seconding_limit).expect("not empty");
		let hash_a = CandidateHash(Hash::repeat_byte(1));
		let hash_b = CandidateHash(Hash::repeat_byte(2));

		// Receive a 'Seconded' statement for candidate A.
		{
			assert_eq!(
				tracker.can_receive(
					ValidatorIndex(200),
					ValidatorIndex(5),
					CompactStatement::Seconded(hash_a),
				),
				Ok(Accept::Ok),
			);
			tracker.note_received(
				ValidatorIndex(200),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			);

			// Pending statements should be updated.
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(5)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
			assert_eq!(tracker.pending_statements_for(ValidatorIndex(200)), vec![]);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(24)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(146)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
		}

		// Receive a 'Valid' statement for candidate B.
		{
			// First, send a `Seconded` statement for the candidate.
			assert_eq!(
				tracker.can_send(
					ValidatorIndex(24),
					ValidatorIndex(200),
					CompactStatement::Seconded(hash_b)
				),
				Ok(())
			);
			tracker.note_sent(
				ValidatorIndex(24),
				ValidatorIndex(200),
				CompactStatement::Seconded(hash_b),
			);

			// We have to see the candidate is known by the sender, e.g. we sent them 'Seconded'.
			assert_eq!(
				tracker.can_receive(
					ValidatorIndex(24),
					ValidatorIndex(200),
					CompactStatement::Valid(hash_b),
				),
				Ok(Accept::Ok),
			);
			tracker.note_received(
				ValidatorIndex(24),
				ValidatorIndex(200),
				CompactStatement::Valid(hash_b),
			);

			// Pending statements should be updated.
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(5)),
				vec![
					(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
					(ValidatorIndex(200), CompactStatement::Valid(hash_b))
				]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(200)),
				vec![(ValidatorIndex(200), CompactStatement::Valid(hash_b))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(24)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(146)),
				vec![
					(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
					(ValidatorIndex(200), CompactStatement::Valid(hash_b))
				]
			);
		}

		// Send a 'Seconded' statement.
		{
			assert_eq!(
				tracker.can_send(
					ValidatorIndex(5),
					ValidatorIndex(5),
					CompactStatement::Seconded(hash_a)
				),
				Ok(())
			);
			tracker.note_sent(
				ValidatorIndex(5),
				ValidatorIndex(5),
				CompactStatement::Seconded(hash_a),
			);

			// Pending statements should be updated.
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(5)),
				vec![(ValidatorIndex(200), CompactStatement::Valid(hash_b))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(200)),
				vec![(ValidatorIndex(200), CompactStatement::Valid(hash_b))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(24)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(146)),
				vec![
					(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
					(ValidatorIndex(200), CompactStatement::Valid(hash_b))
				]
			);
		}

		// Send a 'Valid' statement.
		{
			// First, send a `Seconded` statement for the candidate.
			assert_eq!(
				tracker.can_send(
					ValidatorIndex(5),
					ValidatorIndex(200),
					CompactStatement::Seconded(hash_b)
				),
				Ok(())
			);
			tracker.note_sent(
				ValidatorIndex(5),
				ValidatorIndex(200),
				CompactStatement::Seconded(hash_b),
			);

			// We have to see that the candidate is known by the sender, e.g. we sent them
			// 'Seconded' above.
			assert_eq!(
				tracker.can_send(
					ValidatorIndex(5),
					ValidatorIndex(200),
					CompactStatement::Valid(hash_b)
				),
				Ok(())
			);
			tracker.note_sent(
				ValidatorIndex(5),
				ValidatorIndex(200),
				CompactStatement::Valid(hash_b),
			);

			// Pending statements should be updated.
			assert_eq!(tracker.pending_statements_for(ValidatorIndex(5)), vec![]);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(200)),
				vec![(ValidatorIndex(200), CompactStatement::Valid(hash_b))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(24)),
				vec![(ValidatorIndex(5), CompactStatement::Seconded(hash_a))]
			);
			assert_eq!(
				tracker.pending_statements_for(ValidatorIndex(146)),
				vec![
					(ValidatorIndex(5), CompactStatement::Seconded(hash_a)),
					(ValidatorIndex(200), CompactStatement::Valid(hash_b))
				]
			);
		}
	}
}
