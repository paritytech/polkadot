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

//! Runtime component for handling disputes of parachain candidates.

use sp_std::prelude::*;
use sp_std::result;
#[cfg(feature = "std")]
use sp_std::marker::PhantomData;
use primitives::v1::{
	Id as ParaId, ValidationCode, HeadData, SessionIndex, Hash, BlockNumber, CandidateHash,
	DisputeState, DisputeStatementSet, MultiDisputeStatementSet, ValidatorId, ValidatorSignature,
	DisputeStatement, ValidDisputeStatementKind, InvalidDisputeStatementKind,
	ExplicitDisputeStatement, CompactStatement, SigningContext, ApprovalVote, ValidatorIndex,
	ConsensusLog,
};
use sp_runtime::{
	traits::{One, Zero, Saturating, AppVerify},
	DispatchResult, DispatchError, SaturatedConversion,
};
use frame_system::ensure_root;
use frame_support::{
	decl_storage, decl_module, decl_error, decl_event, ensure,
	traits::Get,
	weights::Weight,
};
use parity_scale_codec::{Encode, Decode};
use bitvec::{bitvec, order::Lsb0 as BitOrderLsb0};
use crate::{
	configuration::{self, HostConfiguration},
	initializer::SessionChangeNotification,
	shared,
	session_info,
};

/// Reward hooks for disputes.
pub trait RewardValidators {
	// Give each validator a reward, likely small, for participating in the dispute.
	fn reward_dispute_statement(session: SessionIndex, validators: impl IntoIterator<Item=ValidatorIndex>);
}

impl RewardValidators for () {
	fn reward_dispute_statement(_: SessionIndex, _: impl IntoIterator<Item=ValidatorIndex>) { }
}

/// Punishment hooks for disputes.
pub trait PunishValidators {
	/// Punish a series of validators who were for an invalid parablock. This is expected to be a major
	/// punishment.
	fn punish_for_invalid(session: SessionIndex, validators: impl IntoIterator<Item=ValidatorIndex>);

	/// Punish a series of validators who were against a valid parablock. This is expected to be a minor
	/// punishment.
	fn punish_against_valid(session: SessionIndex, validators: impl IntoIterator<Item=ValidatorIndex>);

	/// Punish a series of validators who were part of a dispute which never concluded. This is expected
	/// to be a minor punishment.
	fn punish_inconclusive(session: SessionIndex, validators: impl IntoIterator<Item=ValidatorIndex>);
}

pub trait Config:
	frame_system::Config +
	configuration::Config +
	session_info::Config
{
	type Event: From<Event> + Into<<Self as frame_system::Config>::Event>;
	type RewardValidators: RewardValidators;
	type PunishValidators: PunishValidators;
}

decl_storage! {
	trait Store for Module<T: Config> as ParaDisputes {
		/// The last pruneed session, if any. All data stored by this module
		/// references sessions.
		LastPrunedSession: Option<SessionIndex>;
		// All ongoing or concluded disputes for the last several sessions.
		Disputes: double_map
			hasher(twox_64_concat) SessionIndex,
			hasher(blake2_128_concat) CandidateHash
			=> Option<DisputeState<T::BlockNumber>>;
		// All included blocks on the chain, as well as the block number in this chain that
		// should be reverted back to if the candidate is disputed and determined to be invalid.
		Included: double_map
			hasher(twox_64_concat) SessionIndex,
			hasher(blake2_128_concat) CandidateHash
			=> Option<T::BlockNumber>;
		// Maps session indices to a vector indicating the number of potentially-spam disputes
		// each validator is participating in. Potentially-spam disputes are remote disputes which have
		// fewer than `byzantine_threshold + 1` validators.
		//
		// The i'th entry of the vector corresponds to the i'th validator in the session.
		SpamSlots: map hasher(twox_64_concat) SessionIndex => Option<Vec<u32>>;
		// Whether the chain is frozen or not. Starts as `false`. When this is `true`,
		// the chain will not accept any new parachain blocks for backing or inclusion.
		// It can only be set back to `false` by governance intervention.
		Frozen get(fn is_frozen): bool;
	}
}

decl_event! {
	pub enum Event {
		/// A dispute has been initiated. The boolean is true if the dispute is local. \[para_id\]
		DisputeInitiated(ParaId, CandidateHash, bool),
		/// A dispute has concluded for or against a candidate. The boolean is true if the candidate
		/// is deemed valid (for) and false if invalid (against).
		DisputeConcluded(ParaId, CandidateHash, bool),
		/// A dispute has timed out due to insufficient participation.
		DisputeTimedOut(ParaId, CandidateHash),
	}
}

decl_module! {
	/// The parachains configuration module.
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		fn deposit_event() = default;
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// Duplicate dispute statement sets provided.
		DuplicateDisputeStatementSets,
		/// Ancient dispute statement provided.
		AncientDisputeStatement,
		/// Validator index on statement is out of bounds for session.
		ValidatorIndexOutOfBounds,
		/// Invalid signature on statement.
		InvalidSignature,
		/// Validator vote submitted more than once to dispute.
		DuplicateStatement,
		/// Too many spam slots used by some specific validator.
		PotentialSpam,
	}
}

// The maximum number of validators `f` which may safely be faulty.
//
// The total number of validators is `n = 3f + e` where `e in { 1, 2, 3 }`.
fn byzantine_threshold(n: usize) -> usize {
	n.saturating_sub(1) / 3
}

// The supermajority threshold of validators which is required to
// conclude a dispute.
fn supermajority_threshold(n: usize) -> usize {
	n - byzantine_threshold(n)
}

bitflags::bitflags! {
	#[derive(Default)]
	struct DisputeStateFlags: u8 {
		const CONFIRMED = 0b0001;
		const FOR_SUPERMAJORITY = 0b0010;
		const AGAINST_SUPERMAJORITY = 0b0100;
	}
}

impl DisputeStateFlags {
	fn from_state<BlockNumber>(
		state: &DisputeState<BlockNumber>,
	) -> Self {
		let n = state.validators_for.len();

		let byzantine_threshold = byzantine_threshold(n);
		let supermajority_threshold = supermajority_threshold(n);

		let mut flags = DisputeStateFlags::default();
		let all_participants = {
			let mut a = state.validators_for.clone();
			*a |= state.validators_against.iter().by_val();
			a
		};
		if all_participants.count_ones() > byzantine_threshold {
			flags |= DisputeStateFlags::CONFIRMED;
		}

		if state.validators_for.count_ones() >= supermajority_threshold {
			flags |= DisputeStateFlags::FOR_SUPERMAJORITY;
		}

		if state.validators_against.count_ones() >= supermajority_threshold {
			flags |= DisputeStateFlags::AGAINST_SUPERMAJORITY;
		}

		flags
	}
}

enum SpamSlotChange {
	Inc,
	Dec,
}

struct ImportSummary<BlockNumber> {
	// The new state, with all votes imported.
	state: DisputeState<BlockNumber>,
	// Changes to spam slots. Validator index paired with directional change.
	spam_slot_changes: Vec<(ValidatorIndex, SpamSlotChange)>,
	// Validators to slash for being (wrongly) on the AGAINST side.
	slash_against: Vec<ValidatorIndex>,
	// Validators to slash for being (wrongly) on the FOR side.
	slash_for: Vec<ValidatorIndex>,
	// New participants in the dispute.
	new_participants: bitvec::vec::BitVec<BitOrderLsb0, u8>,
	// Difference in state flags from previous.
	new_flags: DisputeStateFlags,
}

enum VoteImportError {
	ValidatorIndexOutOfBounds,
	DuplicateStatement,
}

impl<T: Config> From<VoteImportError> for Error<T> {
	fn from(e: VoteImportError) -> Self {
		match e {
			VoteImportError::ValidatorIndexOutOfBounds => Error::<T>::ValidatorIndexOutOfBounds,
			VoteImportError::DuplicateStatement => Error::<T>::DuplicateStatement,
		}
	}
}

struct DisputeStateImporter<BlockNumber> {
	state: DisputeState<BlockNumber>,
	now: BlockNumber,
	new_participants: bitvec::vec::BitVec<BitOrderLsb0, u8>,
	pre_flags: DisputeStateFlags,
}

impl<BlockNumber: Clone> DisputeStateImporter<BlockNumber> {
	fn new(
		state: DisputeState<BlockNumber>,
		now: BlockNumber,
	) -> Self {
		let pre_flags = DisputeStateFlags::from_state(&state);
		let new_participants = bitvec::bitvec![BitOrderLsb0, u8; 0; state.validators_for.len()];

		DisputeStateImporter {
			state,
			now,
			new_participants,
			pre_flags,
		}
	}

	fn import(&mut self, validator: ValidatorIndex, valid: bool)
		-> Result<(), VoteImportError>
	{
		let (bits, other_bits) = if valid {
			(&mut self.state.validators_for, &mut self.state.validators_against)
		} else {
			(&mut self.state.validators_against, &mut self.state.validators_for)
		};

		// out of bounds or already participated
		match bits.get(validator.0 as usize).map(|b| *b) {
			None => return Err(VoteImportError::ValidatorIndexOutOfBounds),
			Some(true) => return Err(VoteImportError::DuplicateStatement),
			Some(false) => {}
		}

		// inefficient, and just for extra sanity.
		if validator.0 as usize >= self.new_participants.len() {
			return Err(VoteImportError::ValidatorIndexOutOfBounds);
		}

		bits.set(validator.0 as usize, true);

		// New participants tracks those which didn't appear on either
		// side of the dispute until now. So we check the other side
		// and checked the first side before.
		if other_bits.get(validator.0 as usize).map_or(false, |b| !*b) {
			self.new_participants.set(validator.0 as usize, true);
		}

		Ok(())
	}

	fn finish(mut self) -> ImportSummary<BlockNumber> {
		let pre_flags = self.pre_flags;
		let post_flags = DisputeStateFlags::from_state(&self.state);

		let pre_post_contains = |flags| (pre_flags.contains(flags), post_flags.contains(flags));

		// 1. Act on confirmed flag state to inform spam slots changes.
		let spam_slot_changes: Vec<_> = match pre_post_contains(DisputeStateFlags::CONFIRMED) {
			(false, false) => {
				// increment spam slots for all new participants.
				self.new_participants.iter_ones()
					.map(|i| (ValidatorIndex(i as _), SpamSlotChange::Inc))
					.collect()
			}
			(false, true) => {
				let prev_participants = {
					// all participants
					let mut a = self.state.validators_for.clone();
					*a |= self.state.validators_against.iter().by_val();

					// which are not new participants
					*a &= self.new_participants.iter().by_val().map(|b| !b);

					a
				};

				prev_participants.iter_ones()
					.map(|i| (ValidatorIndex(i as _), SpamSlotChange::Dec))
					.collect()
			}
			(true, true) | (true, false) => {
				// nothing to do. (true, false) is also impossible.
				Vec::new()
			}
		};

		// 2. Check for fresh FOR supermajority. Only if not already concluded.
		let slash_against = if let (false, true) = pre_post_contains(DisputeStateFlags::FOR_SUPERMAJORITY) {
			if self.state.concluded_at.is_none() {
				self.state.concluded_at = Some(self.now.clone());
			}

			// provide AGAINST voters to slash.
			self.state.validators_against.iter_ones()
				.map(|i| ValidatorIndex(i as _))
				.collect()
		} else {
			Vec::new()
		};

		// 3. Check for fresh AGAINST supermajority.
		let slash_for = if let (false, true) = pre_post_contains(DisputeStateFlags::AGAINST_SUPERMAJORITY) {
			if self.state.concluded_at.is_none() {
				self.state.concluded_at = Some(self.now.clone());
			}

			// provide FOR voters to slash.
			self.state.validators_for.iter_ones()
				.map(|i| ValidatorIndex(i as _))
				.collect()
		} else {
			Vec::new()
		};

		ImportSummary {
			state: self.state,
			spam_slot_changes,
			slash_against,
			slash_for,
			new_participants: self.new_participants,
			new_flags: post_flags - pre_flags,
		}
	}
}

impl<T: Config> Module<T> {
	/// Called by the iniitalizer to initialize the disputes module.
	pub(crate) fn initializer_initialize(now: T::BlockNumber) -> Weight {
		let config = <configuration::Module<T>>::config();

		let mut weight = 0;
		for (session_index, candidate_hash, mut dispute) in <Disputes<T>>::iter() {
			weight += T::DbWeight::get().reads_writes(1, 0);

			if dispute.concluded_at.is_none()
				&& dispute.start + config.dispute_conclusion_by_time_out_period < now
			{
				dispute.concluded_at = Some(now);
				<Disputes<T>>::insert(session_index, candidate_hash, &dispute);

				if <Included<T>>::contains_key(&session_index, &candidate_hash) {
					// Local disputes don't count towards spam.

					weight += T::DbWeight::get().reads_writes(1, 1);
					continue;
				}

				// mildly punish all validators involved. they've failed to make
				// data available to others, so this is most likely spam.
				SpamSlots::mutate(session_index, |spam_slots| {
					let spam_slots = match spam_slots {
						Some(ref mut s) => s,
						None => return,
					};

					// also reduce spam slots for all validators involved, if the dispute was unconfirmed.
					// this does open us up to more spam, but only for validators who are willing
					// to be punished more.
					//
					// it would be unexpected for any change here to occur when the dispute has not concluded
					// in time, as a dispute guaranteed to have at least one honest participant should
					// conclude quickly.
					let participating = decrement_spam(spam_slots, &dispute);

					// Slight punishment as these validators have failed to make data available to
					// others in a timely manner.
					T::PunishValidators::punish_inconclusive(
						session_index,
						participating.iter_ones().map(|i| ValidatorIndex(i as _)),
					);
				});

				weight += T::DbWeight::get().reads_writes(2, 2);
			}
		}

		weight
	}

	/// Called by the iniitalizer to finalize the disputes module.
	pub(crate) fn initializer_finalize() { }

	/// Called by the iniitalizer to note a new session in the disputes module.
	pub(crate) fn initializer_on_new_session(notification: &SessionChangeNotification<T::BlockNumber>) {
		let config = <configuration::Pallet<T>>::config();

		if notification.session_index <= config.dispute_period + 1 {
			return
		}

		let pruning_target = notification.session_index - config.dispute_period - 1;

		LastPrunedSession::mutate(|last_pruned| {
			if let Some(last_pruned) = last_pruned {
				for to_prune in *last_pruned + 1 ..= pruning_target {
					<Disputes<T>>::remove_prefix(to_prune);
					<Included<T>>::remove_prefix(to_prune);
					SpamSlots::remove(to_prune);
				}
			}

			*last_pruned = Some(pruning_target);
		});
	}

	/// Handle sets of dispute statements corresponding to 0 or more candidates.
	pub(crate) fn provide_multi_dispute_data(statement_sets: MultiDisputeStatementSet)
		-> Result<Vec<(SessionIndex, CandidateHash)>, DispatchError>
	{
		let config = <configuration::Pallet<T>>::config();

		// Deduplicate.
		{
			let mut targets: Vec<_> = statement_sets.iter()
				.map(|set| (set.candidate_hash.0, set.session))
				.collect();

			targets.sort();

			let submitted = targets.len();
			targets.dedup();

			ensure!(submitted == targets.len(), Error::<T>::DuplicateDisputeStatementSets);
		}

		let mut fresh = Vec::with_capacity(statement_sets.len());
		for statement_set in statement_sets {
			let dispute_target = (statement_set.session, statement_set.candidate_hash);
			if Self::provide_dispute_data(&config, statement_set)? {
				fresh.push(dispute_target);
			}
		}

		Ok(fresh)
	}

	/// Handle a set of dispute statements corresponding to a single candidate.
	///
	/// Fails if the dispute data is invalid. Returns a bool indicating whether the
	/// dispute is fresh.
	fn provide_dispute_data(config: &HostConfiguration<T::BlockNumber>, set: DisputeStatementSet)
		-> Result<bool, DispatchError>
	{
		// Dispute statement sets on any dispute which concluded
		// before this point are to be rejected.
		let now = <frame_system::Pallet<T>>::block_number();
		let oldest_accepted = now.saturating_sub(config.dispute_post_conclusion_acceptance_period);

		// Load session info to access validators
		let session_info = match <session_info::Pallet<T>>::session_info(set.session) {
			Some(s) => s,
			None => return Err(Error::<T>::AncientDisputeStatement.into()),
		};

		let n_validators = session_info.validators.len();

		// Check for ancient.
		let (fresh, dispute_state) = {
			if let Some(dispute_state) = <Disputes<T>>::get(&set.session, &set.candidate_hash) {
				ensure!(
					dispute_state.concluded_at.as_ref().map_or(true, |c| c >= &oldest_accepted),
					Error::<T>::AncientDisputeStatement,
				);

				(false, dispute_state)
			} else {
				(
					true,
					DisputeState {
						validators_for: bitvec![BitOrderLsb0, u8; 0; n_validators],
						validators_against: bitvec![BitOrderLsb0, u8; 0; n_validators],
						start: now,
						concluded_at: None,
					}
				)
			}
		};

		// Check and import all votes.
		let summary = {
			let mut importer = DisputeStateImporter::new(dispute_state, now);
			for (statement, validator_index, signature) in &set.statements {
				let validator_public = session_info.validators.get(validator_index.0 as usize)
					.ok_or(Error::<T>::ValidatorIndexOutOfBounds)?;

				// Check signature before importing.
				check_signature(
					&validator_public,
					set.candidate_hash,
					set.session,
					statement,
					signature,
				).map_err(|()| Error::<T>::InvalidSignature)?;

				let valid = match statement {
					DisputeStatement::Valid(_) => true,
					DisputeStatement::Invalid(_) => false,
				};

				importer.import(*validator_index, valid).map_err(Error::<T>::from)?;
			}

			importer.finish()
		};

		// Apply spam slot changes. Bail early if too many occupied.
		if !<Included<T>>::contains_key(&set.session, &set.candidate_hash) {
			let mut spam_slots: Vec<u32> = SpamSlots::get(&set.session)
				.unwrap_or_else(|| vec![0; n_validators]);

			for (validator_index, spam_slot_change) in summary.spam_slot_changes {
				let spam_slot = spam_slots.get_mut(validator_index.0 as usize)
					.expect("index is in-bounds, as checked above; qed");

				match spam_slot_change {
					SpamSlotChange::Inc => {
						ensure!(
							*spam_slot < config.dispute_max_spam_slots,
							Error::<T>::PotentialSpam,
						);

						*spam_slot += 1;
					}
					SpamSlotChange::Dec => {
						*spam_slot = spam_slot.saturating_sub(1);
					}
				}
			}

			SpamSlots::insert(&set.session, spam_slots);
		}

		// Reward statements.
		T::RewardValidators::reward_dispute_statement(
			set.session,
			summary.new_participants.iter_ones().map(|i| ValidatorIndex(i as _)),
		);

		// Slash participants on a losing side.
		{
			if summary.new_flags.contains(DisputeStateFlags::FOR_SUPERMAJORITY) {
				// a valid candidate, according to 2/3. Punish those on the 'against' side.
				T::PunishValidators::punish_against_valid(
					set.session,
					summary.state.validators_against.iter_ones().map(|i| ValidatorIndex(i as _)),
				);
			}

			if summary.new_flags.contains(DisputeStateFlags::AGAINST_SUPERMAJORITY) {
				// an invalid candidate, according to 2/3. Punish those on the 'for' side.
				T::PunishValidators::punish_against_valid(
					set.session,
					summary.state.validators_for.iter_ones().map(|i| ValidatorIndex(i as _)),
				);
			}
		}

		<Disputes<T>>::insert(&set.session, &set.candidate_hash, &summary.state);

		// Freeze if just concluded against some local candidate
		if summary.new_flags.contains(DisputeStateFlags::AGAINST_SUPERMAJORITY) {
			if let Some(revert_to) = <Included<T>>::get(&set.session, &set.candidate_hash) {
				Self::revert_and_freeze(revert_to);
			}
		}

		Ok(fresh)
	}

	pub(crate) fn disputes() -> Vec<(SessionIndex, CandidateHash, DisputeState<T::BlockNumber>)> {
		<Disputes<T>>::iter().collect()
	}

	pub(crate) fn note_included(session: SessionIndex, candidate_hash: CandidateHash, included_in: T::BlockNumber) {
		if included_in.is_zero() { return }

		let revert_to = included_in - One::one();

		<Included<T>>::insert(&session, &candidate_hash, revert_to);

		// If we just included a block locally which has a live dispute, decrement spam slots
		// for any involved validators, if the dispute is not already confirmed by f + 1.
		if let Some(state) = <Disputes<T>>::get(&session, candidate_hash) {
			SpamSlots::mutate(&session, |spam_slots| {
				if let Some(ref mut spam_slots) = *spam_slots {
					decrement_spam(spam_slots, &state);
				}
			});

			if has_supermajority_against(&state) {
				Self::revert_and_freeze(revert_to);
			}
		}
	}

	pub(crate) fn could_be_invalid(session: SessionIndex, candidate_hash: CandidateHash) -> bool {
		<Disputes<T>>::get(&session, &candidate_hash).map_or(false, |dispute| {
			// A dispute that is ongoing or has concluded with supermajority-against.
			dispute.concluded_at.is_none() || has_supermajority_against(&dispute)
		})
	}

	pub(crate) fn revert_and_freeze(revert_to: T::BlockNumber) {
		if Self::is_frozen() { return }

		let revert_to = revert_to.saturated_into();
		<frame_system::Pallet<T>>::deposit_log(ConsensusLog::RevertTo(revert_to).into());

		Frozen::set(true);
	}
}

fn has_supermajority_against<BlockNumber>(dispute: &DisputeState<BlockNumber>) -> bool {
	let supermajority_threshold = supermajority_threshold(dispute.validators_against.len());
	dispute.validators_against.count_ones() >= supermajority_threshold
}

// If the dispute had not enough validators to confirm, decrement spam slots for all the participating
// validators.
//
// returns the set of participating validators as a bitvec.
fn decrement_spam<BlockNumber>(
	spam_slots: &mut [u32],
	dispute: &DisputeState<BlockNumber>,
) -> bitvec::vec::BitVec<BitOrderLsb0, u8> {
	let byzantine_threshold = byzantine_threshold(spam_slots.len());

	let participating = dispute.validators_for.clone() | dispute.validators_against.iter().by_val();
	let decrement_spam = participating.count_ones() <= byzantine_threshold;
	for validator_index in participating.iter_ones() {
		if decrement_spam {
			if let Some(occupied) = spam_slots.get_mut(validator_index as usize) {
				*occupied = occupied.saturating_sub(1);
			}
		}
	}

	participating
}

fn check_signature(
	validator_public: &ValidatorId,
	candidate_hash: CandidateHash,
	session: SessionIndex,
	statement: &DisputeStatement,
	validator_signature: &ValidatorSignature,
) -> Result<(), ()> {
	let payload = match *statement {
		DisputeStatement::Valid(ValidDisputeStatementKind::Explicit) => {
			ExplicitDisputeStatement {
				valid: true,
				candidate_hash,
				session,
			}.signing_payload()
		},
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(inclusion_parent)) => {
			CompactStatement::Seconded(candidate_hash).signing_payload(&SigningContext {
				session_index: session,
				parent_hash: inclusion_parent,
			})
		},
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(inclusion_parent)) => {
			CompactStatement::Valid(candidate_hash).signing_payload(&SigningContext {
				session_index: session,
				parent_hash: inclusion_parent,
			})
		},
		DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking) => {
			ApprovalVote(candidate_hash).signing_payload(session)
		},
		DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit) => {
			ExplicitDisputeStatement {
				valid: false,
				candidate_hash,
				session,
			}.signing_payload()
		},
	};

	if validator_signature.verify(&payload[..] , &validator_public) {
		Ok(())
	} else {
		Err(())
	}
}
