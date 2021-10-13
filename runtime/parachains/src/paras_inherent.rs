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

use crate::{configuration::Config, disputes::DisputesHandler, inclusion, scheduler::{self, CoreAssignment, FreedReason}, shared, ump};
use bitvec::prelude::BitVec;
use frame_support::{
	inherent::{InherentData, InherentIdentifier, MakeFatalError, ProvideInherent},
	pallet_prelude::*,
	fail,
};
use frame_system::pallet_prelude::*;
use primitives::v1::{
	AvailabilityBitfield, ValidatorIndex,
	BackedCandidate, CandidateHash, CoreIndex,
	InherentData as ParachainsInherentData, ScrapedOnChainVotes, SessionIndex,
	SigningContext, UncheckedSignedAvailabilityBitfields, ValidatorId,
	PARACHAINS_INHERENT_IDENTIFIER,
};
use scale_info::TypeInfo;
use sp_runtime::traits::Header as HeaderT;
use sp_std::{
	collections::{btree_map::BTreeMap, btree_set::BTreeSet},
	prelude::*,
};

pub use pallet::*;

const LOG_TARGET: &str = "runtime::inclusion-inherent";
// In the future, we should benchmark these consts; these are all untested assumptions for now.
const BACKED_CANDIDATE_WEIGHT: Weight = 100_000;
const INCLUSION_INHERENT_CLAIMED_WEIGHT: Weight = 1_000_000_000;
// we assume that 75% of an paras inherent's weight is used processing backed candidates
const MINIMAL_INCLUSION_INHERENT_WEIGHT: Weight = INCLUSION_INHERENT_CLAIMED_WEIGHT / 4;

/// A bitfield concering concluded disputes for candidates
/// associated to the core index equivalent to the bit position.
#[derive(Default, PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
pub struct DisputedBitfield(pub BitVec<bitvec::order::Lsb0, u8>);

impl From<BitVec<bitvec::order::Lsb0, u8>> for DisputedBitfield {
	fn from(inner: BitVec<bitvec::order::Lsb0, u8>) -> Self {
		Self(inner)
	}
}

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

			// Sanity check: session changes can invalidate an inherent, and we _really_ don't want that to happen.
			// See <https://github.com/paritytech/polkadot/issues/1327>
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
			MINIMAL_INCLUSION_INHERENT_WEIGHT + data.backed_candidates.len() as Weight * BACKED_CANDIDATE_WEIGHT,
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

			let expected_bits = <scheduler::Pallet<T>>::availability_cores().len();

			// Handle disputes logic.
			let current_session = <shared::Pallet<T>>::session_index();
			let (disputed_bits, concluded_invalid_disputed_candidates) = {
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

				let (mut freed_disputed, concluded_invalid_disputed_candidates) =
					if !new_current_dispute_sets.is_empty() {
						let concluded_invalid_disputes = new_current_dispute_sets
							.iter()
							.filter(|(session, candidate)| {
								T::DisputesHandler::concluded_invalid(*session, *candidate)
							})
							.map(|(_, candidate)| *candidate)
							.collect::<BTreeSet<CandidateHash>>();

						let freed_disputed =
							<inclusion::Pallet<T>>::collect_disputed(&concluded_invalid_disputes)
								.into_iter()
								.map(|core| (core, FreedReason::Concluded))
								.collect();
						(freed_disputed, concluded_invalid_disputes)
					} else {
						(Vec::new(), BTreeSet::new())
					};

				// create a bit index from the set of core indicies.
				let disputed_bitfield = {
					let mut bitvec = BitVec::with_capacity(expected_bits);
					if expected_bits > 0 {
						bitvec.set(expected_bits.saturating_sub(1), false);
						for (core_idx, _) in &freed_disputed {
							let core_idx = core_idx.0 as usize;
							if core_idx < expected_bits {
								bitvec.set(core_idx, true);
							}
						}
					}
					DisputedBitfield::from(bitvec)
				};

				if !freed_disputed.is_empty() {
					// unstable sort is fine, because core indices are unique
					// i.e. the same candidate can't occupy 2 cores at once.
					freed_disputed.sort_unstable_by_key(|pair| pair.0); // sort by core index
					<scheduler::Pallet<T>>::free_cores(freed_disputed);
				}

				(disputed_bitfield, concluded_invalid_disputed_candidates)
			};

			let validators = shared::Pallet::<T>::active_validator_keys();

			let checked_bitfields = sanitize_bitfields::<T, true>(
				signed_bitfields,
				disputed_bits,
				expected_bits,
				parent_hash,
				current_session,
				&validators,
			)?;

			// Process new availability bitfields, yielding any availability cores whose
			// work has now concluded.
			let freed_concluded = <inclusion::Pallet<T>>::process_bitfields(
				expected_bits,
				checked_bitfields,
				<scheduler::Pallet<T>>::core_para,
				&validators,
			);

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
			let freed = freed_concluded
				.into_iter()
				.map(|(c, _hash)| (c, FreedReason::Concluded))
				.chain(freed_timeout.into_iter().map(|c| (c, FreedReason::TimedOut)))
				.collect::<BTreeMap<CoreIndex, FreedReason>>();

			<scheduler::Pallet<T>>::clear();
			<scheduler::Pallet<T>>::schedule(freed, <frame_system::Pallet<T>>::block_number());

			let scheduled = <scheduler::Pallet<T>>::scheduled();
			let backed_candidates = sanitize_backed_candidates::<T, true>(
				parent_hash,
				backed_candidates,
				concluded_invalid_disputed_candidates,
				&scheduled[..],
			)?;
			let backed_candidates = limit_backed_candidates::<T>(backed_candidates);
			let backed_candidates_len = backed_candidates.len() as Weight;

			// Process backed candidates according to scheduled cores.
			let parent_storage_root = parent_header.state_root().clone();
			let inclusion::ProcessedCandidates::<<T::Header as HeaderT>::Hash> {
				core_indices: occupied,
				candidate_receipt_with_backing_validator_indices,
			} = <inclusion::Pallet<T>>::process_candidates(
				parent_storage_root,
				backed_candidates,
				scheduled,
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
					(backed_candidates_len * BACKED_CANDIDATE_WEIGHT),
			)
			.into())
		}
	}
}

macro_rules! ensure2 {
	($condition:expr, $err:expr, $action:ident) => {
		let condition = $condition;
		if !condition {
			if $action {
				ensure!(condition, $err);
			}
		}
	};
}

/// Filter bitfields based on freed core indices, validity, and other sanity checks.
///
/// Do sanity checks on the bitfields:
///
///  1. no more than one bitfield per validator
///  2. bitfields are ascending by validator index.
///  3. each bitfield has exactly `expected_bits`
///  4. signature is valid
///  5. remove any disputed core indices
///
/// If any of those is not passed, the bitfield is dropped.
fn sanitize_bitfields<T: Config + crate::inclusion::Config, const EARLY_RETURN: bool>(
	unchecked_bitfields: UncheckedSignedAvailabilityBitfields,
	disputed_bits: DisputedBitfield,
	expected_bits: usize,
	parent_hash: T::Hash,
	session_index: SessionIndex,
	validators: &[ValidatorId],
) -> Result<Vec<(AvailabilityBitfield, ValidatorIndex)>, DispatchError> {
	let mut bitfields = Vec::with_capacity(unchecked_bitfields.len());

	let mut last_index = None;

	if EARLY_RETURN && disputed_bits.0.len() != expected_bits{
		return Ok(Default::default())
	}

	for unchecked_bitfield in unchecked_bitfields {
		let signing_context = SigningContext { parent_hash, session_index };

		ensure2!(
			unchecked_bitfield.unchecked_payload().0.len() == expected_bits,
			crate::inclusion::pallet::Error::<T>::WrongBitfieldSize,
			EARLY_RETURN
		);

		ensure2!(
			last_index
				.map_or(true, |last| last < unchecked_bitfield.unchecked_validator_index()),
				crate::inclusion::pallet::Error::<T>::BitfieldDuplicateOrUnordered,
			EARLY_RETURN
		);

		ensure2!(
			(unchecked_bitfield.unchecked_validator_index().0 as usize) < validators.len(),
			crate::inclusion::pallet::Error::<T>::ValidatorIndexOutOfBounds,
			EARLY_RETURN
		);

		let validator_index = unchecked_bitfield.unchecked_validator_index();

		let validator_public = &validators[validator_index.0 as usize];

		let signed_bitfield = if let Ok(signed_bitfield) =
			unchecked_bitfield.try_into_checked(&signing_context, validator_public)
		{
			signed_bitfield
		} else if EARLY_RETURN {
			fail!(crate::inclusion::pallet::Error::<T>::InvalidBitfieldSignature);
		} else {
			continue;
		};

		last_index = Some(validator_index);

		let mut checked_bitfield = signed_bitfield.into_payload();

		// filter the bitfields only
		if !EARLY_RETURN {
			checked_bitfield.0.iter_mut()
				.enumerate()
				.for_each(|(core_idx, mut bit)| {
					*bit = if *bit {
						// ignore those that have matching invalid dispute bits
						// which always works, since we checked for uniformat bit length
						// before
						!disputed_bits.0[core_idx]
					} else {
						// bit wasn't set in the first place
						false
					};
				});
		}

		bitfields.push((checked_bitfield, validator_index));
	}
	Ok(bitfields)
}

/// Filter out any candidates, that have a concluded invalid dispute.
///
/// `scheduled` follows the same naming scheme as provided in the
/// guide: Currently `free` but might become `occupied`.
/// For the filtering here the relevant part is only the current `free`
/// state.
fn sanitize_backed_candidates<T: Config + crate::paras_inherent::Config, const EARLY_RETURN: bool>(
	relay_parent: T::Hash,
	mut backed_candidates: Vec<BackedCandidate<T::Hash>>,
	disputed_candidates: BTreeSet<CandidateHash>,
	scheduled: &[CoreAssignment],
) -> Result<Vec<BackedCandidate<T::Hash>>, Error::<T>> {
	let n = backed_candidates.len();
	// Remove any candidates that were concluded invalid.
	backed_candidates.retain(|backed_candidate| {
		let candidate_hash = backed_candidate.candidate.hash();
		!disputed_candidates.contains(&candidate_hash)
	});
	ensure2!(backed_candidates.len() == n, Error::<T>::CandidateConcludedInvalid, EARLY_RETURN);

	// Assure the backed candidate's `ParaId`'s core is free.
	// This holds under the assumption that `Scheduler::schedule` is called _before_.
	// Also checks the candidate references the correct relay parent.
	backed_candidates.retain(|backed_candidate| {
		let desc = backed_candidate.descriptor();
		desc.relay_parent == relay_parent
		&& scheduled.iter().any(|core| core.para_id == desc.para_id)
	});
	ensure2!(backed_candidates.len() == n, Error::<T>::CandidateConcludedInvalid, EARLY_RETURN);

	// Limit weight, to avoid overweight block.
	Ok(backed_candidates)
}

/// Limit the number of backed candidates processed in order to stay within block weight limits.
///
/// Use a configured assumption about the weight required to process a backed candidate and the
/// current block weight as of the execution of this function to ensure that we don't overload
/// the block with candidate processing.
///
/// If the backed candidates exceed the available block weight remaining, then skips all of them.
/// This is somewhat less desirable than attempting to fit some of them, but is more fair in the
/// even that we can't trust the provisioner to provide a fair / random ordering of candidates.
fn limit_backed_candidates<T: Config>(
	mut backed_candidates: Vec<BackedCandidate<T::Hash>>,
) -> Vec<BackedCandidate<T::Hash>> {
	const MAX_CODE_UPGRADES: usize = 1;

	// Ignore any candidates beyond one that contain code upgrades.
	//
	// This is an artificial limitation that does not appear in the guide as it is a practical
	// concern around execution.
	{
		let mut code_upgrades = 0;
		backed_candidates.retain(|c| {
			if c.candidate.commitments.new_validation_code.is_some() {
				if code_upgrades >= MAX_CODE_UPGRADES {
					return false
				}

				code_upgrades += 1;
			}

			true
		});
	}

	// the weight of the paras inherent is already included in the current block weight,
	// so our operation is simple: if the block is currently overloaded, make this intrinsic smaller
	if frame_system::Pallet::<T>::block_weight().total() >
		<T as frame_system::Config>::BlockWeights::get().max_block
	{
		Vec::new()
	} else {
		backed_candidates
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::mock::{new_test_ext, MockGenesisConfig, System, Test};

	mod limit_backed_candidates {
		use super::*;

		#[test]
		fn does_not_truncate_on_empty_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let backed_candidates = vec![BackedCandidate::default()];
				System::set_block_consumed_resources(0, 0);
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 1);
			});
		}

		#[test]
		fn does_not_truncate_on_exactly_full_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let backed_candidates = vec![BackedCandidate::default()];
				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight, 0);
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 1);
			});
		}

		#[test]
		fn truncates_on_over_full_block() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let backed_candidates = vec![BackedCandidate::default()];
				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight + 1, 0);
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 0);
			});
		}

		#[test]
		fn all_backed_candidates_get_truncated() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let backed_candidates = vec![BackedCandidate::default(); 10];
				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				// if the consumed resources are precisely equal to the max block weight, we do not truncate.
				System::set_block_consumed_resources(max_block_weight + 1, 0);
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 0);
			});
		}

		#[test]
		fn ignores_subsequent_code_upgrades() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let mut backed = BackedCandidate::default();
				backed.candidate.commitments.new_validation_code = Some(Vec::new().into());
				let backed_candidates = (0..3).map(|_| backed.clone()).collect();
				assert_eq!(limit_backed_candidates::<Test>(backed_candidates).len(), 1);
			});
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
				let signed_bitfields = Vec::new();
				// backed candidates must not be empty, so we can demonstrate that the weight has not changed
				let backed_candidates = vec![BackedCandidate::default(); 10];

				// the expected weight with no blocks is just the minimum weight
				let expected_weight = MINIMAL_INCLUSION_INHERENT_WEIGHT;

				// oops, looks like this mandatory call pushed the block weight over the limit
				let max_block_weight =
					<Test as frame_system::Config>::BlockWeights::get().max_block;
				let used_block_weight = max_block_weight + 1;
				System::set_block_consumed_resources(used_block_weight, 0);

				// execute the paras inherent
				let post_info = Call::<Test>::enter {
					data: ParachainsInherentData {
						bitfields: signed_bitfields,
						backed_candidates,
						disputes: Vec::new(),
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
