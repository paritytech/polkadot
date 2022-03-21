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

use crate::{configuration, initializer::SessionChangeNotification, session_info};
use bitvec::{bitvec, order::Lsb0 as BitOrderLsb0};
use frame_support::{ensure, traits::Get, weights::Weight};
use frame_system::pallet_prelude::*;
use parity_scale_codec::{Decode, Encode};
use primitives::v2::{
	byzantine_threshold, supermajority_threshold, ApprovalVote, CandidateHash,
	CheckedDisputeStatementSet, CheckedMultiDisputeStatementSet, CompactStatement, ConsensusLog,
	DisputeState, DisputeStatement, DisputeStatementSet, ExplicitDisputeStatement,
	InvalidDisputeStatementKind, MultiDisputeStatementSet, SessionIndex, SigningContext,
	ValidDisputeStatementKind, ValidatorId, ValidatorIndex, ValidatorSignature,
};
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{AppVerify, One, Saturating, Zero},
	DispatchError, RuntimeDebug, SaturatedConversion,
};
use sp_std::{cmp::Ordering, prelude::*};

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use self::tests::run_to_block;

#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

/// Whether the dispute is local or remote.
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum DisputeLocation {
	Local,
	Remote,
}

/// The result of a dispute, whether the candidate is deemed valid (for) or invalid (against).
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
pub enum DisputeResult {
	Valid,
	Invalid,
}

/// Reward hooks for disputes.
pub trait RewardValidators {
	// Give each validator a reward, likely small, for participating in the dispute.
	fn reward_dispute_statement(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	);
}

impl RewardValidators for () {
	fn reward_dispute_statement(_: SessionIndex, _: impl IntoIterator<Item = ValidatorIndex>) {}
}

/// Punishment hooks for disputes.
pub trait PunishValidators {
	/// Punish a series of validators who were for an invalid parablock. This is expected to be a major
	/// punishment.
	fn punish_for_invalid(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	);

	/// Punish a series of validators who were against a valid parablock. This is expected to be a minor
	/// punishment.
	fn punish_against_valid(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	);

	/// Punish a series of validators who were part of a dispute which never concluded. This is expected
	/// to be a minor punishment.
	fn punish_inconclusive(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	);
}

impl PunishValidators for () {
	fn punish_for_invalid(_: SessionIndex, _: impl IntoIterator<Item = ValidatorIndex>) {}

	fn punish_against_valid(_: SessionIndex, _: impl IntoIterator<Item = ValidatorIndex>) {}

	fn punish_inconclusive(_: SessionIndex, _: impl IntoIterator<Item = ValidatorIndex>) {}
}

/// Binary discriminator to determine if the expensive signature
/// checks are necessary.
#[derive(Clone, Copy)]
pub enum VerifyDisputeSignatures {
	/// Yes, verify the signatures.
	Yes,
	/// No, skip the signature verification.
	///
	/// Only done if there exists an invariant that
	/// can guaranteed the signature was checked before.
	Skip,
}

/// Provide a `Ordering` for the two provided dispute statement sets according to the
/// following prioritization:
///  1. Prioritize local disputes over remote disputes
///  2. Prioritize older disputes over newer disputes
fn dispute_ordering_compare<T: DisputesHandler<BlockNumber>, BlockNumber: Ord>(
	a: &DisputeStatementSet,
	b: &DisputeStatementSet,
) -> Ordering
where
	T: ?Sized,
{
	let a_local_block =
		<T as DisputesHandler<BlockNumber>>::included_state(a.session, a.candidate_hash);
	let b_local_block =
		<T as DisputesHandler<BlockNumber>>::included_state(b.session, b.candidate_hash);
	match (a_local_block, b_local_block) {
		// Prioritize local disputes over remote disputes.
		(None, Some(_)) => Ordering::Greater,
		(Some(_), None) => Ordering::Less,
		// For local disputes, prioritize those that occur at an earlier height.
		(Some(a_height), Some(b_height)) =>
			a_height.cmp(&b_height).then_with(|| a.candidate_hash.cmp(&b.candidate_hash)),
		// Prioritize earlier remote disputes using session as rough proxy.
		(None, None) => {
			let session_ord = a.session.cmp(&b.session);
			if session_ord == Ordering::Equal {
				// sort by hash as last resort, to make below dedup work consistently
				a.candidate_hash.cmp(&b.candidate_hash)
			} else {
				session_ord
			}
		},
	}
}

use super::paras_inherent::IsSortedBy;

/// Returns `true` if duplicate items were found, otherwise `false`.
///
/// `check_equal(a: &T, b: &T)` _must_ return `true`, iff `a` and `b` are equal, otherwise `false.
/// The definition of _equal_ is to be defined by the user.
///
/// Attention: Requires the input `iter` to be sorted, such that _equals_
/// would be adjacent in respect whatever `check_equal` defines as equality!
fn contains_duplicates_in_sorted_iter<
	'a,
	T: 'a,
	I: 'a + IntoIterator<Item = &'a T>,
	C: 'static + FnMut(&T, &T) -> bool,
>(
	iter: I,
	mut check_equal: C,
) -> bool {
	let mut iter = iter.into_iter();
	if let Some(mut previous) = iter.next() {
		while let Some(current) = iter.next() {
			if check_equal(previous, current) {
				return true
			}
			previous = current;
		}
	}
	return false
}

/// Hook into disputes handling.
///
/// Allows decoupling parachains handling from disputes so that it can
/// potentially be disabled when instantiating a specific runtime.
pub trait DisputesHandler<BlockNumber: Ord> {
	/// Whether the chain is frozen, if the chain is frozen it will not accept
	/// any new parachain blocks for backing or inclusion.
	fn is_frozen() -> bool;

	/// Assure sanity
	fn assure_deduplicated_and_sorted(statement_sets: &MultiDisputeStatementSet) -> Result<(), ()> {
		if !IsSortedBy::is_sorted_by(
			statement_sets.as_slice(),
			dispute_ordering_compare::<Self, BlockNumber>,
		) {
			return Err(())
		}
		// Sorted, so according to session and candidate hash, this will detect duplicates.
		if contains_duplicates_in_sorted_iter(statement_sets, |previous, current| {
			current.session == previous.session && current.candidate_hash == previous.candidate_hash
		}) {
			return Err(())
		}
		Ok(())
	}

	/// Remove dispute statement duplicates and sort the non-duplicates based on
	/// local (lower indicies) vs remotes (higher indices) and age (older with lower indices).
	///
	/// Returns `Ok(())` if no duplicates were present, `Err(())` otherwise.
	///
	/// Unsorted data does not change the return value, while the node side
	/// is generally expected to pass them in sorted.
	fn deduplicate_and_sort_dispute_data(
		statement_sets: &mut MultiDisputeStatementSet,
	) -> Result<(), ()> {
		// TODO: Consider trade-of to avoid `O(n * log(n))` average lookups of `included_state`
		// TODO: instead make a single pass and store the values lazily.
		// TODO: https://github.com/paritytech/polkadot/issues/4527
		let n = statement_sets.len();

		statement_sets.sort_by(dispute_ordering_compare::<Self, BlockNumber>);
		statement_sets
			.dedup_by(|a, b| a.session == b.session && a.candidate_hash == b.candidate_hash);

		// if there were any duplicates, indicate that to the caller.
		if n == statement_sets.len() {
			Ok(())
		} else {
			Err(())
		}
	}

	/// Filter a single dispute statement set.
	///
	/// Used in cases where more granular control is required, i.e. when
	/// accounting for maximum block weight.
	fn filter_dispute_data(
		statement_set: DisputeStatementSet,
		max_spam_slots: u32,
		post_conclusion_acceptance_period: BlockNumber,
		verify_sigs: VerifyDisputeSignatures,
	) -> Option<CheckedDisputeStatementSet>;

	/// Handle sets of dispute statements corresponding to 0 or more candidates.
	/// Returns a vector of freshly created disputes.
	fn process_checked_multi_dispute_data(
		statement_sets: CheckedMultiDisputeStatementSet,
	) -> Result<Vec<(SessionIndex, CandidateHash)>, DispatchError>;

	/// Note that the given candidate has been included.
	fn note_included(
		session: SessionIndex,
		candidate_hash: CandidateHash,
		included_in: BlockNumber,
	);

	/// Retrieve the included state of a given candidate in a particular session. If it
	/// returns `Some`, then we have a local dispute for the given `candidate_hash`.
	fn included_state(session: SessionIndex, candidate_hash: CandidateHash) -> Option<BlockNumber>;

	/// Whether the given candidate concluded invalid in a dispute with supermajority.
	fn concluded_invalid(session: SessionIndex, candidate_hash: CandidateHash) -> bool;

	/// Called by the initializer to initialize the disputes pallet.
	fn initializer_initialize(now: BlockNumber) -> Weight;

	/// Called by the initializer to finalize the disputes pallet.
	fn initializer_finalize();

	/// Called by the initializer to note that a new session has started.
	fn initializer_on_new_session(notification: &SessionChangeNotification<BlockNumber>);
}

impl<BlockNumber: Ord> DisputesHandler<BlockNumber> for () {
	fn is_frozen() -> bool {
		false
	}

	fn deduplicate_and_sort_dispute_data(
		statement_sets: &mut MultiDisputeStatementSet,
	) -> Result<(), ()> {
		statement_sets.clear();
		Ok(())
	}

	fn filter_dispute_data(
		_set: DisputeStatementSet,
		_max_spam_slots: u32,
		_post_conclusion_acceptance_period: BlockNumber,
		_verify_sigs: VerifyDisputeSignatures,
	) -> Option<CheckedDisputeStatementSet> {
		None
	}

	fn process_checked_multi_dispute_data(
		_statement_sets: CheckedMultiDisputeStatementSet,
	) -> Result<Vec<(SessionIndex, CandidateHash)>, DispatchError> {
		Ok(Vec::new())
	}

	fn note_included(
		_session: SessionIndex,
		_candidate_hash: CandidateHash,
		_included_in: BlockNumber,
	) {
	}

	fn included_state(
		_session: SessionIndex,
		_candidate_hash: CandidateHash,
	) -> Option<BlockNumber> {
		None
	}

	fn concluded_invalid(_session: SessionIndex, _candidate_hash: CandidateHash) -> bool {
		false
	}

	fn initializer_initialize(_now: BlockNumber) -> Weight {
		0
	}

	fn initializer_finalize() {}

	fn initializer_on_new_session(_notification: &SessionChangeNotification<BlockNumber>) {}
}

impl<T: Config> DisputesHandler<T::BlockNumber> for pallet::Pallet<T>
where
	T::BlockNumber: Ord,
{
	fn is_frozen() -> bool {
		pallet::Pallet::<T>::is_frozen()
	}

	fn filter_dispute_data(
		set: DisputeStatementSet,
		max_spam_slots: u32,
		post_conclusion_acceptance_period: T::BlockNumber,
		verify_sigs: VerifyDisputeSignatures,
	) -> Option<CheckedDisputeStatementSet> {
		pallet::Pallet::<T>::filter_dispute_data(
			&set,
			post_conclusion_acceptance_period,
			max_spam_slots,
			verify_sigs,
		)
		.filter_statement_set(set)
	}

	fn process_checked_multi_dispute_data(
		statement_sets: CheckedMultiDisputeStatementSet,
	) -> Result<Vec<(SessionIndex, CandidateHash)>, DispatchError> {
		pallet::Pallet::<T>::process_checked_multi_dispute_data(statement_sets)
	}

	fn note_included(
		session: SessionIndex,
		candidate_hash: CandidateHash,
		included_in: T::BlockNumber,
	) {
		pallet::Pallet::<T>::note_included(session, candidate_hash, included_in)
	}

	fn included_state(
		session: SessionIndex,
		candidate_hash: CandidateHash,
	) -> Option<T::BlockNumber> {
		pallet::Pallet::<T>::included_state(session, candidate_hash)
	}

	fn concluded_invalid(session: SessionIndex, candidate_hash: CandidateHash) -> bool {
		pallet::Pallet::<T>::concluded_invalid(session, candidate_hash)
	}

	fn initializer_initialize(now: T::BlockNumber) -> Weight {
		pallet::Pallet::<T>::initializer_initialize(now)
	}

	fn initializer_finalize() {
		pallet::Pallet::<T>::initializer_finalize()
	}

	fn initializer_on_new_session(notification: &SessionChangeNotification<T::BlockNumber>) {
		pallet::Pallet::<T>::initializer_on_new_session(notification)
	}
}

pub trait WeightInfo {
	fn force_unfreeze() -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn force_unfreeze() -> Weight {
		0
	}
}

pub use pallet::*;
#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + session_info::Config {
		type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
		type RewardValidators: RewardValidators;
		type PunishValidators: PunishValidators;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::pallet]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	/// The last pruned session, if any. All data stored by this module
	/// references sessions.
	#[pallet::storage]
	pub(super) type LastPrunedSession<T> = StorageValue<_, SessionIndex>;

	/// All ongoing or concluded disputes for the last several sessions.
	#[pallet::storage]
	pub(super) type Disputes<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Blake2_128Concat,
		CandidateHash,
		DisputeState<T::BlockNumber>,
	>;

	/// All included blocks on the chain, as well as the block number in this chain that
	/// should be reverted back to if the candidate is disputed and determined to be invalid.
	#[pallet::storage]
	pub(super) type Included<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Blake2_128Concat,
		CandidateHash,
		T::BlockNumber,
	>;

	/// Maps session indices to a vector indicating the number of potentially-spam disputes
	/// each validator is participating in. Potentially-spam disputes are remote disputes which have
	/// fewer than `byzantine_threshold + 1` validators.
	///
	/// The i'th entry of the vector corresponds to the i'th validator in the session.
	#[pallet::storage]
	pub(super) type SpamSlots<T> = StorageMap<_, Twox64Concat, SessionIndex, Vec<u32>>;

	/// Whether the chain is frozen. Starts as `None`. When this is `Some`,
	/// the chain will not accept any new parachain blocks for backing or inclusion,
	/// and its value indicates the last valid block number in the chain.
	/// It can only be set back to `None` by governance intervention.
	#[pallet::storage]
	#[pallet::getter(fn last_valid_block)]
	pub(super) type Frozen<T: Config> = StorageValue<_, Option<T::BlockNumber>, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub fn deposit_event)]
	pub enum Event<T: Config> {
		/// A dispute has been initiated. \[candidate hash, dispute location\]
		DisputeInitiated(CandidateHash, DisputeLocation),
		/// A dispute has concluded for or against a candidate.
		/// `\[para id, candidate hash, dispute result\]`
		DisputeConcluded(CandidateHash, DisputeResult),
		/// A dispute has timed out due to insufficient participation.
		/// `\[para id, candidate hash\]`
		DisputeTimedOut(CandidateHash),
		/// A dispute has concluded with supermajority against a candidate.
		/// Block authors should no longer build on top of this head and should
		/// instead revert the block at the given height. This should be the
		/// number of the child of the last known valid block in the chain.
		Revert(T::BlockNumber),
	}

	#[pallet::error]
	pub enum Error<T> {
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
		/// A dispute where there are only votes on one side.
		SingleSidedDispute,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::weight(<T as Config>::WeightInfo::force_unfreeze())]
		pub fn force_unfreeze(origin: OriginFor<T>) -> DispatchResult {
			ensure_root(origin)?;
			Frozen::<T>::set(None);
			Ok(())
		}
	}
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
	fn from_state<BlockNumber>(state: &DisputeState<BlockNumber>) -> Self {
		let n = state.validators_for.len();

		let byzantine_threshold = byzantine_threshold(n);
		let supermajority_threshold = supermajority_threshold(n);

		let mut flags = DisputeStateFlags::default();
		let all_participants = state.validators_for.clone() | state.validators_against.clone();
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

#[derive(PartialEq, RuntimeDebug)]
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
	new_participants: bitvec::vec::BitVec<u8, BitOrderLsb0>,
	// Difference in state flags from previous.
	new_flags: DisputeStateFlags,
}

#[derive(RuntimeDebug, PartialEq, Eq)]
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

#[derive(RuntimeDebug, PartialEq, Eq)]
struct ImportUndo {
	validator_index: ValidatorIndex,
	valid: bool,
	new_participant: bool,
}

struct DisputeStateImporter<BlockNumber> {
	state: DisputeState<BlockNumber>,
	now: BlockNumber,
	new_participants: bitvec::vec::BitVec<u8, BitOrderLsb0>,
	pre_flags: DisputeStateFlags,
}

impl<BlockNumber: Clone> DisputeStateImporter<BlockNumber> {
	fn new(state: DisputeState<BlockNumber>, now: BlockNumber) -> Self {
		let pre_flags = DisputeStateFlags::from_state(&state);
		let new_participants = bitvec::bitvec![u8, BitOrderLsb0; 0; state.validators_for.len()];

		DisputeStateImporter { state, now, new_participants, pre_flags }
	}

	fn import(
		&mut self,
		validator: ValidatorIndex,
		valid: bool,
	) -> Result<ImportUndo, VoteImportError> {
		let (bits, other_bits) = if valid {
			(&mut self.state.validators_for, &mut self.state.validators_against)
		} else {
			(&mut self.state.validators_against, &mut self.state.validators_for)
		};

		// out of bounds or already participated
		match bits.get(validator.0 as usize).map(|b| *b) {
			None => return Err(VoteImportError::ValidatorIndexOutOfBounds),
			Some(true) => return Err(VoteImportError::DuplicateStatement),
			Some(false) => {},
		}

		// inefficient, and just for extra sanity.
		if validator.0 as usize >= self.new_participants.len() {
			return Err(VoteImportError::ValidatorIndexOutOfBounds)
		}

		let mut undo = ImportUndo { validator_index: validator, valid, new_participant: false };

		bits.set(validator.0 as usize, true);

		// New participants tracks those which didn't appear on either
		// side of the dispute until now. So we check the other side
		// and checked the first side before.
		if other_bits.get(validator.0 as usize).map_or(false, |b| !*b) {
			undo.new_participant = true;
			self.new_participants.set(validator.0 as usize, true);
		}

		Ok(undo)
	}

	fn undo(&mut self, undo: ImportUndo) {
		if undo.valid {
			self.state.validators_for.set(undo.validator_index.0 as usize, false);
		} else {
			self.state.validators_against.set(undo.validator_index.0 as usize, false);
		}

		if undo.new_participant {
			self.new_participants.set(undo.validator_index.0 as usize, false);
		}
	}

	fn finish(mut self) -> ImportSummary<BlockNumber> {
		let pre_flags = self.pre_flags;
		let post_flags = DisputeStateFlags::from_state(&self.state);

		let pre_post_contains = |flags| (pre_flags.contains(flags), post_flags.contains(flags));

		// 1. Act on confirmed flag state to inform spam slots changes.
		let spam_slot_changes: Vec<_> = match pre_post_contains(DisputeStateFlags::CONFIRMED) {
			(false, false) => {
				// increment spam slots for all new participants.
				self.new_participants
					.iter_ones()
					.map(|i| (ValidatorIndex(i as _), SpamSlotChange::Inc))
					.collect()
			},
			(false, true) => {
				// all participants, which are not new participants
				let prev_participants = (self.state.validators_for.clone() |
					self.state.validators_against.clone()) &
					!self.new_participants.clone();

				prev_participants
					.iter_ones()
					.map(|i| (ValidatorIndex(i as _), SpamSlotChange::Dec))
					.collect()
			},
			(true, true) | (true, false) => {
				// nothing to do. (true, false) is also impossible.
				Vec::new()
			},
		};

		// 2. Check for fresh FOR supermajority. Only if not already concluded.
		let slash_against =
			if let (false, true) = pre_post_contains(DisputeStateFlags::FOR_SUPERMAJORITY) {
				if self.state.concluded_at.is_none() {
					self.state.concluded_at = Some(self.now.clone());
				}

				// provide AGAINST voters to slash.
				self.state
					.validators_against
					.iter_ones()
					.map(|i| ValidatorIndex(i as _))
					.collect()
			} else {
				Vec::new()
			};

		// 3. Check for fresh AGAINST supermajority.
		let slash_for =
			if let (false, true) = pre_post_contains(DisputeStateFlags::AGAINST_SUPERMAJORITY) {
				if self.state.concluded_at.is_none() {
					self.state.concluded_at = Some(self.now.clone());
				}

				// provide FOR voters to slash.
				self.state.validators_for.iter_ones().map(|i| ValidatorIndex(i as _)).collect()
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

// A filter on a dispute statement set.
#[derive(PartialEq)]
#[cfg_attr(test, derive(Debug))]
enum StatementSetFilter {
	// Remove the entire dispute statement set.
	RemoveAll,
	// Remove the votes with given index from the statement set.
	RemoveIndices(Vec<usize>),
}

impl StatementSetFilter {
	fn filter_statement_set(
		self,
		mut statement_set: DisputeStatementSet,
	) -> Option<CheckedDisputeStatementSet> {
		match self {
			StatementSetFilter::RemoveAll => None,
			StatementSetFilter::RemoveIndices(mut indices) => {
				indices.sort();
				indices.dedup();

				// reverse order ensures correctness
				for index in indices.into_iter().rev() {
					// `swap_remove` guarantees linear complexity.
					statement_set.statements.swap_remove(index);
				}

				if statement_set.statements.is_empty() {
					None
				} else {
					// we just checked correctness when filtering.
					Some(CheckedDisputeStatementSet::unchecked_from_unchecked(statement_set))
				}
			},
		}
	}

	fn remove_index(&mut self, i: usize) {
		if let StatementSetFilter::RemoveIndices(ref mut indices) = *self {
			indices.push(i)
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the disputes module.
	pub(crate) fn initializer_initialize(now: T::BlockNumber) -> Weight {
		let config = <configuration::Pallet<T>>::config();

		let mut weight = 0;
		for (session_index, candidate_hash, mut dispute) in <Disputes<T>>::iter() {
			weight += T::DbWeight::get().reads_writes(1, 0);

			if dispute.concluded_at.is_none() &&
				dispute.start + config.dispute_conclusion_by_time_out_period < now
			{
				Self::deposit_event(Event::DisputeTimedOut(candidate_hash));

				dispute.concluded_at = Some(now);
				<Disputes<T>>::insert(session_index, candidate_hash, &dispute);

				if <Included<T>>::contains_key(&session_index, &candidate_hash) {
					// Local disputes don't count towards spam.

					weight += T::DbWeight::get().reads_writes(1, 1);
					continue
				}

				// mildly punish all validators involved. they've failed to make
				// data available to others, so this is most likely spam.
				SpamSlots::<T>::mutate(session_index, |spam_slots| {
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

	/// Called by the initializer to finalize the disputes pallet.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note a new session in the disputes pallet.
	pub(crate) fn initializer_on_new_session(
		notification: &SessionChangeNotification<T::BlockNumber>,
	) {
		let config = <configuration::Pallet<T>>::config();

		if notification.session_index <= config.dispute_period + 1 {
			return
		}

		let pruning_target = notification.session_index - config.dispute_period - 1;

		LastPrunedSession::<T>::mutate(|last_pruned| {
			let to_prune = if let Some(last_pruned) = last_pruned {
				*last_pruned + 1..=pruning_target
			} else {
				pruning_target..=pruning_target
			};

			for to_prune in to_prune {
				// This should be small, as disputes are rare, so `None` is fine.
				<Disputes<T>>::remove_prefix(to_prune, None);

				// This is larger, and will be extracted to the `shared` pallet for more proper pruning.
				// TODO: https://github.com/paritytech/polkadot/issues/3469
				<Included<T>>::remove_prefix(to_prune, None);
				SpamSlots::<T>::remove(to_prune);
			}

			*last_pruned = Some(pruning_target);
		});
	}

	/// Handle sets of dispute statements corresponding to 0 or more candidates.
	/// Returns a vector of freshly created disputes.
	///
	/// Assumes `statement_sets` were already de-duplicated.
	///
	/// # Warning
	///
	/// This functions modifies the state when failing. It is expected to be called in inherent,
	/// and to fail the extrinsic on error. As invalid inherents are not allowed, the dirty state
	/// is not committed.
	pub(crate) fn process_checked_multi_dispute_data(
		statement_sets: CheckedMultiDisputeStatementSet,
	) -> Result<Vec<(SessionIndex, CandidateHash)>, DispatchError> {
		let config = <configuration::Pallet<T>>::config();

		let mut fresh = Vec::with_capacity(statement_sets.len());
		for statement_set in statement_sets {
			let dispute_target = {
				let statement_set: &DisputeStatementSet = statement_set.as_ref();
				(statement_set.session, statement_set.candidate_hash)
			};
			if Self::process_checked_dispute_data(
				statement_set,
				config.dispute_post_conclusion_acceptance_period,
			)? {
				fresh.push(dispute_target);
			}
		}

		Ok(fresh)
	}

	// Given a statement set, this produces a filter to be applied to the statement set.
	// It either removes the entire dispute statement set or some specific votes from it.
	//
	// Votes which are duplicate or already known by the chain are filtered out.
	// The entire set is removed if the dispute is both, ancient and concluded.
	fn filter_dispute_data(
		set: &DisputeStatementSet,
		post_conclusion_acceptance_period: <T as frame_system::Config>::BlockNumber,
		max_spam_slots: u32,
		verify_sigs: VerifyDisputeSignatures,
	) -> StatementSetFilter {
		let mut filter = StatementSetFilter::RemoveIndices(Vec::new());

		// Dispute statement sets on any dispute which concluded
		// before this point are to be rejected.
		let now = <frame_system::Pallet<T>>::block_number();
		let oldest_accepted = now.saturating_sub(post_conclusion_acceptance_period);

		// Load session info to access validators
		let session_info = match <session_info::Pallet<T>>::session_info(set.session) {
			Some(s) => s,
			None => return StatementSetFilter::RemoveAll,
		};

		let n_validators = session_info.validators.len();

		// Check for ancient.
		let dispute_state = {
			if let Some(dispute_state) = <Disputes<T>>::get(&set.session, &set.candidate_hash) {
				if dispute_state.concluded_at.as_ref().map_or(false, |c| c < &oldest_accepted) {
					return StatementSetFilter::RemoveAll
				}

				dispute_state
			} else {
				DisputeState {
					validators_for: bitvec![u8, BitOrderLsb0; 0; n_validators],
					validators_against: bitvec![u8, BitOrderLsb0; 0; n_validators],
					start: now,
					concluded_at: None,
				}
			}
		};

		// Check and import all votes.
		let summary = {
			let mut importer = DisputeStateImporter::new(dispute_state, now);
			for (i, (statement, validator_index, signature)) in set.statements.iter().enumerate() {
				let validator_public = match session_info.validators.get(validator_index.0 as usize)
				{
					None => {
						filter.remove_index(i);
						continue
					},
					Some(v) => v,
				};

				let valid = statement.indicates_validity();

				let undo = match importer.import(*validator_index, valid) {
					Ok(u) => u,
					Err(_) => {
						filter.remove_index(i);
						continue
					},
				};

				// Avoid checking signatures repeatedly.
				if let VerifyDisputeSignatures::Yes = verify_sigs {
					// Check signature after attempting import.
					//
					// Since we expect that this filter will be applied to
					// disputes long after they're concluded, 99% of the time,
					// the duplicate filter above will catch them before needing
					// to do a heavy signature check.
					//
					// This is only really important until the post-conclusion acceptance threshold
					// is reached, and then no part of this loop will be hit.
					if let Err(()) = check_signature(
						&validator_public,
						set.candidate_hash,
						set.session,
						statement,
						signature,
					) {
						importer.undo(undo);
						filter.remove_index(i);
						continue
					}
				}
			}

			importer.finish()
		};

		// Reject disputes which don't have at least one vote on each side.
		if summary.state.validators_for.count_ones() == 0 ||
			summary.state.validators_against.count_ones() == 0
		{
			return StatementSetFilter::RemoveAll
		}

		// Apply spam slot changes. Bail early if too many occupied.
		let is_local = <Included<T>>::contains_key(&set.session, &set.candidate_hash);
		if !is_local {
			let mut spam_slots: Vec<u32> =
				SpamSlots::<T>::get(&set.session).unwrap_or_else(|| vec![0; n_validators]);

			for (validator_index, spam_slot_change) in summary.spam_slot_changes {
				let spam_slot = spam_slots
					.get_mut(validator_index.0 as usize)
					.expect("index is in-bounds, as checked above; qed");

				if let SpamSlotChange::Inc = spam_slot_change {
					if *spam_slot >= max_spam_slots {
						// Find the vote by this validator and filter it out.
						let first_index_in_set = set
							.statements
							.iter()
							.position(|(_, v_i, _)| &validator_index == v_i)
							.expect(
								"spam slots are only incremented when a new statement \
									from a validator is included; qed",
							);

						// Note that there may be many votes by the validator in the statement
						// set. There are not supposed to be, but the purpose of this function
						// is to filter out invalid submissions, after all.
						//
						// This is fine - we only need to handle the first one, because all
						// subsequent votes' indices have been added to the filter already
						// by the duplicate checks above. It's only the first one which
						// may not already have been filtered out.
						filter.remove_index(first_index_in_set);
					}

					// It's also worth noting that the `DisputeStateImporter`
					// which produces these spam slot updates only produces
					// one spam slot update per validator because it rejects
					// duplicate votes.
					//
					// So we don't need to worry about spam slots being
					// updated incorrectly after receiving duplicates.
					*spam_slot += 1;
				} else {
					*spam_slot = spam_slot.saturating_sub(1);
				}
			}

			// We write the spam slots here because sequential calls to
			// `filter_dispute_data` have a dependency on each other.
			//
			// For example, if a validator V occupies 1 spam slot and
			// max is 2, then 2 sequential calls incrementing spam slot
			// cannot be allowed.
			//
			// However, 3 sequential calls, where the first increments,
			// the second decrements, and the third increments would be allowed.
			SpamSlots::<T>::insert(&set.session, spam_slots);
		}

		filter
	}

	/// Handle a set of dispute statements corresponding to a single candidate.
	///
	/// Fails if the dispute data is invalid. Returns a boolean indicating whether the
	/// dispute is fresh.
	fn process_checked_dispute_data(
		set: CheckedDisputeStatementSet,
		dispute_post_conclusion_acceptance_period: T::BlockNumber,
	) -> Result<bool, DispatchError> {
		// Dispute statement sets on any dispute which concluded
		// before this point are to be rejected.
		let now = <frame_system::Pallet<T>>::block_number();
		let oldest_accepted = now.saturating_sub(dispute_post_conclusion_acceptance_period);

		let set = set.as_ref();

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
						validators_for: bitvec![u8, BitOrderLsb0; 0; n_validators],
						validators_against: bitvec![u8, BitOrderLsb0; 0; n_validators],
						start: now,
						concluded_at: None,
					},
				)
			}
		};

		// Import all votes. They were pre-checked.
		let summary = {
			let mut importer = DisputeStateImporter::new(dispute_state, now);
			for (statement, validator_index, _signature) in &set.statements {
				let valid = statement.indicates_validity();

				importer.import(*validator_index, valid).map_err(Error::<T>::from)?;
			}

			importer.finish()
		};

		// Reject disputes which don't have at least one vote on each side.
		ensure!(
			summary.state.validators_for.count_ones() > 0 &&
				summary.state.validators_against.count_ones() > 0,
			Error::<T>::SingleSidedDispute,
		);

		let DisputeStatementSet { session, candidate_hash, .. } = set.clone();

		// we can omit spam slot checks, `fn filter_disputes_data` is
		// always called before calling this `fn`.

		if fresh {
			let is_local = <Included<T>>::contains_key(&session, &candidate_hash);

			Self::deposit_event(Event::DisputeInitiated(
				candidate_hash,
				if is_local { DisputeLocation::Local } else { DisputeLocation::Remote },
			));
		}

		{
			if summary.new_flags.contains(DisputeStateFlags::FOR_SUPERMAJORITY) {
				Self::deposit_event(Event::DisputeConcluded(candidate_hash, DisputeResult::Valid));
			}

			// It is possible, although unexpected, for a dispute to conclude twice.
			// This would require f+1 validators to vote in both directions.
			// A dispute cannot conclude more than once in each direction.

			if summary.new_flags.contains(DisputeStateFlags::AGAINST_SUPERMAJORITY) {
				Self::deposit_event(Event::DisputeConcluded(
					candidate_hash,
					DisputeResult::Invalid,
				));
			}
		}

		// Reward statements.
		T::RewardValidators::reward_dispute_statement(
			session,
			summary.new_participants.iter_ones().map(|i| ValidatorIndex(i as _)),
		);

		// Slash participants on a losing side.
		{
			// a valid candidate, according to 2/3. Punish those on the 'against' side.
			T::PunishValidators::punish_against_valid(session, summary.slash_against);

			// an invalid candidate, according to 2/3. Punish those on the 'for' side.
			T::PunishValidators::punish_for_invalid(session, summary.slash_for);
		}

		<Disputes<T>>::insert(&session, &candidate_hash, &summary.state);

		// Freeze if just concluded against some local candidate
		if summary.new_flags.contains(DisputeStateFlags::AGAINST_SUPERMAJORITY) {
			if let Some(revert_to) = <Included<T>>::get(&session, &candidate_hash) {
				Self::revert_and_freeze(revert_to);
			}
		}

		Ok(fresh)
	}

	#[allow(unused)]
	pub(crate) fn disputes() -> Vec<(SessionIndex, CandidateHash, DisputeState<T::BlockNumber>)> {
		<Disputes<T>>::iter().collect()
	}

	pub(crate) fn note_included(
		session: SessionIndex,
		candidate_hash: CandidateHash,
		included_in: T::BlockNumber,
	) {
		if included_in.is_zero() {
			return
		}

		let revert_to = included_in - One::one();

		<Included<T>>::insert(&session, &candidate_hash, revert_to);

		// If we just included a block locally which has a live dispute, decrement spam slots
		// for any involved validators, if the dispute is not already confirmed by f + 1.
		if let Some(state) = <Disputes<T>>::get(&session, candidate_hash) {
			SpamSlots::<T>::mutate(&session, |spam_slots| {
				if let Some(ref mut spam_slots) = *spam_slots {
					decrement_spam(spam_slots, &state);
				}
			});

			if has_supermajority_against(&state) {
				Self::revert_and_freeze(revert_to);
			}
		}
	}

	pub(crate) fn included_state(
		session: SessionIndex,
		candidate_hash: CandidateHash,
	) -> Option<T::BlockNumber> {
		<Included<T>>::get(session, candidate_hash)
	}

	pub(crate) fn concluded_invalid(session: SessionIndex, candidate_hash: CandidateHash) -> bool {
		<Disputes<T>>::get(&session, &candidate_hash).map_or(false, |dispute| {
			// A dispute that has concluded with supermajority-against.
			has_supermajority_against(&dispute)
		})
	}

	pub(crate) fn is_frozen() -> bool {
		Self::last_valid_block().is_some()
	}

	pub(crate) fn revert_and_freeze(revert_to: T::BlockNumber) {
		if Self::last_valid_block().map_or(true, |last| last > revert_to) {
			Frozen::<T>::set(Some(revert_to));

			// The `Revert` log is about reverting a block, not reverting to a block.
			// If we want to revert to block X in the current chain, we need to revert
			// block X+1.
			let revert = revert_to + One::one();
			Self::deposit_event(Event::Revert(revert));
			frame_system::Pallet::<T>::deposit_log(
				ConsensusLog::Revert(revert.saturated_into()).into(),
			);
		}
	}
}

fn has_supermajority_against<BlockNumber>(dispute: &DisputeState<BlockNumber>) -> bool {
	let supermajority_threshold = supermajority_threshold(dispute.validators_against.len());
	dispute.validators_against.count_ones() >= supermajority_threshold
}

// If the dispute had not enough validators to confirm, decrement spam slots for all the participating
// validators.
//
// Returns the set of participating validators as a bitvec.
fn decrement_spam<BlockNumber>(
	spam_slots: &mut [u32],
	dispute: &DisputeState<BlockNumber>,
) -> bitvec::vec::BitVec<u8, BitOrderLsb0> {
	let byzantine_threshold = byzantine_threshold(spam_slots.len());

	let participating = dispute.validators_for.clone() | dispute.validators_against.clone();
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
		DisputeStatement::Valid(ValidDisputeStatementKind::Explicit) =>
			ExplicitDisputeStatement { valid: true, candidate_hash, session }.signing_payload(),
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(inclusion_parent)) =>
			CompactStatement::Seconded(candidate_hash).signing_payload(&SigningContext {
				session_index: session,
				parent_hash: inclusion_parent,
			}),
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(inclusion_parent)) =>
			CompactStatement::Valid(candidate_hash).signing_payload(&SigningContext {
				session_index: session,
				parent_hash: inclusion_parent,
			}),
		DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking) =>
			ApprovalVote(candidate_hash).signing_payload(session),
		DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit) =>
			ExplicitDisputeStatement { valid: false, candidate_hash, session }.signing_payload(),
	};

	if validator_signature.verify(&payload[..], &validator_public) {
		Ok(())
	} else {
		Err(())
	}
}
