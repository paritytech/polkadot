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

//! The [`Candidates`] store tracks information about advertised candidates
//! as well as which peers have advertised them.
//!
//! Due to the request-oriented nature of this protocol, we often learn
//! about candidates just as a hash, alongside claimed properties that the
//! receipt would commit to. However, it is only later on that we can
//! confirm those claimed properties. This store lets us keep track of the
//! all candidates which are currently 'relevant' after spam-protection, and
//! gives us the ability to detect mis-advertisements after the fact
//! and punish them accordingly.

use polkadot_node_network_protocol::PeerId;
use polkadot_primitives::vstaging::{
	CandidateHash, CommittedCandidateReceipt, GroupIndex, Hash, Id as ParaId,
	PersistedValidationData,
};

use std::collections::{
	hash_map::{Entry, HashMap},
	HashSet,
};

/// A tracker for all known candidates in the view.
///
/// See module docs for more info.
#[derive(Default)]
pub struct Candidates {
	candidates: HashMap<CandidateHash, CandidateState>,
	by_parent: HashMap<(Hash, ParaId), HashSet<CandidateHash>>,
}

impl Candidates {
	/// Insert an advertisement.
	///
	/// This should be invoked only after performing
	/// spam protection and only for advertisements that
	/// are valid within the current view. [`Candidates`] never prunes
	/// candidate by peer ID, to avoid peers skirting misbehavior
	/// reports by disconnecting intermittently. Therefore, this presumes
	/// that spam protection limits the peers which can send advertisements
	/// about unconfirmed candidates.
	///
	/// It returns either `Ok(())` or an immediate error in the
	/// case that the candidate is already known and reality conflicts
	/// with the advertisement.
	pub fn insert_unconfirmed(
		&mut self,
		peer: PeerId,
		candidate_hash: CandidateHash,
		claimed_relay_parent: Hash,
		claimed_group_index: GroupIndex,
		claimed_parent_hash_and_id: Option<(Hash, ParaId)>,
	) -> Result<(), BadAdvertisement> {
		let entry = self.candidates.entry(candidate_hash).or_insert_with(|| {
			CandidateState::Unconfirmed(UnconfirmedCandidate {
				claims: Vec::new(),
				parent_claims: HashMap::new(),
				unconfirmed_importable_under: HashSet::new(),
			})
		});

		match entry {
			CandidateState::Confirmed(ref c) => {
				if c.relay_parent() != claimed_relay_parent {
					return Err(BadAdvertisement)
				}

				if c.group_index() != claimed_group_index {
					return Err(BadAdvertisement)
				}

				if let Some((claimed_parent_hash, claimed_id)) = claimed_parent_hash_and_id {
					if c.parent_hash() != claimed_parent_hash {
						return Err(BadAdvertisement)
					}

					if c.para_id() != claimed_id {
						return Err(BadAdvertisement)
					}
				}
			},
			CandidateState::Unconfirmed(ref mut c) => {
				c.add_claims(
					peer,
					CandidateClaims {
						relay_parent: claimed_relay_parent,
						group_index: claimed_group_index,
						parent_hash_and_id: claimed_parent_hash_and_id,
					},
				);

				if let Some(parent_claims) = claimed_parent_hash_and_id {
					self.by_parent.entry(parent_claims).or_default().insert(candidate_hash);
				}
			},
		}

		Ok(())
	}

	/// Note that a candidate has been confirmed,
	/// yielding lists of peers which advertised it
	/// both correctly and incorrectly.
	///
	/// This does no sanity-checking of input data.
	pub fn confirm_candidate(
		&mut self,
		candidate_hash: CandidateHash,
		candidate_receipt: CommittedCandidateReceipt,
		persisted_validation_data: PersistedValidationData,
		assigned_group: GroupIndex,
	) -> Option<PostConfirmationReckoning> {
		let parent_hash = persisted_validation_data.parent_head.hash();
		let relay_parent = candidate_receipt.descriptor().relay_parent;
		let para_id = candidate_receipt.descriptor().para_id;

		let prev_state = self.candidates.insert(
			candidate_hash,
			CandidateState::Confirmed(ConfirmedCandidate {
				receipt: candidate_receipt,
				persisted_validation_data,
				assigned_group,
				parent_hash,
				importable_under: HashSet::new(),
			}),
		);

		self.by_parent.entry((parent_hash, para_id)).or_default().insert(candidate_hash);

		match prev_state {
			None => None,
			Some(CandidateState::Confirmed(_)) => None,
			Some(CandidateState::Unconfirmed(u)) => Some({
				let mut reckoning = PostConfirmationReckoning {
					correct: HashSet::new(),
					incorrect: HashSet::new(),
				};

				for (peer, claims) in u.claims {
					// Update the by-parent-hash index not to store any outdated
					// claims.
					if let Some((claimed_parent_hash, claimed_id)) = claims.parent_hash_and_id {
						if claimed_parent_hash != parent_hash || claimed_id != para_id {
							if let Entry::Occupied(mut e) =
								self.by_parent.entry((claimed_parent_hash, claimed_id))
							{
								e.get_mut().remove(&candidate_hash);
								if e.get().is_empty() {
									e.remove();
								}
							}
						}
					}

					if claims.check(relay_parent, assigned_group, parent_hash, para_id) {
						reckoning.correct.insert(peer);
					} else {
						reckoning.incorrect.insert(peer);
					}
				}

				reckoning
			}),
		}
	}

	/// Whether a candidate is confirmed.
	pub fn is_confirmed(&self, candidate_hash: &CandidateHash) -> bool {
		match self.candidates.get(candidate_hash) {
			Some(CandidateState::Confirmed(_)) => true,
			_ => false,
		}
	}

	/// Get a reference to the candidate, if it's known and confirmed.
	pub fn get_confirmed(&self, candidate_hash: &CandidateHash) -> Option<&ConfirmedCandidate> {
		match self.candidates.get(candidate_hash) {
			Some(CandidateState::Confirmed(ref c)) => Some(c),
			_ => None,
		}
	}

	/// Prune all candidates according to the relay-parent predicate
	/// provided.
	pub fn on_deactivate_leaves(
		&mut self,
		leaves: &[Hash],
		relay_parent_live: impl Fn(&Hash) -> bool,
	) {
		let by_parent = &mut self.by_parent;
		let mut remove_parent_claims = |c_hash, parent_hash, id| {
			if let Entry::Occupied(mut e) = by_parent.entry((parent_hash, id)) {
				e.get_mut().remove(&c_hash);
				if e.get().is_empty() {
					e.remove();
				}
			}
		};
		self.candidates.retain(|c_hash, state| match state {
			CandidateState::Confirmed(ref mut c) =>
				if !relay_parent_live(&c.relay_parent()) {
					remove_parent_claims(*c_hash, c.parent_hash(), c.para_id());
					false
				} else {
					for leaf_hash in leaves {
						c.importable_under.remove(leaf_hash);
					}
					true
				},
			CandidateState::Unconfirmed(ref mut c) => {
				c.on_deactivate_leaves(
					leaves,
					|parent_hash, id| remove_parent_claims(*c_hash, parent_hash, id),
					&relay_parent_live,
				);
				c.has_claims()
			},
		})
	}
}

/// This encapsulates the correct and incorrect advertisers
/// post-confirmation of a candidate.
pub struct PostConfirmationReckoning {
	/// Peers which advertised correctly.
	pub correct: HashSet<PeerId>,
	/// Peers which advertised the candidate incorrectly.
	pub incorrect: HashSet<PeerId>,
}

/// A bad advertisement was recognized.
#[derive(Debug)]
pub struct BadAdvertisement;

enum CandidateState {
	Unconfirmed(UnconfirmedCandidate),
	Confirmed(ConfirmedCandidate),
}

/// Claims made alongside the advertisement of a candidate.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CandidateClaims {
	/// The relay-parent committed to by the candidate.
	relay_parent: Hash,
	/// The group index assigned to this candidate.
	group_index: GroupIndex,
	/// The hash of the parent head-data and the ParaId. This is optional,
	/// as only some types of advertisements include this data.
	parent_hash_and_id: Option<(Hash, ParaId)>,
}

impl CandidateClaims {
	fn check(
		&self,
		relay_parent: Hash,
		group_index: GroupIndex,
		parent_hash: Hash,
		para_id: ParaId,
	) -> bool {
		self.relay_parent == relay_parent &&
			self.group_index == group_index &&
			self.parent_hash_and_id.map_or(true, |p| p == (parent_hash, para_id))
	}
}

// properties of an unconfirmed but hypothetically importable candidate.
#[derive(Hash, PartialEq, Eq)]
struct UnconfirmedImportable {
	relay_parent: Hash,
	parent_hash: Hash,
	para_id: ParaId,
}

// An unconfirmed candidate may have have been advertised under
// multiple identifiers. We track here, on the basis of unique identifier,
// the peers which advertised each candidate in a specific way.
struct UnconfirmedCandidate {
	claims: Vec<(PeerId, CandidateClaims)>,
	// ref-counted
	parent_claims: HashMap<(Hash, ParaId), usize>,
	unconfirmed_importable_under: HashSet<(Hash, UnconfirmedImportable)>,
}

impl UnconfirmedCandidate {
	fn add_claims(&mut self, peer: PeerId, claims: CandidateClaims) {
		// This does no deduplication, but this is only called after
		// spam prevention is already done. In practice we expect that
		// each peer will be able to announce the same candidate about 1 time per live relay-parent,
		// but in doing so it limits the amount of other candidates it can advertise. on balance,
		// memory consumption is bounded in the same way.
		if let Some(parent_claims) = claims.parent_hash_and_id {
			*self.parent_claims.entry(parent_claims).or_default() += 1;
		}
		self.claims.push((peer, claims));
	}

	fn note_maybe_importable_under(
		&mut self,
		active_leaf: Hash,
		unconfirmed_importable: UnconfirmedImportable,
	) {
		self.unconfirmed_importable_under.insert((active_leaf, unconfirmed_importable));
	}

	fn on_deactivate_leaves(
		&mut self,
		leaves: &[Hash],
		mut remove_parent_index: impl FnMut(Hash, ParaId),
		relay_parent_live: impl Fn(&Hash) -> bool,
	) {
		self.claims.retain(|c| {
			if relay_parent_live(&c.1.relay_parent) {
				true
			} else {
				if let Some(parent_claims) = c.1.parent_hash_and_id {
					if let Entry::Occupied(mut e) = self.parent_claims.entry(parent_claims) {
						*e.get_mut() -= 1;
						if *e.get() == 0 {
							remove_parent_index(parent_claims.0, parent_claims.1);
							e.remove();
						}
					}
				}

				false
			}
		});

		self.unconfirmed_importable_under
			.retain(|(l, props)| leaves.contains(l) && relay_parent_live(&props.relay_parent));
	}

	fn has_claims(&self) -> bool {
		!self.claims.is_empty()
	}
}

/// A confirmed candidate.
pub struct ConfirmedCandidate {
	receipt: CommittedCandidateReceipt,
	persisted_validation_data: PersistedValidationData,
	assigned_group: GroupIndex,
	parent_hash: Hash,
	// active leaves statements about this candidate are importable under.
	importable_under: HashSet<Hash>,
}

impl ConfirmedCandidate {
	/// Get the relay-parent of the candidate.
	pub fn relay_parent(&self) -> Hash {
		self.receipt.descriptor().relay_parent
	}

	/// Get the para-id of the candidate.
	pub fn para_id(&self) -> ParaId {
		self.receipt.descriptor().para_id
	}

	/// Whether the candidate is importable.
	pub fn is_importable<'a>(&self, under_active_leaf: impl Into<Option<&'a Hash>>) -> bool {
		match under_active_leaf.into() {
			Some(h) => self.importable_under.contains(h),
			None => !self.importable_under.is_empty(),
		}
	}

	fn group_index(&self) -> GroupIndex {
		self.assigned_group
	}

	fn parent_hash(&self) -> Hash {
		self.parent_hash
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// TODO [now]: test that inserting unconfirmed rejects if claims are
	// incomptable.

	// TODO [now]: test that confirming correctly maintains the parent hash index

	// TODO [now]: test that pruning unconfirmed claims correctly maintains the parent hash
	// index
}
