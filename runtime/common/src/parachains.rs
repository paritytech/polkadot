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

//! Main parachains logic. For now this is just the determination of which validators do what.

use sp_std::prelude::*;
use sp_std::result;
use codec::{Decode, Encode};
use sp_runtime::{
	KeyTypeId, Perbill, RuntimeDebug,
	traits::{
		Hash as HashT, BlakeTwo256, Saturating, One, Zero, Dispatchable,
		AccountIdConversion, BadOrigin, Convert, SignedExtension, AppVerify,
		DispatchInfoOf,
	},
	transaction_validity::{TransactionValidityError, ValidTransaction, TransactionValidity},
};
use sp_staking::{
	SessionIndex,
	offence::{ReportOffence, Offence, Kind},
};
use frame_support::{
	traits::KeyOwnerProofSystem,
	dispatch::{IsSubType},
	weights::{DispatchClass, Weight},
};
use primitives::{
	Balance,
	BlockNumber,
	parachain::{
		Id as ParaId, Chain, DutyRoster, AttestedCandidate, Statement, ParachainDispatchOrigin,
		UpwardMessage, ValidatorId, ActiveParas, CollatorId, Retriable, OmittedValidationData,
		CandidateReceipt, GlobalValidationSchedule, AbridgedCandidateReceipt,
		LocalValidationData, Scheduling, ValidityAttestation, NEW_HEADS_IDENTIFIER, PARACHAIN_KEY_TYPE_ID,
		ValidatorSignature, SigningContext, HeadData, ValidationCode,
	},
};
use frame_support::{
	Parameter, dispatch::DispatchResult, decl_storage, decl_module, decl_error, ensure,
	traits::{Currency, Get, WithdrawReason, ExistenceRequirement, Randomness},
};
use sp_runtime::{
	transaction_validity::InvalidTransaction,
};

use inherents::{ProvideInherent, InherentData, MakeFatalError, InherentIdentifier};

use system::{
	ensure_none, ensure_signed,
	offchain::{CreateSignedTransaction, SendSignedTransaction, Signer},
};
use crate::attestations::{self, IncludedBlocks};
use crate::registrar::Registrar;

// ranges for iteration of general block number don't work, so this
// is a utility to get around that.
struct BlockNumberRange<N> {
	low: N,
	high: N,
}

impl<N: Saturating + One + PartialOrd + PartialEq + Clone> Iterator for BlockNumberRange<N> {
	type Item = N;

	fn next(&mut self) -> Option<N> {
		if self.low >= self.high {
			return None
		}

		let item = self.low.clone();
		self.low = self.low.clone().saturating_add(One::one());
		Some(item)
	}
}

// wrapper trait because an associated type of `Currency<Self::AccountId,Balance=Balance>`
// doesn't work.`
pub trait ParachainCurrency<AccountId> {
	fn free_balance(para_id: ParaId) -> Balance;
	fn deduct(para_id: ParaId, amount: Balance) -> DispatchResult;
}

impl<AccountId, T: Currency<AccountId>> ParachainCurrency<AccountId> for T where
	T::Balance: From<Balance> + Into<Balance>,
	ParaId: AccountIdConversion<AccountId>,
{
	fn free_balance(para_id: ParaId) -> Balance {
		let para_account = para_id.into_account();
		T::free_balance(&para_account).into()
	}

	fn deduct(para_id: ParaId, amount: Balance) -> DispatchResult {
		let para_account = para_id.into_account();

		// burn the fee.
		let _ = T::withdraw(
			&para_account,
			amount.into(),
			WithdrawReason::Fee.into(),
			ExistenceRequirement::KeepAlive,
		)?;

		Ok(())
	}
}

/// Interface to the persistent (stash) identities of the current validators.
pub struct ValidatorIdentities<T>(sp_std::marker::PhantomData<T>);

/// A structure used to report conflicting votes by validators.
///
/// It is generic over two parameters:
/// `Proof` - proof of historical ownership of a key by some validator.
/// `Hash` - a type of a hash used in the runtime.
#[derive(RuntimeDebug, Encode, Decode)]
#[derive(Clone, Eq, PartialEq)]
pub struct DoubleVoteReport<Proof> {
	/// Identity of the double-voter.
	pub identity: ValidatorId,
	/// First vote of the double-vote.
	pub first: (Statement, ValidatorSignature),
	/// Second vote of the double-vote.
	pub second: (Statement, ValidatorSignature),
	/// Proof that the validator with `identity` id was actually a validator at `parent_hash`.
	pub proof: Proof,
	/// A `SigningContext` with a session and a parent hash of the moment this offence was commited.
	pub signing_context: SigningContext,
}

impl<Proof: Parameter + GetSessionNumber> DoubleVoteReport<Proof> {
	fn verify<T: Trait<Proof = Proof>>(
		&self,
	) -> Result<(), DoubleVoteValidityError> {
		let first = self.first.clone();
		let second = self.second.clone();
		let id = self.identity.clone();

		T::KeyOwnerProofSystem::check_proof((PARACHAIN_KEY_TYPE_ID, id), self.proof.clone())
			.ok_or(DoubleVoteValidityError::InvalidProof)?;

		if self.proof.session() != self.signing_context.session_index {
			return Err(DoubleVoteValidityError::InvalidReport);
		}

		// Check signatures.
		Self::verify_vote(
			&first,
			&self.signing_context,
			&self.identity,
		)?;
		Self::verify_vote(
			&second,
			&self.signing_context,
			&self.identity,
		)?;

		match (&first.0, &second.0) {
			// If issuing a `Candidate` message on a parachain block, neither a `Valid` or
			// `Invalid` vote cannot be issued on that parachain block, as the `Candidate`
			// message is an implicit validity vote.
			(Statement::Candidate(candidate_hash), Statement::Valid(hash)) |
			(Statement::Candidate(candidate_hash), Statement::Invalid(hash)) |
			(Statement::Valid(hash), Statement::Candidate(candidate_hash)) |
			(Statement::Invalid(hash), Statement::Candidate(candidate_hash))
			if *candidate_hash == *hash => {},
			// Otherwise, it is illegal to cast both a `Valid` and
			// `Invalid` vote on a given parachain block.
			(Statement::Valid(hash_1), Statement::Invalid(hash_2)) |
			(Statement::Invalid(hash_1), Statement::Valid(hash_2))
			if *hash_1 == *hash_2 => {},
			_ => {
				return Err(DoubleVoteValidityError::NotDoubleVote);
			}
		}

		Ok(())
	}

	fn verify_vote(
		vote: &(Statement, ValidatorSignature),
		signing_context: &SigningContext,
		authority: &ValidatorId,
	) -> Result<(), DoubleVoteValidityError> {
		let payload = localized_payload(vote.0.clone(), signing_context);

		if !vote.1.verify(&payload[..], authority) {
			return Err(DoubleVoteValidityError::InvalidSignature);
		}

		Ok(())
	}
}

impl<T: session::Trait> Get<Vec<T::ValidatorId>> for ValidatorIdentities<T> {
	fn get() -> Vec<T::ValidatorId> {
		<session::Module<T>>::validators()
	}
}

/// A trait to get a session number the `MembershipProof` belongs to.
pub trait GetSessionNumber {
	fn session(&self) -> SessionIndex;
}

impl GetSessionNumber for sp_session::MembershipProof {
	fn session(&self) -> SessionIndex {
		self.session
	}
}

pub trait Trait: CreateSignedTransaction<Call<Self>> + attestations::Trait + session::historical::Trait {
	// The transaction signing authority
	type AuthorityId: system::offchain::AppCrypto<Self::Public, Self::Signature>;

	/// The outer origin type.
	type Origin: From<Origin> + From<system::RawOrigin<Self::AccountId>>;

	/// The outer call dispatch type.
	type Call: Parameter + Dispatchable<Origin=<Self as Trait>::Origin> + From<Call<Self>>;

	/// Some way of interacting with balances for fees.
	type ParachainCurrency: ParachainCurrency<Self::AccountId>;

	/// Polkadot in practice will always use the `BlockNumber` type.
	/// Substrate isn't good at giving us ways to bound the supertrait
	/// associated type, so we introduce this conversion.
	type BlockNumberConversion: Convert<Self::BlockNumber, BlockNumber>;

	/// Something that provides randomness in the runtime.
	type Randomness: Randomness<Self::Hash>;

	/// Means to determine what the current set of active parachains are.
	type ActiveParachains: ActiveParas;

	/// The way that we are able to register parachains.
	type Registrar: Registrar<Self::AccountId>;

	/// Maximum code size for parachains, in bytes. Note that this is not
	/// the entire storage burden of the parachain, as old code is stored for
	/// `SlashPeriod` blocks.
	type MaxCodeSize: Get<u32>;

	/// Max head data size.
	type MaxHeadDataSize: Get<u32>;
	/// The frequency at which paras can upgrade their validation function.
	/// This is an integer number of relay-chain blocks that must pass between
	/// code upgrades.
	type ValidationUpgradeFrequency: Get<Self::BlockNumber>;

	/// The delay before a validation function upgrade is applied.
	type ValidationUpgradeDelay: Get<Self::BlockNumber>;

	/// The period (in blocks) that slash reports are permitted against an
	/// included candidate.
	///
	/// After validation function upgrades, the old code is persisted on-chain
	/// for this period, to ensure that candidates validated under old functions
	/// can be re-checked.
	type SlashPeriod: Get<Self::BlockNumber>;

	/// Proof type.
	///
	/// We need this type to bind the `KeyOwnerProofSystem::Proof` to necessary bounds.
	/// As soon as https://rust-lang.github.io/rfcs/2289-associated-type-bounds.html
	/// gets in this can be simplified.
	type Proof: Parameter + GetSessionNumber;

	/// Compute and check proofs of historical key owners.
	type KeyOwnerProofSystem: KeyOwnerProofSystem<
		(KeyTypeId, ValidatorId),
		Proof = Self::Proof,
		IdentificationTuple = Self::IdentificationTuple,
	>;

	/// An identification tuple type bound to `Parameter`.
	type IdentificationTuple: Parameter;

	/// Report an offence.
	type ReportOffence: ReportOffence<
		Self::AccountId,
		Self::IdentificationTuple,
		DoubleVoteOffence<Self::IdentificationTuple>,
	>;

	/// A type that converts the opaque hash type to exact one.
	type BlockHashConversion: Convert<Self::Hash, primitives::Hash>;
}

/// Origin for the parachains module.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Origin {
	/// It comes from a parachain.
	Parachain(ParaId),
}

/// An offence that is filed if the validator has submitted a double vote.
#[derive(RuntimeDebug)]
#[cfg_attr(feature = "std", derive(Clone, PartialEq, Eq))]
pub struct DoubleVoteOffence<Offender> {
	/// The current session index in which we report a validator.
	session_index: SessionIndex,
	/// The size of the validator set in current session/era.
	validator_set_count: u32,
	/// An offender that has submitted two conflicting votes.
	offender: Offender,
}

impl<Offender: Clone> Offence<Offender> for DoubleVoteOffence<Offender> {
	const ID: Kind = *b"para:double-vote";
	type TimeSlot = SessionIndex;

	fn offenders(&self) -> Vec<Offender> {
		vec![self.offender.clone()]
	}

	fn session_index(&self) -> SessionIndex {
		self.session_index
	}

	fn validator_set_count(&self) -> u32 {
		self.validator_set_count
	}

	fn time_slot(&self) -> Self::TimeSlot {
		self.session_index
	}

	fn slash_fraction(_offenders_count: u32, _validator_set_count: u32) -> Perbill {
		// Slash 100%.
		Perbill::from_percent(100)
	}
}

/// Total number of individual messages allowed in the parachain -> relay-chain message queue.
const MAX_QUEUE_COUNT: usize = 100;
/// Total size of messages allowed in the parachain -> relay-chain message queue before which no
/// further messages may be added to it. If it exceeds this then the queue may contain only a
/// single message.
const WATERMARK_QUEUE_SIZE: usize = 20000;

/// Metadata used to track previous parachain validation code that we keep in
/// the state.
#[derive(Default, Encode, Decode)]
#[cfg_attr(test, derive(Debug, Clone, PartialEq))]
pub struct ParaPastCodeMeta<N> {
	// Block numbers where the code was replaced. These can be used as indices
	// into the `PastCode` map along with the `ParaId` to fetch the code itself.
	upgrade_times: Vec<N>,
	// This tracks the highest pruned code-replacement, if any.
	last_pruned: Option<N>,
}

#[cfg_attr(test, derive(Debug, PartialEq))]
enum UseCodeAt<N> {
	// Use the current code.
	Current,
	// Use the code that was replaced at the given block number.
	ReplacedAt(N),
}

impl<N: Ord + Copy> ParaPastCodeMeta<N> {
	// note a replacement has occurred at a given block number.
	fn note_replacement(&mut self, at: N) {
		self.upgrade_times.insert(0, at)
	}

	// Yields the block number of the code that should be used for validating at
	// the given block number.
	//
	// a return value of `None` means that there is no code we are aware of that
	// should be used to validate at the given height.
	fn code_at(&self, at: N) -> Option<UseCodeAt<N>> {
		// The `PastCode` map stores the code which was replaced at `t`.
		let end_position = self.upgrade_times.iter().position(|&t| t < at);
		if let Some(end_position) = end_position {
			Some(if end_position != 0 {
				// `end_position` gives us the replacement time where the code used at `at`
				// was set. But that code has been replaced: `end_position - 1` yields
				// that index.
				UseCodeAt::ReplacedAt(self.upgrade_times[end_position - 1])
			} else {
				// the most recent tracked replacement is before `at`.
				// this means that the code put in place then (i.e. the current code)
				// is correct for validating at `at`.
				UseCodeAt::Current
			})
		} else {
			if self.last_pruned.as_ref().map_or(true, |&n| n < at) {
				// Our `last_pruned` is before `at`, so we still have the code!
				// but no code upgrade entries found before the `at` parameter.
				//
				// this means one of two things is true:
				// 1. there are no non-pruned upgrade logs. in this case use `Current`
				// 2. there are non-pruned upgrade logs all after `at`.
				//    in this case use the oldest upgrade log.
				Some(self.upgrade_times.last()
					.map(|n| UseCodeAt::ReplacedAt(*n))
					.unwrap_or(UseCodeAt::Current)
				)
			} else {
				// We don't have the code anymore.
				None
			}
		}
	}

	// The block at which the most recently tracked code change occurred.
	fn most_recent_change(&self) -> Option<N> {
		self.upgrade_times.first().map(|x| x.clone())
	}

	// prunes all code upgrade logs occurring at or before `max`.
	// note that code replaced at `x` is the code used to validate all blocks before
	// `x`. Thus, `max` should be outside of the slashing window when this is invoked.
	//
	// returns an iterator of block numbers at which code was replaced, where the replaced
	// code should be now pruned, in ascending order.
	fn prune_up_to(&'_ mut self, max: N) -> impl Iterator<Item=N> + '_ {
		match self.upgrade_times.iter().position(|&t| t <= max) {
			None => {
				// this is a no-op `drain` - desired because all
				// logged code upgrades occurred after `max`.
				self.upgrade_times.drain(self.upgrade_times.len()..).rev()
			}
			Some(pos) => {
				self.last_pruned = Some(self.upgrade_times[pos]);
				self.upgrade_times.drain(pos..).rev()
			}
		}
	}
}

decl_storage! {
	trait Store for Module<T: Trait> as Parachains
	{
		/// All authorities' keys at the moment.
		pub Authorities get(fn authorities): Vec<ValidatorId>;
		/// The active code of a currently-registered parachain.
		pub Code get(fn parachain_code): map hasher(twox_64_concat) ParaId => Option<ValidationCode>;
		/// Past code of parachains. The parachains themselves may not be registered anymore,
		/// but we also keep their code on-chain for the same amount of time as outdated code
		/// to assist with availability.
		PastCodeMeta get(fn past_code_meta): map hasher(twox_64_concat) ParaId => ParaPastCodeMeta<T::BlockNumber>;
		/// Actual past code, indicated by the parachain and the block number at which it
		/// became outdated.
		PastCode: map hasher(twox_64_concat) (ParaId, T::BlockNumber) => Option<ValidationCode>;
		/// Past code pruning, in order of priority.
		PastCodePruning get(fn past_code_pruning_tasks): Vec<(ParaId, T::BlockNumber)>;
		// The block number at which the planned code change is expected for a para.
		// The change will be applied after the first parablock for this ID included which executes
		// in the context of a relay chain block with a number >= `expected_at`.
		FutureCodeUpgrades get(fn code_upgrade_schedule): map hasher(twox_64_concat) ParaId => Option<T::BlockNumber>;
		// The actual future code of a para.
		FutureCode: map hasher(twox_64_concat) ParaId => ValidationCode;

		/// The heads of the parachains registered at present.
		pub Heads get(fn parachain_head): map hasher(twox_64_concat) ParaId => Option<HeadData>;
		/// Messages ready to be dispatched onto the relay chain. It is subject to
		/// `MAX_MESSAGE_COUNT` and `WATERMARK_MESSAGE_SIZE`.
		pub RelayDispatchQueue: map hasher(twox_64_concat) ParaId => Vec<UpwardMessage>;
		/// Size of the dispatch queues. Separated from actual data in order to avoid costly
		/// decoding when checking receipt validity. First item in tuple is the count of messages
		/// second if the total length (in bytes) of the message payloads.
		pub RelayDispatchQueueSize: map hasher(twox_64_concat) ParaId => (u32, u32);
		/// The ordered list of ParaIds that have a `RelayDispatchQueue` entry.
		NeedsDispatch: Vec<ParaId>;

		/// `Some` if the parachain heads get updated in this block, along with the parachain IDs
		/// that did update. Ordered in the same way as `registrar::Active` (i.e. by ParaId).
		///
		/// `None` if not yet updated.
		pub DidUpdate: Option<Vec<ParaId>>;
	}
	add_extra_genesis {
		config(authorities): Vec<ValidatorId>;
		build(|config| Module::<T>::initialize_authorities(&config.authorities))
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Parachain heads must be updated only once in the block.
		TooManyHeadUpdates,
		/// Too many parachain candidates.
		TooManyParaCandidates,
		/// Proposed heads must be ascending order by parachain ID without duplicate.
		HeadsOutOfOrder,
		/// Candidate is for an unregistered parachain.
		UnregisteredPara,
		/// Invalid collator.
		InvalidCollator,
		/// The message queue is full. Messages will be added when there is space.
		QueueFull,
		/// The message origin is invalid.
		InvalidMessageOrigin,
		/// No validator group for parachain.
		NoValidatorGroup,
		/// Not enough validity votes for candidate.
		NotEnoughValidityVotes,
		/// The number of attestations exceeds the number of authorities.
		VotesExceedsAuthorities,
		/// Attesting validator not on this chain's validation duty.
		WrongValidatorAttesting,
		/// Invalid signature from attester.
		InvalidSignature,
		/// Extra untagged validity votes along with candidate.
		UntaggedVotes,
		/// Wrong parent head for parachain receipt.
		ParentMismatch,
		/// Head data was too large.
		HeadDataTooLarge,
		/// New validation code was too large.
		ValidationCodeTooLarge,
		/// Disallowed code upgrade.
		DisallowedCodeUpgrade,
		/// Para does not have enough balance to pay fees.
		CannotPayFees,
		/// Unexpected relay-parent for a candidate receipt.
		UnexpectedRelayParent,
	}
}

decl_module! {
	/// Parachains module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		type Error = Error<T>;

		fn on_initialize(now: T::BlockNumber) -> Weight {
			<Self as Store>::DidUpdate::kill();

			Self::do_old_code_pruning(now);

			// TODO https://github.com/paritytech/polkadot/issues/977: set correctly
			0
		}

		fn on_finalize() {
			assert!(<Self as Store>::DidUpdate::exists(), "Parachain heads must be updated once in the block");
		}

		/// Provide candidate receipts for parachains, in ascending order by id.
		#[weight = (1_000_000_000, DispatchClass::Mandatory)]
		pub fn set_heads(origin, heads: Vec<AttestedCandidate>) -> DispatchResult {
			ensure_none(origin)?;
			ensure!(!<DidUpdate>::exists(), Error::<T>::TooManyHeadUpdates);

			let active_parachains = Self::active_parachains();

			let parachain_count = active_parachains.len();
			ensure!(heads.len() <= parachain_count, Error::<T>::TooManyParaCandidates);

			let mut proceeded = Vec::with_capacity(heads.len());

			let schedule = Self::global_validation_schedule();

			if !active_parachains.is_empty() {
				// perform integrity checks before writing to storage.
				{
					let mut last_id = None;

					let mut iter = active_parachains.iter();
					for head in &heads {
						let id = head.parachain_index();
						// proposed heads must be ascending order by parachain ID without duplicate.
						ensure!(
							last_id.as_ref().map_or(true, |x| x < &id),
							Error::<T>::HeadsOutOfOrder
						);

						// must be unknown since active parachains are always sorted.
						let (_, maybe_required_collator) = iter.find(|para| para.0 == id)
							.ok_or(Error::<T>::UnregisteredPara)?;

						if let Some((required_collator, _)) = maybe_required_collator {
							ensure!(required_collator == &head.candidate.collator, Error::<T>::InvalidCollator);
						}

						Self::check_upward_messages(
							id,
							&head.candidate.commitments.upward_messages,
							MAX_QUEUE_COUNT,
							WATERMARK_QUEUE_SIZE,
						)?;

						let id = head.parachain_index();
						proceeded.push(id);
						last_id = Some(id);
					}
				}

				let para_blocks = Self::check_candidates(
					&schedule,
					&heads,
					&active_parachains,
				)?;

				<attestations::Module<T>>::note_included(&heads, para_blocks);

				Self::update_routing(
					&heads,
				);

				// note: we dispatch new messages _after_ the call to `check_candidates`
				// which deducts any fees. if that were not the case, an upward message
				// could be dispatched and spend money that invalidated a candidate.
				Self::dispatch_upward_messages(
					MAX_QUEUE_COUNT,
					WATERMARK_QUEUE_SIZE,
					Self::dispatch_message,
				);
			}

			DidUpdate::put(proceeded);

			Ok(())
		}

		/// Provide a proof that some validator has commited a double-vote.
		///
		/// The weight is 0; in order to avoid DoS a `SignedExtension` validation
		/// is implemented.
		#[weight = 0]
		pub fn report_double_vote(
			origin,
			report: DoubleVoteReport<
				<T::KeyOwnerProofSystem as KeyOwnerProofSystem<(KeyTypeId, ValidatorId)>>::Proof,
			>,
		) -> DispatchResult {
			let reporter = ensure_signed(origin)?;

			let validators = <session::Module<T>>::validators();
			let validator_set_count = validators.len() as u32;

			let session_index = report.proof.session();
			let DoubleVoteReport { identity, proof, .. } = report;

			// We have already checked this proof in `SignedExtension`, but we need
			// this here to get the full identification of the offender.
			let offender = T::KeyOwnerProofSystem::check_proof(
					(PARACHAIN_KEY_TYPE_ID, identity),
					proof,
				).ok_or("Invalid/outdated key ownership proof.")?;

			let offence = DoubleVoteOffence {
				session_index,
				validator_set_count,
				offender,
			};

			// Checks if this is actually a double vote are
			// implemented in `ValidateDoubleVoteReports::validete`.
			T::ReportOffence::report_offence(vec![reporter], offence)
				.map_err(|_| "Failed to report offence")?;

			Ok(())
		}
	}
}

fn majority_of(list_len: usize) -> usize {
	list_len / 2 + list_len % 2
}

fn localized_payload(
	statement: Statement,
	signing_context: &SigningContext,
) -> Vec<u8> {
	let mut encoded = statement.encode();
	signing_context.using_encoded(|s| encoded.extend(s));
	encoded
}

impl<T: Trait> Module<T> {
	/// Initialize the state of a new parachain/parathread.
	pub fn initialize_para(
		id: ParaId,
		code: ValidationCode,
		initial_head_data: HeadData,
	) {
		<Code>::insert(id, code);
		<Heads>::insert(id, initial_head_data);
	}

	/// Cleanup all storage related to a para. Some pieces of data may remain
	/// available in the on-chain state.
	pub fn cleanup_para(
		id: ParaId,
	) {
		let code = <Code>::take(id);
		<Heads>::remove(id);

		// clean up from all code-upgrade maps.
		// we don't clean up the meta or planned-code maps as that's handled
		// by the pruning process.
		if let Some(_planned_future_at) = <Self as Store>::FutureCodeUpgrades::take(&id) {
			<Self as Store>::FutureCode::remove(&id);
		}

		if let Some(code) = code {
			Self::note_past_code(id, <system::Module<T>>::block_number(), code);
		}
	}

	// note replacement of the code of para with given `id`, which occured in the
	// context of the given relay-chain block number. provide the replaced code.
	//
	// `at` for para-triggered replacement is the block number of the relay-chain
	// block in whose context the parablock was executed
	// (i.e. number of `relay_parent` in the receipt)
	fn note_past_code(id: ParaId, at: T::BlockNumber, old_code: ValidationCode) {
		<Self as Store>::PastCodeMeta::mutate(&id, |past_meta| {
			past_meta.note_replacement(at);
		});

		<Self as Store>::PastCode::insert(&(id, at), old_code);

		// Schedule pruning for this past-code to be removed as soon as it
		// exits the slashing window.
		<Self as Store>::PastCodePruning::mutate(|pruning| {
			let insert_idx = pruning.binary_search_by_key(&at, |&(_, b)| b)
				.unwrap_or_else(|idx| idx);
			pruning.insert(insert_idx, (id, at));
		})
	}

	// does old code pruning.
	fn do_old_code_pruning(now: T::BlockNumber) {
		let slash_period = T::SlashPeriod::get();
		if now <= slash_period { return }

		// The height of any changes we no longer should keep around.
		let pruning_height = now - (slash_period + One::one());

		<Self as Store>::PastCodePruning::mutate(|pruning_tasks: &mut Vec<(_, T::BlockNumber)>| {
			let pruning_tasks_to_do = {
				// find all past code that has just exited the pruning window.
				let up_to_idx = pruning_tasks.iter()
					.take_while(|&(_, at)| at <= &pruning_height)
					.count();
				pruning_tasks.drain(..up_to_idx)
			};

			for (para_id, _) in pruning_tasks_to_do {
				let full_deactivate = <Self as Store>::PastCodeMeta::mutate(&para_id, |meta| {
					for pruned_repl_at in meta.prune_up_to(pruning_height) {
						<Self as Store>::PastCode::remove(&(para_id, pruned_repl_at));
					}

					meta.most_recent_change().is_none() && Self::parachain_head(&para_id).is_none()
				});

				// This parachain has been removed and now the vestigial code
				// has been removed from the state. clean up meta as well.
				if full_deactivate {
					<Self as Store>::PastCodeMeta::remove(&para_id);
				}
			}
		});
	}

	// Performs a code upgrade of a parachain.
	fn do_code_upgrade(id: ParaId, at: T::BlockNumber, new_code: &ValidationCode) {
		let old_code = Self::parachain_code(&id).unwrap_or_default();
		Code::insert(&id, new_code);

		Self::note_past_code(id, at, old_code);
	}

	/// Get a `SigningContext` with a current `SessionIndex` and parent hash.
	pub fn signing_context() -> SigningContext {
		let session_index = <session::Module<T>>::current_index();
		let parent_hash = <system::Module<T>>::parent_hash();

		SigningContext {
			session_index,
			parent_hash: T::BlockHashConversion::convert(parent_hash),
		}
	}

	/// Submit a double vote report.
	pub fn submit_double_vote_report(
		report: DoubleVoteReport<T::Proof>,
	) -> Option<()> {
		Signer::<T, T::AuthorityId>::all_accounts()
			.send_signed_transaction(
				move |_account| {
					Call::report_double_vote(report.clone())
				}
			)
			.iter()
			.find_map(|(_, res)| res.ok().map(|_| ()))
	}

	/// Dispatch some messages from a parachain.
	fn dispatch_message(
		id: ParaId,
		origin: ParachainDispatchOrigin,
		data: &[u8],
	) {
		if let Ok(message_call) = <T as Trait>::Call::decode(&mut &data[..]) {
			let origin: <T as Trait>::Origin = match origin {
				ParachainDispatchOrigin::Signed =>
					system::RawOrigin::Signed(id.into_account()).into(),
				ParachainDispatchOrigin::Parachain =>
					Origin::Parachain(id).into(),
				ParachainDispatchOrigin::Root =>
					system::RawOrigin::Root.into(),
			};
			let _ok = message_call.dispatch(origin).is_ok();
			// Not much to do with the result as it is. It's up to the parachain to ensure that the
			// message makes sense.
		}
	}

	/// Ensure all is well with the upward messages.
	fn check_upward_messages(
		id: ParaId,
		upward_messages: &[UpwardMessage],
		max_queue_count: usize,
		watermark_queue_size: usize,
	) -> DispatchResult {
		// Either there are no more messages to add...
		if !upward_messages.is_empty() {
			let (count, size) = <RelayDispatchQueueSize>::get(id);
			ensure!(
				// ...or we are appending one message onto an empty queue...
				upward_messages.len() + count as usize == 1
				// ...or...
				|| (
				// ...the total messages in the queue ends up being no greater than the
				// limit...
					upward_messages.len() + count as usize <= max_queue_count
				&&
					// ...and the total size of the payloads in the queue ends up being no
					// greater than the limit.
					upward_messages.iter()
						.fold(size as usize, |a, x| a + x.data.len())
					<= watermark_queue_size
				),
				Error::<T>::QueueFull
			);
			if !id.is_system() {
				for m in upward_messages.iter() {
					ensure!(m.origin != ParachainDispatchOrigin::Root, Error::<T>::InvalidMessageOrigin);
				}
			}
		}
		Ok(())
	}

	/// Update routing information from the parachain heads. This queues upwards
	/// messages to the relay chain as well.
	fn update_routing(
		heads: &[AttestedCandidate],
	) {
		// we sort them in order to provide a fast lookup to ensure we can avoid duplicates in the
		// needs_dispatch queue.
		let mut ordered_needs_dispatch = NeedsDispatch::get();

		for head in heads.iter() {
			let id = head.parachain_index();
			Heads::insert(id, &head.candidate.head_data);

			// Queue up upwards messages (from parachains to relay chain).
			Self::queue_upward_messages(
				id,
				&head.candidate.commitments.upward_messages,
				&mut ordered_needs_dispatch,
			);
		}

		NeedsDispatch::put(ordered_needs_dispatch);
	}

	/// Place any new upward messages into our queue for later dispatch.
	///
	/// `ordered_needs_dispatch` is mutated to ensure it reflects the new value of
	/// `RelayDispatchQueueSize`. It is up to the caller to guarantee that it gets written into
	/// storage after this call.
	fn queue_upward_messages(
		id: ParaId,
		upward_messages: &[UpwardMessage],
		ordered_needs_dispatch: &mut Vec<ParaId>,
	) {
		if !upward_messages.is_empty() {
			RelayDispatchQueueSize::mutate(id, |&mut(ref mut count, ref mut len)| {
				*count += upward_messages.len() as u32;
				*len += upward_messages.iter()
					.fold(0, |a, x| a + x.data.len()) as u32;
			});

			upward_messages.iter().for_each(|m| RelayDispatchQueue::append(id, m));

			if let Err(i) = ordered_needs_dispatch.binary_search(&id) {
				// same.
				ordered_needs_dispatch.insert(i, id);
			} else {
				sp_runtime::print("ordered_needs_dispatch contains id?!");
			}
		}
	}

	/// Simple FIFO dispatcher. This must be called after parachain fees are checked,
	/// as dispatched messages may spend parachain funds.
	fn dispatch_upward_messages(
		max_queue_count: usize,
		watermark_queue_size: usize,
		mut dispatch_message: impl FnMut(ParaId, ParachainDispatchOrigin, &[u8]),
	) {
		let queueds = NeedsDispatch::get();
		let mut drained_count = 0usize;
		let mut dispatched_count = 0usize;
		let mut dispatched_size = 0usize;
		for id in queueds.iter() {
			drained_count += 1;

			let (count, size) = <RelayDispatchQueueSize>::get(id);
			let count = count as usize;
			let size = size as usize;
			if dispatched_count == 0 || (
				dispatched_count + count <= max_queue_count
					&& dispatched_size + size <= watermark_queue_size
			) {
				if count > 0 {
					// still dispatching messages...
					RelayDispatchQueueSize::remove(id);
					let messages = RelayDispatchQueue::take(id);
					for UpwardMessage { origin, data } in messages.into_iter() {
						dispatch_message(*id, origin, &data);
					}
					dispatched_count += count;
					dispatched_size += size;
					if dispatched_count >= max_queue_count
						|| dispatched_size >= watermark_queue_size
					{
						break
					}
				}
			}
		}
		NeedsDispatch::put(&queueds[drained_count..]);
	}

	/// Calculate the current block's duty roster using system's random seed.
	/// Returns the duty roster along with the random seed.
	pub fn calculate_duty_roster() -> (DutyRoster, [u8; 32]) {
		let parachains = Self::active_parachains();
		let parachain_count = parachains.len();

		// TODO: use decode length. substrate #2794
		let validator_count = Self::authorities().len();
		let validators_per_parachain =
			if parachain_count == 0 {
				0
			} else {
				(validator_count - 1) / parachain_count
			};

		let mut roles_val = (0..validator_count).map(|i| match i {
			i if i < parachain_count * validators_per_parachain => {
				let idx = i / validators_per_parachain;
				Chain::Parachain(parachains[idx].0.clone())
			}
			_ => Chain::Relay,
		}).collect::<Vec<_>>();

		let mut seed = {
			let phrase = b"validator_role_pairs";
			let seed = T::Randomness::random(&phrase[..]);
			let seed_len = seed.as_ref().len();
			let needed_bytes = validator_count * 4;

			// hash only the needed bits of the random seed.
			// if earlier bits are influencable, they will not factor into
			// the seed used here.
			let seed_off = if needed_bytes >= seed_len {
				0
			} else {
				seed_len - needed_bytes
			};

			BlakeTwo256::hash(&seed.as_ref()[seed_off..])
		};

		let orig_seed = seed.clone().to_fixed_bytes();

		// shuffle
		for i in 0..(validator_count.saturating_sub(1)) {
			// 4 bytes of entropy used per cycle, 32 bytes entropy per hash
			let offset = (i * 4 % 32) as usize;

			// number of roles remaining to select from.
			let remaining = sp_std::cmp::max(1, (validator_count - i) as usize);

			// 8 32-bit ints per 256-bit seed.
			let val_index = u32::decode(&mut &seed[offset..offset + 4])
				.expect("using 4 bytes for a 32-bit quantity") as usize % remaining;

			if offset == 28 {
				// into the last 4 bytes - rehash to gather new entropy
				seed = BlakeTwo256::hash(seed.as_ref());
			}

			// exchange last item with randomly chosen first.
			roles_val.swap(remaining - 1, val_index);
		}

		(DutyRoster { validator_duty: roles_val, }, orig_seed)
	}

	/// Get the global validation schedule for all parachains.
	pub fn global_validation_schedule() -> GlobalValidationSchedule {
		let now = <system::Module<T>>::block_number();
		GlobalValidationSchedule {
			max_code_size: T::MaxCodeSize::get(),
			max_head_data_size: T::MaxHeadDataSize::get(),
			block_number: T::BlockNumberConversion::convert(if now.is_zero() {
				now
			} else {
				// parablocks included in this block will execute in the context
				// of the current block's parent.
				now - One::one()
			}),
		}
	}

	/// Get the local validation schedule for a particular parachain.
	pub fn local_validation_data(id: &ParaId, perceived_height: T::BlockNumber) -> Option<LocalValidationData> {
		if perceived_height + One::one() != <system::Module<T>>::block_number() {
			// sanity-check - no non-direct-parent blocks allowed at the moment.
			return None
		}

		let code_upgrade_allowed: Option<BlockNumber> = (|| {
			match T::Registrar::para_info(*id)?.scheduling {
				Scheduling::Always => {},
				Scheduling::Dynamic => return None, // parathreads can't upgrade code.
			}

			// if perceived-height were not the parent of `now`, then this should
			// not be drawn from current-runtime configuration. however the sanity-check
			// above prevents that.
			let min_upgrade_frequency = T::ValidationUpgradeFrequency::get();
			let upgrade_delay = T::ValidationUpgradeDelay::get();

			let no_planned = Self::code_upgrade_schedule(id)
				.map_or(true, |expected: T::BlockNumber| expected <= perceived_height);

			let can_upgrade_code = no_planned &&
				Self::past_code_meta(id).most_recent_change()
					.map_or(true, |at| at + min_upgrade_frequency < perceived_height);

			if can_upgrade_code {
				let applied_at = perceived_height + upgrade_delay;
				Some(T::BlockNumberConversion::convert(applied_at))
			} else {
				None
			}
		})();

		Self::parachain_head(id).map(|parent_head| LocalValidationData {
			parent_head,
			balance: T::ParachainCurrency::free_balance(*id),
			code_upgrade_allowed,
		})
	}

	/// Get the local validation data for a particular parent w.r.t. the current
	/// block height.
	pub fn current_local_validation_data(id: &ParaId) -> Option<LocalValidationData> {
		let now: T::BlockNumber = <system::Module<T>>::block_number();
		if now >= One::one() {
			Self::local_validation_data(id, now - One::one())
		} else {
			None
		}
	}

	/// Fetch the code used for verifying a parachain at a particular height.
	pub fn parachain_code_at(id: &ParaId, at: T::BlockNumber) -> Option<ValidationCode> {
		// note - we don't check that the parachain is currently registered
		// as this might be a deregistered parachain whose old code should still
		// stick around on-chain for some time.
		Self::past_code_meta(id).code_at(at).and_then(|to_use| match to_use {
			UseCodeAt::Current => Self::parachain_code(id),
			UseCodeAt::ReplacedAt(replaced_at) =>
				<Self as Store>::PastCode::get(&(*id, replaced_at)),
		})
	}

	/// Get the currently active set of parachains.
	pub fn active_parachains() -> Vec<(ParaId, Option<(CollatorId, Retriable)>)> {
		T::ActiveParachains::active_paras()
	}

	// check the attestations on these candidates. The candidates should have been checked
	// that each candidates' chain ID is valid.
	fn check_candidates(
		schedule: &GlobalValidationSchedule,
		attested_candidates: &[AttestedCandidate],
		active_parachains: &[(ParaId, Option<(CollatorId, Retriable)>)]
	) -> sp_std::result::Result<IncludedBlocks<T>, sp_runtime::DispatchError>
	{
		// returns groups of slices that have the same chain ID.
		// assumes the inner slice is sorted by id.
		struct GroupedDutyIter<'a> {
			next_idx: usize,
			inner: &'a [(usize, ParaId)],
		}

		impl<'a> GroupedDutyIter<'a> {
			fn new(inner: &'a [(usize, ParaId)]) -> Self {
				GroupedDutyIter { next_idx: 0, inner }
			}

			fn group_for(&mut self, wanted_id: ParaId) -> Option<&'a [(usize, ParaId)]> {
				while let Some((id, keys)) = self.next() {
					if wanted_id == id {
						return Some(keys)
					}
				}

				None
			}
		}

		impl<'a> Iterator for GroupedDutyIter<'a> {
			type Item = (ParaId, &'a [(usize, ParaId)]);

			fn next(&mut self) -> Option<Self::Item> {
				if self.next_idx == self.inner.len() { return None }
				let start_idx = self.next_idx;
				self.next_idx += 1;
				let start_id = self.inner[start_idx].1;

				while self.inner.get(self.next_idx).map_or(false, |&(_, ref id)| id == &start_id) {
					self.next_idx += 1;
				}

				Some((start_id, &self.inner[start_idx..self.next_idx]))
			}
		}

		let authorities = Self::authorities();
		let (duty_roster, random_seed) = Self::calculate_duty_roster();

		// convert a duty roster, which is originally a Vec<Chain>, where each
		// item corresponds to the same position in the session keys, into
		// a list containing (index, parachain duty) where indices are into the session keys.
		// this list is sorted ascending by parachain duty, just like the
		// parachain candidates are.
		let make_sorted_duties = |duty: &[Chain]| {
			let mut sorted_duties = Vec::with_capacity(duty.len());
			for (val_idx, duty) in duty.iter().enumerate() {
				let id = match duty {
					Chain::Relay => continue,
					Chain::Parachain(id) => id,
				};

				let idx = sorted_duties.binary_search_by_key(&id, |&(_, ref id)| id)
					.unwrap_or_else(|idx| idx);

				sorted_duties.insert(idx, (val_idx, *id));
			}

			sorted_duties
		};

		// computes the omitted validation data for a particular parachain.
		//
		// pass the perceived relay chain height of the para-block. This is the block number of
		// `abridged.relay_parent`.
		let full_candidate = |
			abridged: &AbridgedCandidateReceipt,
			perceived_height: T::BlockNumber,
		|
			-> sp_std::result::Result<CandidateReceipt, sp_runtime::DispatchError>
		{
			let para_id = abridged.parachain_index;
			let local_validation = Self::local_validation_data(&para_id, perceived_height)
				.ok_or(Error::<T>::ParentMismatch)?;

			let omitted = OmittedValidationData {
				global_validation: schedule.clone(),
				local_validation,
			};

			Ok(abridged.clone().complete(omitted))
		};

		let sorted_validators = make_sorted_duties(&duty_roster.validator_duty);

		let relay_height_now = <system::Module<T>>::block_number();
		let parent_hash = <system::Module<T>>::parent_hash();
		let signing_context = Self::signing_context();
		let localized_payload = |statement: Statement| localized_payload(statement, &signing_context);
		let code_upgrade_delay = T::ValidationUpgradeDelay::get();

		let mut validator_groups = GroupedDutyIter::new(&sorted_validators[..]);

		let mut para_block_hashes = Vec::new();

		for candidate in attested_candidates {
			let para_id = candidate.parachain_index();
			let validator_group = validator_groups.group_for(para_id)
				.ok_or(Error::<T>::NoValidatorGroup)?;

			// NOTE: when changing this to allow older blocks,
			// care must be taken in the availability store pruning to ensure that
			// data is stored correctly. A block containing a candidate C can be
			// orphaned before a block containing C is finalized. Care must be taken
			// not to prune the data for C simply because an orphaned block contained
			// it.

			ensure!(
				candidate.candidate().relay_parent.as_ref() == parent_hash.as_ref(),
				Error::<T>::UnexpectedRelayParent,
			);

			// Since we only allow execution in context of parent hash.
			let perceived_relay_block_height = <system::Module<T>>::block_number() - One::one();

			ensure!(
				candidate.validity_votes.len() >= majority_of(validator_group.len()),
				Error::<T>::NotEnoughValidityVotes,
			);

			ensure!(
				candidate.validity_votes.len() <= authorities.len(),
				Error::<T>::VotesExceedsAuthorities,
			);

			ensure!(
				schedule.max_head_data_size as usize >= candidate.candidate().head_data.0.len(),
				Error::<T>::HeadDataTooLarge,
			);

			let full_candidate = full_candidate(
				candidate.candidate(),
				perceived_relay_block_height,
			)?;

			// apply any scheduled code upgrade.
			if let Some(expected_at) = Self::code_upgrade_schedule(&para_id) {
				if expected_at <= perceived_relay_block_height {
					let new_code = FutureCode::take(&para_id);
					<Self as Store>::FutureCodeUpgrades::remove(&para_id);

					Self::do_code_upgrade(para_id, perceived_relay_block_height, &new_code);
				}
			}

			if let Some(ref new_code) = full_candidate.commitments.new_validation_code {
				ensure!(
					full_candidate.local_validation.code_upgrade_allowed.is_some(),
					Error::<T>::DisallowedCodeUpgrade,
				);
				ensure!(
					schedule.max_code_size >= new_code.0.len() as u32,
					Error::<T>::ValidationCodeTooLarge,
				);

				if code_upgrade_delay.is_zero() {
					Self::do_code_upgrade(para_id, perceived_relay_block_height, new_code);
				} else {
					<Self as Store>::FutureCodeUpgrades::insert(
						&para_id,
						&(perceived_relay_block_height + code_upgrade_delay),
					);
					FutureCode::insert(
						&para_id,
						new_code,
					);
				}
			}

			let fees = full_candidate.commitments.fees;

			ensure!(
				full_candidate.local_validation.balance >= full_candidate.commitments.fees,
				Error::<T>::CannotPayFees,
			);

			T::ParachainCurrency::deduct(para_id, fees)?;

			let candidate_hash = candidate.candidate().hash();
			let mut encoded_implicit = None;
			let mut encoded_explicit = None;

			let mut expected_votes_len = 0;
			for (vote_index, (auth_index, _)) in candidate.validator_indices
				.iter()
				.enumerate()
				.filter(|(_, bit)| **bit)
				.enumerate()
			{
				let validity_attestation = match candidate.validity_votes.get(vote_index) {
					None => Err(Error::<T>::NotEnoughValidityVotes)?,
					Some(v) => {
						expected_votes_len = vote_index + 1;
						v
					}
				};

				if validator_group.iter().find(|&(idx, _)| *idx == auth_index).is_none() {
					Err(Error::<T>::WrongValidatorAttesting)?
				}

				let (payload, sig) = match validity_attestation {
					ValidityAttestation::Implicit(sig) => {
						let payload = encoded_implicit.get_or_insert_with(|| localized_payload(
							Statement::Candidate(candidate_hash),
						));

						(payload, sig)
					}
					ValidityAttestation::Explicit(sig) => {
						let payload = encoded_explicit.get_or_insert_with(|| localized_payload(
							Statement::Valid(candidate_hash),
						));

						(payload, sig)
					}
				};

				ensure!(
					sig.verify(&payload[..], &authorities[auth_index]),
					Error::<T>::InvalidSignature,
				);
			}

			ensure!(
				candidate.validity_votes.len() == expected_votes_len,
				Error::<T>::UntaggedVotes
			);

			para_block_hashes.push(candidate_hash);
		}

		Ok(IncludedBlocks {
			actual_number: relay_height_now,
			session: <session::Module<T>>::current_index(),
			random_seed,
			active_parachains: active_parachains.iter().map(|x| x.0).collect(),
			para_blocks: para_block_hashes,
		})
	}

	fn initialize_authorities(authorities: &[ValidatorId]) {
		if !authorities.is_empty() {
			assert!(Authorities::get().is_empty(), "Authorities are already initialized!");
			Authorities::put(authorities);
		}
	}

/*
	// TODO: Consider integrating if needed. (https://github.com/paritytech/polkadot/issues/223)
	/// Extract the parachain heads from the block.
	pub fn parachain_heads(&self) -> &[CandidateReceipt] {
		let x = self.inner.extrinsics.get(PARACHAINS_SET_POSITION as usize).and_then(|xt| match xt.function {
			Call::Parachains(ParachainsCall::set_heads(ref x)) => Some(&x[..]),
			_ => None
		});

		match x {
			Some(x) => x,
			None => panic!("Invalid polkadot block asserted at {:?}", self.file_line),
		}
	}
*/
}

impl<T: Trait> sp_runtime::BoundToRuntimeAppPublic for Module<T> {
	type Public = ValidatorId;
}

impl<T: Trait> session::OneSessionHandler<T::AccountId> for Module<T> {
	type Key = ValidatorId;

	fn on_genesis_session<'a, I: 'a>(validators: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		Self::initialize_authorities(&validators.map(|(_, key)| key).collect::<Vec<_>>());
	}

	fn on_new_session<'a, I: 'a>(changed: bool, validators: I, _queued: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		if changed {
			<Self as Store>::Authorities::put(validators.map(|(_, key)| key).collect::<Vec<_>>());
		}
	}

	fn on_disabled(_i: usize) { }
}

pub type InherentType = Vec<AttestedCandidate>;

impl<T: Trait> ProvideInherent for Module<T> {
	type Call = Call<T>;
	type Error = MakeFatalError<inherents::Error>;
	const INHERENT_IDENTIFIER: InherentIdentifier = NEW_HEADS_IDENTIFIER;

	fn create_inherent(data: &InherentData) -> Option<Self::Call> {
		let data = data.get_data::<InherentType>(&NEW_HEADS_IDENTIFIER)
			.expect("Parachain heads could not be decoded.")
			.expect("No parachain heads found in inherent data.");

		Some(Call::set_heads(data))
	}
}

/// Ensure that the origin `o` represents a parachain.
/// Returns `Ok` with the parachain ID that effected the extrinsic or an `Err` otherwise.
pub fn ensure_parachain<OuterOrigin>(o: OuterOrigin) -> result::Result<ParaId, BadOrigin>
	where OuterOrigin: Into<result::Result<Origin, OuterOrigin>>
{
	match o.into() {
		Ok(Origin::Parachain(id)) => Ok(id),
		_ => Err(BadOrigin),
	}
}


/// Ensure that double vote reports are only processed if valid.
#[derive(Encode, Decode, Clone, Eq, PartialEq)]
pub struct ValidateDoubleVoteReports<T>(sp_std::marker::PhantomData<T>);

impl<T> sp_std::fmt::Debug for ValidateDoubleVoteReports<T> where
{
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(f, "ValidateDoubleVoteReports<T>")
	}
}

impl<T> ValidateDoubleVoteReports<T> {
	/// Create a new `ValidateDoubleVoteReports` struct.
	pub fn new() -> Self {
		ValidateDoubleVoteReports(sp_std::marker::PhantomData)
	}
}

/// Custom validity error used while validating double vote reports.
#[derive(RuntimeDebug)]
#[repr(u8)]
pub enum DoubleVoteValidityError {
	/// The authority being reported is not in the authority set.
	NotAnAuthority = 0,

	/// Failed to convert offender's `FullIdentificationOf`.
	FailedToConvertId = 1,

	/// The signature on one or both of the statements in the report is wrong.
	InvalidSignature = 2,

	/// The two statements in the report are not conflicting.
	NotDoubleVote = 3,

	/// Invalid report. Indicates that statement doesn't match the attestation on one of the votes.
	InvalidReport = 4,

	/// The proof provided in the report is not valid.
	InvalidProof = 5,
}

impl<T: Trait + Send + Sync> SignedExtension for ValidateDoubleVoteReports<T> where
	<T as system::Trait>::Call: IsSubType<Module<T>, T>
{
	const IDENTIFIER: &'static str = "ValidateDoubleVoteReports";
	type AccountId = T::AccountId;
	type Call = <T as system::Trait>::Call;
	type AdditionalSigned = ();
	type Pre = ();

	fn additional_signed(&self)
		-> sp_std::result::Result<Self::AdditionalSigned, TransactionValidityError>
	{
		Ok(())
	}

	fn validate(
		&self,
		_who: &Self::AccountId,
		call: &Self::Call,
		_info: &DispatchInfoOf<Self::Call>,
		_len: usize,
	) -> TransactionValidity {
		let r = ValidTransaction::default();

		if let Some(local_call) = call.is_sub_type() {
			if let Call::report_double_vote(report) = local_call {
				let validators = <session::Module<T>>::validators();

				let expected_session = report.signing_context.session_index;
				let session = report.proof.session();

				if session != expected_session {
					return Err(InvalidTransaction::BadProof.into());
				}

				let authorities = Module::<T>::authorities();
				let offender_idx = match authorities.iter().position(|a| *a == report.identity) {
					Some(idx) => idx,
					None => return Err(InvalidTransaction::Custom(
						DoubleVoteValidityError::NotAnAuthority as u8).into()
					),
				};

				if T::FullIdentificationOf::convert(validators[offender_idx].clone()).is_none() {
					return Err(InvalidTransaction::Custom(
						DoubleVoteValidityError::FailedToConvertId as u8).into()
					);
				}

				report
					.verify::<T>()
					.map_err(|e| TransactionValidityError::from(InvalidTransaction::Custom(e as u8)))?;
			}
		}

		Ok(r)
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use super::Call as ParachainsCall;
	use bitvec::{bitvec, vec::BitVec};
	use sp_io::TestExternalities;
	use sp_core::{H256, Blake2Hasher, sr25519};
	use sp_trie::NodeCodec;
	use sp_runtime::{
		impl_opaque_keys,
		Perbill, curve::PiecewiseLinear,
		traits::{
			BlakeTwo256, IdentityLookup, SaturatedConversion,
			OpaqueKeys, Extrinsic as ExtrinsicT,
		},
		testing::TestXt,
	};
	use primitives::{
		parachain::{
			CandidateReceipt, ValidityAttestation, ValidatorId, Info as ParaInfo,
			Scheduling, CandidateCommitments,
		},
		BlockNumber,
		Header,
	};
	use keyring::Sr25519Keyring;
	use frame_support::{
		impl_outer_origin, impl_outer_dispatch, assert_ok, assert_err, parameter_types,
		traits::{OnInitialize, OnFinalize},
		weights::DispatchInfo,
	};
	use crate::parachains;
	use crate::registrar;
	use crate::slots;
	use session::{SessionHandler, SessionManager};
	use staking::EraIndex;

	// result of <NodeCodec<Blake2Hasher> as trie_db::NodeCodec<Blake2Hasher>>::hashed_null_node()
	const EMPTY_TRIE_ROOT: [u8; 32] = [
		3, 23, 10, 46, 117, 151, 183, 183, 227, 216, 76, 5, 57, 29, 19, 154,
		98, 177, 87, 231, 135, 134, 216, 192, 130, 242, 157, 207, 76, 17, 19, 20
	];

	impl_outer_origin! {
		pub enum Origin for Test {
			parachains
		}
	}

	impl_outer_dispatch! {
		pub enum Call for Test where origin: Origin {
			parachains::Parachains,
			staking::Staking,
		}
	}

	impl_opaque_keys! {
		pub struct TestSessionKeys {
			pub parachain_validator: super::Module<Test>,
		}
	}

	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub const MaximumBlockWeight: Weight = 4 * 1024 * 1024;
		pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	}

	impl system::Trait for Test {
		type BaseCallFilter = ();
		type Origin = Origin;
		type Call = Call;
		type Index = u64;
		type BlockNumber = BlockNumber;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type DbWeight = ();
		type BlockExecutionWeight = ();
		type ExtrinsicBaseWeight = ();
		type MaximumExtrinsicWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
		type ModuleToIndex = ();
		type AccountData = balances::AccountData<u128>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
	}

	impl<C> system::offchain::SendTransactionTypes<C> for Test where
		Call: From<C>,
	{
		type OverarchingCall = Call;
		type Extrinsic = TestXt<Call, ()>;
	}

	parameter_types! {
		pub const Period: BlockNumber = 1;
		pub const Offset: BlockNumber = 0;
		pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(17);
	}

	/// Custom `SessionHandler` since we use `TestSessionKeys` as `Keys`.
	pub struct TestSessionHandler;
	impl<AId> SessionHandler<AId> for TestSessionHandler {
		const KEY_TYPE_IDS: &'static [KeyTypeId] = &[PARACHAIN_KEY_TYPE_ID];

		fn on_genesis_session<Ks: OpaqueKeys>(_: &[(AId, Ks)]) {}

		fn on_new_session<Ks: OpaqueKeys>(_: bool, _: &[(AId, Ks)], _: &[(AId, Ks)]) {}

		fn on_before_session_ending() {}

		fn on_disabled(_: usize) {}
	}

	impl session::Trait for Test {
		type Event = ();
		type ValidatorId = u64;
		type ValidatorIdOf = staking::StashOf<Self>;
		type ShouldEndSession = session::PeriodicSessions<Period, Offset>;
		type NextSessionRotation = session::PeriodicSessions<Period, Offset>;
		type SessionManager = session::historical::NoteHistoricalRoot<Self, Staking>;
		type SessionHandler = TestSessionHandler;
		type Keys = TestSessionKeys;
		type DisabledValidatorsThreshold = DisabledValidatorsThreshold;
	}

	impl session::historical::Trait for Test {
		type FullIdentification = staking::Exposure<u64, Balance>;
		type FullIdentificationOf = staking::ExposureOf<Self>;
	}

	parameter_types! {
		pub const MinimumPeriod: u64 = 3;
	}
	impl timestamp::Trait for Test {
		type Moment = u64;
		type OnTimestampSet = ();
		type MinimumPeriod = MinimumPeriod;
	}

	mod time {
		use primitives::{Moment, BlockNumber};
		pub const MILLISECS_PER_BLOCK: Moment = 6000;
		pub const EPOCH_DURATION_IN_BLOCKS: BlockNumber = 1 * HOURS;
		// These time units are defined in number of blocks.
		const MINUTES: BlockNumber = 60_000 / (MILLISECS_PER_BLOCK as BlockNumber);
		const HOURS: BlockNumber = MINUTES * 60;
	}
	parameter_types! {
		pub const EpochDuration: BlockNumber = time::EPOCH_DURATION_IN_BLOCKS;
		pub const ExpectedBlockTime: u64 = time::MILLISECS_PER_BLOCK;
	}

	impl babe::Trait for Test {
		type EpochDuration = EpochDuration;
		type ExpectedBlockTime = ExpectedBlockTime;

		// session module is the trigger
		type EpochChangeTrigger = babe::ExternalTrigger;
	}

	parameter_types! {
		pub const ExistentialDeposit: Balance = 1;
	}

	impl balances::Trait for Test {
		type Balance = u128;
		type DustRemoval = ();
		type Event = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
	}

	pallet_staking_reward_curve::build! {
		const REWARD_CURVE: PiecewiseLinear<'static> = curve!(
			min_inflation: 0_025_000u64,
			max_inflation: 0_100_000,
			ideal_stake: 0_500_000,
			falloff: 0_050_000,
			max_piece_count: 40,
			test_precision: 0_005_000,
		);
	}

	parameter_types! {
		pub const SessionsPerEra: sp_staking::SessionIndex = 3;
		pub const BondingDuration: staking::EraIndex = 3;
		pub const SlashDeferDuration: staking::EraIndex = 0;
		pub const AttestationPeriod: BlockNumber = 100;
		pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
		pub const MaxNominatorRewardedPerValidator: u32 = 64;
		pub const ElectionLookahead: BlockNumber = 0;
		pub const StakingUnsignedPriority: u64 = u64::max_value() / 2;
	}

	pub struct CurrencyToVoteHandler;

	impl Convert<u128, u128> for CurrencyToVoteHandler {
		fn convert(x: u128) -> u128 { x }
	}

	impl Convert<u128, u64> for CurrencyToVoteHandler {
		fn convert(x: u128) -> u64 { x.saturated_into() }
	}

	impl staking::Trait for Test {
		type RewardRemainder = ();
		type CurrencyToVote = CurrencyToVoteHandler;
		type Event = ();
		type Currency = Balances;
		type Slash = ();
		type Reward = ();
		type SessionsPerEra = SessionsPerEra;
		type BondingDuration = BondingDuration;
		type SlashDeferDuration = SlashDeferDuration;
		type SlashCancelOrigin = system::EnsureRoot<Self::AccountId>;
		type SessionInterface = Self;
		type UnixTime = timestamp::Module<Test>;
		type RewardCurve = RewardCurve;
		type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
		type NextNewSession = Session;
		type ElectionLookahead = ElectionLookahead;
		type Call = Call;
		type UnsignedPriority = StakingUnsignedPriority;
		type MaxIterations = ();
		type MinSolutionScoreBump = ();
	}

	impl attestations::Trait for Test {
		type AttestationPeriod = AttestationPeriod;
		type ValidatorIdentities = ValidatorIdentities<Test>;
		type RewardAttestation = ();
	}

	parameter_types!{
		pub const LeasePeriod: BlockNumber = 10;
		pub const EndingPeriod: BlockNumber = 3;
	}

	impl slots::Trait for Test {
		type Event = ();
		type Currency = Balances;
		type Parachains = registrar::Module<Test>;
		type EndingPeriod = EndingPeriod;
		type LeasePeriod = LeasePeriod;
		type Randomness = RandomnessCollectiveFlip;
	}

	parameter_types! {
		pub const ParathreadDeposit: Balance = 10;
		pub const QueueSize: usize = 2;
		pub const MaxRetries: u32 = 3;
	}

	impl registrar::Trait for Test {
		type Event = ();
		type Origin = Origin;
		type Currency = Balances;
		type ParathreadDeposit = ParathreadDeposit;
		type SwapAux = slots::Module<Test>;
		type QueueSize = QueueSize;
		type MaxRetries = MaxRetries;
	}

	parameter_types! {
		pub OffencesWeightSoftLimit: Weight = Perbill::from_percent(60) * MaximumBlockWeight::get();
	}

	impl offences::Trait for Test {
		type Event = ();
		type IdentificationTuple = session::historical::IdentificationTuple<Self>;
		type OnOffenceHandler = Staking;
		type WeightSoftLimit = OffencesWeightSoftLimit;
	}

	parameter_types! {
		pub const MaxHeadDataSize: u32 = 100;
		pub const MaxCodeSize: u32 = 100;

		pub const ValidationUpgradeFrequency: BlockNumber = 10;
		pub const ValidationUpgradeDelay: BlockNumber = 2;
		pub const SlashPeriod: BlockNumber = 50;
	}

	// This is needed for a custom `AccountId` type which is `u64` in testing here.
	pub mod test_keys {
		use sp_core::{crypto::KeyTypeId, sr25519};
		pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"test");

		mod app {
			use sp_application_crypto::{app_crypto, sr25519};
			use super::super::Parachains;

			app_crypto!(sr25519, super::KEY_TYPE);

			impl sp_runtime::traits::IdentifyAccount for Public {
				type AccountId = u64;

				fn into_account(self) -> Self::AccountId {
					Parachains::authorities().iter().position(|b| *b == self.0.clone().into()).unwrap() as u64
				}
			}
		}

		pub type ReporterId = app::Public;
		pub struct ReporterAuthorityId;
		impl system::offchain::AppCrypto<ReporterId, sr25519::Signature> for ReporterAuthorityId {
			type RuntimeAppPublic = ReporterId;
			type GenericSignature = sr25519::Signature;
			type GenericPublic = sr25519::Public;
		}
	}

	impl Trait for Test {
		type AuthorityId = test_keys::ReporterAuthorityId;
		type Origin = Origin;
		type Call = Call;
		type ParachainCurrency = Balances;
		type BlockNumberConversion = sp_runtime::traits::Identity;
		type Randomness = RandomnessCollectiveFlip;
		type ActiveParachains = registrar::Module<Test>;
		type Registrar = registrar::Module<Test>;
		type MaxCodeSize = MaxCodeSize;
		type MaxHeadDataSize = MaxHeadDataSize;
		type ValidationUpgradeFrequency = ValidationUpgradeFrequency;
		type ValidationUpgradeDelay = ValidationUpgradeDelay;
		type SlashPeriod = SlashPeriod;
		type Proof =
			<Historical as KeyOwnerProofSystem<(KeyTypeId, ValidatorId)>>::Proof;
		type IdentificationTuple =
			<Historical as KeyOwnerProofSystem<(KeyTypeId, ValidatorId)>>::IdentificationTuple;
		type ReportOffence = Offences;
		type BlockHashConversion = sp_runtime::traits::Identity;
		type KeyOwnerProofSystem = Historical;
	}

	type Extrinsic = TestXt<Call, ()>;

	impl<LocalCall> system::offchain::CreateSignedTransaction<LocalCall> for Test where
		Call: From<LocalCall>,
	{
		fn create_transaction<C: system::offchain::AppCrypto<Self::Public, Self::Signature>>(
			call: Call,
			_public: test_keys::ReporterId,
			_account: <Test as system::Trait>::AccountId,
			nonce: <Test as system::Trait>::Index,
		) -> Option<(Call, <Extrinsic as ExtrinsicT>::SignaturePayload)> {
			Some((call, (nonce, ())))
		}
	}

	impl system::offchain::SigningTypes for Test {
		type Public = test_keys::ReporterId;
		type Signature = sr25519::Signature;
	}

	type Parachains = Module<Test>;
	type Balances = balances::Module<Test>;
	type System = system::Module<Test>;
	type Offences = offences::Module<Test>;
	type Staking = staking::Module<Test>;
	type Session = session::Module<Test>;
	type Timestamp = timestamp::Module<Test>;
	type RandomnessCollectiveFlip = randomness_collective_flip::Module<Test>;
	type Registrar = registrar::Module<Test>;
	type Historical = session::historical::Module<Test>;

	fn new_test_ext(parachains: Vec<(ParaId, ValidationCode, HeadData)>) -> TestExternalities {
		use staking::StakerStatus;
		use babe::AuthorityId as BabeAuthorityId;

		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();

		let authority_keys = [
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Eve,
			Sr25519Keyring::Ferdie,
			Sr25519Keyring::One,
			Sr25519Keyring::Two,
		];

		// stashes are the index.
		let session_keys: Vec<_> = authority_keys.iter().enumerate()
			.map(|(i, k)| (i as u64, i as u64, TestSessionKeys {
				parachain_validator: ValidatorId::from(k.public()),
			}))
			.collect();

		let authorities: Vec<_> = authority_keys.iter().map(|k| ValidatorId::from(k.public())).collect();
		let babe_authorities: Vec<_> = authority_keys.iter()
			.map(|k| BabeAuthorityId::from(k.public()))
			.map(|k| (k, 1))
			.collect();

		// controllers are the index + 1000
		let stakers: Vec<_> = (0..authority_keys.len()).map(|i| (
			i as u64,
			i as u64 + 1000,
			10_000,
			StakerStatus::<u64>::Validator,
		)).collect();

		let balances: Vec<_> = (0..authority_keys.len()).map(|i| (i as u64, 10_000_000)).collect();

		GenesisConfig {
			authorities: authorities.clone(),
		}.assimilate_storage::<Test>(&mut t).unwrap();

		registrar::GenesisConfig::<Test> {
			parachains,
			_phdata: Default::default(),
		}.assimilate_storage(&mut t).unwrap();

		session::GenesisConfig::<Test> {
			keys: session_keys,
		}.assimilate_storage(&mut t).unwrap();

		babe::GenesisConfig {
			authorities: babe_authorities,
		}.assimilate_storage::<Test>(&mut t).unwrap();

		balances::GenesisConfig::<Test> {
			balances,
		}.assimilate_storage(&mut t).unwrap();

		staking::GenesisConfig::<Test> {
			stakers,
			validator_count: 8,
			force_era: staking::Forcing::ForceNew,
			minimum_validator_count: 0,
			invulnerables: vec![],
			.. Default::default()
		}.assimilate_storage(&mut t).unwrap();

		t.into()
	}

	fn set_heads(v: Vec<AttestedCandidate>) -> ParachainsCall<Test> {
		ParachainsCall::set_heads(v)
	}

	fn report_double_vote(
		report: DoubleVoteReport<sp_session::MembershipProof>,
	) -> Result<ParachainsCall<Test>, TransactionValidityError> {
		let inner = ParachainsCall::report_double_vote(report);
		let call = Call::Parachains(inner.clone());

		ValidateDoubleVoteReports::<Test>(sp_std::marker::PhantomData)
			.validate(&0, &call, &DispatchInfo::default(), 0)?;

		Ok(inner)
	}

	// creates a template candidate which pins to correct relay-chain state.
	fn raw_candidate(para_id: ParaId) -> CandidateReceipt {
		let mut head_data = Parachains::parachain_head(&para_id).unwrap();
		head_data.0.extend(para_id.encode());

		CandidateReceipt {
			parachain_index: para_id,
			relay_parent: System::parent_hash(),
			head_data,
			collator: Default::default(),
			signature: Default::default(),
			pov_block_hash: Default::default(),
			global_validation: Parachains::global_validation_schedule(),
			local_validation: Parachains::current_local_validation_data(&para_id).unwrap(),
			commitments: CandidateCommitments::default(),
		}
	}

	// makes a blank attested candidate from a `CandidateReceipt`.
	fn make_blank_attested(candidate: CandidateReceipt) -> AttestedCandidate {
		let (candidate, _) = candidate.abridge();

		AttestedCandidate {
			validity_votes: vec![],
			validator_indices: BitVec::new(),
			candidate,
		}
	}

	fn make_attestations(candidate: &mut AttestedCandidate) {
		let mut vote_implicit = false;

		let (duty_roster, _) = Parachains::calculate_duty_roster();
		let candidate_hash = candidate.candidate.hash();

		let authorities = Parachains::authorities();
		let extract_key = |public: ValidatorId| {
			let mut raw_public = [0; 32];
			raw_public.copy_from_slice(public.as_ref());
			Sr25519Keyring::from_raw_public(raw_public).unwrap()
		};

		let validation_entries = duty_roster.validator_duty.iter()
			.enumerate();

		let mut validator_indices = BitVec::new();
		for (idx, &duty) in validation_entries {
			if duty != Chain::Parachain(candidate.parachain_index()) { continue }
			vote_implicit = !vote_implicit;

			let key = extract_key(authorities[idx].clone());

			let statement = if vote_implicit {
				Statement::Candidate(candidate_hash.clone())
			} else {
				Statement::Valid(candidate_hash.clone())
			};

			let signing_context = Parachains::signing_context();
			let payload = localized_payload(statement, &signing_context);
			let signature = key.sign(&payload[..]).into();

			candidate.validity_votes.push(if vote_implicit {
				ValidityAttestation::Implicit(signature)
			} else {
				ValidityAttestation::Explicit(signature)
			});

			if validator_indices.len() <= idx {
				validator_indices.resize(idx + 1, false);
			}
			validator_indices.set(idx, true);
		}
		candidate.validator_indices = validator_indices;
	}

	fn new_candidate_with_upward_messages(
		id: u32,
		upward_messages: Vec<(ParachainDispatchOrigin, Vec<u8>)>
	) -> AttestedCandidate {
		let mut raw_candidate = raw_candidate(id.into());
		raw_candidate.commitments.upward_messages = upward_messages.into_iter()
			.map(|x| UpwardMessage { origin: x.0, data: x.1 })
			.collect();

		make_blank_attested(raw_candidate)
	}

	fn start_session(session_index: SessionIndex) {
		let mut parent_hash = System::parent_hash();

		for i in Session::current_index()..session_index {
			println!("session index {}", i);
			Staking::on_finalize(System::block_number());
			System::set_block_number((i + 1).into());
			Timestamp::set_timestamp(System::block_number() as primitives::Moment * 6000);

			// In order to be able to use `System::parent_hash()` in the tests
			// we need to first get it via `System::finalize` and then set it
			// the `System::initialize`. However, it is needed to be taken into
			// consideration that finalizing will prune some data in `System`
			// storage including old values `BlockHash` if that reaches above
			// `BlockHashCount` capacity.
			if System::block_number() > 1 {
				let hdr = System::finalize();
				parent_hash = hdr.hash();
			}

			System::initialize(
				&(i as BlockNumber + 1),
				&parent_hash,
				&Default::default(),
				&Default::default(),
				Default::default(),
			);
			init_block();
		}

		assert_eq!(Session::current_index(), session_index);
	}

	fn start_era(era_index: EraIndex) {
		start_session((era_index * 3).into());
		assert_eq!(Staking::current_era(), Some(era_index));
	}

	fn init_block() {
		println!("Initializing {}", System::block_number());
		Session::on_initialize(System::block_number());
		System::on_initialize(System::block_number());
		Registrar::on_initialize(System::block_number());
		Parachains::on_initialize(System::block_number());
	}
	fn run_to_block(n: BlockNumber) {
		println!("Running until block {}", n);
		while System::block_number() < n {
			if System::block_number() > 1 {
				println!("Finalizing {}", System::block_number());
				if !DidUpdate::get().is_some() {
					Parachains::set_heads(Origin::none(), vec![]).unwrap();
				}

				Parachains::on_finalize(System::block_number());
				Registrar::on_finalize(System::block_number());
				System::on_finalize(System::block_number());
			}
			Staking::new_session(System::block_number() as u32);
			System::set_block_number(System::block_number() + 1);
			init_block();
		}
	}

	fn queue_upward_messages(id: ParaId, upward_messages: &[UpwardMessage]) {
		NeedsDispatch::mutate(|nd|
			Parachains::queue_upward_messages(id, upward_messages, nd)
		);
	}

	#[test]
	fn check_dispatch_upward_works() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
			(2u32.into(), vec![].into(), vec![].into()),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			init_block();
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![0; 4] }
			]);
			queue_upward_messages(1.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1; 4] }
			]);
			let mut dispatched: Vec<(ParaId, ParachainDispatchOrigin, Vec<u8>)> = vec![];
			let dummy = |id, origin, data: &[u8]| dispatched.push((id, origin, data.to_vec()));
			Parachains::dispatch_upward_messages(2, 3, dummy);
			assert_eq!(dispatched, vec![
				(0.into(), ParachainDispatchOrigin::Parachain, vec![0; 4])
			]);
			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(1)).len(), 1);
		});
		new_test_ext(parachains.clone()).execute_with(|| {
			init_block();
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![0; 2] }
			]);
			queue_upward_messages(1.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1; 2] }
			]);
			queue_upward_messages(2.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![2] }
			]);
			let mut dispatched: Vec<(ParaId, ParachainDispatchOrigin, Vec<u8>)> = vec![];
			let dummy = |id, origin, data: &[u8]| dispatched.push((id, origin, data.to_vec()));
			Parachains::dispatch_upward_messages(2, 3, dummy);
			assert_eq!(dispatched, vec![
				(0.into(), ParachainDispatchOrigin::Parachain, vec![0; 2]),
				(2.into(), ParachainDispatchOrigin::Parachain, vec![2])
			]);
			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(1)).len(), 1);
			assert!(<RelayDispatchQueue>::get(ParaId::from(2)).is_empty());
		});
		new_test_ext(parachains.clone()).execute_with(|| {
			init_block();
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![0; 2] }
			]);
			queue_upward_messages(1.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1; 2] }
			]);
			queue_upward_messages(2.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![2] }
			]);
			let mut dispatched: Vec<(ParaId, ParachainDispatchOrigin, Vec<u8>)> = vec![];
			let dummy = |id, origin, data: &[u8]| dispatched.push((id, origin, data.to_vec()));
			Parachains::dispatch_upward_messages(2, 3, dummy);
			assert_eq!(dispatched, vec![
				(0.into(), ParachainDispatchOrigin::Parachain, vec![0; 2]),
				(2.into(), ParachainDispatchOrigin::Parachain, vec![2])
			]);
			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(1)).len(), 1);
			assert!(<RelayDispatchQueue>::get(ParaId::from(2)).is_empty());
		});
		new_test_ext(parachains.clone()).execute_with(|| {
			init_block();
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![0; 2] }
			]);
			queue_upward_messages(1.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1; 2] }
			]);
			queue_upward_messages(2.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![2] }
			]);
			let mut dispatched: Vec<(ParaId, ParachainDispatchOrigin, Vec<u8>)> = vec![];
			let dummy = |id, origin, data: &[u8]| dispatched.push((id, origin, data.to_vec()));
			Parachains::dispatch_upward_messages(2, 3, dummy);
			assert_eq!(dispatched, vec![
				(0.into(), ParachainDispatchOrigin::Parachain, vec![0; 2]),
				(2.into(), ParachainDispatchOrigin::Parachain, vec![2]),
			]);
			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(1)).len(), 1);
			assert!(<RelayDispatchQueue>::get(ParaId::from(2)).is_empty());
		});
	}

	#[test]
	fn check_queue_upward_messages_works() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] }
			];
			assert_ok!(Parachains::check_upward_messages(0.into(), &messages, 2, 3));

			// all good.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1, 2] }
			];
			assert_ok!(Parachains::check_upward_messages(0.into(), &messages, 2, 3));
			queue_upward_messages(0.into(), &messages);
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(0)), vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1, 2] },
			]);
		});
	}

	#[test]
	fn check_queue_full_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// oversize, but ok since it's just one and the queue is empty.
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0; 4] },
			];
			assert_ok!(Parachains::check_upward_messages(0.into(), &messages, 2, 3));

			// oversize and bad since it's not just one.
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0; 4] },
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				Error::<Test>::QueueFull
			);

			// too many messages.
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![1] },
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![2] },
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				Error::<Test>::QueueFull
			);
		});
	}

	#[test]
	fn check_queued_too_many_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// too many messages.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![1] },
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![2] },
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				Error::<Test>::QueueFull
			);
		});
	}

	#[test]
	fn check_queued_total_oversize_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// too much data.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0, 1] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![2, 3] },
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				Error::<Test>::QueueFull
			);
		});
	}

	#[test]
	fn check_queued_pre_jumbo_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// bad - already an oversize messages queued.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0; 4] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] }
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				Error::<Test>::QueueFull
			);
		});
	}

	#[test]
	fn check_queued_post_jumbo_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// bad - oversized and already a message queued.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0; 4] }
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				Error::<Test>::QueueFull
			);
		});
	}

	#[test]
	fn upward_queuing_works() {
		// That the list of egress queue roots is in ascending order by `ParaId`.
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// parachain 0 is self
			let mut candidates = vec![
				new_candidate_with_upward_messages(0, vec![
					(ParachainDispatchOrigin::Signed, vec![1]),
				]),
				new_candidate_with_upward_messages(1, vec![
					(ParachainDispatchOrigin::Parachain, vec![2]),
				])
			];
			candidates.iter_mut().for_each(make_attestations);

			assert_ok!(Call::from(set_heads(candidates)).dispatch(Origin::none()));

			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert!(<RelayDispatchQueue>::get(ParaId::from(1)).is_empty());
		});
	}

	#[test]
	fn active_parachains_should_work() {
		let parachains = vec![
			(5u32.into(), vec![1,2,3].into(), vec![1].into()),
			(100u32.into(), vec![4,5,6].into(), vec![2].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			assert_eq!(Parachains::active_parachains(), vec![(5u32.into(), None), (100u32.into(), None)]);
			assert_eq!(Parachains::parachain_code(ParaId::from(5u32)), Some(vec![1, 2, 3].into()));
			assert_eq!(Parachains::parachain_code(ParaId::from(100u32)), Some(vec![4, 5, 6].into()));
		});
	}

	#[test]
	fn register_deregister() {
		let parachains = vec![
			(5u32.into(), vec![1,2,3].into(), vec![1].into()),
			(100u32.into(), vec![4,5,6].into(), vec![2,].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			assert_eq!(Parachains::active_parachains(), vec![(5u32.into(), None), (100u32.into(), None)]);

			assert_eq!(Parachains::parachain_code(ParaId::from(5u32)), Some(vec![1,2,3].into()));
			assert_eq!(Parachains::parachain_code(ParaId::from(100u32)), Some(vec![4,5,6].into()));

			assert_ok!(Registrar::register_para(
				Origin::root(),
				99u32.into(),
				ParaInfo{scheduling: Scheduling::Always},
				vec![7,8,9].into(),
				vec![1, 1, 1].into(),
			));
			assert_ok!(Parachains::set_heads(Origin::none(), vec![]));

			run_to_block(3);

			assert_eq!(Parachains::active_parachains(), vec![(5u32.into(), None), (99u32.into(), None), (100u32.into(), None)]);
			assert_eq!(Parachains::parachain_code(&ParaId::from(99u32)), Some(vec![7,8,9].into()));

			assert_ok!(Registrar::deregister_para(Origin::root(), 5u32.into()));
			assert_ok!(Parachains::set_heads(Origin::none(), vec![]));

			// parachain still active this block. another block must pass before it's inactive.
			run_to_block(4);

			assert_eq!(Parachains::active_parachains(), vec![(99u32.into(), None), (100u32.into(), None)]);
			assert_eq!(Parachains::parachain_code(&ParaId::from(5u32)), None);
		});
	}

	#[test]
	fn duty_roster_works() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			let check_roster = |duty_roster: &DutyRoster| {
				assert_eq!(duty_roster.validator_duty.len(), 8);
				for i in (0..2).map(ParaId::from) {
					assert_eq!(duty_roster.validator_duty.iter().filter(|&&j| j == Chain::Parachain(i)).count(), 3);
				}
				assert_eq!(duty_roster.validator_duty.iter().filter(|&&j| j == Chain::Relay).count(), 2);
			};

			let duty_roster_0 = Parachains::calculate_duty_roster().0;
			check_roster(&duty_roster_0);

			System::initialize(&1, &H256::from([1; 32]), &Default::default(), &Default::default(), Default::default());
			RandomnessCollectiveFlip::on_initialize(1);
			let duty_roster_1 = Parachains::calculate_duty_roster().0;
			check_roster(&duty_roster_1);
			assert_ne!(duty_roster_0, duty_roster_1);


			System::initialize(&2, &H256::from([2; 32]), &Default::default(), &Default::default(), Default::default());
			RandomnessCollectiveFlip::on_initialize(2);
			let duty_roster_2 = Parachains::calculate_duty_roster().0;
			check_roster(&duty_roster_2);
			assert_ne!(duty_roster_0, duty_roster_2);
			assert_ne!(duty_roster_1, duty_roster_2);
		});
	}

	#[test]
	fn unattested_candidate_is_rejected() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			let candidate = make_blank_attested(raw_candidate(0.into()));
			assert!(Call::from(set_heads(vec![candidate])).dispatch(Origin::none()).is_err());
		})
	}

	#[test]
	fn attested_candidates_accepted_in_order() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			assert_eq!(Parachains::active_parachains().len(), 2);

			let mut candidate_a = make_blank_attested(raw_candidate(0.into()));
			let mut candidate_b = make_blank_attested(raw_candidate(1.into()));

			make_attestations(&mut candidate_a);
			make_attestations(&mut candidate_b);

			assert!(Call::from(set_heads(vec![candidate_b.clone(), candidate_a.clone()]))
				.dispatch(Origin::none()).is_err());

			assert_ok!(Call::from(set_heads(vec![candidate_a.clone(), candidate_b.clone()]))
				.dispatch(Origin::none()));

			assert_eq!(Heads::get(&ParaId::from(0)), Some(candidate_a.candidate.head_data));
			assert_eq!(Heads::get(&ParaId::from(1)), Some(candidate_b.candidate.head_data));
		});
	}

	#[test]
	fn duplicate_vote_is_rejected() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);

			let mut candidate = make_blank_attested(raw_candidate(0.into()));
			make_attestations(&mut candidate);

			let mut double_validity = candidate.clone();
			double_validity.validity_votes.push(candidate.validity_votes[0].clone());
			double_validity.validator_indices.push(true);

			assert!(Call::from(set_heads(vec![double_validity])).dispatch(Origin::none()).is_err());
		});
	}

	#[test]
	fn validators_not_from_group_is_rejected() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);

			let mut candidate = make_blank_attested(raw_candidate(0.into()));
			make_attestations(&mut candidate);

			// Change the last vote index to make it not corresponding to the assigned group.
			assert!(candidate.validator_indices.pop().is_some());
			candidate.validator_indices.append(&mut bitvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

			assert!(Call::from(set_heads(vec![candidate])).dispatch(Origin::none()).is_err());
		});
	}

	#[test]
	fn empty_trie_root_const_is_blake2_hashed_null_node() {
		let hashed_null_node = <NodeCodec<Blake2Hasher> as trie_db::NodeCodec>::hashed_null_node();
		assert_eq!(hashed_null_node, EMPTY_TRIE_ROOT.into())
	}

	#[test]
	fn para_past_code_meta_gives_right_code() {
		let mut past_code = ParaPastCodeMeta::default();
		assert_eq!(past_code.code_at(0u32), Some(UseCodeAt::Current));

		past_code.note_replacement(10);
		assert_eq!(past_code.code_at(0), Some(UseCodeAt::ReplacedAt(10)));
		assert_eq!(past_code.code_at(10), Some(UseCodeAt::ReplacedAt(10)));
		assert_eq!(past_code.code_at(11), Some(UseCodeAt::Current));

		past_code.note_replacement(20);
		assert_eq!(past_code.code_at(1), Some(UseCodeAt::ReplacedAt(10)));
		assert_eq!(past_code.code_at(10), Some(UseCodeAt::ReplacedAt(10)));
		assert_eq!(past_code.code_at(11), Some(UseCodeAt::ReplacedAt(20)));
		assert_eq!(past_code.code_at(20), Some(UseCodeAt::ReplacedAt(20)));
		assert_eq!(past_code.code_at(21), Some(UseCodeAt::Current));

		past_code.last_pruned = Some(5);
		assert_eq!(past_code.code_at(1), None);
		assert_eq!(past_code.code_at(5), None);
		assert_eq!(past_code.code_at(6), Some(UseCodeAt::ReplacedAt(10)));
	}

	#[test]
	fn para_past_code_pruning_works_correctly() {
		let mut past_code = ParaPastCodeMeta::default();
		past_code.note_replacement(10u32);
		past_code.note_replacement(20);
		past_code.note_replacement(30);

		let old = past_code.clone();
		assert!(past_code.prune_up_to(9).collect::<Vec<_>>().is_empty());
		assert_eq!(old, past_code);

		assert_eq!(past_code.prune_up_to(10).collect::<Vec<_>>(), vec![10]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![30, 20],
			last_pruned: Some(10),
		});

		assert_eq!(past_code.prune_up_to(21).collect::<Vec<_>>(), vec![20]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![30],
			last_pruned: Some(20),
		});

		past_code.note_replacement(40);
		past_code.note_replacement(50);
		past_code.note_replacement(60);

		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: vec![60, 50, 40, 30],
			last_pruned: Some(20),
		});

		assert_eq!(past_code.prune_up_to(60).collect::<Vec<_>>(), vec![30, 40, 50, 60]);
		assert_eq!(past_code, ParaPastCodeMeta {
			upgrade_times: Vec::new(),
			last_pruned: Some(60),
		});
	}

	#[test]
	fn para_past_code_pruning_in_initialize() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			let id = ParaId::from(0u32);
			let at_block: BlockNumber = 10;
			<Parachains as Store>::PastCode::insert(&(id, at_block), &ValidationCode(vec![1, 2, 3]));
			<Parachains as Store>::PastCodePruning::put(&vec![(id, at_block)]);

			{
				let mut code_meta = Parachains::past_code_meta(&id);
				code_meta.note_replacement(at_block);
				<Parachains as Store>::PastCodeMeta::insert(&id, &code_meta);
			}

			let pruned_at: BlockNumber = at_block + SlashPeriod::get() + 1;
			assert_eq!(<Parachains as Store>::PastCode::get(&(id, at_block)), Some(vec![1, 2, 3].into()));

			run_to_block(pruned_at - 1);
			assert_eq!(<Parachains as Store>::PastCode::get(&(id, at_block)), Some(vec![1, 2, 3].into()));
			assert_eq!(Parachains::past_code_meta(&id).most_recent_change(), Some(at_block));

			run_to_block(pruned_at);
			assert!(<Parachains as Store>::PastCode::get(&(id, at_block)).is_none());
			assert!(Parachains::past_code_meta(&id).most_recent_change().is_none());
		});
	}

	#[test]
	fn note_past_code_sets_up_pruning_correctly() {
		let parachains = vec![
			(0u32.into(), vec![].into(), vec![].into()),
			(1u32.into(), vec![].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			let id_a = ParaId::from(0u32);
			let id_b = ParaId::from(1u32);

			Parachains::note_past_code(id_a, 10, vec![1, 2, 3].into());
			Parachains::note_past_code(id_b, 20, vec![4, 5, 6].into());

			assert_eq!(Parachains::past_code_pruning_tasks(), vec![(id_a, 10), (id_b, 20)]);
			assert_eq!(
				Parachains::past_code_meta(&id_a),
				ParaPastCodeMeta {
					upgrade_times: vec![10],
					last_pruned: None,
				}
			);
			assert_eq!(
				Parachains::past_code_meta(&id_b),
				ParaPastCodeMeta {
					upgrade_times: vec![20],
					last_pruned: None,
				}
			);
		});
	}

	#[test]
	fn code_upgrade_applied_after_delay() {
		let parachains = vec![
			(0u32.into(), vec![1, 2, 3].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			let para_id = ParaId::from(0);
			let new_code = ValidationCode(vec![4, 5, 6]);

			run_to_block(2);
			assert_eq!(Parachains::active_parachains().len(), 1);
			assert_eq!(Parachains::parachain_code(&para_id), Some(vec![1, 2, 3].into()));

			let applied_after ={
				let raw_candidate = raw_candidate(para_id);
				let applied_after = raw_candidate.local_validation.code_upgrade_allowed.unwrap();
				let mut candidate_a = make_blank_attested(raw_candidate);

				candidate_a.candidate.commitments.new_validation_code = Some(new_code.clone());

				// this parablock is in the context of block 1.
				assert_eq!(applied_after, 1 + ValidationUpgradeDelay::get());
				make_attestations(&mut candidate_a);

				assert_ok!(Call::from(set_heads(vec![candidate_a.clone()])).dispatch(Origin::none()));

				assert!(Parachains::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(Parachains::code_upgrade_schedule(&para_id), Some(applied_after));
				assert_eq!(<Parachains as Store>::FutureCode::get(&para_id), new_code);
				assert_eq!(Parachains::parachain_code(&para_id), Some(vec![1, 2, 3].into()));

				applied_after
			};

			run_to_block(applied_after);

			// the candidate is in the context of the parent of `applied_after`,
			// thus does not trigger the code upgrade.
			{
				let raw_candidate = raw_candidate(para_id);
				assert!(raw_candidate.local_validation.code_upgrade_allowed.is_none());
				let mut candidate_a = make_blank_attested(raw_candidate);

				make_attestations(&mut candidate_a);

				assert_ok!(Call::from(set_heads(vec![candidate_a.clone()])).dispatch(Origin::none()));

				assert!(Parachains::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(Parachains::code_upgrade_schedule(&para_id), Some(applied_after));
				assert_eq!(<Parachains as Store>::FutureCode::get(&para_id), new_code);
				assert_eq!(Parachains::parachain_code(&para_id), Some(vec![1, 2, 3].into()));
			}

			run_to_block(applied_after + 1);

			// the candidate is in the context of `applied_after`, and triggers
			// the upgrade.
			{
				let raw_candidate = raw_candidate(para_id);
				assert!(raw_candidate.local_validation.code_upgrade_allowed.is_some());
				let mut candidate_a = make_blank_attested(raw_candidate);

				make_attestations(&mut candidate_a);

				assert_ok!(Call::from(set_heads(vec![candidate_a.clone()])).dispatch(Origin::none()));

				assert_eq!(
					Parachains::past_code_meta(&para_id).most_recent_change(),
					Some(applied_after),
				);
				assert_eq!(
					<Parachains as Store>::PastCode::get(&(para_id, applied_after)),
					Some(vec![1, 2, 3,].into()),
				);
				assert!(Parachains::code_upgrade_schedule(&para_id).is_none());
				assert!(<Parachains as Store>::FutureCode::get(&para_id).0.is_empty());
				assert_eq!(Parachains::parachain_code(&para_id), Some(new_code));
			}
		});
	}

	#[test]
	fn code_upgrade_applied_after_delay_even_when_late() {
		let parachains = vec![
			(0u32.into(), vec![1, 2, 3].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			let para_id = ParaId::from(0);
			let new_code = ValidationCode(vec![4, 5, 6]);

			run_to_block(2);
			assert_eq!(Parachains::active_parachains().len(), 1);
			assert_eq!(Parachains::parachain_code(&para_id), Some(vec![1, 2, 3].into()));

			let applied_after ={
				let raw_candidate = raw_candidate(para_id);
				let applied_after = raw_candidate.local_validation.code_upgrade_allowed.unwrap();
				let mut candidate_a = make_blank_attested(raw_candidate);

				candidate_a.candidate.commitments.new_validation_code = Some(new_code.clone());

				// this parablock is in the context of block 1.
				assert_eq!(applied_after, 1 + ValidationUpgradeDelay::get());
				make_attestations(&mut candidate_a);

				assert_ok!(Call::from(set_heads(vec![candidate_a.clone()])).dispatch(Origin::none()));

				assert!(Parachains::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(Parachains::code_upgrade_schedule(&para_id), Some(applied_after));
				assert_eq!(<Parachains as Store>::FutureCode::get(&para_id), new_code);
				assert_eq!(Parachains::parachain_code(&para_id), Some(vec![1, 2, 3].into()));

				applied_after
			};

			run_to_block(applied_after + 1 + 4);

			{
				let raw_candidate = raw_candidate(para_id);
				assert!(raw_candidate.local_validation.code_upgrade_allowed.is_some());
				let mut candidate_a = make_blank_attested(raw_candidate);

				make_attestations(&mut candidate_a);

				assert_ok!(Call::from(set_heads(vec![candidate_a.clone()])).dispatch(Origin::none()));

				assert_eq!(
					Parachains::past_code_meta(&para_id).most_recent_change(),
					Some(applied_after + 4),
				);
				assert_eq!(
					<Parachains as Store>::PastCode::get(&(para_id, applied_after + 4)),
					Some(vec![1, 2, 3,].into()),
				);
				assert!(Parachains::code_upgrade_schedule(&para_id).is_none());
				assert!(<Parachains as Store>::FutureCode::get(&para_id).0.is_empty());
				assert_eq!(Parachains::parachain_code(&para_id), Some(new_code));
			}
		});
	}

	#[test]
	fn submit_code_change_when_not_allowed_is_err() {
		let parachains = vec![
			(0u32.into(), vec![1, 2, 3].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			let para_id = ParaId::from(0);
			let new_code = ValidationCode(vec![4, 5, 6]);

			run_to_block(2);

			{
				let raw_candidate = raw_candidate(para_id);
				let mut candidate_a = make_blank_attested(raw_candidate);

				candidate_a.candidate.commitments.new_validation_code = Some(new_code.clone());

				make_attestations(&mut candidate_a);

				assert_ok!(Call::from(set_heads(vec![candidate_a.clone()])).dispatch(Origin::none()));
			};

			run_to_block(3);

			{
				let raw_candidate = raw_candidate(para_id);
				assert!(raw_candidate.local_validation.code_upgrade_allowed.is_none());
				let mut candidate_a = make_blank_attested(raw_candidate);
				candidate_a.candidate.commitments.new_validation_code = Some(vec![1, 2, 3].into());

				make_attestations(&mut candidate_a);

				assert_err!(
					Call::from(set_heads(vec![candidate_a.clone()])).dispatch(Origin::none()),
					Error::<Test>::DisallowedCodeUpgrade,
				);
			}
		});
	}

	#[test]
	fn full_parachain_cleanup_storage() {
		let parachains = vec![
			(0u32.into(), vec![1, 2, 3].into(), vec![].into()),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			let para_id = ParaId::from(0);
			let new_code = ValidationCode(vec![4, 5, 6]);

			run_to_block(2);
			{
				let raw_candidate = raw_candidate(para_id);
				let applied_after = raw_candidate.local_validation.code_upgrade_allowed.unwrap();
				let mut candidate_a = make_blank_attested(raw_candidate);

				candidate_a.candidate.commitments.new_validation_code = Some(new_code.clone());

				// this parablock is in the context of block 1.
				assert_eq!(applied_after, 1 + ValidationUpgradeDelay::get());
				make_attestations(&mut candidate_a);

				assert_ok!(Call::from(set_heads(vec![candidate_a.clone()])).dispatch(Origin::none()));

				assert!(Parachains::past_code_meta(&para_id).most_recent_change().is_none());
				assert_eq!(Parachains::code_upgrade_schedule(&para_id), Some(applied_after));
				assert_eq!(<Parachains as Store>::FutureCode::get(&para_id), new_code);
				assert_eq!(Parachains::parachain_code(&para_id), Some(vec![1, 2, 3].into()));

				assert!(Parachains::past_code_pruning_tasks().is_empty());
			};

			Parachains::cleanup_para(para_id);

			// cleaning up the parachain should place the current parachain code
			// into the past code buffer & schedule cleanup.
			assert_eq!(Parachains::past_code_meta(&para_id).most_recent_change(), Some(2));
			assert_eq!(<Parachains as Store>::PastCode::get(&(para_id, 2)), Some(vec![1, 2, 3].into()));
			assert_eq!(Parachains::past_code_pruning_tasks(), vec![(para_id, 2)]);

			// any future upgrades haven't been used to validate yet, so those
			// are cleaned up immediately.
			assert!(Parachains::code_upgrade_schedule(&para_id).is_none());
			assert!(<Parachains as Store>::FutureCode::get(&para_id).0.is_empty());
			assert!(Parachains::parachain_code(&para_id).is_none());

			let cleaned_up_at = 2 + SlashPeriod::get() + 1;
			run_to_block(cleaned_up_at);

			// now the final cleanup: last past code cleaned up, and this triggers meta cleanup.
			assert_eq!(Parachains::past_code_meta(&para_id), Default::default());
			assert!(<Parachains as Store>::PastCode::get(&(para_id, 2)).is_none());
			assert!(Parachains::past_code_pruning_tasks().is_empty());
		});
	}

	#[test]
	fn double_vote_candidate_and_valid_works() {
		let parachains = vec![
			(1u32.into(), vec![].into(), vec![].into()),
		];

		let extract_key = |public: ValidatorId| {
			let mut raw_public = [0; 32];
			raw_public.copy_from_slice(public.as_ref());
			Sr25519Keyring::from_raw_public(raw_public).unwrap()
		};

		// Test that a Candidate and Valid statements on the same candidate get slashed.
		new_test_ext(parachains.clone()).execute_with(|| {
			assert_eq!(Staking::current_era(), Some(0));
			assert_eq!(Session::current_index(), 0);

			start_era(1);

			let candidate = raw_candidate(1.into()).abridge().0;
			let candidate_hash = candidate.hash();

			let authorities = Parachains::authorities();
			let authority_index = 0;
			let key = extract_key(authorities[authority_index].clone());

			let statement_candidate = Statement::Candidate(candidate_hash.clone());
			let statement_valid = Statement::Valid(candidate_hash.clone());

			let signing_context = Parachains::signing_context();
			let payload_1 = localized_payload(statement_candidate.clone(), &signing_context);
			let payload_2 = localized_payload(statement_valid.clone(), &signing_context);

			let signature_1 = key.sign(&payload_1[..]).into();
			let signature_2 = key.sign(&payload_2[..]).into();

			// Check that in the beginning the genesis balances are there.
			for i in 0..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(1, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}

			let encoded_key = key.encode();
			let proof = Historical::prove((PARACHAIN_KEY_TYPE_ID, &encoded_key[..])).unwrap();

			let report = DoubleVoteReport {
				identity: ValidatorId::from(key.public()),
				first: (statement_candidate, signature_1),
				second: (statement_valid, signature_2),
				proof,
				signing_context,
			};

			let inner = report_double_vote(report).unwrap();

			assert_ok!(Call::from(inner).dispatch(Origin::signed(1)));

			start_era(2);

			// Check that the balance of 0-th validator is slashed 100%.
			assert_eq!(Balances::total_balance(&0), 10_000_000 - 10_000);
			assert_eq!(Staking::slashable_balance_of(&0), 0);

			assert_eq!(
				Staking::eras_stakers(2, 0),
				staking::Exposure {
					total: 0,
					own: 0,
					others: vec![],
				},
			);

			// Check that the balances of all other validators are left intact.
			for i in 1..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(2, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}
		});
	}

	#[test]
	fn double_vote_candidate_and_invalid_works() {
		let parachains = vec![
			(1u32.into(), vec![].into(), vec![].into()),
		];

		let extract_key = |public: ValidatorId| {
			let mut raw_public = [0; 32];
			raw_public.copy_from_slice(public.as_ref());
			Sr25519Keyring::from_raw_public(raw_public).unwrap()
		};

		// Test that a Candidate and Invalid statements on the same candidate get slashed.
		new_test_ext(parachains.clone()).execute_with(|| {
			start_era(1);

			let candidate = raw_candidate(1.into()).abridge().0;
			let candidate_hash = candidate.hash();

			let authorities = Parachains::authorities();
			let authority_index = 0;
			let key = extract_key(authorities[authority_index].clone());

			let statement_candidate = Statement::Candidate(candidate_hash);
			let statement_invalid = Statement::Invalid(candidate_hash.clone());

			let signing_context = Parachains::signing_context();
			let payload_1 = localized_payload(statement_candidate.clone(), &signing_context);
			let payload_2 = localized_payload(statement_invalid.clone(), &signing_context);

			let signature_1 = key.sign(&payload_1[..]).into();
			let signature_2 = key.sign(&payload_2[..]).into();

			// Check that in the beginning the genesis balances are there.
			for i in 0..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(1, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}

			let encoded_key = key.encode();
			let proof = Historical::prove((PARACHAIN_KEY_TYPE_ID, &encoded_key[..])).unwrap();

			let report = DoubleVoteReport {
				identity: ValidatorId::from(key.public()),
				first: (statement_candidate, signature_1),
				second: (statement_invalid, signature_2),
				proof,
				signing_context,
			};

			assert_ok!(Call::from(report_double_vote(report).unwrap()).dispatch(Origin::signed(1)));

			start_era(2);

			// Check that the balance of 0-th validator is slashed 100%.
			assert_eq!(Balances::total_balance(&0), 10_000_000 - 10_000);
			assert_eq!(Staking::slashable_balance_of(&0), 0);

			assert_eq!(
				Staking::eras_stakers(Staking::current_era().unwrap(), 0),
				staking::Exposure {
					total: 0,
					own: 0,
					others: vec![],
				},
			);

			// Check that the balances of all other validators are left intact.
			for i in 1..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(2, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}

		});
	}

	#[test]
	fn double_vote_valid_and_invalid_works() {
		let parachains = vec![
			(1u32.into(), vec![].into(), vec![].into()),
		];

		let extract_key = |public: ValidatorId| {
			let mut raw_public = [0; 32];
			raw_public.copy_from_slice(public.as_ref());
			Sr25519Keyring::from_raw_public(raw_public).unwrap()
		};

		// Test that an Invalid and Valid statements on the same candidate get slashed.
		new_test_ext(parachains.clone()).execute_with(|| {
			start_era(1);

			let candidate = raw_candidate(1.into()).abridge().0;
			let candidate_hash = candidate.hash();

			let authorities = Parachains::authorities();
			let authority_index = 0;
			let key = extract_key(authorities[authority_index].clone());

			let statement_invalid = Statement::Invalid(candidate_hash.clone());
			let statement_valid = Statement::Valid(candidate_hash.clone());

			let signing_context = Parachains::signing_context();
			let payload_1 = localized_payload(statement_invalid.clone(), &signing_context);
			let payload_2 = localized_payload(statement_valid.clone(), &signing_context);

			let signature_1 = key.sign(&payload_1[..]).into();
			let signature_2 = key.sign(&payload_2[..]).into();

			// Check that in the beginning the genesis balances are there.
			for i in 0..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(1, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}

			let encoded_key = key.encode();
			let proof = Historical::prove((PARACHAIN_KEY_TYPE_ID, &encoded_key[..])).unwrap();

			let report = DoubleVoteReport {
				identity: ValidatorId::from(key.public()),
				first: (statement_invalid, signature_1),
				second: (statement_valid, signature_2),
				proof,
				signing_context,
			};

			assert_ok!(Call::from(report_double_vote(report).unwrap()).dispatch(Origin::signed(1)));

			start_era(2);

			// Check that the balance of 0-th validator is slashed 100%.
			assert_eq!(Balances::total_balance(&0), 10_000_000 - 10_000);
			assert_eq!(Staking::slashable_balance_of(&0), 0);

			assert_eq!(
				Staking::eras_stakers(2, 0),
				staking::Exposure {
					total: 0,
					own: 0,
					others: vec![],
				},
			);

			// Check that the balances of all other validators are left intact.
			for i in 1..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(2, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}
		});
	}

	// Check that submitting the same report twice errors.
	#[test]
	fn double_vote_submit_twice_works() {
		let parachains = vec![
			(1u32.into(), vec![].into(), vec![].into()),
		];

		let extract_key = |public: ValidatorId| {
			let mut raw_public = [0; 32];
			raw_public.copy_from_slice(public.as_ref());
			Sr25519Keyring::from_raw_public(raw_public).unwrap()
		};

		// Test that a Candidate and Valid statements on the same candidate get slashed.
		new_test_ext(parachains.clone()).execute_with(|| {
			assert_eq!(Staking::current_era(), Some(0));
			assert_eq!(Session::current_index(), 0);

			start_era(1);

			let candidate = raw_candidate(1.into()).abridge().0;
			let candidate_hash = candidate.hash();

			let authorities = Parachains::authorities();
			let authority_index = 0;
			let key = extract_key(authorities[authority_index].clone());

			let statement_candidate = Statement::Candidate(candidate_hash.clone());
			let statement_valid = Statement::Valid(candidate_hash.clone());

			let signing_context = Parachains::signing_context();
			let payload_1 = localized_payload(statement_candidate.clone(), &signing_context);
			let payload_2 = localized_payload(statement_valid.clone(), &signing_context);

			let signature_1 = key.sign(&payload_1[..]).into();
			let signature_2 = key.sign(&payload_2[..]).into();

			// Check that in the beginning the genesis balances are there.
			for i in 0..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(1, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}

			let encoded_key = key.encode();
			let proof = Historical::prove((PARACHAIN_KEY_TYPE_ID, &encoded_key[..])).unwrap();

			let report = DoubleVoteReport {
				identity: ValidatorId::from(key.public()),
				first: (statement_candidate, signature_1),
				second: (statement_valid, signature_2),
				proof,
				signing_context,
			};

			assert_ok!(Call::from(report_double_vote(report.clone()).unwrap()).dispatch(Origin::signed(1)));

			assert!(Call::from(report_double_vote(report).unwrap()).dispatch(Origin::signed(1)).is_err()
			);

			start_era(2);

			// Check that the balance of 0-th validator is slashed 100%.
			assert_eq!(Balances::total_balance(&0), 10_000_000 - 10_000);
			assert_eq!(Staking::slashable_balance_of(&0), 0);

			assert_eq!(
				Staking::eras_stakers(2, 0),
				staking::Exposure {
					total: 0,
					own: 0,
					others: vec![],
				},
			);

			// Check that the balances of all other validators are left intact.
			for i in 1..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(2, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}
		});
	}

	// Check that submitting invalid reports fail.
	#[test]
	fn double_vote_submit_invalid_works() {
		let parachains = vec![
			(1u32.into(), vec![].into(), vec![].into()),
		];

		let extract_key = |public: ValidatorId| {
			let mut raw_public = [0; 32];
			raw_public.copy_from_slice(public.as_ref());
			Sr25519Keyring::from_raw_public(raw_public).unwrap()
		};

		// Test that a Candidate and Valid statements on the same candidate get slashed.
		new_test_ext(parachains.clone()).execute_with(|| {
			assert_eq!(Staking::current_era(), Some(0));
			assert_eq!(Session::current_index(), 0);

			start_era(1);

			let candidate = raw_candidate(1.into()).abridge().0;
			let candidate_hash = candidate.hash();

			let authorities = Parachains::authorities();
			let authority_1_index = 0;
			let authority_2_index = 1;
			let key_1 = extract_key(authorities[authority_1_index].clone());
			let key_2 = extract_key(authorities[authority_2_index].clone());

			let statement_candidate = Statement::Candidate(candidate_hash.clone());
			let statement_valid = Statement::Valid(candidate_hash.clone());

			let signing_context = Parachains::signing_context();
			let payload_1 = localized_payload(statement_candidate.clone(), &signing_context);
			let payload_2 = localized_payload(statement_valid.clone(), &signing_context);

			let signature_1 = key_1.sign(&payload_1[..]).into();
			let signature_2 = key_2.sign(&payload_2[..]).into();

			let encoded_key = key_1.encode();
			let proof = Historical::prove((PARACHAIN_KEY_TYPE_ID, &encoded_key[..])).unwrap();

			let report = DoubleVoteReport {
				identity: ValidatorId::from(key_1.public()),
				first: (statement_candidate, signature_1),
				second: (statement_valid, signature_2),
				proof,
				signing_context,
			};

			assert_eq!(
				report_double_vote(report.clone()),
				Err(TransactionValidityError::Invalid(
						InvalidTransaction::Custom(DoubleVoteValidityError::InvalidSignature as u8)
					)
				),
			);
		});
	}

	#[test]
	fn double_vote_proof_session_mismatch_fails() {
		let parachains = vec![
			(1u32.into(), vec![].into(), vec![].into()),
		];

		let extract_key = |public: ValidatorId| {
			let mut raw_public = [0; 32];
			raw_public.copy_from_slice(public.as_ref());
			Sr25519Keyring::from_raw_public(raw_public).unwrap()
		};

		// Test that submitting a report with a session mismatch between the `parent_hash`
		// and the proof itself fails.
		new_test_ext(parachains.clone()).execute_with(|| {
			assert_eq!(Staking::current_era(), Some(0));
			assert_eq!(Session::current_index(), 0);

			start_era(1);

			let candidate = raw_candidate(1.into()).abridge().0;
			let candidate_hash = candidate.hash();

			let authorities = Parachains::authorities();
			let authority_index = 0;
			let key = extract_key(authorities[authority_index].clone());

			let statement_candidate = Statement::Candidate(candidate_hash.clone());
			let statement_valid = Statement::Valid(candidate_hash.clone());
			let parent_hash = System::parent_hash();

			let signing_context = SigningContext {
				session_index: Session::current_index() - 1,
				parent_hash,
			};
			let payload_1 = localized_payload(statement_candidate.clone(), &signing_context);
			let payload_2 = localized_payload(statement_valid.clone(), &signing_context);

			let signature_1 = key.sign(&payload_1[..]).into();
			let signature_2 = key.sign(&payload_2[..]).into();

			// Check that in the beginning the genesis balances are there.
			for i in 0..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(1, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}

			// Get the proof from another session.
			start_era(2);
			let encoded_key = key.encode();
			let proof = Historical::prove((PARACHAIN_KEY_TYPE_ID, &encoded_key[..])).unwrap();

			let report = DoubleVoteReport {
				identity: ValidatorId::from(key.public()),
				first: (statement_candidate, signature_1),
				second: (statement_valid, signature_2),
				proof,
				signing_context,
			};

			assert!(report_double_vote(report.clone()).is_err());

			start_era(3);

			// Check that the balances are unchanged.
			for i in 0..authorities.len() {
				assert_eq!(Balances::total_balance(&(i as u64)), 10_000_000);
				assert_eq!(Staking::slashable_balance_of(&(i as u64)), 10_000);

				assert_eq!(
					Staking::eras_stakers(1, i as u64),
					staking::Exposure {
						total: 10_000,
						own: 10_000,
						others: vec![],
					},
				);
			}
		});
	}
}
