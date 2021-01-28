// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! The statement table: generic implementation.
//!
//! This stores messages other authorities issue about candidates.
//!
//! These messages are used to create a proposal submitted to a BFT consensus process.
//!
//! Each parachain is associated with a committee of authorities, who issue statements
//! indicating whether the candidate is valid or invalid. Once a threshold of the committee
//! has signed validity statements, the candidate may be marked includable.

use std::collections::hash_map::{self, Entry, HashMap};
use std::hash::Hash;
use std::fmt::Debug;

use primitives::v1::{ValidityAttestation as PrimitiveValidityAttestation, ValidatorSignature};

use parity_scale_codec::{Encode, Decode};

/// Context for the statement table.
pub trait Context {
	/// An authority ID
	type AuthorityId: Debug + Hash + Eq + Clone;
	/// The digest (hash or other unique attribute) of a candidate.
	type Digest: Debug + Hash + Eq + Clone;
	/// The group ID type
	type GroupId: Debug + Hash + Ord + Eq + Clone;
	/// A signature type.
	type Signature: Debug + Eq + Clone;
	/// Candidate type. In practice this will be a candidate receipt.
	type Candidate: Debug + Ord + Eq + Clone;

	/// get the digest of a candidate.
	fn candidate_digest(candidate: &Self::Candidate) -> Self::Digest;

	/// get the group of a candidate.
	fn candidate_group(candidate: &Self::Candidate) -> Self::GroupId;

	/// Whether a authority is a member of a group.
	/// Members are meant to submit candidates and vote on validity.
	fn is_member_of(&self, authority: &Self::AuthorityId, group: &Self::GroupId) -> bool;

	/// requisite number of votes for validity from a group.
	fn requisite_votes(&self, group: &Self::GroupId) -> usize;
}

/// Statements circulated among peers.
#[derive(PartialEq, Eq, Debug, Clone, Encode, Decode)]
pub enum Statement<Candidate, Digest> {
	/// Broadcast by an authority to indicate that this is its candidate for inclusion.
	///
	/// Broadcasting two different candidate messages per round is not allowed.
	#[codec(index = "1")]
	Candidate(Candidate),
	/// Broadcast by a authority to attest that the candidate with given digest is valid.
	#[codec(index = "2")]
	Valid(Digest),
	/// Broadcast by a authority to attest that the candidate with given digest is invalid.
	#[codec(index = "3")]
	Invalid(Digest),
}

/// A signed statement.
#[derive(PartialEq, Eq, Debug, Clone, Encode, Decode)]
pub struct SignedStatement<Candidate, Digest, AuthorityId, Signature> {
	/// The statement.
	pub statement: Statement<Candidate, Digest>,
	/// The signature.
	pub signature: Signature,
	/// The sender.
	pub sender: AuthorityId,
}

/// Misbehavior: voting more than one way on candidate validity.
///
/// Since there are three possible ways to vote, a double vote is possible in
/// three possible combinations (unordered)
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum ValidityDoubleVote<Candidate, Digest, Signature> {
	/// Implicit vote by issuing and explicitly voting validity.
	IssuedAndValidity((Candidate, Signature), (Digest, Signature)),
	/// Implicit vote by issuing and explicitly voting invalidity
	IssuedAndInvalidity((Candidate, Signature), (Digest, Signature)),
	/// Direct votes for validity and invalidity
	ValidityAndInvalidity(Candidate, Signature, Signature),
}

impl<Candidate, Digest, Signature> ValidityDoubleVote<Candidate, Digest, Signature> {
	/// Deconstruct this misbehavior into two `(Statement, Signature)` pairs, erasing the information
	/// about precisely what the problem was.
	pub fn deconstruct<Ctx>(self) -> (
		(Statement<Candidate, Digest>, Signature),
		(Statement<Candidate, Digest>, Signature),
	)
	where
		Ctx: Context<Candidate=Candidate, Digest=Digest, Signature=Signature>,
		Candidate: Debug + Ord + Eq + Clone,
		Digest: Debug + Hash + Eq + Clone,
		Signature: Debug + Eq + Clone,
	{
		match self {
			Self::IssuedAndValidity((c, s1), (d, s2)) => {
				((Statement::Candidate(c), s1), (Statement::Valid(d), s2))
			}
			Self::IssuedAndInvalidity((c, s1), (d, s2)) => {
				((Statement::Candidate(c), s1), (Statement::Invalid(d), s2))
			}
			Self::ValidityAndInvalidity(c, s1, s2) => {
				(
					(Statement::Valid(Ctx::candidate_digest(&c)), s1),
					(Statement::Invalid(Ctx::candidate_digest(&c)), s2),
				)
			}
		}
	}
}

/// Misbehavior: multiple signatures on same statement.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum DoubleSign<Candidate, Digest, Signature> {
	/// On candidate.
	Candidate(Candidate, Signature, Signature),
	/// On validity.
	Validity(Digest, Signature, Signature),
	/// On invalidity.
	Invalidity(Digest, Signature, Signature),
}

impl<Candidate, Digest, Signature> DoubleSign<Candidate, Digest, Signature> {
	/// Deconstruct this misbehavior into a statement with two signatures, erasing the information about
	/// precisely where in the process the issue was detected.
	pub fn deconstruct(self) -> (Statement<Candidate, Digest>, Signature, Signature) {
		match self {
			Self::Candidate(candidate, a, b) => (Statement::Candidate(candidate), a, b),
			Self::Validity(digest, a, b) => (Statement::Valid(digest), a, b),
			Self::Invalidity(digest, a, b) => (Statement::Invalid(digest), a, b),
		}
	}
}

/// Misbehavior: declaring multiple candidates.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct MultipleCandidates<Candidate, Signature> {
	/// The first candidate seen.
	pub first: (Candidate, Signature),
	/// The second candidate seen.
	pub second: (Candidate, Signature),
}

/// Misbehavior: submitted statement for wrong group.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct UnauthorizedStatement<Candidate, Digest, AuthorityId, Signature> {
	/// A signed statement which was submitted without proper authority.
	pub statement: SignedStatement<Candidate, Digest, AuthorityId, Signature>,
}

/// Different kinds of misbehavior. All of these kinds of malicious misbehavior
/// are easily provable and extremely disincentivized.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum Misbehavior<Candidate, Digest, AuthorityId, Signature> {
	/// Voted invalid and valid on validity.
	ValidityDoubleVote(ValidityDoubleVote<Candidate, Digest, Signature>),
	/// Submitted multiple candidates.
	MultipleCandidates(MultipleCandidates<Candidate, Signature>),
	/// Submitted a message that was unauthorized.
	UnauthorizedStatement(UnauthorizedStatement<Candidate, Digest, AuthorityId, Signature>),
	/// Submitted two valid signatures for the same message.
	DoubleSign(DoubleSign<Candidate, Digest, Signature>),
}

/// Type alias for misbehavior corresponding to context type.
pub type MisbehaviorFor<Ctx> = Misbehavior<
	<Ctx as Context>::Candidate,
	<Ctx as Context>::Digest,
	<Ctx as Context>::AuthorityId,
	<Ctx as Context>::Signature,
>;

// kinds of votes for validity
#[derive(Clone, PartialEq, Eq)]
enum ValidityVote<Signature: Eq + Clone> {
	// implicit validity vote by issuing
	Issued(Signature),
	// direct validity vote
	Valid(Signature),
	// direct invalidity vote
	Invalid(Signature),
}

/// A summary of import of a statement.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Summary<Digest, Group> {
	/// The digest of the candidate referenced.
	pub candidate: Digest,
	/// The group that the candidate is in.
	pub group_id: Group,
	/// How many validity votes are currently witnessed.
	pub validity_votes: usize,
	/// Whether this has been signalled bad by at least one participant.
	pub signalled_bad: bool,
}

/// A validity attestation.
#[derive(Clone, PartialEq, Decode, Encode)]
pub enum ValidityAttestation<Signature> {
	/// implicit validity attestation by issuing.
	/// This corresponds to issuance of a `Candidate` statement.
	Implicit(Signature),
	/// An explicit attestation. This corresponds to issuance of a
	/// `Valid` statement.
	Explicit(Signature),
}

impl Into<PrimitiveValidityAttestation> for ValidityAttestation<ValidatorSignature> {
	fn into(self) -> PrimitiveValidityAttestation {
		match self {
			Self::Implicit(s) => PrimitiveValidityAttestation::Implicit(s),
			Self::Explicit(s) => PrimitiveValidityAttestation::Explicit(s),
		}
	}
}

/// An attested-to candidate.
#[derive(Clone, PartialEq, Decode, Encode)]
pub struct AttestedCandidate<Group, Candidate, AuthorityId, Signature> {
	/// The group ID that the candidate is in.
	pub group_id: Group,
	/// The candidate data.
	pub candidate: Candidate,
	/// Validity attestations.
	pub validity_votes: Vec<(AuthorityId, ValidityAttestation<Signature>)>,
}

/// Stores votes and data about a candidate.
pub struct CandidateData<Ctx: Context> {
	group_id: Ctx::GroupId,
	candidate: Ctx::Candidate,
	validity_votes: HashMap<Ctx::AuthorityId, ValidityVote<Ctx::Signature>>,
	indicated_bad_by: Vec<Ctx::AuthorityId>,
}

impl<Ctx: Context> CandidateData<Ctx> {
	/// whether this has been indicated bad by anyone.
	pub fn indicated_bad(&self) -> bool {
		!self.indicated_bad_by.is_empty()
	}

	/// Yield a full attestation for a candidate.
	/// If the candidate can be included, it will return `Some`.
	pub fn attested(&self, validity_threshold: usize)
		-> Option<AttestedCandidate<
			Ctx::GroupId, Ctx::Candidate, Ctx::AuthorityId, Ctx::Signature,
		>>
	{
		if self.can_be_included(validity_threshold) {
			let validity_votes: Vec<_> = self.validity_votes.iter()
				.filter_map(|(a, v)| match *v {
					ValidityVote::Invalid(_) => None,

					ValidityVote::Valid(ref s) =>
						Some((a, ValidityAttestation::Explicit(s.clone()))),
					ValidityVote::Issued(ref s) =>
						Some((a, ValidityAttestation::Implicit(s.clone()))),
				})
				.take(validity_threshold)
				.map(|(k, v)| (k.clone(), v.clone()))
				.collect();

			assert!(
				validity_votes.len() == validity_threshold,
				"candidate is includable; therefore there are enough validity votes; qed",
			);

			Some(AttestedCandidate {
				group_id: self.group_id.clone(),
				candidate: self.candidate.clone(),
				validity_votes,
			})
		} else {
			None
		}
	}

	// Candidate data can be included in a proposal
	// if it has enough validity votes
	// and no authorities have called it bad.
	fn can_be_included(&self, validity_threshold: usize) -> bool {
		self.validity_votes.len() >= validity_threshold
	}

	fn summary(&self, digest: Ctx::Digest) -> Summary<Ctx::Digest, Ctx::GroupId> {
		Summary {
			candidate: digest,
			group_id: self.group_id.clone(),
			validity_votes: self.validity_votes.len(),
			signalled_bad: self.indicated_bad(),
		}
	}
}

// authority metadata
struct AuthorityData<Ctx: Context> {
	proposal: Option<(Ctx::Digest, Ctx::Signature)>,
}

impl<Ctx: Context> Default for AuthorityData<Ctx> {
	fn default() -> Self {
		AuthorityData {
			proposal: None,
		}
	}
}

/// Type alias for the result of a statement import.
pub type ImportResult<Ctx> = Result<
	Option<Summary<<Ctx as Context>::Digest, <Ctx as Context>::GroupId>>,
	MisbehaviorFor<Ctx>
>;

/// Stores votes
pub struct Table<Ctx: Context> {
	authority_data: HashMap<Ctx::AuthorityId, AuthorityData<Ctx>>,
	detected_misbehavior: HashMap<Ctx::AuthorityId, Vec<MisbehaviorFor<Ctx>>>,
	candidate_votes: HashMap<Ctx::Digest, CandidateData<Ctx>>,
	includable_count: HashMap<Ctx::GroupId, usize>,
}

impl<Ctx: Context> Default for Table<Ctx> {
	fn default() -> Self {
		Table {
			authority_data: HashMap::new(),
			detected_misbehavior: HashMap::new(),
			candidate_votes: HashMap::new(),
			includable_count: HashMap::new(),
		}
	}
}

impl<Ctx: Context> Table<Ctx> {
	/// Produce a set of proposed candidates.
	///
	/// This will be at most one per group, consisting of the
	/// best candidate for each group with requisite votes for inclusion.
	///
	/// The vector is sorted in ascending order by group id.
	pub fn proposed_candidates(&self, context: &Ctx) -> Vec<AttestedCandidate<
		Ctx::GroupId, Ctx::Candidate, Ctx::AuthorityId, Ctx::Signature,
	>> {
		use std::collections::BTreeMap;
		use std::collections::btree_map::Entry as BTreeEntry;

		let mut best_candidates = BTreeMap::new();
		for candidate_data in self.candidate_votes.values() {
			let group_id = &candidate_data.group_id;

			if !self.includable_count.contains_key(group_id) {
				continue
			}

			let threshold = context.requisite_votes(group_id);

			if !candidate_data.can_be_included(threshold) { continue }
			match best_candidates.entry(group_id.clone()) {
				BTreeEntry::Vacant(vacant) => {
					vacant.insert((candidate_data, threshold));
				},
				BTreeEntry::Occupied(mut occ) => {
					let candidate_ref = occ.get_mut();
					if candidate_ref.0.candidate > candidate_data.candidate {
						candidate_ref.0 = candidate_data;
					}
				}
			}
		}

		best_candidates.values()
			.map(|&(candidate_data, threshold)|
				candidate_data.attested(threshold)
					.expect("candidate has been checked includable; \
						therefore an attestation can be constructed; qed")
			)
			.collect::<Vec<_>>()
	}

	/// Whether a candidate can be included.
	pub fn candidate_includable(&self, digest: &Ctx::Digest, context: &Ctx) -> bool {
		self.candidate_votes.get(digest).map_or(false, |data| {
			let v_threshold = context.requisite_votes(&data.group_id);
			data.can_be_included(v_threshold)
		})
	}

	/// Get the attested candidate for `digest`.
	///
	/// Returns `Some(_)` if the candidate exists and is includable.
	pub fn attested_candidate(&self, digest: &Ctx::Digest, context: &Ctx)
		-> Option<AttestedCandidate<
			Ctx::GroupId, Ctx::Candidate, Ctx::AuthorityId, Ctx::Signature,
		>>
	{
		self.candidate_votes.get(digest).and_then(|data| {
			let v_threshold = context.requisite_votes(&data.group_id);
			data.attested(v_threshold)
		})
	}

	/// Import a signed statement. Signatures should be checked for validity, and the
	/// sender should be checked to actually be an authority.
	///
	/// Validity and invalidity statements are only valid if the corresponding
	/// candidate has already been imported.
	///
	/// If this returns `None`, the statement was either duplicate or invalid.
	pub fn import_statement(
		&mut self,
		context: &Ctx,
		statement: SignedStatement<Ctx::Candidate, Ctx::Digest, Ctx::AuthorityId, Ctx::Signature>,
	) -> Option<Summary<Ctx::Digest, Ctx::GroupId>> {
		let SignedStatement { statement, signature, sender: signer } = statement;

		let res = match statement {
			Statement::Candidate(candidate) => self.import_candidate(
				context,
				signer.clone(),
				candidate,
				signature
			),
			Statement::Valid(digest) => self.validity_vote(
				context,
				signer.clone(),
				digest,
				ValidityVote::Valid(signature),
			),
			Statement::Invalid(digest) => self.validity_vote(
				context,
				signer.clone(),
				digest,
				ValidityVote::Invalid(signature),
			),
		};

		match res {
			Ok(maybe_summary) => maybe_summary,
			Err(misbehavior) => {
				// all misbehavior in agreement is provable and actively malicious.
				// punishments may be cumulative.
				self.detected_misbehavior.entry(signer).or_default().push(misbehavior);
				None
			}
		}
	}

	/// Get a candidate by digest.
	pub fn get_candidate(&self, digest: &Ctx::Digest) -> Option<&Ctx::Candidate> {
		self.candidate_votes.get(digest).map(|d| &d.candidate)
	}

	/// Access all witnessed misbehavior.
	pub fn get_misbehavior(&self)
		-> &HashMap<Ctx::AuthorityId, Vec<MisbehaviorFor<Ctx>>>
	{
		&self.detected_misbehavior
	}

	/// Create a draining iterator of misbehaviors.
	///
	/// This consumes all detected misbehaviors, even if the iterator is not completely consumed.
	pub fn drain_misbehaviors(&mut self) -> DrainMisbehaviors<'_, Ctx> {
		self.detected_misbehavior
			.drain()
			.into()
	}

	/// Get the current number of parachains with includable candidates.
	pub fn includable_count(&self) -> usize {
		self.includable_count.len()
	}

	fn import_candidate(
		&mut self,
		context: &Ctx,
		authority: Ctx::AuthorityId,
		candidate: Ctx::Candidate,
		signature: Ctx::Signature,
	) -> ImportResult<Ctx> {
		let group = Ctx::candidate_group(&candidate);
		if !context.is_member_of(&authority, &group) {
			return Err(Misbehavior::UnauthorizedStatement(UnauthorizedStatement {
				statement: SignedStatement {
					signature,
					statement: Statement::Candidate(candidate),
					sender: authority,
				},
			}));
		}

		// check that authority hasn't already specified another candidate.
		let digest = Ctx::candidate_digest(&candidate);

		let new_proposal = match self.authority_data.entry(authority.clone()) {
			Entry::Occupied(mut occ) => {
				// if digest is different, fetch candidate and
				// note misbehavior.
				let existing = occ.get_mut();

				if let Some((ref old_digest, ref old_sig)) = existing.proposal {
					if old_digest != &digest {
						const EXISTENCE_PROOF: &str =
							"when proposal first received from authority, candidate \
							votes entry is created. proposal here is `Some`, therefore \
							candidate votes entry exists; qed";

						let old_candidate = self.candidate_votes.get(old_digest)
							.expect(EXISTENCE_PROOF)
							.candidate
							.clone();

						return Err(Misbehavior::MultipleCandidates(MultipleCandidates {
							first: (old_candidate, old_sig.clone()),
							second: (candidate, signature.clone()),
						}));
					}

					false
				} else {
					existing.proposal = Some((digest.clone(), signature.clone()));
					true
				}
			}
			Entry::Vacant(vacant) => {
				vacant.insert(AuthorityData {
					proposal: Some((digest.clone(), signature.clone())),
				});
				true
			}
		};

		// NOTE: altering this code may affect the existence proof above. ensure it remains
		// valid.
		if new_proposal {
			self.candidate_votes.entry(digest.clone()).or_insert_with(move || CandidateData {
				group_id: group,
				candidate,
				validity_votes: HashMap::new(),
				indicated_bad_by: Vec::new(),
			});
		}

		self.validity_vote(
			context,
			authority,
			digest,
			ValidityVote::Issued(signature),
		)
	}

	fn validity_vote(
		&mut self,
		context: &Ctx,
		from: Ctx::AuthorityId,
		digest: Ctx::Digest,
		vote: ValidityVote<Ctx::Signature>,
	) -> ImportResult<Ctx> {
		let votes = match self.candidate_votes.get_mut(&digest) {
			None => return Ok(None),
			Some(votes) => votes,
		};

		let v_threshold = context.requisite_votes(&votes.group_id);
		let was_includable = votes.can_be_included(v_threshold);

		// check that this authority actually can vote in this group.
		if !context.is_member_of(&from, &votes.group_id) {
			let (sig, valid) = match vote {
				ValidityVote::Valid(s) => (s, true),
				ValidityVote::Invalid(s) => (s, false),
				ValidityVote::Issued(_) =>
					panic!("implicit issuance vote only cast from `import_candidate` after \
							checking group membership of issuer; qed"),
			};

			return Err(Misbehavior::UnauthorizedStatement(UnauthorizedStatement {
				statement: SignedStatement {
					signature: sig,
					sender: from,
					statement: if valid {
						Statement::Valid(digest)
					} else {
						Statement::Invalid(digest)
					}
				}
			}));
		}

		// check for double votes.
		match votes.validity_votes.entry(from.clone()) {
			Entry::Occupied(occ) => {
				let make_vdv = |v| Misbehavior::ValidityDoubleVote(v);
				let make_ds = |ds| Misbehavior::DoubleSign(ds);
				return if occ.get() != &vote {
					Err(match (occ.get().clone(), vote) {
						// valid vote conflicting with candidate statement
						(ValidityVote::Issued(iss), ValidityVote::Valid(good)) |
						(ValidityVote::Valid(good), ValidityVote::Issued(iss)) =>
							make_vdv(ValidityDoubleVote::IssuedAndValidity((votes.candidate.clone(), iss), (digest, good))),

						// invalid vote conflicting with candidate statement
						(ValidityVote::Issued(iss), ValidityVote::Invalid(bad)) |
						(ValidityVote::Invalid(bad), ValidityVote::Issued(iss)) =>
							make_vdv(ValidityDoubleVote::IssuedAndInvalidity((votes.candidate.clone(), iss), (digest, bad))),

						// valid vote conflicting with invalid vote
						(ValidityVote::Valid(good), ValidityVote::Invalid(bad)) |
						(ValidityVote::Invalid(bad), ValidityVote::Valid(good)) =>
							make_vdv(ValidityDoubleVote::ValidityAndInvalidity(votes.candidate.clone(), good, bad)),

						// two signatures on same candidate
						(ValidityVote::Issued(a), ValidityVote::Issued(b)) =>
							make_ds(DoubleSign::Candidate(votes.candidate.clone(), a, b)),

						// two signatures on same validity vote
						(ValidityVote::Valid(a), ValidityVote::Valid(b)) =>
							make_ds(DoubleSign::Validity(digest, a, b)),

						// two signature on same invalidity vote
						(ValidityVote::Invalid(a), ValidityVote::Invalid(b)) =>
							make_ds(DoubleSign::Invalidity(digest, a, b)),
					})
				} else {
					Ok(None)
				}
			}
			Entry::Vacant(vacant) => {
				if let ValidityVote::Invalid(_) = vote {
					votes.indicated_bad_by.push(from.clone());
				}

				vacant.insert(vote);
			}
		}

		let is_includable = votes.can_be_included(v_threshold);
		update_includable_count(&mut self.includable_count, &votes.group_id, was_includable, is_includable);

		Ok(Some(votes.summary(digest)))
	}
}

type Drain<'a, Ctx> = hash_map::Drain<'a, <Ctx as Context>::AuthorityId, Vec<MisbehaviorFor<Ctx>>>;

struct MisbehaviorForAuthority<Ctx: Context> {
	id: Ctx::AuthorityId,
	misbehaviors: Vec<MisbehaviorFor<Ctx>>,
}

impl<Ctx: Context> From<(Ctx::AuthorityId, Vec<MisbehaviorFor<Ctx>>)> for MisbehaviorForAuthority<Ctx> {
	fn from((id, mut misbehaviors): (Ctx::AuthorityId, Vec<MisbehaviorFor<Ctx>>)) -> Self {
		// we're going to be popping items off this list in the iterator, so reverse it now to
		// preserve the original ordering.
		misbehaviors.reverse();
		Self { id, misbehaviors }
	}
}

impl<Ctx: Context> Iterator for MisbehaviorForAuthority<Ctx> {
	type Item = (Ctx::AuthorityId, MisbehaviorFor<Ctx>);

	fn next(&mut self) -> Option<Self::Item> {
		self.misbehaviors.pop().map(|misbehavior| (self.id.clone(), misbehavior))
	}
}

pub struct DrainMisbehaviors<'a, Ctx: Context> {
	drain: Drain<'a, Ctx>,
	in_progress: Option<MisbehaviorForAuthority<Ctx>>,
}

impl<'a, Ctx: Context> From<Drain<'a, Ctx>> for DrainMisbehaviors<'a, Ctx> {
	fn from(drain: Drain<'a, Ctx>) -> Self {
		Self {
			drain,
			in_progress: None,
		}
	}
}

impl<'a, Ctx: Context> DrainMisbehaviors<'a, Ctx> {
	fn maybe_item(&mut self) -> Option<(Ctx::AuthorityId, MisbehaviorFor<Ctx>)> {
		self.in_progress.as_mut().and_then(Iterator::next)
	}
}

impl<'a, Ctx: Context> Iterator for DrainMisbehaviors<'a, Ctx> {
	type Item = (Ctx::AuthorityId, MisbehaviorFor<Ctx>);

	fn next(&mut self) -> Option<Self::Item> {
		// Note: this implementation will prematurely return `None` if `self.drain.next()` ever returns a
		// tuple whose vector is empty. That will never currently happen, as the only modification
		// to the backing map is currently via `drain` and `entry(...).or_default().push(...)`.
		// However, future code changes might change that property.
		self.maybe_item().or_else(|| {
			self.in_progress = self.drain.next().map(Into::into);
			self.maybe_item()
		})
	}
}

fn update_includable_count<Group: Hash + Eq + Clone>(
	map: &mut HashMap<Group, usize>,
	group_id: &Group,
	was_includable: bool,
	is_includable: bool,
) {
	if was_includable && !is_includable {
		if let Entry::Occupied(mut entry) = map.entry(group_id.clone()) {
			*entry.get_mut() -= 1;
			if *entry.get() == 0 {
				entry.remove();
			}
		}
	}

	if !was_includable && is_includable {
		*map.entry(group_id.clone()).or_insert(0) += 1;
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashMap;

	fn create<Candidate: Context>() -> Table<Candidate> {
		Table::default()
	}

	#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
	struct AuthorityId(usize);

	#[derive(Debug, Copy, Clone, Hash, PartialOrd, Ord, PartialEq, Eq)]
	struct GroupId(usize);

	// group, body
	#[derive(Debug, Copy, Clone, Hash, PartialOrd, Ord, PartialEq, Eq)]
	struct Candidate(usize, usize);

	#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
	struct Signature(usize);

	#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
	struct Digest(usize);

	#[derive(Debug, PartialEq, Eq)]
	struct TestContext {
		// v -> parachain group
		authorities: HashMap<AuthorityId, GroupId>
	}

	impl Context for TestContext {
		type AuthorityId = AuthorityId;
		type Digest = Digest;
		type Candidate = Candidate;
		type GroupId = GroupId;
		type Signature = Signature;

		fn candidate_digest(candidate: &Candidate) -> Digest {
			Digest(candidate.1)
		}

		fn candidate_group(candidate: &Candidate) -> GroupId {
			GroupId(candidate.0)
		}

		fn is_member_of(
			&self,
			authority: &AuthorityId,
			group: &GroupId
		) -> bool {
			self.authorities.get(authority).map(|v| v == group).unwrap_or(false)
		}

		fn requisite_votes(&self, id: &GroupId) -> usize {
			let mut total_validity = 0;

			for validity in self.authorities.values() {
				if validity == id { total_validity += 1 }
			}

			total_validity / 2 + 1
		}
	}

	#[test]
	fn submitting_two_candidates_is_misbehavior() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map
			}
		};

		let mut table = create();
		let statement_a = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};

		let statement_b = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 999)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};

		table.import_statement(&context, statement_a);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(1)));

		table.import_statement(&context, statement_b);
		assert_eq!(
			table.detected_misbehavior[&AuthorityId(1)][0],
			Misbehavior::MultipleCandidates(MultipleCandidates {
				first: (Candidate(2, 100), Signature(1)),
				second: (Candidate(2, 999), Signature(1)),
			})
		);
	}

	#[test]
	fn submitting_candidate_from_wrong_group_is_misbehavior() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(3));
				map
			}
		};

		let mut table = create();
		let statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};

		table.import_statement(&context, statement);

		assert_eq!(
			table.detected_misbehavior[&AuthorityId(1)][0],
			Misbehavior::UnauthorizedStatement(UnauthorizedStatement {
				statement: SignedStatement {
					statement: Statement::Candidate(Candidate(2, 100)),
					signature: Signature(1),
					sender: AuthorityId(1),
				},
			})
		);
	}

	#[test]
	fn unauthorized_votes() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map.insert(AuthorityId(2), GroupId(3));
				map
			}
		};

		let mut table = create();

		let candidate_a = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};
		let candidate_a_digest = Digest(100);

		table.import_statement(&context, candidate_a);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(1)));
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(2)));

		// authority 2 votes for validity on 1's candidate.
		let bad_validity_vote = SignedStatement {
			statement: Statement::Valid(candidate_a_digest.clone()),
			signature: Signature(2),
			sender: AuthorityId(2),
		};
		table.import_statement(&context, bad_validity_vote);

		assert_eq!(
			table.detected_misbehavior[&AuthorityId(2)][0],
			Misbehavior::UnauthorizedStatement(UnauthorizedStatement {
				statement: SignedStatement {
					statement: Statement::Valid(candidate_a_digest),
					signature: Signature(2),
					sender: AuthorityId(2),
				},
			})
		);
	}

	#[test]
	fn validity_double_vote_is_misbehavior() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map.insert(AuthorityId(2), GroupId(2));
				map
			}
		};

		let mut table = create();
		let statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};
		let candidate_digest = Digest(100);

		table.import_statement(&context, statement);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(1)));

		let valid_statement = SignedStatement {
			statement: Statement::Valid(candidate_digest.clone()),
			signature: Signature(2),
			sender: AuthorityId(2),
		};

		let invalid_statement = SignedStatement {
			statement: Statement::Invalid(candidate_digest.clone()),
			signature: Signature(2),
			sender: AuthorityId(2),
		};

		table.import_statement(&context, valid_statement);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(2)));

		table.import_statement(&context, invalid_statement);

		assert_eq!(
			table.detected_misbehavior[&AuthorityId(2)][0],
			Misbehavior::ValidityDoubleVote(ValidityDoubleVote::ValidityAndInvalidity(
				Candidate(2, 100),
				Signature(2),
				Signature(2),
			))
		);
	}

	#[test]
	fn candidate_double_signature_is_misbehavior() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map.insert(AuthorityId(2), GroupId(2));
				map
			}
		};

		let mut table = create();
		let statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};

		table.import_statement(&context, statement);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(1)));

		let invalid_statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(999),
			sender: AuthorityId(1),
		};

		table.import_statement(&context, invalid_statement);
		assert!(table.detected_misbehavior.contains_key(&AuthorityId(1)));
	}

	#[test]
	fn validity_invalidity_double_signature_is_misbehavior() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map.insert(AuthorityId(2), GroupId(2));
				map.insert(AuthorityId(3), GroupId(2));
				map
			}
		};

		let mut table = create();
		let statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};

		table.import_statement(&context, statement);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(1)));

		// insert two validity votes from authority 2 with different signatures
		{
			let statement = SignedStatement {
				statement: Statement::Valid(Digest(100)),
				signature: Signature(2),
				sender: AuthorityId(2),
			};

			table.import_statement(&context, statement);
			assert!(!table.detected_misbehavior.contains_key(&AuthorityId(2)));

			let invalid_statement = SignedStatement {
				statement: Statement::Valid(Digest(100)),
				signature: Signature(222),
				sender: AuthorityId(2),
			};

			table.import_statement(&context, invalid_statement);
			assert!(table.detected_misbehavior.contains_key(&AuthorityId(2)));
		}

		// insert two invalidity votes from authority 2 with different signatures
		{
			let statement = SignedStatement {
				statement: Statement::Invalid(Digest(100)),
				signature: Signature(3),
				sender: AuthorityId(3),
			};

			table.import_statement(&context, statement);
			assert!(!table.detected_misbehavior.contains_key(&AuthorityId(3)));

			let invalid_statement = SignedStatement {
				statement: Statement::Invalid(Digest(100)),
				signature: Signature(333),
				sender: AuthorityId(3),
			};

			table.import_statement(&context, invalid_statement);
			assert!(table.detected_misbehavior.contains_key(&AuthorityId(3)));
		}
	}

	#[test]
	fn issue_and_vote_is_misbehavior() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map
			}
		};

		let mut table = create();
		let statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};
		let candidate_digest = Digest(100);

		table.import_statement(&context, statement);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(1)));

		let extra_vote = SignedStatement {
			statement: Statement::Valid(candidate_digest.clone()),
			signature: Signature(1),
			sender: AuthorityId(1),
		};

		table.import_statement(&context, extra_vote);
		assert_eq!(
			table.detected_misbehavior[&AuthorityId(1)][0],
			Misbehavior::ValidityDoubleVote(ValidityDoubleVote::IssuedAndValidity(
				(Candidate(2, 100), Signature(1)),
				(Digest(100), Signature(1)),
			))
		);
	}

	#[test]
	fn candidate_can_be_included() {
		let validity_threshold = 6;

		let mut candidate = CandidateData::<TestContext> {
			group_id: GroupId(4),
			candidate: Candidate(4, 12345),
			validity_votes: HashMap::new(),
			indicated_bad_by: Vec::new(),
		};

		assert!(!candidate.can_be_included(validity_threshold));

		for i in 0..validity_threshold {
			candidate.validity_votes.insert(AuthorityId(i + 100), ValidityVote::Valid(Signature(i + 100)));
		}

		assert!(candidate.can_be_included(validity_threshold));

		candidate.indicated_bad_by.push(AuthorityId(1024));

		assert!(candidate.can_be_included(validity_threshold));
	}

	#[test]
	fn includability_counter() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map.insert(AuthorityId(2), GroupId(2));
				map.insert(AuthorityId(3), GroupId(2));
				map
			}
		};

		// have 2/3 validity guarantors note validity.
		let mut table = create();
		let statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};
		let candidate_digest = Digest(100);

		table.import_statement(&context, statement);

		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(1)));
		assert!(!table.candidate_includable(&candidate_digest, &context));
		assert!(table.includable_count.is_empty());

		let vote = SignedStatement {
			statement: Statement::Valid(candidate_digest.clone()),
			signature: Signature(2),
			sender: AuthorityId(2),
		};

		table.import_statement(&context, vote);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(2)));
		assert!(table.candidate_includable(&candidate_digest, &context));
		assert!(table.includable_count.get(&GroupId(2)).is_some());

		// have the last validity guarantor note invalidity. now it is unincludable.
		let vote = SignedStatement {
			statement: Statement::Invalid(candidate_digest.clone()),
			signature: Signature(3),
			sender: AuthorityId(3),
		};

		table.import_statement(&context, vote);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(3)));
		assert!(table.candidate_includable(&candidate_digest, &context));
		assert!(table.includable_count.get(&GroupId(2)).is_some());
	}

	#[test]
	fn candidate_import_gives_summary() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map
			}
		};

		let mut table = create();
		let statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};

		let summary = table.import_statement(&context, statement)
			.expect("candidate import to give summary");

		assert_eq!(summary.candidate, Digest(100));
		assert_eq!(summary.group_id, GroupId(2));
		assert_eq!(summary.validity_votes, 1);
	}

	#[test]
	fn candidate_vote_gives_summary() {
		let context = TestContext {
			authorities: {
				let mut map = HashMap::new();
				map.insert(AuthorityId(1), GroupId(2));
				map.insert(AuthorityId(2), GroupId(2));
				map
			}
		};

		let mut table = create();
		let statement = SignedStatement {
			statement: Statement::Candidate(Candidate(2, 100)),
			signature: Signature(1),
			sender: AuthorityId(1),
		};
		let candidate_digest = Digest(100);

		table.import_statement(&context, statement);
		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(1)));

		let vote = SignedStatement {
			statement: Statement::Valid(candidate_digest.clone()),
			signature: Signature(2),
			sender: AuthorityId(2),
		};

		let summary = table.import_statement(&context, vote)
			.expect("candidate vote to give summary");

		assert!(!table.detected_misbehavior.contains_key(&AuthorityId(2)));

		assert_eq!(summary.candidate, Digest(100));
		assert_eq!(summary.group_id, GroupId(2));
		assert_eq!(summary.validity_votes, 2);
	}
}
