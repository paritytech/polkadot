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
};
use sp_runtime::{
	traits::{One, Saturating, AppVerify},
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

pub trait Config:
	frame_system::Config +
	configuration::Config +
	session_info::Config
{
	type Event: From<Event> + Into<<Self as frame_system::Config>::Event>;
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
		SpamSlots: map hasher(twox_64_concat) SessionIndex => Vec<u32>;
		// Whether the chain is frozen or not. Starts as `false`. When this is `true`,
		// the chain will not accept any new parachain blocks for backing or inclusion.
		// It can only be set back to `false` by governance intervention.
		Frozen: bool;
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

enum SpamSlotChange {
	Dec,
	Inc,
}

bitflags::bitflags! {
	#[derive(Default)]
	struct DisputeStateFlags: u8 {
		const CONFIRMED = 0b0001;
		const FOR_SUPERMAJORITY = 0b0010;
		const AGAINST_SUPERMAJORITY = 0b0100;
		const CONCLUDED = 0b1000;
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

		if state.concluded_at.is_some() {
			flags |= DisputeStateFlags::CONCLUDED;
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

struct DisputeStateMutator<BlockNumber> {
	state: DisputeState<BlockNumber>,
	pre_flags: DisputeStateFlags,
}

impl<BlockNumber> DisputeStateMutator<BlockNumber> {
	fn initialize(
		state: DisputeState<BlockNumber>,
	) -> Self {
		let pre_flags = DisputeStateFlags::from_state(&state);

		DisputeStateMutator {
			state,
			pre_flags,
		}
	}

	fn push(&mut self, validator: ValidatorIndex, valid: bool)
		-> Result<(), DispatchError>
	{
		// TODO [now]: fail if duplicate.

		Ok(())
	}

	fn conclude(self) -> DisputeState<BlockNumber> {
		let post_flags = DisputeStateFlags::from_state(&self.state);

		// TODO [now]: prepare update to spam slots, rewarding, and slashing.

		self.state
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

				// mildly punish all validators involved. they've failed to make
				// data available to others, so this is most likely spam.
				SpamSlots::mutate(session_index, |spam_slots| {
					let participating = dispute.validators_for | dispute.validators_against;
					for validator_index in participating {
						// TODO [now]: slight punishment.

						// also reduce spam slots for all validators involved. this does
						// open us up to more spam, but only for validators who are willing
						// to be punished more.
						if let Some(occupied) = spam_slots.get_mut(validator_index as usize) {
							*occupied = occupied.saturating_sub(1);
						}
					}
				});

				weight += T::DbWeight::get().reads_writes(1, 2);
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

		// Check all statement signatures
		for (statement, validator_index, signature) in &set.statements {
			let validator_public = session_info.validators.get(validator_index.0 as usize)
				.ok_or(Error::<T>::ValidatorIndexOutOfBounds)?;

			check_signature(
				&validator_public,
				set.candidate_hash,
				set.session,
				statement,
				signature,
			).map_err(|()| Error::<T>::InvalidSignature)?;
		}

		let byzantine_threshold = byzantine_threshold(n_validators);
		let supermajority_threshold = supermajority_threshold(n_validators);

		// TODO [now]: update `DisputeInfo` and determine:
		// 1. Spam slot changes. Bail early if too many spam slots occupied.
		// 2. Dispute state change. Bail early if duplicate

		// TODO [now]: Reward statements based on conclusion.

		// TODO [now]: Slash one side if fresh supermajority on the other.

		// TODO [now]: Freeze if just concluded local.

		Ok(fresh)
	}
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
