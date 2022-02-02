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
use primitives::v1::{
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
	new_participants: bitvec::vec::BitVec<BitOrderLsb0, u8>,
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
	new_participants: bitvec::vec::BitVec<BitOrderLsb0, u8>,
	pre_flags: DisputeStateFlags,
}

impl<BlockNumber: Clone> DisputeStateImporter<BlockNumber> {
	fn new(state: DisputeState<BlockNumber>, now: BlockNumber) -> Self {
		let pre_flags = DisputeStateFlags::from_state(&state);
		let new_participants = bitvec::bitvec![BitOrderLsb0, u8; 0; state.validators_for.len()];

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
				let prev_participants = {
					// all participants
					let mut a = self.state.validators_for.clone();
					*a |= self.state.validators_against.iter().by_val();

					// which are not new participants
					*a &= self.new_participants.iter().by_val().map(|b| !b);

					a
				};

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
					validators_for: bitvec![BitOrderLsb0, u8; 0; n_validators],
					validators_against: bitvec![BitOrderLsb0, u8; 0; n_validators],
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
						validators_for: bitvec![BitOrderLsb0, u8; 0; n_validators],
						validators_against: bitvec![BitOrderLsb0, u8; 0; n_validators],
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

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use self::tests::run_to_block;

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		configuration::HostConfiguration,
		disputes::DisputesHandler,
		mock::{
			new_test_ext, AccountId, AllPalletsWithSystem, Initializer, MockGenesisConfig, System,
			Test, PUNISH_VALIDATORS_AGAINST, PUNISH_VALIDATORS_FOR, PUNISH_VALIDATORS_INCONCLUSIVE,
			REWARD_VALIDATORS,
		},
	};
	use frame_support::{
		assert_err, assert_noop, assert_ok,
		traits::{OnFinalize, OnInitialize},
	};
	use primitives::v1::BlockNumber;
	use sp_core::{crypto::CryptoType, Pair};

	/// Filtering updates the spam slots, as such update them.
	fn update_spam_slots(stmts: MultiDisputeStatementSet) -> CheckedMultiDisputeStatementSet {
		let config = <configuration::Pallet<Test>>::config();
		let max_spam_slots = config.dispute_max_spam_slots;
		let post_conclusion_acceptance_period = config.dispute_post_conclusion_acceptance_period;

		stmts
			.into_iter()
			.filter_map(|set| {
				// updates spam slots implicitly
				let filter = Pallet::<Test>::filter_dispute_data(
					&set,
					post_conclusion_acceptance_period,
					max_spam_slots,
					VerifyDisputeSignatures::Skip,
				);
				filter.filter_statement_set(set)
			})
			.collect::<Vec<_>>()
	}

	// All arguments for `initializer::on_new_session`
	type NewSession<'a> = (
		bool,
		SessionIndex,
		Vec<(&'a AccountId, ValidatorId)>,
		Option<Vec<(&'a AccountId, ValidatorId)>>,
	);

	// Run to specific block, while calling disputes pallet hooks manually, because disputes is not
	// integrated in initializer yet.
	pub(crate) fn run_to_block<'a>(
		to: BlockNumber,
		new_session: impl Fn(BlockNumber) -> Option<NewSession<'a>>,
	) {
		while System::block_number() < to {
			let b = System::block_number();
			if b != 0 {
				// circumvent requirement to have bitfields and headers in block for testing purposes
				crate::paras_inherent::Included::<Test>::set(Some(()));

				AllPalletsWithSystem::on_finalize(b);
				System::finalize();
			}

			System::reset_events();
			System::initialize(&(b + 1), &Default::default(), &Default::default());
			AllPalletsWithSystem::on_initialize(b + 1);

			if let Some(new_session) = new_session(b + 1) {
				Initializer::test_trigger_on_new_session(
					new_session.0,
					new_session.1,
					new_session.2.into_iter(),
					new_session.3.map(|q| q.into_iter()),
				);
			}
		}
	}

	#[test]
	fn test_contains_duplicates_in_sorted_iter() {
		// We here use the implicit ascending sorting and builtin equality of integers
		let v = vec![1, 2, 3, 5, 5, 8];
		assert_eq!(true, contains_duplicates_in_sorted_iter(&v, |a, b| a == b));

		let v = vec![1, 2, 3, 4];
		assert_eq!(false, contains_duplicates_in_sorted_iter(&v, |a, b| a == b));
	}

	#[test]
	fn test_dispute_state_flag_from_state() {
		assert_eq!(
			DisputeStateFlags::from_state(&DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 0, 0, 0, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 0, 0, 0, 0, 0, 0, 0],
				start: 0,
				concluded_at: None,
			}),
			DisputeStateFlags::default(),
		);

		assert_eq!(
			DisputeStateFlags::from_state(&DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 1, 1, 1, 1, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 0, 0, 0, 0, 0, 0],
				start: 0,
				concluded_at: None,
			}),
			DisputeStateFlags::FOR_SUPERMAJORITY | DisputeStateFlags::CONFIRMED,
		);

		assert_eq!(
			DisputeStateFlags::from_state(&DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 0, 0, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 1, 1, 1, 1, 1, 0, 0],
				start: 0,
				concluded_at: None,
			}),
			DisputeStateFlags::AGAINST_SUPERMAJORITY | DisputeStateFlags::CONFIRMED,
		);
	}

	#[test]
	fn test_import_new_participant_spam_inc() {
		let mut importer = DisputeStateImporter::new(
			DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 0, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 0, 0, 0, 0, 0, 0, 0],
				start: 0,
				concluded_at: None,
			},
			0,
		);

		assert_err!(
			importer.import(ValidatorIndex(9), true),
			VoteImportError::ValidatorIndexOutOfBounds,
		);

		assert_err!(importer.import(ValidatorIndex(0), true), VoteImportError::DuplicateStatement);
		assert_ok!(importer.import(ValidatorIndex(0), false));

		assert_ok!(importer.import(ValidatorIndex(2), true));
		assert_err!(importer.import(ValidatorIndex(2), true), VoteImportError::DuplicateStatement);

		assert_ok!(importer.import(ValidatorIndex(2), false));
		assert_err!(importer.import(ValidatorIndex(2), false), VoteImportError::DuplicateStatement);

		let summary = importer.finish();
		assert_eq!(summary.new_flags, DisputeStateFlags::default());
		assert_eq!(
			summary.state,
			DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 1, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 1, 0, 1, 0, 0, 0, 0, 0],
				start: 0,
				concluded_at: None,
			},
		);
		assert_eq!(summary.spam_slot_changes, vec![(ValidatorIndex(2), SpamSlotChange::Inc)]);
		assert!(summary.slash_for.is_empty());
		assert!(summary.slash_against.is_empty());
		assert_eq!(summary.new_participants, bitvec![BitOrderLsb0, u8; 0, 0, 1, 0, 0, 0, 0, 0]);
	}

	#[test]
	fn test_import_prev_participant_spam_dec_confirmed() {
		let mut importer = DisputeStateImporter::new(
			DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 0, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 1, 0, 0, 0, 0, 0, 0],
				start: 0,
				concluded_at: None,
			},
			0,
		);

		assert_ok!(importer.import(ValidatorIndex(2), true));

		let summary = importer.finish();
		assert_eq!(
			summary.state,
			DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 1, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 1, 0, 0, 0, 0, 0, 0],
				start: 0,
				concluded_at: None,
			},
		);
		assert_eq!(
			summary.spam_slot_changes,
			vec![
				(ValidatorIndex(0), SpamSlotChange::Dec),
				(ValidatorIndex(1), SpamSlotChange::Dec),
			],
		);
		assert!(summary.slash_for.is_empty());
		assert!(summary.slash_against.is_empty());
		assert_eq!(summary.new_participants, bitvec![BitOrderLsb0, u8; 0, 0, 1, 0, 0, 0, 0, 0]);
		assert_eq!(summary.new_flags, DisputeStateFlags::CONFIRMED);
	}

	#[test]
	fn test_import_prev_participant_spam_dec_confirmed_slash_for() {
		let mut importer = DisputeStateImporter::new(
			DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 0, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 1, 0, 0, 0, 0, 0, 0],
				start: 0,
				concluded_at: None,
			},
			0,
		);

		assert_ok!(importer.import(ValidatorIndex(2), true));
		assert_ok!(importer.import(ValidatorIndex(2), false));
		assert_ok!(importer.import(ValidatorIndex(3), false));
		assert_ok!(importer.import(ValidatorIndex(4), false));
		assert_ok!(importer.import(ValidatorIndex(5), false));
		assert_ok!(importer.import(ValidatorIndex(6), false));

		let summary = importer.finish();
		assert_eq!(
			summary.state,
			DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 1, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 1, 1, 1, 1, 1, 1, 0],
				start: 0,
				concluded_at: Some(0),
			},
		);
		assert_eq!(
			summary.spam_slot_changes,
			vec![
				(ValidatorIndex(0), SpamSlotChange::Dec),
				(ValidatorIndex(1), SpamSlotChange::Dec),
			],
		);
		assert_eq!(summary.slash_for, vec![ValidatorIndex(0), ValidatorIndex(2)]);
		assert!(summary.slash_against.is_empty());
		assert_eq!(summary.new_participants, bitvec![BitOrderLsb0, u8; 0, 0, 1, 1, 1, 1, 1, 0]);
		assert_eq!(
			summary.new_flags,
			DisputeStateFlags::CONFIRMED | DisputeStateFlags::AGAINST_SUPERMAJORITY,
		);
	}

	#[test]
	fn test_import_slash_against() {
		let mut importer = DisputeStateImporter::new(
			DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 1, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 1, 0, 0, 0, 0, 0, 0],
				start: 0,
				concluded_at: None,
			},
			0,
		);

		assert_ok!(importer.import(ValidatorIndex(3), true));
		assert_ok!(importer.import(ValidatorIndex(4), true));
		assert_ok!(importer.import(ValidatorIndex(5), false));
		assert_ok!(importer.import(ValidatorIndex(6), true));
		assert_ok!(importer.import(ValidatorIndex(7), true));

		let summary = importer.finish();
		assert_eq!(
			summary.state,
			DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 1, 1, 1, 0, 1, 1],
				validators_against: bitvec![BitOrderLsb0, u8; 0, 1, 0, 0, 0, 1, 0, 0],
				start: 0,
				concluded_at: Some(0),
			},
		);
		assert!(summary.spam_slot_changes.is_empty());
		assert!(summary.slash_for.is_empty());
		assert_eq!(summary.slash_against, vec![ValidatorIndex(1), ValidatorIndex(5)]);
		assert_eq!(summary.new_participants, bitvec![BitOrderLsb0, u8; 0, 0, 0, 1, 1, 1, 1, 1]);
		assert_eq!(summary.new_flags, DisputeStateFlags::FOR_SUPERMAJORITY);
	}

	// Test that punish_inconclusive is correctly called.
	#[test]
	fn test_initializer_initialize() {
		let dispute_conclusion_by_time_out_period = 3;
		let start = 10;

		let mock_genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					dispute_conclusion_by_time_out_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(mock_genesis_config).execute_with(|| {
			// We need 6 validators for the byzantine threshold to be 2
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v6 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(start, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
						(&4, v4.public()),
						(&5, v5.public()),
						(&6, v6.public()),
					],
					Some(vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
						(&4, v4.public()),
						(&5, v5.public()),
						(&6, v6.public()),
					]),
				))
			});

			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

			// v0 votes for 3, v6 against.
			let stmts = vec![DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: start - 1,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						v0.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: start - 1,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(6),
						v2.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: start - 1,
							}
							.signing_payload(),
						),
					),
				],
			}];

			let stmts = update_spam_slots(stmts);
			assert_eq!(SpamSlots::<Test>::get(start - 1), Some(vec![1, 0, 0, 0, 0, 0, 1]));

			assert_ok!(
				Pallet::<Test>::process_checked_multi_dispute_data(stmts),
				vec![(9, candidate_hash.clone())],
			);

			// Run to timeout period
			run_to_block(start + dispute_conclusion_by_time_out_period, |_| None);
			assert_eq!(SpamSlots::<Test>::get(start - 1), Some(vec![1, 0, 0, 0, 0, 0, 1]));

			// Run to timeout + 1 in order to executive on_finalize(timeout)
			run_to_block(start + dispute_conclusion_by_time_out_period + 1, |_| None);
			assert_eq!(SpamSlots::<Test>::get(start - 1), Some(vec![0, 0, 0, 0, 0, 0, 0]));
			assert_eq!(
				PUNISH_VALIDATORS_INCONCLUSIVE.with(|r| r.borrow()[0].clone()),
				(9, vec![ValidatorIndex(0), ValidatorIndex(6)]),
			);
		});
	}

	// Test pruning works
	#[test]
	fn test_initializer_on_new_session() {
		let dispute_period = 3;

		let mock_genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration { dispute_period, ..Default::default() },
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(mock_genesis_config).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
			Pallet::<Test>::note_included(0, candidate_hash.clone(), 0);
			Pallet::<Test>::note_included(1, candidate_hash.clone(), 1);
			Pallet::<Test>::note_included(2, candidate_hash.clone(), 2);
			Pallet::<Test>::note_included(3, candidate_hash.clone(), 3);
			Pallet::<Test>::note_included(4, candidate_hash.clone(), 4);
			Pallet::<Test>::note_included(5, candidate_hash.clone(), 5);
			Pallet::<Test>::note_included(6, candidate_hash.clone(), 5);

			run_to_block(7, |b| {
				// a new session at each block
				Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
			});

			// current session is 7,
			// we keep for dispute_period + 1 session and we remove in on_finalize
			// thus we keep info for session 3, 4, 5, 6, 7.
			assert_eq!(Included::<Test>::iter_prefix(0).count(), 0);
			assert_eq!(Included::<Test>::iter_prefix(1).count(), 0);
			assert_eq!(Included::<Test>::iter_prefix(2).count(), 0);
			assert_eq!(Included::<Test>::iter_prefix(3).count(), 1);
			assert_eq!(Included::<Test>::iter_prefix(4).count(), 1);
			assert_eq!(Included::<Test>::iter_prefix(5).count(), 1);
			assert_eq!(Included::<Test>::iter_prefix(6).count(), 1);
		});
	}

	#[test]
	fn test_provide_data_duplicate_error() {
		new_test_ext(Default::default()).execute_with(|| {
			let candidate_hash_1 = CandidateHash(sp_core::H256::repeat_byte(1));
			let candidate_hash_2 = CandidateHash(sp_core::H256::repeat_byte(2));

			let mut stmts = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash_2,
					session: 2,
					statements: vec![],
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash_1,
					session: 1,
					statements: vec![],
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash_2,
					session: 2,
					statements: vec![],
				},
			];

			assert!(Pallet::<Test>::deduplicate_and_sort_dispute_data(&mut stmts).is_err());
			assert_eq!(stmts.len(), 2);
		})
	}

	#[test]
	fn test_provide_multi_dispute_is_providing() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				if b == 1 {
					Some((
						true,
						b,
						vec![(&0, v0.public()), (&1, v1.public())],
						Some(vec![(&0, v0.public()), (&1, v1.public())]),
					))
				} else {
					Some((true, b, vec![(&1, v1.public())], Some(vec![(&1, v1.public())])))
				}
			});

			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
			let stmts = vec![DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 1,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						v0.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 1,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v1.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 1,
							}
							.signing_payload(),
						),
					),
				],
			}];

			assert_ok!(
				Pallet::<Test>::process_checked_multi_dispute_data(
					stmts
						.into_iter()
						.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
						.collect()
				),
				vec![(1, candidate_hash.clone())],
			);
		})
	}

	#[test]
	fn test_freeze_on_note_included() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(6, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![(&0, v0.public()), (&1, v1.public())],
					Some(vec![(&0, v0.public()), (&1, v1.public())]),
				))
			});

			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

			// v0 votes for 3
			let stmts = vec![DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 3,
				statements: vec![
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						v0.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v1.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v1.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
				],
			}];
			assert!(Pallet::<Test>::process_checked_multi_dispute_data(
				stmts
					.into_iter()
					.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
					.collect()
			)
			.is_ok());

			Pallet::<Test>::note_included(3, candidate_hash.clone(), 3);
			assert_eq!(Frozen::<Test>::get(), Some(2));
		});
	}

	#[test]
	fn test_freeze_provided_against_supermajority_for_included() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(6, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![(&0, v0.public()), (&1, v1.public())],
					Some(vec![(&0, v0.public()), (&1, v1.public())]),
				))
			});

			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

			// v0 votes for 3
			let stmts = vec![DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 3,
				statements: vec![
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						v0.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v1.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v1.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
				],
			}];

			Pallet::<Test>::note_included(3, candidate_hash.clone(), 3);
			assert!(Pallet::<Test>::process_checked_multi_dispute_data(
				stmts
					.into_iter()
					.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
					.collect()
			)
			.is_ok());
			assert_eq!(Frozen::<Test>::get(), Some(2));
		});
	}

	// tests for:
	// * provide_multi_dispute: with success scenario
	// * disputes: correctness of datas
	// * could_be_invalid: correctness of datas
	// * note_included: decrement spam correctly
	// * spam slots: correctly incremented and decremented
	// * ensure rewards and punishment are correctly called.
	#[test]
	fn test_provide_multi_dispute_success_and_other() {
		new_test_ext(Default::default()).execute_with(|| {
			// 7 validators needed for byzantine threshold of 2.
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v6 = <ValidatorId as CryptoType>::Pair::generate().0;

			// v0 -> 0
			// v1 -> 3
			// v2 -> 6
			// v3 -> 5
			// v4 -> 1
			// v5 -> 4
			// v6 -> 2

			run_to_block(6, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
						(&4, v4.public()),
						(&5, v5.public()),
						(&6, v6.public()),
					],
					Some(vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
						(&4, v4.public()),
						(&5, v5.public()),
						(&6, v6.public()),
					]),
				))
			});

			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

			// v0 votes for 3, v6 votes against
			let stmts = vec![DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 3,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						v0.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(2),
						v6.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
				],
			}];

			let stmts = update_spam_slots(stmts);
			assert_eq!(SpamSlots::<Test>::get(3), Some(vec![1, 0, 1, 0, 0, 0, 0]));

			assert_ok!(
				Pallet::<Test>::process_checked_multi_dispute_data(stmts),
				vec![(3, candidate_hash.clone())],
			);

			// v1 votes for 4 and for 3, v6 votes against 4.
			let stmts = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 4,
					statements: vec![
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(3),
							v1.sign(
								&ExplicitDisputeStatement {
									valid: true,
									candidate_hash: candidate_hash.clone(),
									session: 4,
								}
								.signing_payload(),
							),
						),
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(2),
							v6.sign(
								&ExplicitDisputeStatement {
									valid: false,
									candidate_hash: candidate_hash.clone(),
									session: 4,
								}
								.signing_payload(),
							),
						),
					],
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 3,
					statements: vec![(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(3),
						v1.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					)],
				},
			];

			let stmts = update_spam_slots(stmts);

			assert_ok!(
				Pallet::<Test>::process_checked_multi_dispute_data(stmts),
				vec![(4, candidate_hash.clone())],
			);
			assert_eq!(SpamSlots::<Test>::get(3), Some(vec![0, 0, 0, 0, 0, 0, 0])); // Confirmed as no longer spam
			assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 1, 1, 0, 0, 0]));

			// v3 votes against 3 and for 5, v6 votes against 5.
			let stmts = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 3,
					statements: vec![(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(5),
						v3.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					)],
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 5,
					statements: vec![
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(5),
							v3.sign(
								&ExplicitDisputeStatement {
									valid: true,
									candidate_hash: candidate_hash.clone(),
									session: 5,
								}
								.signing_payload(),
							),
						),
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(2),
							v6.sign(
								&ExplicitDisputeStatement {
									valid: false,
									candidate_hash: candidate_hash.clone(),
									session: 5,
								}
								.signing_payload(),
							),
						),
					],
				},
			];

			let stmts = update_spam_slots(stmts);
			assert_ok!(
				Pallet::<Test>::process_checked_multi_dispute_data(stmts),
				vec![(5, candidate_hash.clone())],
			);
			assert_eq!(SpamSlots::<Test>::get(3), Some(vec![0, 0, 0, 0, 0, 0, 0]));
			assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 1, 1, 0, 0, 0]));
			assert_eq!(SpamSlots::<Test>::get(5), Some(vec![0, 0, 1, 0, 0, 1, 0]));

			// v2 votes for 3 and against 5
			let stmts = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 3,
					statements: vec![(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(6),
						v2.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					)],
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 5,
					statements: vec![(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(6),
						v2.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 5,
							}
							.signing_payload(),
						),
					)],
				},
			];
			let stmts = update_spam_slots(stmts);
			assert_ok!(Pallet::<Test>::process_checked_multi_dispute_data(stmts), vec![]);
			assert_eq!(SpamSlots::<Test>::get(3), Some(vec![0, 0, 0, 0, 0, 0, 0]));
			assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 1, 1, 0, 0, 0]));
			assert_eq!(SpamSlots::<Test>::get(5), Some(vec![0, 0, 0, 0, 0, 0, 0]));

			let stmts = vec![
				// 0, 4, and 5 vote against 5
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 5,
					statements: vec![
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(0),
							v0.sign(
								&ExplicitDisputeStatement {
									valid: false,
									candidate_hash: candidate_hash.clone(),
									session: 5,
								}
								.signing_payload(),
							),
						),
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(1),
							v4.sign(
								&ExplicitDisputeStatement {
									valid: false,
									candidate_hash: candidate_hash.clone(),
									session: 5,
								}
								.signing_payload(),
							),
						),
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(4),
							v5.sign(
								&ExplicitDisputeStatement {
									valid: false,
									candidate_hash: candidate_hash.clone(),
									session: 5,
								}
								.signing_payload(),
							),
						),
					],
				},
				// 4 and 5 vote for 3
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 3,
					statements: vec![
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(1),
							v4.sign(
								&ExplicitDisputeStatement {
									valid: true,
									candidate_hash: candidate_hash.clone(),
									session: 3,
								}
								.signing_payload(),
							),
						),
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(4),
							v5.sign(
								&ExplicitDisputeStatement {
									valid: true,
									candidate_hash: candidate_hash.clone(),
									session: 3,
								}
								.signing_payload(),
							),
						),
					],
				},
			];
			let stmts = update_spam_slots(stmts);
			assert_ok!(Pallet::<Test>::process_checked_multi_dispute_data(stmts), vec![]);

			assert_eq!(
				Pallet::<Test>::disputes(),
				vec![
					(
						5,
						candidate_hash.clone(),
						DisputeState {
							validators_for: bitvec![BitOrderLsb0, u8; 0, 0, 0, 0, 0, 1, 0],
							validators_against: bitvec![BitOrderLsb0, u8; 1, 1, 1, 0, 1, 0, 1],
							start: 6,
							concluded_at: Some(6), // 5 vote against
						}
					),
					(
						3,
						candidate_hash.clone(),
						DisputeState {
							validators_for: bitvec![BitOrderLsb0, u8; 1, 1, 0, 1, 1, 0, 1],
							validators_against: bitvec![BitOrderLsb0, u8; 0, 0, 1, 0, 0, 1, 0],
							start: 6,
							concluded_at: Some(6), // 5 vote for
						}
					),
					(
						4,
						candidate_hash.clone(),
						DisputeState {
							validators_for: bitvec![BitOrderLsb0, u8; 0, 0, 0, 1, 0, 0, 0],
							validators_against: bitvec![BitOrderLsb0, u8; 0, 0, 1, 0, 0, 0, 0],
							start: 6,
							concluded_at: None,
						}
					),
				]
			);

			assert!(!Pallet::<Test>::concluded_invalid(3, candidate_hash.clone()));
			assert!(!Pallet::<Test>::concluded_invalid(4, candidate_hash.clone()));
			assert!(Pallet::<Test>::concluded_invalid(5, candidate_hash.clone()));

			// Ensure inclusion removes spam slots
			assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 1, 1, 0, 0, 0]));
			Pallet::<Test>::note_included(4, candidate_hash.clone(), 4);
			assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 0, 0, 0, 0, 0]));

			// Ensure the `reward_validator` function was correctly called
			assert_eq!(
				REWARD_VALIDATORS.with(|r| r.borrow().clone()),
				vec![
					(3, vec![ValidatorIndex(0), ValidatorIndex(2)]),
					(4, vec![ValidatorIndex(2), ValidatorIndex(3)]),
					(3, vec![ValidatorIndex(3)]),
					(3, vec![ValidatorIndex(5)]),
					(5, vec![ValidatorIndex(2), ValidatorIndex(5)]),
					(3, vec![ValidatorIndex(6)]),
					(5, vec![ValidatorIndex(6)]),
					(5, vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(4)]),
					(3, vec![ValidatorIndex(1), ValidatorIndex(4)]),
				],
			);

			// Ensure punishment against is called
			assert_eq!(
				PUNISH_VALIDATORS_AGAINST.with(|r| r.borrow().clone()),
				vec![
					(3, vec![]),
					(4, vec![]),
					(3, vec![]),
					(3, vec![]),
					(5, vec![]),
					(3, vec![]),
					(5, vec![]),
					(5, vec![]),
					(3, vec![ValidatorIndex(2), ValidatorIndex(5)]),
				],
			);

			// Ensure punishment for is called
			assert_eq!(
				PUNISH_VALIDATORS_FOR.with(|r| r.borrow().clone()),
				vec![
					(3, vec![]),
					(4, vec![]),
					(3, vec![]),
					(3, vec![]),
					(5, vec![]),
					(3, vec![]),
					(5, vec![]),
					(5, vec![ValidatorIndex(5)]),
					(3, vec![]),
				],
			);
		})
	}

	#[test]
	fn test_revert_and_freeze() {
		new_test_ext(Default::default()).execute_with(|| {
			// events are ignored for genesis block
			System::set_block_number(1);

			Frozen::<Test>::put(Some(0));
			assert_noop!(
				{
					Pallet::<Test>::revert_and_freeze(0);
					Result::<(), ()>::Err(()) // Just a small trick in order to use `assert_noop`.
				},
				(),
			);

			Frozen::<Test>::kill();
			Pallet::<Test>::revert_and_freeze(0);

			assert_eq!(Frozen::<Test>::get(), Some(0));
			assert_eq!(System::digest().logs[0], ConsensusLog::Revert(1).into());
			System::assert_has_event(Event::Revert(1).into());
		})
	}

	#[test]
	fn test_revert_and_freeze_merges() {
		new_test_ext(Default::default()).execute_with(|| {
			Frozen::<Test>::put(Some(10));
			assert_noop!(
				{
					Pallet::<Test>::revert_and_freeze(10);
					Result::<(), ()>::Err(()) // Just a small trick in order to use `assert_noop`.
				},
				(),
			);

			Pallet::<Test>::revert_and_freeze(8);
			assert_eq!(Frozen::<Test>::get(), Some(8));
		})
	}

	#[test]
	fn test_has_supermajority_against() {
		assert_eq!(
			has_supermajority_against(&DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 1, 0, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 1, 1, 1, 1, 1, 0, 0, 0],
				start: 0,
				concluded_at: None,
			}),
			false,
		);

		assert_eq!(
			has_supermajority_against(&DisputeState {
				validators_for: bitvec![BitOrderLsb0, u8; 1, 1, 0, 0, 0, 0, 0, 0],
				validators_against: bitvec![BitOrderLsb0, u8; 1, 1, 1, 1, 1, 1, 0, 0],
				start: 0,
				concluded_at: None,
			}),
			true,
		);
	}

	#[test]
	fn test_decrement_spam() {
		let original_spam_slots = vec![0, 1, 2, 3, 4, 5, 6, 7];

		// Test confirm is no-op
		let mut spam_slots = original_spam_slots.clone();
		let dispute_state_confirm = DisputeState {
			validators_for: bitvec![BitOrderLsb0, u8; 1, 1, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![BitOrderLsb0, u8; 1, 0, 1, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		};
		assert_eq!(
			DisputeStateFlags::from_state(&dispute_state_confirm),
			DisputeStateFlags::CONFIRMED
		);
		assert_eq!(
			decrement_spam(spam_slots.as_mut(), &dispute_state_confirm),
			bitvec![BitOrderLsb0, u8; 1, 1, 1, 0, 0, 0, 0, 0],
		);
		assert_eq!(spam_slots, original_spam_slots);

		// Test not confirm is decreasing spam
		let mut spam_slots = original_spam_slots.clone();
		let dispute_state_no_confirm = DisputeState {
			validators_for: bitvec![BitOrderLsb0, u8; 1, 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![BitOrderLsb0, u8; 1, 0, 1, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		};
		assert_eq!(
			DisputeStateFlags::from_state(&dispute_state_no_confirm),
			DisputeStateFlags::default()
		);
		assert_eq!(
			decrement_spam(spam_slots.as_mut(), &dispute_state_no_confirm),
			bitvec![BitOrderLsb0, u8; 1, 0, 1, 0, 0, 0, 0, 0],
		);
		assert_eq!(spam_slots, vec![0, 1, 1, 3, 4, 5, 6, 7]);
	}

	#[test]
	fn test_check_signature() {
		let validator_id = <ValidatorId as CryptoType>::Pair::generate().0;
		let wrong_validator_id = <ValidatorId as CryptoType>::Pair::generate().0;

		let session = 0;
		let wrong_session = 1;
		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
		let wrong_candidate_hash = CandidateHash(sp_core::H256::repeat_byte(2));
		let inclusion_parent = sp_core::H256::repeat_byte(3);
		let wrong_inclusion_parent = sp_core::H256::repeat_byte(4);

		let statement_1 = DisputeStatement::Valid(ValidDisputeStatementKind::Explicit);
		let statement_2 = DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(
			inclusion_parent.clone(),
		));
		let wrong_statement_2 = DisputeStatement::Valid(
			ValidDisputeStatementKind::BackingSeconded(wrong_inclusion_parent.clone()),
		);
		let statement_3 = DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
			inclusion_parent.clone(),
		));
		let wrong_statement_3 = DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
			wrong_inclusion_parent.clone(),
		));
		let statement_4 = DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking);
		let statement_5 = DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit);

		let signed_1 = validator_id.sign(
			&ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash.clone(),
				session,
			}
			.signing_payload(),
		);
		let signed_2 =
			validator_id.sign(&CompactStatement::Seconded(candidate_hash.clone()).signing_payload(
				&SigningContext { session_index: session, parent_hash: inclusion_parent.clone() },
			));
		let signed_3 =
			validator_id.sign(&CompactStatement::Valid(candidate_hash.clone()).signing_payload(
				&SigningContext { session_index: session, parent_hash: inclusion_parent.clone() },
			));
		let signed_4 =
			validator_id.sign(&ApprovalVote(candidate_hash.clone()).signing_payload(session));
		let signed_5 = validator_id.sign(
			&ExplicitDisputeStatement {
				valid: false,
				candidate_hash: candidate_hash.clone(),
				session,
			}
			.signing_payload(),
		);

		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_1,
			&signed_1
		)
		.is_ok());
		assert!(check_signature(
			&wrong_validator_id.public(),
			candidate_hash,
			session,
			&statement_1,
			&signed_1
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			wrong_candidate_hash,
			session,
			&statement_1,
			&signed_1
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			wrong_session,
			&statement_1,
			&signed_1
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_2,
			&signed_1
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_3,
			&signed_1
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_4,
			&signed_1
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_5,
			&signed_1
		)
		.is_err());

		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_2,
			&signed_2
		)
		.is_ok());
		assert!(check_signature(
			&wrong_validator_id.public(),
			candidate_hash,
			session,
			&statement_2,
			&signed_2
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			wrong_candidate_hash,
			session,
			&statement_2,
			&signed_2
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			wrong_session,
			&statement_2,
			&signed_2
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&wrong_statement_2,
			&signed_2
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_1,
			&signed_2
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_3,
			&signed_2
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_4,
			&signed_2
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_5,
			&signed_2
		)
		.is_err());

		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_3,
			&signed_3
		)
		.is_ok());
		assert!(check_signature(
			&wrong_validator_id.public(),
			candidate_hash,
			session,
			&statement_3,
			&signed_3
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			wrong_candidate_hash,
			session,
			&statement_3,
			&signed_3
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			wrong_session,
			&statement_3,
			&signed_3
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&wrong_statement_3,
			&signed_3
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_1,
			&signed_3
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_2,
			&signed_3
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_4,
			&signed_3
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_5,
			&signed_3
		)
		.is_err());

		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_4,
			&signed_4
		)
		.is_ok());
		assert!(check_signature(
			&wrong_validator_id.public(),
			candidate_hash,
			session,
			&statement_4,
			&signed_4
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			wrong_candidate_hash,
			session,
			&statement_4,
			&signed_4
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			wrong_session,
			&statement_4,
			&signed_4
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_1,
			&signed_4
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_2,
			&signed_4
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_3,
			&signed_4
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_5,
			&signed_4
		)
		.is_err());

		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_5,
			&signed_5
		)
		.is_ok());
		assert!(check_signature(
			&wrong_validator_id.public(),
			candidate_hash,
			session,
			&statement_5,
			&signed_5
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			wrong_candidate_hash,
			session,
			&statement_5,
			&signed_5
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			wrong_session,
			&statement_5,
			&signed_5
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_1,
			&signed_5
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_2,
			&signed_5
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_3,
			&signed_5
		)
		.is_err());
		assert!(check_signature(
			&validator_id.public(),
			candidate_hash,
			session,
			&statement_4,
			&signed_5
		)
		.is_err());
	}

	#[test]
	fn deduplication_and_sorting_works() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v3 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
					],
					Some(vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
					]),
				))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));
			let candidate_hash_b = CandidateHash(sp_core::H256::repeat_byte(2));
			let candidate_hash_c = CandidateHash(sp_core::H256::repeat_byte(3));

			let create_explicit_statement = |vidx: ValidatorIndex,
			                                 validator: &<ValidatorId as CryptoType>::Pair,
			                                 c_hash: &CandidateHash,
			                                 valid,
			                                 session| {
				let payload =
					ExplicitDisputeStatement { valid, candidate_hash: c_hash.clone(), session }
						.signing_payload();
				let sig = validator.sign(&payload);
				(DisputeStatement::Valid(ValidDisputeStatementKind::Explicit), vidx, sig.clone())
			};

			let explicit_triple_a =
				create_explicit_statement(ValidatorIndex(0), &v0, &candidate_hash_a, true, 1);
			let explicit_triple_a_bad =
				create_explicit_statement(ValidatorIndex(1), &v1, &candidate_hash_a, false, 1);

			let explicit_triple_b =
				create_explicit_statement(ValidatorIndex(0), &v0, &candidate_hash_b, true, 2);
			let explicit_triple_b_bad =
				create_explicit_statement(ValidatorIndex(1), &v1, &candidate_hash_b, false, 2);

			let explicit_triple_c =
				create_explicit_statement(ValidatorIndex(0), &v0, &candidate_hash_c, true, 2);
			let explicit_triple_c_bad =
				create_explicit_statement(ValidatorIndex(1), &v1, &candidate_hash_c, false, 2);

			let mut disputes = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash_b.clone(),
					session: 2,
					statements: vec![explicit_triple_b.clone(), explicit_triple_b_bad.clone()],
				},
				// same session as above
				DisputeStatementSet {
					candidate_hash: candidate_hash_c.clone(),
					session: 2,
					statements: vec![explicit_triple_c, explicit_triple_c_bad],
				},
				// the duplicate set
				DisputeStatementSet {
					candidate_hash: candidate_hash_b.clone(),
					session: 2,
					statements: vec![explicit_triple_b.clone(), explicit_triple_b_bad.clone()],
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements: vec![explicit_triple_a, explicit_triple_a_bad],
				},
			];

			let disputes_orig = disputes.clone();

			<Pallet<Test> as DisputesHandler<
					<Test as frame_system::Config>::BlockNumber,
				>>::deduplicate_and_sort_dispute_data(&mut disputes).unwrap_err();

			// assert ordering of local only disputes, and at the same time, and being free of duplicates
			assert_eq!(disputes_orig.len(), disputes.len() + 1);

			let are_these_equal = |a: &DisputeStatementSet, b: &DisputeStatementSet| {
				use core::cmp::Ordering;
				// we only have local disputes here, so sorting of those adheres to the
				// simplified sorting logic
				let cmp =
					a.session.cmp(&b.session).then_with(|| a.candidate_hash.cmp(&b.candidate_hash));
				assert_ne!(cmp, Ordering::Greater);
				cmp == Ordering::Equal
			};

			assert_eq!(false, contains_duplicates_in_sorted_iter(&disputes, are_these_equal));
		})
	}

	fn apply_filter_all<T: Config, I: IntoIterator<Item = DisputeStatementSet>>(
		sets: I,
	) -> Vec<CheckedDisputeStatementSet> {
		let config = <configuration::Pallet<T>>::config();
		let max_spam_slots = config.dispute_max_spam_slots;
		let post_conclusion_acceptance_period = config.dispute_post_conclusion_acceptance_period;

		let mut acc = Vec::<CheckedDisputeStatementSet>::new();
		for dispute_statement in sets {
			if let Some(checked) =
				<Pallet<T> as DisputesHandler<<T>::BlockNumber>>::filter_dispute_data(
					dispute_statement,
					max_spam_slots,
					post_conclusion_acceptance_period,
					VerifyDisputeSignatures::Yes,
				) {
				acc.push(checked);
			}
		}
		acc
	}

	#[test]
	fn filter_removes_duplicates_within_set() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![(&0, v0.public()), (&1, v1.public())],
					Some(vec![(&0, v0.public()), (&1, v1.public())]),
				))
			});

			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

			let payload = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash.clone(),
				session: 1,
			}
			.signing_payload();

			let payload_against = ExplicitDisputeStatement {
				valid: false,
				candidate_hash: candidate_hash.clone(),
				session: 1,
			}
			.signing_payload();

			let sig_a = v0.sign(&payload);
			let sig_b = v0.sign(&payload);
			let sig_c = v0.sign(&payload);
			let sig_d = v1.sign(&payload_against);

			let statements = DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 1,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_a.clone(),
					),
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_b,
					),
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_c,
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						sig_d.clone(),
					),
				],
			};

			let max_spam_slots = 10;
			let post_conclusion_acceptance_period = 10;
			let statements = <Pallet<Test> as DisputesHandler<
				<Test as frame_system::Config>::BlockNumber,
			>>::filter_dispute_data(
				statements,
				max_spam_slots,
				post_conclusion_acceptance_period,
				VerifyDisputeSignatures::Yes,
			);

			assert_eq!(
				statements,
				Some(CheckedDisputeStatementSet::unchecked_from_unchecked(DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 1,
					statements: vec![
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(0),
							sig_a,
						),
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(1),
							sig_d,
						),
					]
				}))
			);
		})
	}

	#[test]
	fn filter_bad_signatures_correctly_detects_single_sided() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v3 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
					],
					Some(vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
					]),
				))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

			let payload = |c_hash: &CandidateHash, valid| {
				ExplicitDisputeStatement { valid, candidate_hash: c_hash.clone(), session: 1 }
					.signing_payload()
			};

			let payload_a = payload(&candidate_hash_a, true);
			let payload_a_bad = payload(&candidate_hash_a, false);

			let sig_0 = v0.sign(&payload_a);
			let sig_1 = v1.sign(&payload_a_bad);

			let statements = vec![DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_0.clone(),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(2),
						sig_1.clone(),
					),
				],
			}];

			let statements = apply_filter_all::<Test, _>(statements);

			assert!(statements.is_empty());
		})
	}

	#[test]
	fn filter_correctly_accounts_spam_slots() {
		let dispute_max_spam_slots = 2;

		let mock_genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration { dispute_max_spam_slots, ..Default::default() },
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(mock_genesis_config).execute_with(|| {
			// We need 7 validators for the byzantine threshold to be 2
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v6 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
						(&4, v4.public()),
						(&5, v5.public()),
						(&6, v6.public()),
					],
					Some(vec![
						(&0, v0.public()),
						(&1, v1.public()),
						(&2, v2.public()),
						(&3, v3.public()),
						(&4, v4.public()),
						(&5, v5.public()),
						(&6, v6.public()),
					]),
				))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));
			let candidate_hash_b = CandidateHash(sp_core::H256::repeat_byte(2));
			let candidate_hash_c = CandidateHash(sp_core::H256::repeat_byte(3));

			let payload = |c_hash: &CandidateHash, valid| {
				ExplicitDisputeStatement { valid, candidate_hash: c_hash.clone(), session: 1 }
					.signing_payload()
			};

			let payload_a = payload(&candidate_hash_a, true);
			let payload_b = payload(&candidate_hash_b, true);
			let payload_c = payload(&candidate_hash_c, true);

			let payload_a_bad = payload(&candidate_hash_a, false);
			let payload_b_bad = payload(&candidate_hash_b, false);
			let payload_c_bad = payload(&candidate_hash_c, false);

			let sig_0a = v0.sign(&payload_a);
			let sig_0b = v0.sign(&payload_b);
			let sig_0c = v0.sign(&payload_c);

			let sig_1b = v1.sign(&payload_b);

			let sig_2a = v2.sign(&payload_a_bad);
			let sig_2b = v2.sign(&payload_b_bad);
			let sig_2c = v2.sign(&payload_c_bad);

			let statements = vec![
				// validators 0 and 2 get 1 spam slot from this.
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements: vec![
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(0),
							sig_0a.clone(),
						),
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(6),
							sig_2a.clone(),
						),
					],
				},
				// Validators 0, 2, and 3 get no spam slots for this
				DisputeStatementSet {
					candidate_hash: candidate_hash_b.clone(),
					session: 1,
					statements: vec![
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(0),
							sig_0b.clone(),
						),
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(3),
							sig_1b.clone(),
						),
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(6),
							sig_2b.clone(),
						),
					],
				},
				// Validators 0 and 2 get an extra spam slot for this.
				DisputeStatementSet {
					candidate_hash: candidate_hash_c.clone(),
					session: 1,
					statements: vec![
						(
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
							ValidatorIndex(0),
							sig_0c.clone(),
						),
						(
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
							ValidatorIndex(6),
							sig_2c.clone(),
						),
					],
				},
			];

			let old_statements = statements
				.clone()
				.into_iter()
				.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
				.collect::<Vec<_>>();
			let statements = apply_filter_all::<Test, _>(statements);

			assert_eq!(statements, old_statements);
		})
	}

	#[test]
	fn filter_removes_session_out_of_bounds() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
			});

			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

			let payload = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash.clone(),
				session: 1,
			}
			.signing_payload();

			let sig_a = v0.sign(&payload);

			let statements = vec![DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 100,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a,
				)],
			}];

			let statements = apply_filter_all::<Test, _>(statements);

			assert!(statements.is_empty());
		})
	}

	#[test]
	fn filter_removes_concluded_ancient() {
		let dispute_post_conclusion_acceptance_period = 2;

		let mock_genesis_config = MockGenesisConfig {
			configuration: crate::configuration::GenesisConfig {
				config: HostConfiguration {
					dispute_post_conclusion_acceptance_period,
					..Default::default()
				},
				..Default::default()
			},
			..Default::default()
		};

		new_test_ext(mock_genesis_config).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));
			let candidate_hash_b = CandidateHash(sp_core::H256::repeat_byte(2));

			<Disputes<Test>>::insert(
				&1,
				&candidate_hash_a,
				DisputeState {
					validators_for: bitvec![BitOrderLsb0, u8; 0; 4],
					validators_against: bitvec![BitOrderLsb0, u8; 1; 4],
					start: 0,
					concluded_at: Some(0),
				},
			);

			<Disputes<Test>>::insert(
				&1,
				&candidate_hash_b,
				DisputeState {
					validators_for: bitvec![BitOrderLsb0, u8; 0; 4],
					validators_against: bitvec![BitOrderLsb0, u8; 1; 4],
					start: 0,
					concluded_at: Some(1),
				},
			);

			let payload_a = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let payload_b = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash_b.clone(),
				session: 1,
			}
			.signing_payload();

			let sig_a = v0.sign(&payload_a);
			let sig_b = v0.sign(&payload_b);

			let statements = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements: vec![(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_a,
					)],
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash_b.clone(),
					session: 1,
					statements: vec![(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_b.clone(),
					)],
				},
			];

			let statements = apply_filter_all::<Test, _>(statements);

			assert_eq!(
				statements,
				vec![CheckedDisputeStatementSet::unchecked_from_unchecked(DisputeStatementSet {
					candidate_hash: candidate_hash_b.clone(),
					session: 1,
					statements: vec![(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_b,
					),]
				})]
			);
		})
	}

	#[test]
	fn filter_removes_duplicate_statements_sets() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![(&0, v0.public()), (&1, v1.public())],
					Some(vec![(&0, v0.public()), (&1, v1.public())]),
				))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

			let payload = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let payload_against = ExplicitDisputeStatement {
				valid: false,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let sig_a = v0.sign(&payload);
			let sig_a_against = v1.sign(&payload_against);

			let statements = vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a.clone(),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					sig_a_against.clone(),
				),
			];

			let mut sets = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements: statements.clone(),
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements: statements.clone(),
				},
			];

			// `Err(())` indicates presence of duplicates
			assert!(<Pallet::<Test> as DisputesHandler<
				<Test as frame_system::Config>::BlockNumber,
			>>::deduplicate_and_sort_dispute_data(&mut sets)
			.is_err());

			assert_eq!(
				sets,
				vec![DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements,
				}]
			);
		})
	}

	#[test]
	fn assure_no_duplicate_statements_sets_are_fine() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![(&0, v0.public()), (&1, v1.public())],
					Some(vec![(&0, v0.public()), (&1, v1.public())]),
				))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

			let payload = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let payload_against = ExplicitDisputeStatement {
				valid: false,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let sig_a = v0.sign(&payload);
			let sig_a_against = v1.sign(&payload_against);

			let statements = vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a.clone(),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					sig_a_against.clone(),
				),
			];

			let sets = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements: statements.clone(),
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 2,
					statements: statements.clone(),
				},
			];

			// `Err(())` indicates presence of duplicates
			assert!(<Pallet::<Test> as DisputesHandler<
				<Test as frame_system::Config>::BlockNumber,
			>>::assure_deduplicated_and_sorted(&sets)
			.is_ok());
		})
	}

	#[test]
	fn assure_detects_duplicate_statements_sets() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
			let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((
					true,
					b,
					vec![(&0, v0.public()), (&1, v1.public())],
					Some(vec![(&0, v0.public()), (&1, v1.public())]),
				))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

			let payload = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let payload_against = ExplicitDisputeStatement {
				valid: false,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let sig_a = v0.sign(&payload);
			let sig_a_against = v1.sign(&payload_against);

			let statements = vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a.clone(),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					sig_a_against.clone(),
				),
			];

			let sets = vec![
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements: statements.clone(),
				},
				DisputeStatementSet {
					candidate_hash: candidate_hash_a.clone(),
					session: 1,
					statements: statements.clone(),
				},
			];

			// `Err(())` indicates presence of duplicates
			assert!(<Pallet::<Test> as DisputesHandler<
				<Test as frame_system::Config>::BlockNumber,
			>>::assure_deduplicated_and_sorted(&sets)
			.is_err());
		})
	}

	#[test]
	fn filter_ignores_single_sided() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

			let payload = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let sig_a = v0.sign(&payload);

			let statements = vec![DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a.clone(),
				)],
			}];

			let statements = apply_filter_all::<Test, _>(statements);

			assert!(statements.is_empty());
		})
	}

	#[test]
	fn import_ignores_single_sided() {
		new_test_ext(Default::default()).execute_with(|| {
			let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

			run_to_block(3, |b| {
				// a new session at each block
				Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
			});

			let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

			let payload = ExplicitDisputeStatement {
				valid: true,
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
			}
			.signing_payload();

			let sig_a = v0.sign(&payload);

			let statements = vec![DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a.clone(),
				)],
			}];

			let statements = apply_filter_all::<Test, _>(statements);
			assert!(statements.is_empty());
		})
	}
}
