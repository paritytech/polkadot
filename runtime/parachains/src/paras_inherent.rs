// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Provides glue code over the scheduler and inclusion modules, and accepting
//! one inherent per block that can include new para candidates and bitfields.
//!
//! Unlike other modules in this crate, it does not need to be initialized by the initializer,
//! as it has no initialization logic and its finalization logic depends only on the details of
//! this module.

use no_std_compat::cmp::Ordering;

use crate::{
	disputes::DisputesHandler,
	inclusion,
	scheduler::{self, FreedReason},
	shared, ump,
};
use frame_support::{
	inherent::{InherentData, InherentIdentifier, MakeFatalError, ProvideInherent},
	pallet_prelude::*,
};
use frame_system::pallet_prelude::*;
use primitives::v1::{
	BackedCandidate, DisputeStatementSet, InherentData as ParachainsInherentData,
	MultiDisputeStatementSet, ScrapedOnChainVotes, UncheckedSignedAvailabilityBitfield,
	UncheckedSignedAvailabilityBitfields, PARACHAINS_INHERENT_IDENTIFIER,
};
use sp_runtime::traits::Header as HeaderT;
use sp_std::prelude::*;

pub use pallet::*;

const LOG_TARGET: &str = "runtime::inclusion-inherent";
// In the future, we should benchmark these consts; these are all untested assumptions for now.
const BACKED_CANDIDATE_WEIGHT: Weight = 100_000;
const CODE_UPGRADE_WEIGHT_FIXED: Weight = 400_000;
const CODE_UPGRADE_WEIGHT_VARIABLE: Weight = 400_000;
const DISPUTE_PER_STATEMENT_WEIGHT: Weight = 200_000;
const BITFIELD_WEIGHT: Weight = 200_000;
const INCLUSION_INHERENT_CLAIMED_WEIGHT: Weight = 1_000_000_000;
// we assume that 75% of an paras inherent's weight is used processing backed candidates
const MINIMAL_INCLUSION_INHERENT_WEIGHT: Weight = INCLUSION_INHERENT_CLAIMED_WEIGHT / 4;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	#[pallet::disable_frame_system_supertrait_check]
	pub trait Config: inclusion::Config + scheduler::Config {}

	#[pallet::error]
	pub enum Error<T> {
		/// Inclusion inherent called more than once per block.
		TooManyInclusionInherents,
		/// The hash of the submitted parent header doesn't correspond to the saved block hash of
		/// the parent.
		InvalidParentHeader,
		/// Disputed candidate that was concluded invalid.
		CandidateConcludedInvalid,
	}

	/// Whether the paras inherent was included within this block.
	///
	/// The `Option<()>` is effectively a `bool`, but it never hits storage in the `None` variant
	/// due to the guarantees of FRAME's storage APIs.
	///
	/// If this is `None` at the end of the block, we panic and render the block invalid.
	#[pallet::storage]
	pub(crate) type Included<T> = StorageValue<_, ()>;

	/// Scraped on chain data for extracting resolved disputes as well as backing votes.
	#[pallet::storage]
	#[pallet::getter(fn on_chain_votes)]
	pub(crate) type OnChainVotes<T: Config> = StorageValue<_, ScrapedOnChainVotes<T::Hash>>;

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
		fn on_initialize(_: T::BlockNumber) -> Weight {
			T::DbWeight::get().reads_writes(1, 1) // in on_finalize.
		}

		fn on_finalize(_: T::BlockNumber) {
			if Included::<T>::take().is_none() {
				panic!("Bitfields and heads must be included every block");
			}
		}
	}

	#[pallet::inherent]
	impl<T: Config> ProvideInherent for Pallet<T> {
		type Call = Call<T>;
		type Error = MakeFatalError<()>;
		const INHERENT_IDENTIFIER: InherentIdentifier = PARACHAINS_INHERENT_IDENTIFIER;

		fn create_inherent(data: &InherentData) -> Option<Self::Call> {
			let mut inherent_data: ParachainsInherentData<T::Header> =
				match data.get_data(&Self::INHERENT_IDENTIFIER) {
					Ok(Some(d)) => d,
					Ok(None) => return None,
					Err(_) => {
						log::warn!(target: LOG_TARGET, "ParachainsInherentData failed to decode");

						return None
					},
				};

			// filter out any unneeded dispute statements
			T::DisputesHandler::filter_multi_dispute_data(&mut inherent_data.disputes);

			// Limit the total number of inherent data in the block
			limit_paras_inherent::<T>(
				&mut inherent_data.disputes,
				&mut inherent_data.bitfields,
				&mut inherent_data.backed_candidates,
			);

			// Sanity check: session changes can invalidate an inherent, and we _really_ don't want that to happen.
			// See github.com/paritytech/polkadot/issues/1327
			let inherent_data =
				match Self::enter(frame_system::RawOrigin::None.into(), inherent_data.clone()) {
					Ok(_) => inherent_data,
					Err(err) => {
						log::warn!(
						target: LOG_TARGET,
						"dropping signed_bitfields and backed_candidates because they produced \
						an invalid paras inherent: {:?}",
						err.error,
					);

						ParachainsInherentData {
							bitfields: Vec::new(),
							backed_candidates: Vec::new(),
							disputes: Vec::new(),
							parent_header: inherent_data.parent_header,
						}
					},
				};

			Some(Call::enter { data: inherent_data })
		}

		fn is_inherent(call: &Self::Call) -> bool {
			matches!(call, Call::enter { .. })
		}
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Enter the paras inherent. This will process bitfields and backed candidates.
		#[pallet::weight((
			MINIMAL_INCLUSION_INHERENT_WEIGHT +
				dispute_statements_weight(&data.disputes) +
				signed_bitfields_weight(&data.bitfields) +
				backed_candidates_weight::<T>(&data.backed_candidates),
			DispatchClass::Mandatory,
		))]
		pub fn enter(
			origin: OriginFor<T>,
			data: ParachainsInherentData<T::Header>,
		) -> DispatchResultWithPostInfo {
			let ParachainsInherentData {
				bitfields: signed_bitfields,
				backed_candidates,
				parent_header,
				disputes,
			} = data;

			ensure_none(origin)?;
			ensure!(!Included::<T>::exists(), Error::<T>::TooManyInclusionInherents);

			// Check that the submitted parent header indeed corresponds to the previous block hash.
			let parent_hash = <frame_system::Pallet<T>>::parent_hash();
			ensure!(
				parent_header.hash().as_ref() == parent_hash.as_ref(),
				Error::<T>::InvalidParentHeader,
			);

			let disputes_weight = dispute_statements_weight(&disputes);
			let bitfields_weight = signed_bitfields_weight(&signed_bitfields);
			let backed_candidates_weight = backed_candidates_weight::<T>(&backed_candidates);

			// Handle disputes logic.
			let current_session = <shared::Pallet<T>>::session_index();
			{
				let new_current_dispute_sets: Vec<_> = disputes
					.iter()
					.filter(|s| s.session == current_session)
					.map(|s| (s.session, s.candidate_hash))
					.collect();

				let _ = T::DisputesHandler::provide_multi_dispute_data(disputes.clone())?;
				if T::DisputesHandler::is_frozen() {
					// The relay chain we are currently on is invalid. Proceed no further on parachains.
					Included::<T>::set(Some(()));
					return Ok(Some(MINIMAL_INCLUSION_INHERENT_WEIGHT).into())
				}

				let mut freed_disputed = if !new_current_dispute_sets.is_empty() {
					let concluded_invalid_disputes: Vec<_> = new_current_dispute_sets
						.iter()
						.filter(|(s, c)| T::DisputesHandler::concluded_invalid(*s, *c))
						.map(|(_, c)| *c)
						.collect();

					<inclusion::Pallet<T>>::collect_disputed(concluded_invalid_disputes)
						.into_iter()
						.map(|core| (core, FreedReason::Concluded))
						.collect()
				} else {
					Vec::new()
				};

				if !freed_disputed.is_empty() {
					// unstable sort is fine, because core indices are unique
					// i.e. the same candidate can't occupy 2 cores at once.
					freed_disputed.sort_unstable_by_key(|pair| pair.0); // sort by core index
					<scheduler::Pallet<T>>::free_cores(freed_disputed);
				}
			};

			// Process new availability bitfields, yielding any availability cores whose
			// work has now concluded.
			let expected_bits = <scheduler::Pallet<T>>::availability_cores().len();
			let freed_concluded = <inclusion::Pallet<T>>::process_bitfields(
				expected_bits,
				signed_bitfields,
				<scheduler::Pallet<T>>::core_para,
			)?;

			// Inform the disputes module of all included candidates.
			let now = <frame_system::Pallet<T>>::block_number();
			for (_, candidate_hash) in &freed_concluded {
				T::DisputesHandler::note_included(current_session, *candidate_hash, now);
			}

			// Handle timeouts for any availability core work.
			let availability_pred = <scheduler::Pallet<T>>::availability_timeout_predicate();
			let freed_timeout = if let Some(pred) = availability_pred {
				<inclusion::Pallet<T>>::collect_pending(pred)
			} else {
				Vec::new()
			};

			// Schedule paras again, given freed cores, and reasons for freeing.
			let mut freed = freed_concluded
				.into_iter()
				.map(|(c, _hash)| (c, FreedReason::Concluded))
				.chain(freed_timeout.into_iter().map(|c| (c, FreedReason::TimedOut)))
				.collect::<Vec<_>>();

			// unstable sort is fine, because core indices are unique.
			freed.sort_unstable_by_key(|pair| pair.0); // sort by core index

			<scheduler::Pallet<T>>::clear();
			<scheduler::Pallet<T>>::schedule(freed, <frame_system::Pallet<T>>::block_number());

			// Refuse to back any candidates that were disputed and are concluded invalid.
			for candidate in &backed_candidates {
				ensure!(
					!T::DisputesHandler::concluded_invalid(
						current_session,
						candidate.candidate.hash(),
					),
					Error::<T>::CandidateConcludedInvalid,
				);
			}

			// Process backed candidates according to scheduled cores.
			let parent_storage_root = parent_header.state_root().clone();
			let inclusion::ProcessedCandidates::<<T::Header as HeaderT>::Hash> {
				core_indices: occupied,
				candidate_receipt_with_backing_validator_indices,
			} = <inclusion::Pallet<T>>::process_candidates(
				parent_storage_root,
				backed_candidates,
				<scheduler::Pallet<T>>::scheduled(),
				<scheduler::Pallet<T>>::group_validators,
			)?;

			// The number of disputes included in a block is
			// limited by the weight as well as the number of candidate blocks.
			OnChainVotes::<T>::put(ScrapedOnChainVotes::<<T::Header as HeaderT>::Hash> {
				session: current_session,
				backing_validators_per_candidate: candidate_receipt_with_backing_validator_indices,
				disputes,
			});

			// Note which of the scheduled cores were actually occupied by a backed candidate.
			<scheduler::Pallet<T>>::occupied(&occupied);

			// Give some time slice to dispatch pending upward messages.
			<ump::Pallet<T>>::process_pending_upward_messages();

			// And track that we've finished processing the inherent for this block.
			Included::<T>::set(Some(()));

			Ok(Some(
				MINIMAL_INCLUSION_INHERENT_WEIGHT +
					disputes_weight + bitfields_weight +
					backed_candidates_weight,
			)
			.into())
		}
	}
}

fn dispute_statements_weight(disputes: &[DisputeStatementSet]) -> Weight {
	disputes
		.iter()
		.map(|d| d.statements.len() as Weight * DISPUTE_PER_STATEMENT_WEIGHT)
		.sum()
}

fn signed_bitfields_weight(bitfields: &[UncheckedSignedAvailabilityBitfield]) -> Weight {
	bitfields.len() as Weight * BITFIELD_WEIGHT
}

fn backed_candidate_weight<T: frame_system::Config>(
	candidate: &BackedCandidate<T::Hash>,
) -> Weight {
	match &candidate.candidate.commitments.new_validation_code {
		Some(v) => v.0.len() as u64 * CODE_UPGRADE_WEIGHT_VARIABLE + CODE_UPGRADE_WEIGHT_FIXED,
		_ => 0 as Weight,
	}
}

fn backed_candidates_weight<T: frame_system::Config>(
	candidates: &[BackedCandidate<T::Hash>],
) -> Weight {
	candidates.iter().map(|c| backed_candidate_weight::<T>(c)).sum()
}

/// Limit the number of disputes, signed bitfields and backed candidates processed in order to stay
/// within block weight limits.
///
/// Use a configured assumption about the weight required to process a backed candidate and the
/// current block weight as of the execution of this function to ensure that we don't overload
/// the block with candidate processing.
///
/// If the backed candidates exceed the available block weight remaining, then skips all of them.
/// This is somewhat less desirable than attempting to fit some of them, but is more fair in the
/// even that we can't trust the provisioner to provide a fair / random ordering of candidates.
fn limit_paras_inherent<T: Config>(
	disputes: &mut MultiDisputeStatementSet,
	bitfields: &mut UncheckedSignedAvailabilityBitfields,
	backed_candidates: &mut Vec<BackedCandidate<T::Hash>>,
) {
	let available_block_weight = <T as frame_system::Config>::BlockWeights::get()
		.max_block
		.saturating_sub(frame_system::Pallet::<T>::block_weight().total());

	let disputes_weight = dispute_statements_weight(&disputes);
	if disputes_weight > available_block_weight {
		// Sort the dispute statements according to the following prioritization:
		//  1. Prioritize local disputes over remote disputes.
		//  2. Prioritize older disputes over newer disputes.
		//  3. Prioritize local disputes withan invalid vote over valid votes.
		disputes.sort_by(|a, b| {
			let a_local_block = T::DisputesHandler::included_state(a.session, a.candidate_hash);
			let b_local_block = T::DisputesHandler::included_state(b.session, b.candidate_hash);
			match (a_local_block, b_local_block) {
				// Prioritize local disputes over remote disputes.
				(None, Some(_)) => Ordering::Less,
				(Some(_), None) => Ordering::Greater,
				// For local disputes, prioritize those that occur at an earlier height.
				(Some(a_height), Some(b_height)) => a_height.cmp(&b_height),
				// Prioritize earlier remote disputes using session as rough proxy.
				(None, None) => a.session.cmp(&b.session),
			}
		});

		let mut total_weight = 0;
		disputes.retain(|d| {
			total_weight += d.statements.len() as Weight * DISPUTE_PER_STATEMENT_WEIGHT;
			total_weight <= available_block_weight
		});

		bitfields.clear();
		backed_candidates.clear();
		return
	}

	let bitfields_weight = signed_bitfields_weight(&bitfields);
	if disputes_weight + bitfields_weight > available_block_weight {
		let num_bitfields =
			available_block_weight.saturating_sub(disputes_weight) / BITFIELD_WEIGHT;
		let _ = bitfields.drain(num_bitfields as usize..);
		backed_candidates.clear();
		return
	}

	let block_weight_available_for_candidates = available_block_weight
		.saturating_sub(disputes_weight)
		.saturating_sub(bitfields_weight);

	let mut total_weight = 0;
	backed_candidates.retain(|c| {
		let code_upgrade_weight = backed_candidate_weight::<T>(c);
		total_weight +=
			c.validity_votes.len() as Weight * BACKED_CANDIDATE_WEIGHT + code_upgrade_weight;
		total_weight < block_weight_available_for_candidates
	});
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{new_test_ext, MockGenesisConfig, System, Test};
	use primitives::{
		v0::{ValidatorSignature, ValidityAttestation},
		v1::{
			CandidateHash, DisputeStatement, SessionIndex, ValidDisputeStatementKind,
			ValidatorIndex,
		},
	};

	mod limit_backed_candidates {
		use super::*;
		use no_std_compat::cmp::min;
		use primitives::v0::ValidatorPair;
		use sp_core::Pair;

		fn sign_hash(index: u64, candidate_hash: &CandidateHash) -> ValidatorSignature {
			let mut seed = [0u8; 32usize];
			seed[31] = (index % (1 << 8)) as u8;
			seed[30] = ((index >> 8) % (1 << 8)) as u8;
			seed[29] = ((index >> 16) % (1 << 8)) as u8;
			seed[29] = ((index >> 24) % (1 << 8)) as u8;
			let pair = ValidatorPair::from_seed_slice(&seed).unwrap();
			pair.sign(candidate_hash.0.as_ref())
		}

		fn make_dispute(num_statements: u32, session: SessionIndex) -> DisputeStatementSet {
			let mut dispute = DisputeStatementSet {
				candidate_hash: CandidateHash::default(),
				session,
				statements: vec![],
			};
			let candidate_hash = &dispute.candidate_hash;
			for i in 0..num_statements {
				dispute.statements.push((
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(i),
					sign_hash(i as u64, candidate_hash),
				));
			}
			dispute
		}

		fn make_backed_candidate(
			num_signatures: u64,
			includes_code_upgrade: bool,
		) -> BackedCandidate {
			let mut backed = BackedCandidate::default();
			let candidate_hash = backed.hash();
			for i in 0..num_signatures {
				backed
					.validity_votes
					.push(ValidityAttestation::Implicit(sign_hash(i, &candidate_hash)));
			}
			if includes_code_upgrade {
				backed.candidate.commitments.new_validation_code = Some(Vec::new().into());
			}
			backed
		}

		fn make_inherent_data(
			num_statements: u32,
			dispute_sessions: Vec<SessionIndex>,
			num_candidates: u64,
			num_signatures: u64,
			includes_code_upgrade: bool,
		) -> (MultiDisputeStatementSet, Vec<BackedCandidate>) {
			(
				dispute_sessions
					.into_iter()
					.map(|session| make_dispute(num_statements, session))
					.collect(),
				(0..num_candidates)
					.map(|_| make_backed_candidate(num_signatures, includes_code_upgrade))
					.collect(),
			)
		}

		#[test]
		fn does_not_truncate_on_empty_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let mut disputes = vec![];
				let mut bitfields = vec![];
				let mut backed_candidates = vec![BackedCandidate::default()];

				System::set_block_consumed_resources(0, 0);
				limit_paras_inherent::<Test>(&mut disputes, &mut bitfields, &mut backed_candidates);
				assert_eq!(backed_candidates.len(), 1);
			});
		}

		#[test]
		fn does_not_truncate_on_exactly_full_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let mut disputes = vec![];
				let mut bitfields = vec![];
				let mut backed_candidates = vec![BackedCandidate::default()];

				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight, 0);
				limit_paras_inherent::<Test>(&mut disputes, &mut bitfields, &mut backed_candidates);
				// TODO ladi - Does set_block_consumed_resources include backed candidates?
				assert_eq!(backed_candidates.len(), 0);
			});
		}

		#[test]
		fn truncates_on_over_full_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let mut disputes = vec![];
				let mut bitfields = vec![];
				let mut backed_candidates = vec![BackedCandidate::default()];

				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight + 1, 0);
				limit_paras_inherent::<Test>(&mut disputes, &mut bitfields, &mut backed_candidates);
				assert_eq!(backed_candidates.len(), 0);
			});
		}

		#[test]
		fn all_backed_candidates_get_truncated() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let mut disputes = vec![];
				let mut bitfields = vec![];
				let mut backed_candidates = vec![BackedCandidate::default()];

				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight + 1, 0);
				limit_paras_inherent::<Test>(&mut disputes, &mut bitfields, &mut backed_candidates);
			});
		}

		#[test]
		fn remote_disputes_untouched_when_not_overlength() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let (mut disputes, _) = make_inherent_data(5, vec![3, 2, 1], 0, 0, true);
				let mut bitfields = Vec::new();
				let mut backed_candidates = Vec::new();

				limit_paras_inherent::<Test>(&mut disputes, &mut bitfields, &mut backed_candidates);
				assert_eq!(disputes.len(), 3);
				assert_eq!(disputes[0].session, 3);
				assert_eq!(disputes[1].session, 2);
				assert_eq!(disputes[2].session, 1);
			});
		}

		#[test]
		fn remote_disputes_sorted_by_session_when_overlength() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let (mut disputes, _) = make_inherent_data(10, vec![3, 2, 1], 0, 0, true);
				let mut bitfields = Vec::new();
				let mut backed_candidates = Vec::new();

				limit_paras_inherent::<Test>(&mut disputes, &mut bitfields, &mut backed_candidates);
				assert_eq!(disputes.len(), 2);
				assert_eq!(disputes[0].session, 1);
				assert_eq!(disputes[1].session, 2);
			});
		}

		#[test]
		fn does_not_ignore_subsequent_code_upgrades() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let mut disputes = vec![];
				let mut bitfields = vec![];
				let (_, mut backed_candidates) = make_inherent_data(0, Vec::new(), 3, 1, true);

				limit_paras_inherent::<Test>(&mut disputes, &mut bitfields, &mut backed_candidates);
				assert_eq!(backed_candidates.len(), 3);
			});
		}

		fn truncates_full_block(num_candidates: u64, num_signatures: u64, code_upgrades: bool) {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				let mut disputes = vec![];
				let mut bitfields = vec![];
				let (_, mut backed_candidates) = make_inherent_data(
					0,
					Vec::new(),
					num_candidates,
					num_signatures,
					code_upgrades,
				);

				limit_paras_inherent::<Test>(&mut disputes, &mut bitfields, &mut backed_candidates);
				assert_eq!(
					backed_candidates.len() as u64,
					min(
						num_candidates,
						max_block_weight /
							(num_signatures as u64 * BACKED_CANDIDATE_WEIGHT +
								if code_upgrades { CODE_UPGRADE_WEIGHT_FIXED } else { 0 })
					)
				);
			});
		}

		fn truncates_full_block_matrix_test(code_upgrades: bool) {
			for i in 1..10 {
				for j in 1..10 {
					truncates_full_block(i, j, code_upgrades);
				}
			}
		}

		#[test]
		fn truncates_full_block_without_code_upgrade() {
			truncates_full_block_matrix_test(false);
		}

		#[test]
		fn truncates_full_block_with_code_upgrade() {
			truncates_full_block_matrix_test(true);
		}
	}

	mod paras_inherent_weight {
		use super::*;

		use crate::mock::{new_test_ext, MockGenesisConfig, System, Test};
		use primitives::v1::Header;

		use frame_support::traits::UnfilteredDispatchable;

		fn default_header() -> Header {
			Header {
				parent_hash: Default::default(),
				number: 0,
				state_root: Default::default(),
				extrinsics_root: Default::default(),
				digest: Default::default(),
			}
		}

		/// We expect the weight of the paras inherent not to change when no truncation occurs:
		/// its weight is dynamically computed from the size of the backed candidates list, and is
		/// already incorporated into the current block weight when it is selected by the provisioner.
		#[test]
		fn weight_does_not_change_on_happy_path() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let header = default_header();
				System::set_block_number(1);
				System::set_parent_hash(header.hash());

				// number of bitfields doesn't affect the paras inherent weight, so we can mock it with an empty one
				let signed_bitfields = Vec::new();
				// backed candidates must not be empty, so we can demonstrate that the weight has not changed
				let backed_candidates = vec![BackedCandidate::default(); 10];

				// the expected weight can always be computed by this formula
				let expected_weight = MINIMAL_INCLUSION_INHERENT_WEIGHT +
					(backed_candidates.len() as Weight * BACKED_CANDIDATE_WEIGHT);

				// we've used half the block weight; there's plenty of margin
				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				let used_block_weight = max_block_weight / 2;
				System::set_block_consumed_resources(used_block_weight, 0);

				// execute the paras inherent
				let post_info = Call::<Test>::enter {
					data: ParachainsInherentData {
						bitfields: signed_bitfields,
						backed_candidates,
						disputes: Vec::new(),
						parent_header: default_header(),
					},
				}
				.dispatch_bypass_filter(None.into())
				.unwrap_err()
				.post_info;

				// we don't directly check the block's weight post-call. Instead, we check that the
				// call has returned the appropriate post-dispatch weight for refund, and trust
				// Substrate to do the right thing with that information.
				//
				// In this case, the weight system can update the actual weight with the same amount,
				// or return `None` to indicate that the pre-computed weight should not change.
				// Either option is acceptable for our purposes.
				if let Some(actual_weight) = post_info.actual_weight {
					assert_eq!(actual_weight, expected_weight);
				}
			});
		}

		/// We expect the weight of the paras inherent to change when truncation occurs: its
		/// weight was initially dynamically computed from the size of the backed candidates list,
		/// but was reduced by truncation.
		#[test]
		fn weight_changes_when_backed_candidates_are_truncated() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let header = default_header();
				System::set_block_number(1);
				System::set_parent_hash(header.hash());

				// number of bitfields doesn't affect the paras inherent weight, so we can mock it with an empty one
				let mut signed_bitfields = Vec::new();
				// backed candidates must not be empty, so we can demonstrate that the weight has not changed
				let mut backed_candidates = vec![BackedCandidate::default(); 10];

				// the expected weight with no blocks is just the minimum weight
				let expected_weight = MINIMAL_INCLUSION_INHERENT_WEIGHT;

				// oops, looks like this mandatory call pushed the block weight over the limit
				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				let used_block_weight = max_block_weight + 1;

				System::set_block_consumed_resources(used_block_weight, 0);

				let mut disputes = Vec::new();
				limit_paras_inherent::<Test>(
					&mut disputes,
					&mut signed_bitfields,
					&mut backed_candidates,
				);

				// execute the paras inherent
				let post_info = Call::<Test>::enter {
					data: ParachainsInherentData {
						bitfields: signed_bitfields,
						backed_candidates,
						disputes,
						parent_header: header,
					},
				}
				.dispatch_bypass_filter(None.into())
				.unwrap();

				// we don't directly check the block's weight post-call. Instead, we check that the
				// call has returned the appropriate post-dispatch weight for refund, and trust
				// Substrate to do the right thing with that information.
				assert_eq!(post_info.actual_weight.unwrap(), expected_weight);
			});
		}
	}
}
