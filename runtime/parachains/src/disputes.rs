// Copyright (C) Parity Technologies (UK) Ltd.
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

use crate::{
	configuration, initializer::SessionChangeNotification, metrics::METRICS, session_info,
};
use bitvec::{bitvec, order::Lsb0 as BitOrderLsb0};
use frame_support::{ensure, weights::Weight};
use frame_system::pallet_prelude::*;
use parity_scale_codec::{Decode, Encode};
use polkadot_runtime_metrics::get_current_time;
use primitives::{
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
use sp_std::{cmp::Ordering, collections::btree_set::BTreeSet, prelude::*};

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use self::tests::run_to_block;

pub mod slashing;
#[cfg(test)]
mod tests;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

pub mod migration;

const LOG_TARGET: &str = "runtime::disputes";

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
pub trait SlashingHandler<BlockNumber> {
	/// Punish a series of validators who were for an invalid parablock. This is
	/// expected to be a major punishment.
	fn punish_for_invalid(
		session: SessionIndex,
		candidate_hash: CandidateHash,
		losers: impl IntoIterator<Item = ValidatorIndex>,
		backers: impl IntoIterator<Item = ValidatorIndex>,
	);

	/// Punish a series of validators who were against a valid parablock. This
	/// is expected to be a minor punishment.
	fn punish_against_valid(
		session: SessionIndex,
		candidate_hash: CandidateHash,
		losers: impl IntoIterator<Item = ValidatorIndex>,
		backers: impl IntoIterator<Item = ValidatorIndex>,
	);

	/// Called by the initializer to initialize the slashing pallet.
	fn initializer_initialize(now: BlockNumber) -> Weight;

	/// Called by the initializer to finalize the slashing pallet.
	fn initializer_finalize();

	/// Called by the initializer to note that a new session has started.
	fn initializer_on_new_session(session_index: SessionIndex);
}

impl<BlockNumber> SlashingHandler<BlockNumber> for () {
	fn punish_for_invalid(
		_: SessionIndex,
		_: CandidateHash,
		_: impl IntoIterator<Item = ValidatorIndex>,
		_: impl IntoIterator<Item = ValidatorIndex>,
	) {
	}

	fn punish_against_valid(
		_: SessionIndex,
		_: CandidateHash,
		_: impl IntoIterator<Item = ValidatorIndex>,
		_: impl IntoIterator<Item = ValidatorIndex>,
	) {
	}

	fn initializer_initialize(_now: BlockNumber) -> Weight {
		Weight::zero()
	}

	fn initializer_finalize() {}

	fn initializer_on_new_session(_: SessionIndex) {}
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

/// Hook into disputes handling.
///
/// Allows decoupling parachains handling from disputes so that it can
/// potentially be disabled when instantiating a specific runtime.
pub trait DisputesHandler<BlockNumber: Ord> {
	/// Whether the chain is frozen, if the chain is frozen it will not accept
	/// any new parachain blocks for backing or inclusion.
	fn is_frozen() -> bool;

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
		post_conclusion_acceptance_period: BlockNumber,
	) -> Option<CheckedDisputeStatementSet>;

	/// Handle sets of dispute statements corresponding to 0 or more candidates.
	/// Returns a vector of freshly created disputes.
	fn process_checked_multi_dispute_data(
		statement_sets: &CheckedMultiDisputeStatementSet,
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
		_post_conclusion_acceptance_period: BlockNumber,
	) -> Option<CheckedDisputeStatementSet> {
		None
	}

	fn process_checked_multi_dispute_data(
		_statement_sets: &CheckedMultiDisputeStatementSet,
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
		Weight::zero()
	}

	fn initializer_finalize() {}

	fn initializer_on_new_session(_notification: &SessionChangeNotification<BlockNumber>) {}
}

impl<T: Config> DisputesHandler<BlockNumberFor<T>> for pallet::Pallet<T>
where
	BlockNumberFor<T>: Ord,
{
	fn is_frozen() -> bool {
		pallet::Pallet::<T>::is_frozen()
	}

	fn filter_dispute_data(
		set: DisputeStatementSet,
		post_conclusion_acceptance_period: BlockNumberFor<T>,
	) -> Option<CheckedDisputeStatementSet> {
		pallet::Pallet::<T>::filter_dispute_data(&set, post_conclusion_acceptance_period)
			.filter_statement_set(set)
	}

	fn process_checked_multi_dispute_data(
		statement_sets: &CheckedMultiDisputeStatementSet,
	) -> Result<Vec<(SessionIndex, CandidateHash)>, DispatchError> {
		pallet::Pallet::<T>::process_checked_multi_dispute_data(statement_sets)
	}

	fn note_included(
		session: SessionIndex,
		candidate_hash: CandidateHash,
		included_in: BlockNumberFor<T>,
	) {
		pallet::Pallet::<T>::note_included(session, candidate_hash, included_in)
	}

	fn included_state(
		session: SessionIndex,
		candidate_hash: CandidateHash,
	) -> Option<BlockNumberFor<T>> {
		pallet::Pallet::<T>::included_state(session, candidate_hash)
	}

	fn concluded_invalid(session: SessionIndex, candidate_hash: CandidateHash) -> bool {
		pallet::Pallet::<T>::concluded_invalid(session, candidate_hash)
	}

	fn initializer_initialize(now: BlockNumberFor<T>) -> Weight {
		pallet::Pallet::<T>::initializer_initialize(now)
	}

	fn initializer_finalize() {
		pallet::Pallet::<T>::initializer_finalize()
	}

	fn initializer_on_new_session(notification: &SessionChangeNotification<BlockNumberFor<T>>) {
		pallet::Pallet::<T>::initializer_on_new_session(notification)
	}
}

pub trait WeightInfo {
	fn force_unfreeze() -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn force_unfreeze() -> Weight {
		Weight::zero()
	}
}

pub use pallet::*;
#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;

	#[pallet::config]
	pub trait Config: frame_system::Config + configuration::Config + session_info::Config {
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
		type RewardValidators: RewardValidators;
		type SlashingHandler: SlashingHandler<BlockNumberFor<Self>>;

		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	/// The current storage version.
	const STORAGE_VERSION: StorageVersion = StorageVersion::new(1);

	#[pallet::pallet]
	#[pallet::without_storage_info]
	#[pallet::storage_version(STORAGE_VERSION)]
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
		DisputeState<BlockNumberFor<T>>,
	>;

	/// Backing votes stored for each dispute.
	/// This storage is used for slashing.
	#[pallet::storage]
	pub(super) type BackersOnDisputes<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Blake2_128Concat,
		CandidateHash,
		BTreeSet<ValidatorIndex>,
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
		BlockNumberFor<T>,
	>;

	/// Whether the chain is frozen. Starts as `None`. When this is `Some`,
	/// the chain will not accept any new parachain blocks for backing or inclusion,
	/// and its value indicates the last valid block number in the chain.
	/// It can only be set back to `None` by governance intervention.
	#[pallet::storage]
	#[pallet::getter(fn last_valid_block)]
	pub(super) type Frozen<T: Config> = StorageValue<_, Option<BlockNumberFor<T>>, ValueQuery>;

	#[pallet::event]
	#[pallet::generate_deposit(pub fn deposit_event)]
	pub enum Event<T: Config> {
		/// A dispute has been initiated. \[candidate hash, dispute location\]
		DisputeInitiated(CandidateHash, DisputeLocation),
		/// A dispute has concluded for or against a candidate.
		/// `\[para id, candidate hash, dispute result\]`
		DisputeConcluded(CandidateHash, DisputeResult),
		/// A dispute has concluded with supermajority against a candidate.
		/// Block authors should no longer build on top of this head and should
		/// instead revert the block at the given height. This should be the
		/// number of the child of the last known valid block in the chain.
		Revert(BlockNumberFor<T>),
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
		/// A dispute where there are only votes on one side.
		SingleSidedDispute,
		/// A dispute vote from a malicious backer.
		MaliciousBacker,
		/// No backing votes were provides along dispute statements.
		MissingBackingVotes,
		/// Unconfirmed dispute statement sets provided.
		UnconfirmedDispute,
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		#[pallet::call_index(0)]
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
		/// The byzantine threshold of `f + 1` votes (and hence participating validators) was reached.
		const CONFIRMED = 0b0001;
		/// Is the supermajority for validity of the dispute reached.
		const FOR_SUPERMAJORITY = 0b0010;
		/// Is the supermajority against the validity of the block reached.
		const AGAINST_SUPERMAJORITY = 0b0100;
		/// Is there f+1 against the validity of the block reached
		const AGAINST_BYZANTINE = 0b1000;
	}
}

impl DisputeStateFlags {
	fn from_state<BlockNumber>(state: &DisputeState<BlockNumber>) -> Self {
		// Only correct since `DisputeState` is _always_ initialized
		// with the validator set based on the session.
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

		if state.validators_against.count_ones() > byzantine_threshold {
			flags |= DisputeStateFlags::AGAINST_BYZANTINE;
		}

		if state.validators_against.count_ones() >= supermajority_threshold {
			flags |= DisputeStateFlags::AGAINST_SUPERMAJORITY;
		}

		flags
	}
}

struct ImportSummary<BlockNumber> {
	/// The new state, with all votes imported.
	state: DisputeState<BlockNumber>,
	/// List of validators who backed the candidate being disputed.
	backers: BTreeSet<ValidatorIndex>,
	/// Validators to slash for being (wrongly) on the AGAINST side.
	slash_against: Vec<ValidatorIndex>,
	/// Validators to slash for being (wrongly) on the FOR side.
	slash_for: Vec<ValidatorIndex>,
	// New participants in the dispute.
	new_participants: bitvec::vec::BitVec<u8, BitOrderLsb0>,
	// Difference in state flags from previous.
	new_flags: DisputeStateFlags,
}

#[derive(RuntimeDebug, PartialEq, Eq)]
enum VoteImportError {
	/// Validator index was outside the range of valid validator indices in the given session.
	ValidatorIndexOutOfBounds,
	/// Found a duplicate statement in the dispute statement set.
	DuplicateStatement,
	/// Found an explicit valid statement after backing statement.
	/// Backers should not participate in explicit voting so this is
	/// only possible on malicious backers.
	MaliciousBacker,
}

#[derive(RuntimeDebug, Copy, Clone, PartialEq, Eq)]
enum VoteKind {
	/// A backing vote that is counted as "for" vote in dispute resolution.
	Backing,
	/// Either an approval vote or and explicit dispute "for" vote.
	ExplicitValid,
	/// An explicit dispute "against" vote.
	Invalid,
}

impl From<&DisputeStatement> for VoteKind {
	fn from(statement: &DisputeStatement) -> Self {
		if statement.is_backing() {
			Self::Backing
		} else if statement.indicates_validity() {
			Self::ExplicitValid
		} else {
			Self::Invalid
		}
	}
}

impl VoteKind {
	fn is_valid(&self) -> bool {
		match self {
			Self::Backing | Self::ExplicitValid => true,
			Self::Invalid => false,
		}
	}

	fn is_backing(&self) -> bool {
		match self {
			Self::Backing => true,
			Self::Invalid | Self::ExplicitValid => false,
		}
	}
}

impl<T: Config> From<VoteImportError> for Error<T> {
	fn from(e: VoteImportError) -> Self {
		match e {
			VoteImportError::ValidatorIndexOutOfBounds => Error::<T>::ValidatorIndexOutOfBounds,
			VoteImportError::DuplicateStatement => Error::<T>::DuplicateStatement,
			VoteImportError::MaliciousBacker => Error::<T>::MaliciousBacker,
		}
	}
}

/// A transport statement bit change for a single validator.
#[derive(RuntimeDebug, PartialEq, Eq)]
struct ImportUndo {
	/// The validator index to which to associate the statement import.
	validator_index: ValidatorIndex,
	/// The kind and direction of the vote.
	vote_kind: VoteKind,
	/// Has the validator participated before, i.e. in backing or
	/// with an opposing vote.
	new_participant: bool,
}

struct DisputeStateImporter<BlockNumber> {
	state: DisputeState<BlockNumber>,
	backers: BTreeSet<ValidatorIndex>,
	now: BlockNumber,
	new_participants: bitvec::vec::BitVec<u8, BitOrderLsb0>,
	pre_flags: DisputeStateFlags,
	pre_state: DisputeState<BlockNumber>,
	// The list of backing votes before importing the batch of votes. This field should be
	// initialized as empty on the first import of the dispute votes and should remain non-empty
	// afterwards.
	//
	// If a dispute has concluded and the candidate was found invalid, we may want to slash as many
	// backers as possible. This list allows us to slash these backers once their votes have been
	// imported post dispute conclusion.
	pre_backers: BTreeSet<ValidatorIndex>,
}

impl<BlockNumber: Clone> DisputeStateImporter<BlockNumber> {
	fn new(
		state: DisputeState<BlockNumber>,
		backers: BTreeSet<ValidatorIndex>,
		now: BlockNumber,
	) -> Self {
		let pre_flags = DisputeStateFlags::from_state(&state);
		let new_participants = bitvec::bitvec![u8, BitOrderLsb0; 0; state.validators_for.len()];
		// consistency checks
		for i in backers.iter() {
			debug_assert_eq!(state.validators_for.get(i.0 as usize).map(|b| *b), Some(true));
		}
		let pre_state = state.clone();
		let pre_backers = backers.clone();

		DisputeStateImporter {
			state,
			backers,
			now,
			new_participants,
			pre_flags,
			pre_state,
			pre_backers,
		}
	}

	fn import(
		&mut self,
		validator: ValidatorIndex,
		kind: VoteKind,
	) -> Result<ImportUndo, VoteImportError> {
		let (bits, other_bits) = if kind.is_valid() {
			(&mut self.state.validators_for, &mut self.state.validators_against)
		} else {
			(&mut self.state.validators_against, &mut self.state.validators_for)
		};

		// out of bounds or already participated
		match bits.get(validator.0 as usize).map(|b| *b) {
			None => return Err(VoteImportError::ValidatorIndexOutOfBounds),
			Some(true) => {
				// We allow backing statements to be imported after an
				// explicit "for" vote, but not the other way around.
				match (kind.is_backing(), self.backers.contains(&validator)) {
					(true, true) | (false, false) =>
						return Err(VoteImportError::DuplicateStatement),
					(false, true) => return Err(VoteImportError::MaliciousBacker),
					(true, false) => {},
				}
			},
			Some(false) => {},
		}

		// consistency check
		debug_assert!((validator.0 as usize) < self.new_participants.len());

		let mut undo =
			ImportUndo { validator_index: validator, vote_kind: kind, new_participant: false };

		bits.set(validator.0 as usize, true);
		if kind.is_backing() {
			let is_new = self.backers.insert(validator);
			// invariant check
			debug_assert!(is_new);
		}

		// New participants tracks those validators by index, which didn't appear on either
		// side of the dispute until now (so they make a first appearance).
		// To verify this we need to assure they also were not part of the opposing side before.
		if other_bits.get(validator.0 as usize).map_or(false, |b| !*b) {
			undo.new_participant = true;
			self.new_participants.set(validator.0 as usize, true);
		}

		Ok(undo)
	}

	/// Revert a done transaction.
	fn undo(&mut self, undo: ImportUndo) {
		if undo.vote_kind.is_valid() {
			self.state.validators_for.set(undo.validator_index.0 as usize, false);
		} else {
			self.state.validators_against.set(undo.validator_index.0 as usize, false);
		}

		if undo.vote_kind.is_backing() {
			self.backers.remove(&undo.validator_index);
		}

		if undo.new_participant {
			self.new_participants.set(undo.validator_index.0 as usize, false);
		}
	}

	/// Collect all dispute votes.
	fn finish(mut self) -> ImportSummary<BlockNumber> {
		let pre_flags = self.pre_flags;
		let post_flags = DisputeStateFlags::from_state(&self.state);

		let pre_post_contains = |flags| (pre_flags.contains(flags), post_flags.contains(flags));

		// 1. Check for FOR supermajority.
		let slash_against = match pre_post_contains(DisputeStateFlags::FOR_SUPERMAJORITY) {
			(false, true) => {
				if self.state.concluded_at.is_none() {
					self.state.concluded_at = Some(self.now.clone());
				}

				// provide AGAINST voters to slash.
				self.state
					.validators_against
					.iter_ones()
					.map(|i| ValidatorIndex(i as _))
					.collect()
			},
			(true, true) => {
				// provide new AGAINST voters to slash.
				self.state
					.validators_against
					.iter_ones()
					.filter(|i| self.pre_state.validators_against.get(*i).map_or(false, |b| !*b))
					.map(|i| ValidatorIndex(i as _))
					.collect()
			},
			(true, false) => {
				log::error!("Dispute statements are never removed. This is a bug");
				Vec::new()
			},
			(false, false) => Vec::new(),
		};

		// 2. Check for AGAINST supermajority.
		let slash_for = match pre_post_contains(DisputeStateFlags::AGAINST_SUPERMAJORITY) {
			(false, true) => {
				if self.state.concluded_at.is_none() {
					self.state.concluded_at = Some(self.now.clone());
				}

				// provide FOR voters to slash.
				self.state.validators_for.iter_ones().map(|i| ValidatorIndex(i as _)).collect()
			},
			(true, true) => {
				// provide new FOR voters to slash including new backers
				// who might have voted FOR before
				let new_backing_vote = |i: &ValidatorIndex| -> bool {
					!self.pre_backers.contains(i) && self.backers.contains(i)
				};
				self.state
					.validators_for
					.iter_ones()
					.filter(|i| {
						self.pre_state.validators_for.get(*i).map_or(false, |b| !*b) ||
							new_backing_vote(&ValidatorIndex(*i as _))
					})
					.map(|i| ValidatorIndex(i as _))
					.collect()
			},
			(true, false) => {
				log::error!("Dispute statements are never removed. This is a bug");
				Vec::new()
			},
			(false, false) => Vec::new(),
		};

		ImportSummary {
			state: self.state,
			backers: self.backers,
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
	pub(crate) fn initializer_initialize(_now: BlockNumberFor<T>) -> Weight {
		Weight::zero()
	}

	/// Called by the initializer to finalize the disputes pallet.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note a new session in the disputes pallet.
	pub(crate) fn initializer_on_new_session(
		notification: &SessionChangeNotification<BlockNumberFor<T>>,
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
				#[allow(deprecated)]
				<Disputes<T>>::remove_prefix(to_prune, None);
				#[allow(deprecated)]
				<BackersOnDisputes<T>>::remove_prefix(to_prune, None);

				// This is larger, and will be extracted to the `shared` pallet for more proper
				// pruning. TODO: https://github.com/paritytech/polkadot/issues/3469
				#[allow(deprecated)]
				<Included<T>>::remove_prefix(to_prune, None);
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
		statement_sets: &CheckedMultiDisputeStatementSet,
	) -> Result<Vec<(SessionIndex, CandidateHash)>, DispatchError> {
		let config = <configuration::Pallet<T>>::config();

		let mut fresh = Vec::with_capacity(statement_sets.len());
		for statement_set in statement_sets {
			let dispute_target = {
				let statement_set = statement_set.as_ref();
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
	// Disputes without enough votes to get confirmed are also filtered out.
	fn filter_dispute_data(
		set: &DisputeStatementSet,
		post_conclusion_acceptance_period: BlockNumberFor<T>,
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
				// No state in storage, this indicates it's the first dispute statement set as well.
				DisputeState {
					validators_for: bitvec![u8, BitOrderLsb0; 0; n_validators],
					validators_against: bitvec![u8, BitOrderLsb0; 0; n_validators],
					start: now,
					concluded_at: None,
				}
			}
		};

		let backers =
			<BackersOnDisputes<T>>::get(&set.session, &set.candidate_hash).unwrap_or_default();

		// Check and import all votes.
		let summary = {
			let mut importer = DisputeStateImporter::new(dispute_state, backers, now);
			for (i, (statement, validator_index, signature)) in set.statements.iter().enumerate() {
				// ensure the validator index is present in the session info
				// and the signature is valid
				let validator_public = match session_info.validators.get(*validator_index) {
					None => {
						filter.remove_index(i);
						continue
					},
					Some(v) => v,
				};

				let kind = VoteKind::from(statement);

				let undo = match importer.import(*validator_index, kind) {
					Ok(u) => u,
					Err(_) => {
						filter.remove_index(i);
						continue
					},
				};

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
				};
			}

			importer.finish()
		};

		// Reject disputes which don't have at least one vote on each side.
		if summary.state.validators_for.count_ones() == 0 ||
			summary.state.validators_against.count_ones() == 0
		{
			return StatementSetFilter::RemoveAll
		}

		// Reject disputes containing less votes than needed for confirmation.
		if (summary.state.validators_for.clone() | &summary.state.validators_against).count_ones() <=
			byzantine_threshold(summary.state.validators_for.len())
		{
			return StatementSetFilter::RemoveAll
		}

		filter
	}

	/// Handle a set of dispute statements corresponding to a single candidate.
	///
	/// Fails if the dispute data is invalid. Returns a Boolean indicating whether the
	/// dispute is fresh.
	fn process_checked_dispute_data(
		set: &CheckedDisputeStatementSet,
		dispute_post_conclusion_acceptance_period: BlockNumberFor<T>,
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

		let backers =
			<BackersOnDisputes<T>>::get(&set.session, &set.candidate_hash).unwrap_or_default();

		// Import all votes. They were pre-checked.
		let summary = {
			let mut importer = DisputeStateImporter::new(dispute_state, backers, now);
			for (statement, validator_index, _signature) in &set.statements {
				let kind = VoteKind::from(statement);

				importer.import(*validator_index, kind).map_err(Error::<T>::from)?;
			}

			importer.finish()
		};

		// Reject disputes which don't have at least one vote on each side.
		ensure!(
			summary.state.validators_for.count_ones() > 0 &&
				summary.state.validators_against.count_ones() > 0,
			Error::<T>::SingleSidedDispute,
		);

		// Reject disputes containing less votes than needed for confirmation.
		ensure!(
			(summary.state.validators_for.clone() | &summary.state.validators_against).count_ones() >
				byzantine_threshold(summary.state.validators_for.len()),
			Error::<T>::UnconfirmedDispute,
		);
		let backers = summary.backers;
		// Reject statements with no accompanying backing votes.
		ensure!(!backers.is_empty(), Error::<T>::MissingBackingVotes);
		<BackersOnDisputes<T>>::insert(&set.session, &set.candidate_hash, backers.clone());
		// AUDIT: from now on, no error should be returned.

		let DisputeStatementSet { ref session, ref candidate_hash, .. } = set;
		let session = *session;
		let candidate_hash = *candidate_hash;

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
			T::SlashingHandler::punish_against_valid(
				session,
				candidate_hash,
				summary.slash_against,
				backers.clone(),
			);

			// an invalid candidate, according to 2/3. Punish those on the 'for' side.
			T::SlashingHandler::punish_for_invalid(
				session,
				candidate_hash,
				summary.slash_for,
				backers,
			);
		}

		<Disputes<T>>::insert(&session, &candidate_hash, &summary.state);

		// Freeze if the INVALID votes against some local candidate are above the byzantine
		// threshold
		if summary.new_flags.contains(DisputeStateFlags::AGAINST_BYZANTINE) {
			if let Some(revert_to) = <Included<T>>::get(&session, &candidate_hash) {
				Self::revert_and_freeze(revert_to);
			}
		}

		Ok(fresh)
	}

	#[allow(unused)]
	pub(crate) fn disputes() -> Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumberFor<T>>)>
	{
		<Disputes<T>>::iter().collect()
	}

	pub(crate) fn note_included(
		session: SessionIndex,
		candidate_hash: CandidateHash,
		included_in: BlockNumberFor<T>,
	) {
		if included_in.is_zero() {
			return
		}

		let revert_to = included_in - One::one();

		<Included<T>>::insert(&session, &candidate_hash, revert_to);

		if let Some(state) = <Disputes<T>>::get(&session, candidate_hash) {
			if has_supermajority_against(&state) {
				Self::revert_and_freeze(revert_to);
			}
		}
	}

	pub(crate) fn included_state(
		session: SessionIndex,
		candidate_hash: CandidateHash,
	) -> Option<BlockNumberFor<T>> {
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

	pub(crate) fn revert_and_freeze(revert_to: BlockNumberFor<T>) {
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

	let start = get_current_time();

	let res =
		if validator_signature.verify(&payload[..], &validator_public) { Ok(()) } else { Err(()) };

	let end = get_current_time();

	METRICS.on_signature_check_complete(end.saturating_sub(start)); // ns

	res
}
