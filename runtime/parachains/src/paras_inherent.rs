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

use crate::{
	disputes::DisputesHandler,
	inclusion,
	scheduler::{self, CoreAssignment, FreedReason},
	shared, ump,
};
use bitvec::prelude::BitVec;
use frame_support::{
	fail,
	inherent::{InherentData, InherentIdentifier, MakeFatalError, ProvideInherent},
	pallet_prelude::*,
	traits::Randomness,
};
use frame_system::pallet_prelude::*;
use pallet_babe::CurrentBlockRandomness;
use primitives::v1::{
	BackedCandidate, CandidateHash, CoreIndex, InherentData as ParachainsInherentData,
	ScrapedOnChainVotes, SessionIndex, SigningContext, UncheckedSignedAvailabilityBitfield,
	UncheckedSignedAvailabilityBitfields, ValidatorId, PARACHAINS_INHERENT_IDENTIFIER,
};
use rand::Rng;
use scale_info::TypeInfo;
use sp_runtime::traits::Header as HeaderT;
use sp_std::{
	collections::{btree_map::BTreeMap, btree_set::BTreeSet},
	prelude::*,
};

pub use pallet::*;

const LOG_TARGET: &str = "runtime::inclusion-inherent";
// In the future, we should benchmark these consts; these are all untested assumptions for now.
const INCLUSION_INHERENT_CLAIMED_WEIGHT: Weight = 1_000_000_000;
// we assume that 75% of an paras inherent's weight is used processing backed candidates
const MINIMAL_INCLUSION_INHERENT_WEIGHT: Weight = INCLUSION_INHERENT_CLAIMED_WEIGHT / 4;

/// Weights
///
/// TODO replaced this with the true weights
/// TODO as part of <https://github.com/paritytech/polkadot/pull/4044>
/// TODO The current ones are _preliminary_ copies of those.
fn paras_inherent_total_weight<T: Config>(
	n_backed_candidates: u32,
	n_bitfields: u32,
	n_disputes: u32,
) -> Weight {
	let weights = T::DbWeight::get();
	// static
	(10_901_789_000 as Weight)
		// backed candidates
		.saturating_add((424_633_000 as Weight).saturating_mul(n_backed_candidates as Weight))
		.saturating_add(weights.reads(58 as Weight))
		.saturating_add(weights.reads((8 as Weight).saturating_mul(n_backed_candidates as Weight)))
		.saturating_add(weights.writes(212 as Weight))
		.saturating_add(weights.writes((2 as Weight).saturating_mul(n_backed_candidates as Weight)))
		// disputes
		.saturating_add((103_899_000 as Weight).saturating_mul(n_disputes as Weight))
		.saturating_add(weights.reads(61 as Weight))
		.saturating_add(weights.reads((1 as Weight).saturating_mul(n_disputes as Weight)))
		.saturating_add(weights.writes(212 as Weight))
		.saturating_add(weights.writes((2 as Weight).saturating_mul(n_disputes as Weight)))
		// bitfields
		.saturating_add((10_000_000 as Weight).saturating_mul(n_disputes as Weight))
		.saturating_add(weights.reads(10 as Weight))
		.saturating_add(weights.reads((20 as Weight).saturating_mul(n_bitfields as Weight)))
		.saturating_add(weights.writes(10 as Weight))
		.saturating_add(weights.writes((20 as Weight).saturating_mul(n_bitfields as Weight)))
}

/// Extract the static weight.
fn static_weight<T: Config>() -> Weight {
	paras_inherent_total_weight::<T>(0, 0, 0)
}

/// Extract the weight that is added _per_ bitfield.
fn bitfield_weight<T: Config>() -> Weight {
	paras_inherent_total_weight::<T>(0, 1, 0) - static_weight::<T>()
}

/// Extract the weight that is adder _per_ backed candidate.
fn backed_candidate_weight<T: Config>() -> Weight {
	paras_inherent_total_weight::<T>(1, 0, 0) - static_weight::<T>()
}

/// A bitfield concerning concluded disputes for candidates
/// associated to the core index equivalent to the bit position.
#[derive(Default, PartialEq, Eq, Clone, Encode, Decode, RuntimeDebug, TypeInfo)]
pub(crate) struct DisputedBitfield(pub(crate) BitVec<bitvec::order::Lsb0, u8>);

impl From<BitVec<bitvec::order::Lsb0, u8>> for DisputedBitfield {
	fn from(inner: BitVec<bitvec::order::Lsb0, u8>) -> Self {
		Self(inner)
	}
}

#[cfg(test)]
impl DisputedBitfield {
	/// Create a new bitfield, where each bit is set to `false`.
	pub fn zeros(n: usize) -> Self {
		Self::from(BitVec::<bitvec::order::Lsb0, u8>::repeat(false, n))
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
	pub trait Config: inclusion::Config + scheduler::Config + pallet_babe::Config {}

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
			let parent_hash = <frame_system::Pallet<T>>::parent_hash();

			let ParachainsInherentData::<T::Header> {
				bitfields,
				backed_candidates,
				mut disputes,
				parent_header,
			} = match data.get_data(&Self::INHERENT_IDENTIFIER) {
				Ok(Some(d)) => d,
				Ok(None) => return None,
				Err(_) => {
					log::warn!(target: LOG_TARGET, "ParachainsInherentData failed to decode");
					return None
				},
			};

			if parent_hash != parent_header.hash() {
				log::warn!(
					target: LOG_TARGET,
					"ParachainsInherentData references a different parent header hash than frame"
				);
				return None
			}

			let current_session = <shared::Pallet<T>>::session_index();
			let expected_bits = <scheduler::Pallet<T>>::availability_cores().len();
			let validator_public = shared::Pallet::<T>::active_validator_keys();

			T::DisputesHandler::filter_multi_dispute_data(&mut disputes);

			let scheduled: Vec<CoreAssignment> = <scheduler::Pallet<T>>::scheduled();

			let (freed_cores, concluded_invalid_disputes) =
				frame_support::storage::with_transaction(|| {
					// we don't care about fresh or not disputes
					// this writes them to storage, so let's query it via those means
					// if this fails for whatever reason, that's ok
					let _ = T::DisputesHandler::provide_multi_dispute_data(disputes.clone())
						.map_err(|e| {
							log::warn!(
								target: LOG_TARGET,
								"MultiDisputesData failed to update: {:?}",
								e
							);
							e
						});

					// current concluded invalid disputes, only including the current block's votes
					let current_concluded_invalid_disputes = disputes
						.iter()
						.filter(|dss| dss.session == current_session)
						.map(|dss| (dss.session, dss.candidate_hash))
						.filter(|(session, candidate)| {
							<T>::DisputesHandler::concluded_invalid(*session, *candidate)
						})
						.map(|(_session, candidate)| candidate)
						.collect::<BTreeSet<CandidateHash>>();

					let freed_cores = <inclusion::Pallet<T>>::collect_disputed(
						&current_concluded_invalid_disputes,
					);

					// all concluded invalid disputes, that are relevant for the set of candidates
					// the inherent provided
					let concluded_invalid_disputes = backed_candidates
						.iter()
						.map(|backed_candidate| backed_candidate.hash())
						.filter(|candidate| {
							<T>::DisputesHandler::concluded_invalid(current_session, *candidate)
						})
						.collect::<BTreeSet<CandidateHash>>();

					frame_support::storage::TransactionOutcome::Rollback((
						freed_cores,
						concluded_invalid_disputes,
					))
				});

			let disputed_bitfield = create_disputed_bitfield(expected_bits, freed_cores.iter());

			let bitfields = sanitize_bitfields::<T, false>(
				bitfields,
				disputed_bitfield,
				expected_bits,
				parent_hash,
				current_session,
				&validator_public[..],
			)
			.ok()?; // by convention, when called with `EARLY_RETURN=false`, will always return `Ok()`

			let backed_candidates = sanitize_backed_candidates::<T, _, false>(
				parent_hash,
				backed_candidates,
				move |candidate_hash: CandidateHash| -> bool {
					concluded_invalid_disputes.contains(&candidate_hash)
				},
				&scheduled[..],
			)
			.ok()?; // by convention, when called with `EARLY_RETURN=false`, will always return `Ok()`

			let entropy = {
				const CANDIDATE_SEED_SUBJECT: [u8; 32] = *b"candidate-seed-selection-subject";
				let vrf_random = CurrentBlockRandomness::<T>::random(&CANDIDATE_SEED_SUBJECT[..]).0;
				let mut entropy: [u8; 32] = CANDIDATE_SEED_SUBJECT.clone();
				if let Some(vrf_random) = vrf_random {
					entropy.as_mut().copy_from_slice(vrf_random.as_ref());
				} else {
					// in case there is no vrf randomness present, we utilize the relay parent
					// as seed, it's better than a static value.
					log::warn!(
						target: LOG_TARGET,
						"CurrentBlockRandomness did not provide entropy"
					);
					entropy.as_mut().copy_from_slice(parent_hash.as_ref());
				}
				entropy
			};

			let remaining_weight = <T as frame_system::Config>::BlockWeights::get()
				.max_block
				.saturating_sub(static_weight::<T>());
			let (_backed_candidates_weight, backed_candidates, bitfields) =
				apply_weight_limit::<T>(backed_candidates, bitfields, entropy, remaining_weight);

			let inherent_data = ParachainsInherentData::<T::Header> {
				bitfields,
				backed_candidates,
				disputes,
				parent_header,
			};

			// Sanity check: session changes can invalidate an inherent,
			// and we _really_ don't want that to happen.
			// See <https://github.com/paritytech/polkadot/issues/1327>

			// Calling `Self::enter` here is a safe-guard, to avoid any discrepancy
			// between on-chain logic and the logic that is executed
			// at the block producer off-chain (this code path).
			let inherent_data =
				match Self::enter(frame_system::RawOrigin::None.into(), inherent_data.clone()) {
					Ok(_) => inherent_data,
					Err(err) => {
						log::error!(
							target: LOG_TARGET,
							"dropping paras inherent data because they produced \
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
			paras_inherent_total_weight::<T>(
				data.backed_candidates.len() as u32,
				data.bitfields.len() as u32,
				data.disputes.len() as u32,
			),
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
			let disputes_len = disputes.len() as u32;
			let bitfields_len = signed_bitfields.len() as u32;

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
			let disputed_bitfield = {
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
					freed_disputed
				} else {
					Vec::new()
				};

				// create a bit index from the set of core indicies
				// where each index corresponds to a core index
				// that was freed due to a dispute
				let disputed_bitfield = create_disputed_bitfield(
					expected_bits,
					freed_disputed.iter().map(|(core_index, _)| core_index),
				);

				if !freed_disputed.is_empty() {
					// unstable sort is fine, because core indices are unique
					// i.e. the same candidate can't occupy 2 cores at once.
					freed_disputed.sort_unstable_by_key(|pair| pair.0); // sort by core index
					<scheduler::Pallet<T>>::free_cores(freed_disputed);
				}

				disputed_bitfield
			};

			// Process new availability bitfields, yielding any availability cores whose
			// work has now concluded.
			let freed_concluded = <inclusion::Pallet<T>>::process_bitfields(
				expected_bits,
				signed_bitfields,
				disputed_bitfield,
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
			let freed = freed_concluded
				.into_iter()
				.map(|(c, _hash)| (c, FreedReason::Concluded))
				.chain(freed_timeout.into_iter().map(|c| (c, FreedReason::TimedOut)))
				.collect::<BTreeMap<CoreIndex, FreedReason>>();

			<scheduler::Pallet<T>>::clear();
			<scheduler::Pallet<T>>::schedule(freed, <frame_system::Pallet<T>>::block_number());

			let scheduled = <scheduler::Pallet<T>>::scheduled();
			let backed_candidates = sanitize_backed_candidates::<T, _, true>(
				parent_hash,
				backed_candidates,
				move |candidate_hash: CandidateHash| -> bool {
					<T>::DisputesHandler::concluded_invalid(current_session, candidate_hash)
				},
				&scheduled[..],
			)
			.unwrap_or_else(|err| {
				log::error!(
					target: LOG_TARGET,
					"dropping all backed candidates due to sanitization error: {:?}",
					err,
				);
				Vec::new()
			});

			let backed_candidates = limit_backed_candidates::<T>(backed_candidates);
			let backed_candidates_len = backed_candidates.len() as u32;

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

			Ok(Some(paras_inherent_total_weight::<T>(
				backed_candidates_len,
				bitfields_len,
				disputes_len,
			))
			.into())
		}
	}
}

macro_rules! ensure2 {
	($condition:expr, $err:expr, $action:ident $(, $alt:expr)? $(,)?) => {
		let condition = $condition;
		if !condition {
			if $action {
				ensure!(condition, $err);
			} else {
				$($alt)?
			}
		}
	};
}

/// Derive a bitfield from dispute
pub(super) fn create_disputed_bitfield<'a, I: 'a + IntoIterator<Item = &'a CoreIndex>>(
	expected_bits: usize,
	freed_cores: I,
) -> DisputedBitfield {
	let mut bitvec = BitVec::repeat(false, expected_bits);
	for core_idx in freed_cores {
		let core_idx = core_idx.0 as usize;
		if core_idx < expected_bits {
			bitvec.set(core_idx, true);
		}
	}
	DisputedBitfield::from(bitvec)
}

/// Considers an upper threshold that the candidates must not exceed.
///
/// If there is sufficient space, all bitfields and candidates will be included.
///
/// Otherwise tries to include all bitfields, and fills in the remaining weight with candidates.
///
/// If even the bitfields are too large to fit into the `max_weight` limit, bitfields are randomly
/// picked and _no_ candidates will be included.
fn apply_weight_limit<T: Config + inclusion::Config>(
	candidates: Vec<BackedCandidate<<T>::Hash>>,
	bitfields: UncheckedSignedAvailabilityBitfields,
	entropy: [u8; 32],
	max_weight: Weight,
) -> (Weight, Vec<BackedCandidate<<T>::Hash>>, UncheckedSignedAvailabilityBitfields) {
	let total_bitfields_weight = (bitfields.len() as Weight).saturating_mul(bitfield_weight::<T>());

	let total_candidates_weight =
		(candidates.len() as Weight).saturating_mul(backed_candidate_weight::<T>());

	let total = total_bitfields_weight + total_candidates_weight;

	// everything fits into the block
	if max_weight < total {
		return (total, candidates, bitfields)
	}

	use rand_chacha::rand_core::SeedableRng;
	let mut rng = rand_chacha::ChaChaRng::from_seed(entropy.into());

	fn random_sel<X, F: Fn(&X) -> Weight>(
		rng: &mut rand_chacha::ChaChaRng,
		selectables: &[X],
		weight_fn: F,
		weight_limit: Weight,
	) -> (Weight, Vec<usize>) {
		let mut indices = (0..selectables.len()).into_iter().collect::<Vec<_>>();
		let mut picked_indices = Vec::with_capacity(selectables.len().saturating_sub(1));

		let mut weight_acc = 0 as Weight;
		while weight_acc < weight_limit || !selectables.is_empty() {
			// randomly pick an index
			let pick = rng.gen_range(0..indices.len());
			// remove the index from the available set of indices
			let idx = indices.swap_remove(pick);

			let item = &selectables[idx];

			picked_indices.push(idx);
			weight_acc = weight_fn(item);
		}
		// sorting indices, so the ordering is retained
		// unstable sorting is fine, since there are no duplicates
		picked_indices.sort_unstable();
		(weight_acc, picked_indices)
	}

	// There is weight remaining to be consumed by a subset of candidates
	// which are going to be picked now.
	if let Some(remaining_weight) = max_weight.checked_sub(total_bitfields_weight) {
		let (acc_candidate_weight, indices) = random_sel::<BackedCandidate<<T>::Hash>, _>(
			&mut rng,
			&candidates[..],
			|_| backed_candidate_weight::<T>(),
			remaining_weight,
		);
		let candidates =
			indices.into_iter().map(move |idx| candidates[idx].clone()).collect::<Vec<_>>();
		// pick all bitfields, and
		// fill the remaining space with candidates
		let total = acc_candidate_weight + total_bitfields_weight;
		return (total, candidates, bitfields)
	}

	// insufficient space for even the bitfields alone, so only try to fit as many of those
	// into the block and skip the candidates entirely
	let (total, indices) = random_sel::<UncheckedSignedAvailabilityBitfield, _>(
		&mut rng,
		&bitfields[..],
		|_| bitfield_weight::<T>(),
		max_weight,
	);
	let bitfields = indices.into_iter().map(move |idx| bitfields[idx].clone()).collect::<Vec<_>>();
	(total, vec![], bitfields)
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
///
/// While this function technically returns a set of unchecked bitfields,
/// they were actually checked and filtered to allow using it in both
/// cases, as `filtering` and `checking` stage.
///
/// `EARLY_RETURN` determines the behavior.
/// `false` assures that all inputs are filtered, and invalid ones are filtered out.
/// It also skips signature verification.
/// `true` returns an `Err(_)` on the first check failing.
pub(crate) fn sanitize_bitfields<T: crate::inclusion::Config, const EARLY_RETURN: bool>(
	unchecked_bitfields: UncheckedSignedAvailabilityBitfields,
	disputed_bitfield: DisputedBitfield,
	expected_bits: usize,
	parent_hash: T::Hash,
	session_index: SessionIndex,
	validators: &[ValidatorId],
) -> Result<UncheckedSignedAvailabilityBitfields, DispatchError> {
	let mut bitfields = Vec::with_capacity(unchecked_bitfields.len());

	let mut last_index = None;

	ensure2!(
		disputed_bitfield.0.len() == expected_bits,
		crate::inclusion::Error::<T>::WrongBitfieldSize,
		EARLY_RETURN
	);

	let all_zeros = BitVec::<bitvec::order::Lsb0, u8>::repeat(false, expected_bits);
	let signing_context = SigningContext { parent_hash, session_index };
	for unchecked_bitfield in unchecked_bitfields {
		ensure2!(
			unchecked_bitfield.unchecked_payload().0.len() == expected_bits,
			crate::inclusion::Error::<T>::WrongBitfieldSize,
			EARLY_RETURN,
			continue
		);

		ensure2!(
			unchecked_bitfield.unchecked_payload().0.clone() & disputed_bitfield.0.clone() ==
				all_zeros,
			crate::inclusion::Error::<T>::BitfieldReferencesFreedCore,
			EARLY_RETURN,
			continue
		);

		ensure2!(
			last_index.map_or(true, |last| last < unchecked_bitfield.unchecked_validator_index()),
			crate::inclusion::Error::<T>::BitfieldDuplicateOrUnordered,
			EARLY_RETURN,
			continue
		);

		ensure2!(
			(unchecked_bitfield.unchecked_validator_index().0 as usize) < validators.len(),
			crate::inclusion::Error::<T>::ValidatorIndexOutOfBounds,
			EARLY_RETURN,
			continue
		);

		let validator_index = unchecked_bitfield.unchecked_validator_index();

		let validator_public = &validators[validator_index.0 as usize];

		// only check the signatures when returning early
		if EARLY_RETURN {
			let signed_bitfield = if let Ok(signed_bitfield) =
				unchecked_bitfield.try_into_checked(&signing_context, validator_public)
			{
				signed_bitfield
			} else {
				fail!(crate::inclusion::Error::<T>::InvalidBitfieldSignature);
			};
			bitfields.push(signed_bitfield.into_unchecked());
		} else {
			bitfields.push(unchecked_bitfield);
		}

		last_index = Some(validator_index);
	}
	Ok(bitfields)
}

/// Filter out any candidates, that have a concluded invalid dispute.
///
/// `scheduled` follows the same naming scheme as provided in the
/// guide: Currently `free` but might become `occupied`.
/// For the filtering here the relevant part is only the current `free`
/// state.
///
/// `candidate_has_concluded_invalid_dispute` must return `true` if the candidate
/// is disputed, false otherwise
fn sanitize_backed_candidates<
	T: crate::inclusion::Config,
	F: Fn(CandidateHash) -> bool,
	const EARLY_RETURN: bool,
>(
	relay_parent: T::Hash,
	mut backed_candidates: Vec<BackedCandidate<T::Hash>>,
	candidate_has_concluded_invalid_dispute: F,
	scheduled: &[CoreAssignment],
) -> Result<Vec<BackedCandidate<T::Hash>>, Error<T>> {
	let n = backed_candidates.len();
	// Remove any candidates that were concluded invalid.
	backed_candidates.retain(|backed_candidate| {
		!candidate_has_concluded_invalid_dispute(backed_candidate.candidate.hash())
	});
	ensure2!(backed_candidates.len() == n, Error::<T>::CandidateConcludedInvalid, EARLY_RETURN);

	// Assure the backed candidate's `ParaId`'s core is free.
	// This holds under the assumption that `Scheduler::schedule` is called _before_.
	// Also checks the candidate references the correct relay parent.
	backed_candidates.retain(|backed_candidate| {
		let desc = backed_candidate.descriptor();
		desc.relay_parent == relay_parent &&
			scheduled.iter().any(|core| core.para_id == desc.para_id)
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

	fn default_header() -> primitives::v1::Header {
		primitives::v1::Header {
			parent_hash: Default::default(),
			number: 0,
			state_root: Default::default(),
			extrinsics_root: Default::default(),
			digest: Default::default(),
		}
	}

	mod sanitizers {
		use super::*;
		use assert_matches::assert_matches;

		use crate::inclusion::tests::{
			back_candidate, collator_sign_candidate, BackingKind, TestCandidateBuilder,
		};
		use bitvec::order::Lsb0;
		use primitives::v1::{
			AvailabilityBitfield, GroupIndex, Hash, Id as ParaId, SignedAvailabilityBitfield,
			ValidatorIndex,
		};

		use crate::mock::Test;
		use futures::executor::block_on;
		use keyring::Sr25519Keyring;
		use primitives::v0::PARACHAIN_KEY_TYPE_ID;
		use sc_keystore::LocalKeystore;
		use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
		use std::sync::Arc;

		fn validator_pubkeys(val_ids: &[keyring::Sr25519Keyring]) -> Vec<ValidatorId> {
			val_ids.iter().map(|v| v.public().into()).collect()
		}

		#[test]
		fn bitfields() {
			let header = default_header();
			let parent_hash = header.hash();
			// 2 cores means two bits
			let expected_bits = 2;
			let session_index = SessionIndex::from(0_u32);

			let crypto_store = LocalKeystore::in_memory();
			let crypto_store = Arc::new(crypto_store) as SyncCryptoStorePtr;
			let signing_context = SigningContext { parent_hash, session_index };

			let validators = vec![
				keyring::Sr25519Keyring::Alice,
				keyring::Sr25519Keyring::Bob,
				keyring::Sr25519Keyring::Charlie,
				keyring::Sr25519Keyring::Dave,
			];
			for validator in validators.iter() {
				SyncCryptoStore::sr25519_generate_new(
					&*crypto_store,
					PARACHAIN_KEY_TYPE_ID,
					Some(&validator.to_seed()),
				)
				.unwrap();
			}
			let validator_public = validator_pubkeys(&validators);

			let unchecked_bitfields = [
				BitVec::<Lsb0, u8>::repeat(true, expected_bits),
				BitVec::<Lsb0, u8>::repeat(true, expected_bits),
				{
					let mut bv = BitVec::<Lsb0, u8>::repeat(false, expected_bits);
					bv.set(expected_bits - 1, true);
					bv
				},
			]
			.iter()
			.enumerate()
			.map(|(vi, ab)| {
				let validator_index = ValidatorIndex::from(vi as u32);
				block_on(SignedAvailabilityBitfield::sign(
					&crypto_store,
					AvailabilityBitfield::from(ab.clone()),
					&signing_context,
					validator_index,
					&validator_public[vi],
				))
				.unwrap()
				.unwrap()
				.into_unchecked()
			})
			.collect::<Vec<_>>();

			let disputed_bitfield = DisputedBitfield::zeros(expected_bits);

			{
				assert_matches!(
					sanitize_bitfields::<Test, true>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					Ok(_)
				);
				assert_eq!(
					sanitize_bitfields::<Test, false>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					Ok(unchecked_bitfields.clone())
				);
			}

			// disputed bitfield is non-zero
			{
				let mut disputed_bitfield = DisputedBitfield::zeros(expected_bits);
				// pretend the first core was freed by either a malicious validator
				// or by resolved dispute
				disputed_bitfield.0.set(0, true);

				assert_eq!(
					sanitize_bitfields::<Test, true>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					Err(inclusion::Error::<Test>::BitfieldReferencesFreedCore.into())
				);
				assert_matches!(
					sanitize_bitfields::<Test, false>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					Ok(unchecked_bitfields) => {
						assert_eq!(unchecked_bitfields.len(), 1);
					}
				);
			}

			// bitfield size mismatch
			{
				assert_eq!(
					sanitize_bitfields::<Test, true>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits + 1,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					Err(inclusion::Error::<Test>::WrongBitfieldSize.into())
				);
				assert_matches!(
					sanitize_bitfields::<Test, false>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits + 1,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					Ok(unchecked_bitfields) => {
						assert!(unchecked_bitfields.is_empty());
					}
				);
			}

			// remove the last validator
			{
				let shortened = validator_public.len() - 2;
				assert_eq!(
					sanitize_bitfields::<Test, true>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..shortened]
					),
					Err(inclusion::Error::<Test>::ValidatorIndexOutOfBounds.into())
				);
				assert_matches!(
					sanitize_bitfields::<Test, false>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..shortened]
					),
					Ok(unchecked_bitfields_filtered) => {
						assert_eq!(
							&unchecked_bitfields_filtered[..],
							&unchecked_bitfields[..shortened]
						);
					}
				);
			}

			// switch ordering of bitfields, bail
			{
				let mut unchecked_bitfields = unchecked_bitfields.clone();
				let x = unchecked_bitfields.swap_remove(0);
				unchecked_bitfields.push(x);
				assert_eq!(
					sanitize_bitfields::<Test, true>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					Err(inclusion::Error::<Test>::BitfieldDuplicateOrUnordered.into())
				);
				assert_matches!(
					sanitize_bitfields::<Test, false>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					Ok(unchecked_bitfields_filtered) => {
						// the last element is out of order
						// hence sanitization will have removed only that
						// one
						assert_eq!(
							&unchecked_bitfields[..(unchecked_bitfields.len()-2)],
							&unchecked_bitfields_filtered[..]
							);
					}
				);
			}
		}

		#[test]
		fn candidates() {
			const RELAY_PARENT_NUM: u32 = 3;

			let header = default_header();
			let relay_parent = header.hash();
			let session_index = SessionIndex::from(0_u32);

			let keystore = LocalKeystore::in_memory();
			let keystore = Arc::new(keystore) as SyncCryptoStorePtr;
			let signing_context = SigningContext { parent_hash: relay_parent, session_index };

			let validators = vec![
				keyring::Sr25519Keyring::Alice,
				keyring::Sr25519Keyring::Bob,
				keyring::Sr25519Keyring::Charlie,
				keyring::Sr25519Keyring::Dave,
			];
			for validator in validators.iter() {
				SyncCryptoStore::sr25519_generate_new(
					&*keystore,
					PARACHAIN_KEY_TYPE_ID,
					Some(&validator.to_seed()),
				)
				.unwrap();
			}

			let has_concluded_invalid = |_candidate: CandidateHash| -> bool { false };

			let scheduled = (0_usize..2)
				.into_iter()
				.map(|idx| {
					let ca = CoreAssignment {
						kind: scheduler::AssignmentKind::Parachain,
						group_idx: GroupIndex::from(idx as u32),
						para_id: ParaId::from(1_u32 + idx as u32),
						core: CoreIndex::from(idx as u32),
					};
					ca
				})
				.collect::<Vec<_>>();
			let scheduled = &scheduled[..];

			let group_validators = |group_index: GroupIndex| {
				match group_index {
					group_index if group_index == GroupIndex::from(0) => Some(vec![0, 1]),
					group_index if group_index == GroupIndex::from(1) => Some(vec![2, 3]),
					_ => panic!("Group index out of bounds for 2 parachains and 1 parathread core"),
				}
				.map(|m| m.into_iter().map(ValidatorIndex).collect::<Vec<_>>())
			};

			let backed_candidates = (0_usize..2)
				.into_iter()
				.map(|idx0| {
					let idx1 = idx0 + 1;
					let mut candidate = TestCandidateBuilder {
						para_id: ParaId::from(idx1),
						relay_parent,
						pov_hash: Hash::repeat_byte(idx1 as u8),
						persisted_validation_data_hash: [42u8; 32].into(),
						hrmp_watermark: RELAY_PARENT_NUM,
						..Default::default()
					}
					.build();

					collator_sign_candidate(Sr25519Keyring::One, &mut candidate);

					let backed = block_on(back_candidate(
						candidate,
						&validators,
						group_validators(GroupIndex::from(idx0 as u32)).unwrap().as_ref(),
						&keystore,
						&signing_context,
						BackingKind::Threshold,
					));
					backed
				})
				.collect::<Vec<_>>();

			{
				assert_matches!(
					sanitize_backed_candidates::<Test, _, true>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					),
					Ok(sanitized_backed_candidates) => {
						assert_eq!(backed_candidates, sanitized_backed_candidates);
					}
				);
				assert_matches!(
					sanitize_backed_candidates::<Test, _, false>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					),
					Ok(sanitized_backed_candidates) => {
						assert_eq!(backed_candidates, sanitized_backed_candidates);
					}
				);
			}

			// nothing is scheduled, so no paraids match,
			// hence no backed candidate makes it through
			{
				let scheduled = &[][..];
				assert_matches!(
					sanitize_backed_candidates::<Test, _, true>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					),
					Err(Error::<Test>::CandidateConcludedInvalid)
				);
				assert_matches!(
					sanitize_backed_candidates::<Test, _, false>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					),
					Ok(sanitized_backed_candidates) => {
						assert_eq!(0, sanitized_backed_candidates.len());
					}
				);
			}

			// relay parent mismatch
			{
				let relay_parent = Hash::repeat_byte(0xFA);
				assert_matches!(
					sanitize_backed_candidates::<Test, _, true>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					),
					Err(Error::<Test>::CandidateConcludedInvalid)
				);
				assert_matches!(
					sanitize_backed_candidates::<Test, _, false>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					),
					Ok(sanitized_backed_candidates) => {
						assert_eq!(0, sanitized_backed_candidates.len());
					}
				);
			}

			// relay parent mismatch
			{
				// mark every second one as concluded invalid
				let set = {
					let mut set = std::collections::HashSet::new();
					for (idx, backed_candidate) in backed_candidates.iter().enumerate() {
						if idx & 0x01 == 0 {
							set.insert(backed_candidate.hash().clone());
						}
					}
					set
				};
				let has_concluded_invalid = |candidate: CandidateHash| set.contains(&candidate);
				assert_matches!(
					sanitize_backed_candidates::<Test, _, true>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					),
					Err(Error::<Test>::CandidateConcludedInvalid)
				);
				assert_matches!(
					sanitize_backed_candidates::<Test, _, false>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					),
					Ok(sanitized_backed_candidates) => {
						assert_eq!(0, sanitized_backed_candidates.len() / 2);
					}
				);
			}
		}
	}

	mod paras_inherent_weight {
		use super::*;
		use crate::inclusion::tests::*;

		use primitives::v1::{GroupIndex, Hash, Id as ParaId, ValidatorIndex};

		use crate::mock::{new_test_ext, MockGenesisConfig, System, Test};
		use frame_support::traits::UnfilteredDispatchable;
		use futures::executor::block_on;
		use primitives::v0::PARACHAIN_KEY_TYPE_ID;
		use sc_keystore::LocalKeystore;
		use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};
		use std::sync::Arc;

		/// We expect the weight of the paras inherent not to change when no truncation occurs:
		/// its weight is dynamically computed from the size of the backed candidates list, and is
		/// already incorporated into the current block weight when it is selected by the provisioner.
		#[test]
		fn weight_does_not_change_on_happy_path() {
			let chain_a = ParaId::from(1);
			let chain_b = ParaId::from(2);
			let chains = vec![chain_a, chain_b];

			new_test_ext(genesis_config(vec![(chain_a, true), (chain_b, true)])).execute_with(
				|| {
					let header = default_header();
					System::set_block_number(1);
					System::set_parent_hash(header.hash());
					let session_index = SessionIndex::from(0_u32);
					crate::shared::CurrentSessionIndex::<Test>::set(session_index);
					let keystore: SyncCryptoStorePtr = Arc::new(LocalKeystore::in_memory());
					let validators = vec![
						keyring::Sr25519Keyring::Alice,
						keyring::Sr25519Keyring::Bob,
						keyring::Sr25519Keyring::Charlie,
						keyring::Sr25519Keyring::Dave,
					];
					for validator in validators.iter() {
						SyncCryptoStore::sr25519_generate_new(
							&*keystore,
							PARACHAIN_KEY_TYPE_ID,
							Some(&validator.to_seed()),
						)
						.unwrap();
					}
					let validator_public = validator_pubkeys(&validators);

					shared::Pallet::<Test>::set_active_validators_ascending(
						validator_public.clone(),
					);

					let group_validators = |group_index: GroupIndex| {
						match group_index {
							group_index if group_index == GroupIndex::from(0) => vec![0, 1],
							group_index if group_index == GroupIndex::from(1) => vec![2, 3],
							x => panic!("Group index out of bounds for 2 parachains and 1 parathread core {}", x.0),
						}
						.into_iter().map(ValidatorIndex).collect::<Vec<_>>()
					};

					let signing_context =
						SigningContext { parent_hash: System::parent_hash(), session_index };

					// number of bitfields doesn't affect the paras inherent weight, so we can mock it with an empty one
					let signed_bitfields = Vec::new();
					// backed candidates must not be empty, so we can demonstrate that the weight has not changed
					crate::paras::Parachains::<Test>::set(chains.clone());

					crate::scheduler::ValidatorGroups::<Test>::set(vec![
						group_validators(GroupIndex::from(0)),
						group_validators(GroupIndex::from(1)),
					]);

					crate::scheduler::AvailabilityCores::<Test>::set(vec![None, None]);

					let core_a = CoreIndex::from(0);
					let grp_idx_a = crate::scheduler::Pallet::<Test>::group_assigned_to_core(
						core_a,
						System::block_number(),
					)
					.unwrap();

					let core_b = CoreIndex::from(1);
					let grp_idx_b = crate::scheduler::Pallet::<Test>::group_assigned_to_core(
						core_b,
						System::block_number(),
					)
					.unwrap();

					let grp_indices = vec![grp_idx_a.clone(), grp_idx_b.clone()];

					let backed_candidates = chains
						.iter()
						.cloned()
						.enumerate()
						.map(|(idx, para_id)| {
							let mut candidate = TestCandidateBuilder {
								para_id,
								relay_parent: header.hash(),
								pov_hash: Hash::repeat_byte(1_u8 + idx as u8),
								persisted_validation_data_hash: make_vdata_hash(para_id).unwrap(),
								hrmp_watermark: 0,
								..Default::default()
							}
							.build();

							collator_sign_candidate(keyring::Sr25519Keyring::One, &mut candidate);

							let group_to_use_for_signing = grp_indices[idx].clone();
							let backing_validators = group_validators(group_to_use_for_signing);
							let backed_candidate = block_on(back_candidate(
								candidate,
								&validators,
								backing_validators.as_slice(),
								&keystore,
								&signing_context,
								BackingKind::Threshold,
							));
							backed_candidate
						})
						.collect::<Vec<_>>();

					// the expected weight can always be computed by this formula
					let expected_weight = MINIMAL_INCLUSION_INHERENT_WEIGHT +
						(backed_candidates.len() as Weight * BACKED_CANDIDATE_WEIGHT);

					// we've used half the block weight; there's plenty of margin
					let max_block_weight =
						<Test as frame_system::Config>::BlockWeights::get().max_block;

					let used_block_weight = max_block_weight / 2;
					System::set_block_consumed_resources(used_block_weight, 0);

					// no need to schedule anything, `schedule(..)` is called
					// form the `fn` under test.

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
					//
					// In this case, the weight system can update the actual weight with the same amount,
					// or return `None` to indicate that the pre-computed weight should not change.
					// Either option is acceptable for our purposes.
					if let Some(actual_weight) = post_info.actual_weight {
						assert_eq!(actual_weight, expected_weight);
					}
				},
			);
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
				assert_eq!(post_info.actual_weight, Some(expected_weight));
			});
		}
	}
}
