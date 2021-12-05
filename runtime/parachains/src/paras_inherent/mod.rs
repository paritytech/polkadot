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
	inclusion::{CandidateCheckContext, FullCheck},
	initializer,
	scheduler::{self, CoreAssignment, FreedReason},
	shared, ump, ParaId,
};
use bitvec::prelude::BitVec;
use frame_support::{
	inherent::{InherentData, InherentIdentifier, MakeFatalError, ProvideInherent},
	pallet_prelude::*,
	traits::Randomness,
};
use frame_system::pallet_prelude::*;
use pallet_babe::{self, CurrentBlockRandomness};
use primitives::v1::{
	BackedCandidate, CandidateHash, CoreIndex, DisputeStatementSet,
	InherentData as ParachainsInherentData, MultiDisputeStatementSet, ScrapedOnChainVotes,
	SessionIndex, SigningContext, UncheckedSignedAvailabilityBitfield,
	UncheckedSignedAvailabilityBitfields, ValidatorId, ValidatorIndex,
	PARACHAINS_INHERENT_IDENTIFIER,
};
use rand::{seq::SliceRandom, SeedableRng};

use scale_info::TypeInfo;
use sp_runtime::traits::{Header as HeaderT, One};
use sp_std::{
	cmp::Ordering,
	collections::{btree_map::BTreeMap, btree_set::BTreeSet},
	prelude::*,
	vec::Vec,
};

mod misc;
mod weights;

pub use self::{
	misc::IndexedRetain,
	weights::{
		backed_candidate_weight, backed_candidates_weight, dispute_statements_weight,
		paras_inherent_total_weight, signed_bitfields_weight, TestWeightInfo, WeightInfo,
	},
};

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "runtime::inclusion-inherent";

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

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	#[pallet::disable_frame_system_supertrait_check]
	pub trait Config:
		inclusion::Config + scheduler::Config + initializer::Config + pallet_babe::Config
	{
		/// Weight information for extrinsics in this pallet.
		type WeightInfo: WeightInfo;
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Inclusion inherent called more than once per block.
		TooManyInclusionInherents,
		/// The hash of the submitted parent header doesn't correspond to the saved block hash of
		/// the parent.
		InvalidParentHeader,
		/// Disputed candidate that was concluded invalid.
		CandidateConcludedInvalid,
		/// The data given to the inherent will result in an overweight block.
		InherentOverweight,
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
			let inherent_data = Self::create_inherent_inner(data)?;
			// Sanity check: session changes can invalidate an inherent,
			// and we _really_ don't want that to happen.
			// See <https://github.com/paritytech/polkadot/issues/1327>

			// Calling `Self::enter` here is a safe-guard, to avoid any discrepancy between on-chain checks
			// (`enter`) and the off-chain checks by the block author (this function). Once we are confident
			// in all the logic in this module this check should be removed to optimize performance.

			let inherent_data = match Self::enter_inner(inherent_data.clone(), FullCheck::Skip) {
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

	/// Collect all freed cores based on storage data. (i.e. append cores freed from timeouts to
	/// the given `freed_concluded`).
	///
	/// The parameter `freed_concluded` contains all core indicies that became
	/// free due to candidate that became available.
	pub(crate) fn collect_all_freed_cores<T, I>(
		freed_concluded: I,
	) -> BTreeMap<CoreIndex, FreedReason>
	where
		I: core::iter::IntoIterator<Item = (CoreIndex, CandidateHash)>,
		T: Config,
	{
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
		freed
	}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Enter the paras inherent. This will process bitfields and backed candidates.
		#[pallet::weight((
			paras_inherent_total_weight::<T>(
				data.backed_candidates.as_slice(),
				data.bitfields.as_slice(),
				data.disputes.as_slice(),
			),
			DispatchClass::Mandatory,
		))]
		pub fn enter(
			origin: OriginFor<T>,
			data: ParachainsInherentData<T::Header>,
		) -> DispatchResultWithPostInfo {
			ensure_none(origin)?;

			ensure!(!Included::<T>::exists(), Error::<T>::TooManyInclusionInherents);
			Included::<T>::set(Some(()));

			Self::enter_inner(data, FullCheck::Yes)
		}
	}
}

impl<T: Config> Pallet<T> {
	pub(crate) fn enter_inner(
		data: ParachainsInherentData<T::Header>,
		full_check: FullCheck,
	) -> DispatchResultWithPostInfo {
		let ParachainsInherentData {
			bitfields: mut signed_bitfields,
			mut backed_candidates,
			parent_header,
			mut disputes,
		} = data;

		let parent_header_hash = parent_header.hash();

		log::debug!(
			target: LOG_TARGET,
			"[enter_inner] parent_header={:?} bitfields.len(): {}, backed_candidates.len(): {}, disputes.len(): {}",
			parent_header_hash,
			signed_bitfields.len(),
			backed_candidates.len(),
			disputes.len()
		);

		// Check that the submitted parent header indeed corresponds to the previous block hash.
		let parent_hash = <frame_system::Pallet<T>>::parent_hash();
		ensure!(
			parent_header_hash.as_ref() == parent_hash.as_ref(),
			Error::<T>::InvalidParentHeader,
		);

		let now = <frame_system::Pallet<T>>::block_number();

		let mut candidate_weight = backed_candidates_weight::<T>(&backed_candidates);
		let mut bitfields_weight = signed_bitfields_weight::<T>(signed_bitfields.len());
		let disputes_weight = dispute_statements_weight::<T>(&disputes);

		let max_block_weight = <T as frame_system::Config>::BlockWeights::get().max_block;

		// Potentially trim inherent data to ensure processing will be within weight limits
		let total_weight = {
			if candidate_weight
				.saturating_add(bitfields_weight)
				.saturating_add(disputes_weight) >
				max_block_weight
			{
				// if the total weight is over the max block weight, first try clearing backed
				// candidates and bitfields.
				backed_candidates.clear();
				candidate_weight = 0;
				signed_bitfields.clear();
				bitfields_weight = 0;
			}

			if disputes_weight > max_block_weight {
				// if disputes are by themselves overweight already, trim the disputes.
				debug_assert!(candidate_weight == 0 && bitfields_weight == 0);

				let entropy = compute_entropy::<T>(parent_hash);
				let mut rng = rand_chacha::ChaChaRng::from_seed(entropy.into());

				let remaining_weight =
					limit_disputes::<T>(&mut disputes, max_block_weight, &mut rng);
				max_block_weight.saturating_sub(remaining_weight)
			} else {
				candidate_weight
					.saturating_add(bitfields_weight)
					.saturating_add(disputes_weight)
			}
		};

		let expected_bits = <scheduler::Pallet<T>>::availability_cores().len();

		// Handle disputes logic.
		let current_session = <shared::Pallet<T>>::session_index();
		let disputed_bitfield = {
			let new_current_dispute_sets: Vec<_> = disputes
				.iter()
				.filter(|s| s.session == current_session)
				.map(|s| (s.session, s.candidate_hash))
				.collect();

			// Note that `provide_multi_dispute_data` will iterate, verify, and import each
			// dispute; so the input here must be reasonably bounded.
			let _ = T::DisputesHandler::provide_multi_dispute_data(disputes.clone())?;
			if T::DisputesHandler::is_frozen() {
				// The relay chain we are currently on is invalid. Proceed no further on parachains.
				return Ok(Some(dispute_statements_weight::<T>(&disputes)).into())
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

			// Create a bit index from the set of core indices where each index corresponds to
			// a core index that was freed due to a dispute.
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
		);

		// Inform the disputes module of all included candidates.
		for (_, candidate_hash) in &freed_concluded {
			T::DisputesHandler::note_included(current_session, *candidate_hash, now);
		}

		let freed = collect_all_freed_cores::<T, _>(freed_concluded.iter().cloned());

		<scheduler::Pallet<T>>::clear();
		<scheduler::Pallet<T>>::schedule(freed, now);

		let scheduled = <scheduler::Pallet<T>>::scheduled();
		let backed_candidates = sanitize_backed_candidates::<T, _>(
			parent_hash,
			backed_candidates,
			move |_candidate_index: usize, backed_candidate: &BackedCandidate<T::Hash>| -> bool {
				<T>::DisputesHandler::concluded_invalid(current_session, backed_candidate.hash())
				// `fn process_candidates` does the verification checks
			},
			&scheduled[..],
		);

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
			full_check,
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
		// this is max config.ump_service_total_weight
		let _ump_weight = <ump::Pallet<T>>::process_pending_upward_messages();

		Ok(Some(total_weight).into())
	}
}

impl<T: Config> Pallet<T> {
	/// Create the `ParachainsInherentData` that gets passed to [`Self::enter`] in [`Self::create_inherent`].
	/// This code is pulled out of [`Self::create_inherent`] so it can be unit tested.
	fn create_inherent_inner(data: &InherentData) -> Option<ParachainsInherentData<T::Header>> {
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

		log::debug!(
			target: LOG_TARGET,
			"[create_inherent_inner] bitfields.len(): {}, backed_candidates.len(): {}, disputes.len() {}",
			bitfields.len(),
			backed_candidates.len(),
			disputes.len()
		);

		let parent_hash = <frame_system::Pallet<T>>::parent_hash();

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

		let (mut backed_candidates, mut bitfields) =
			frame_support::storage::with_transaction(|| {
				// we don't care about fresh or not disputes
				// this writes them to storage, so let's query it via those means
				// if this fails for whatever reason, that's ok
				let _ =
					T::DisputesHandler::provide_multi_dispute_data(disputes.clone()).map_err(|e| {
						log::warn!(
							target: LOG_TARGET,
							"MultiDisputesData failed to update: {:?}",
							e
						);
						e
					});

				// Contains the disputes that are concluded in the current session only,
				// since these are the only ones that are relevant for the occupied cores
				// and lightens the load on `collect_disputed` significantly.
				// Cores can't be occupied with candidates of the previous sessions, and only
				// things with new votes can have just concluded. We only need to collect
				// cores with disputes that conclude just now, because disputes that
				// concluded longer ago have already had any corresponding cores cleaned up.
				let current_concluded_invalid_disputes = disputes
					.iter()
					.filter(|dss| dss.session == current_session)
					.map(|dss| (dss.session, dss.candidate_hash))
					.filter(|(session, candidate)| {
						<T>::DisputesHandler::concluded_invalid(*session, *candidate)
					})
					.map(|(_session, candidate)| candidate)
					.collect::<BTreeSet<CandidateHash>>();

				// All concluded invalid disputes, that are relevant for the set of candidates
				// the inherent provided.
				let concluded_invalid_disputes = backed_candidates
					.iter()
					.map(|backed_candidate| backed_candidate.hash())
					.filter(|candidate| {
						<T>::DisputesHandler::concluded_invalid(current_session, *candidate)
					})
					.collect::<BTreeSet<CandidateHash>>();

				let mut freed_disputed: Vec<_> =
					<inclusion::Pallet<T>>::collect_disputed(&current_concluded_invalid_disputes)
						.into_iter()
						.map(|core| (core, FreedReason::Concluded))
						.collect();

				let disputed_bitfield =
					create_disputed_bitfield(expected_bits, freed_disputed.iter().map(|(x, _)| x));

				if !freed_disputed.is_empty() {
					// unstable sort is fine, because core indices are unique
					// i.e. the same candidate can't occupy 2 cores at once.
					freed_disputed.sort_unstable_by_key(|pair| pair.0); // sort by core index
					<scheduler::Pallet<T>>::free_cores(freed_disputed.clone());
				}

				// The following 3 calls are equiv to a call to `process_bitfields`
				// but we can retain access to `bitfields`.
				let bitfields = sanitize_bitfields::<T>(
					bitfields,
					disputed_bitfield,
					expected_bits,
					parent_hash,
					current_session,
					&validator_public[..],
					FullCheck::Skip,
				);

				let freed_concluded =
					<inclusion::Pallet<T>>::update_pending_availability_and_get_freed_cores::<
						_,
						false,
					>(
						expected_bits,
						&validator_public[..],
						bitfields.clone(),
						<scheduler::Pallet<T>>::core_para,
					);

				let freed = collect_all_freed_cores::<T, _>(freed_concluded.iter().cloned());

				<scheduler::Pallet<T>>::clear();
				let now = <frame_system::Pallet<T>>::block_number();
				<scheduler::Pallet<T>>::schedule(freed, now);

				let scheduled = <scheduler::Pallet<T>>::scheduled();

				let relay_parent_number = now - One::one();

				let check_ctx = CandidateCheckContext::<T>::new(now, relay_parent_number);
				let backed_candidates = sanitize_backed_candidates::<T, _>(
					parent_hash,
					backed_candidates,
					move |candidate_idx: usize,
					      backed_candidate: &BackedCandidate<<T as frame_system::Config>::Hash>|
					      -> bool {
						// never include a concluded-invalid candidate
						concluded_invalid_disputes.contains(&backed_candidate.hash()) ||
							// Instead of checking the candidates with code upgrades twice
							// move the checking up here and skip it in the training wheels fallback.
							// That way we avoid possible duplicate checks while assuring all
							// backed candidates fine to pass on.
							check_ctx
								.verify_backed_candidate(parent_hash, candidate_idx, backed_candidate)
								.is_err()
					},
					&scheduled[..],
				);

				frame_support::storage::TransactionOutcome::Rollback((
					// filtered backed candidates
					backed_candidates,
					// filtered bitfields
					bitfields,
				))
			});

		let entropy = compute_entropy::<T>(parent_hash);
		let mut rng = rand_chacha::ChaChaRng::from_seed(entropy.into());

		// Assure the maximum block weight is adhered.
		let max_block_weight = <T as frame_system::Config>::BlockWeights::get().max_block;
		let _consumed_weight = apply_weight_limit::<T>(
			&mut backed_candidates,
			&mut bitfields,
			&mut disputes,
			max_block_weight,
			&mut rng,
		);

		Some(ParachainsInherentData::<T::Header> {
			bitfields,
			backed_candidates,
			disputes,
			parent_header,
		})
	}
}

/// Derive a bitfield from dispute
pub(super) fn create_disputed_bitfield<'a, I>(
	expected_bits: usize,
	freed_cores: I,
) -> DisputedBitfield
where
	I: 'a + IntoIterator<Item = &'a CoreIndex>,
{
	let mut bitvec = BitVec::repeat(false, expected_bits);
	for core_idx in freed_cores {
		let core_idx = core_idx.0 as usize;
		if core_idx < expected_bits {
			bitvec.set(core_idx, true);
		}
	}
	DisputedBitfield::from(bitvec)
}

/// Select a random subset, with preference for certain indices.
///
/// Adds random items to the set until all candidates
/// are tried or the remaining weight is depleted.
///
/// Returns the weight of all selected items from `selectables`
/// as well as their indices in ascending order.
fn random_sel<X, F: Fn(&X) -> Weight>(
	rng: &mut rand_chacha::ChaChaRng,
	selectables: Vec<X>,
	mut preferred_indices: Vec<usize>,
	weight_fn: F,
	weight_limit: Weight,
) -> (Weight, Vec<usize>) {
	if selectables.is_empty() {
		return (0 as Weight, Vec::new())
	}
	// all indices that are not part of the preferred set
	let mut indices = (0..selectables.len())
		.into_iter()
		.filter(|idx| !preferred_indices.contains(idx))
		.collect::<Vec<_>>();
	let mut picked_indices = Vec::with_capacity(selectables.len().saturating_sub(1));

	let mut weight_acc = 0 as Weight;

	preferred_indices.shuffle(rng);
	for preferred_idx in preferred_indices {
		// preferred indices originate from outside
		if let Some(item) = selectables.get(preferred_idx) {
			let updated = weight_acc.saturating_add(weight_fn(item));
			if updated > weight_limit {
				continue
			}
			weight_acc = updated;
			picked_indices.push(preferred_idx);
		}
	}

	indices.shuffle(rng);
	for idx in indices {
		let item = &selectables[idx];
		let updated = weight_acc.saturating_add(weight_fn(item));

		if updated > weight_limit {
			continue
		}
		weight_acc = updated;

		picked_indices.push(idx);
	}

	// sorting indices, so the ordering is retained
	// unstable sorting is fine, since there are no duplicates
	picked_indices.sort_unstable();
	(weight_acc, picked_indices)
}

/// Considers an upper threshold that the inherent data must not exceed.
///
/// If there is sufficient space, all disputes, all bitfields and all candidates
/// will be included.
///
/// Otherwise tries to include all disputes, and then tries to fill the remaining space with bitfields and then candidates.
///
/// The selection process is random. For candidates, there is an exception for code upgrades as they are preferred.
/// And for disputes, local and older disputes are preferred (see `limit_disputes`).
/// for backed candidates, since with a increasing number of parachains their chances of
/// inclusion become slim. All backed candidates  are checked beforehands in `fn create_inherent_inner`
/// which guarantees sanity.
fn apply_weight_limit<T: Config + inclusion::Config>(
	candidates: &mut Vec<BackedCandidate<<T>::Hash>>,
	bitfields: &mut UncheckedSignedAvailabilityBitfields,
	disputes: &mut MultiDisputeStatementSet,
	max_block_weight: Weight,
	rng: &mut rand_chacha::ChaChaRng,
) -> Weight {
	// include as many disputes as possible, always
	let remaining_weight = limit_disputes::<T>(disputes, max_block_weight, rng);

	let total_candidates_weight = backed_candidates_weight::<T>(candidates.as_slice());

	let total_bitfields_weight = signed_bitfields_weight::<T>(bitfields.len());

	let total = total_bitfields_weight.saturating_add(total_candidates_weight);

	// candidates + bitfields fit into the block
	if remaining_weight >= total {
		return total
	}

	// Prefer code upgrades, they tend to be large and hence stand no chance to be picked
	// late while maintaining the weight bounds
	let preferred_indices = candidates
		.iter()
		.enumerate()
		.filter_map(|(idx, candidate)| {
			candidate.candidate.commitments.new_validation_code.as_ref().map(|_code| idx)
		})
		.collect::<Vec<usize>>();

	// There is weight remaining to be consumed by a subset of candidates
	// which are going to be picked now.
	if let Some(remaining_weight) = remaining_weight.checked_sub(total_bitfields_weight) {
		let (acc_candidate_weight, indices) =
			random_sel::<BackedCandidate<<T as frame_system::Config>::Hash>, _>(
				rng,
				candidates.clone(),
				preferred_indices,
				|c| backed_candidate_weight::<T>(c),
				remaining_weight,
			);
		candidates.indexed_retain(|idx, _backed_candidate| indices.binary_search(&idx).is_ok());
		// pick all bitfields, and
		// fill the remaining space with candidates
		let total = acc_candidate_weight.saturating_add(total_bitfields_weight);
		return total
	}

	candidates.clear();

	// insufficient space for even the bitfields alone, so only try to fit as many of those
	// into the block and skip the candidates entirely
	let (total, indices) = random_sel::<UncheckedSignedAvailabilityBitfield, _>(
		rng,
		bitfields.clone(),
		vec![],
		|_| <<T as Config>::WeightInfo as WeightInfo>::enter_bitfields(),
		remaining_weight,
	);

	bitfields.indexed_retain(|idx, _bitfield| indices.binary_search(&idx).is_ok());

	total
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
/// `full_check` determines if validator signatures are checked. If `::Yes`,
/// bitfields that have an invalid signature will be filtered out.
pub(crate) fn sanitize_bitfields<T: crate::inclusion::Config>(
	unchecked_bitfields: UncheckedSignedAvailabilityBitfields,
	disputed_bitfield: DisputedBitfield,
	expected_bits: usize,
	parent_hash: T::Hash,
	session_index: SessionIndex,
	validators: &[ValidatorId],
	full_check: FullCheck,
) -> UncheckedSignedAvailabilityBitfields {
	let mut bitfields = Vec::with_capacity(unchecked_bitfields.len());

	let mut last_index: Option<ValidatorIndex> = None;

	if disputed_bitfield.0.len() != expected_bits {
		// This is a system logic error that should never occur, but we want to handle it gracefully
		// so we just drop all bitfields
		log::error!(target: LOG_TARGET, "BUG: disputed_bitfield != expected_bits");
		return vec![]
	}

	let all_zeros = BitVec::<bitvec::order::Lsb0, u8>::repeat(false, expected_bits);
	let signing_context = SigningContext { parent_hash, session_index };
	for unchecked_bitfield in unchecked_bitfields {
		// Find and skip invalid bitfields.
		if unchecked_bitfield.unchecked_payload().0.len() != expected_bits {
			log::trace!(
				target: LOG_TARGET,
				"[{:?}] bad bitfield length: {} != {:?}",
				full_check,
				unchecked_bitfield.unchecked_payload().0.len(),
				expected_bits,
			);
			continue
		}

		if unchecked_bitfield.unchecked_payload().0.clone() & disputed_bitfield.0.clone() !=
			all_zeros
		{
			log::trace!(
				target: LOG_TARGET,
				"[{:?}] bitfield contains disputed cores: {:?}",
				full_check,
				unchecked_bitfield.unchecked_payload().0.clone() & disputed_bitfield.0.clone()
			);
			continue
		}

		let validator_index = unchecked_bitfield.unchecked_validator_index();

		if !last_index.map_or(true, |last_index: ValidatorIndex| last_index < validator_index) {
			log::trace!(
				target: LOG_TARGET,
				"[{:?}] bitfield validator index is not greater than last: !({:?} < {})",
				full_check,
				last_index.as_ref().map(|x| x.0),
				validator_index.0
			);
			continue
		}

		if unchecked_bitfield.unchecked_validator_index().0 as usize >= validators.len() {
			log::trace!(
				target: LOG_TARGET,
				"[{:?}] bitfield validator index is out of bounds: {} >= {}",
				full_check,
				validator_index.0,
				validators.len(),
			);
			continue
		}

		let validator_public = &validators[validator_index.0 as usize];

		if let FullCheck::Yes = full_check {
			if let Ok(signed_bitfield) =
				unchecked_bitfield.try_into_checked(&signing_context, validator_public)
			{
				bitfields.push(signed_bitfield.into_unchecked());
			} else {
				log::warn!(target: LOG_TARGET, "Invalid bitfield signature");
			};
		} else {
			bitfields.push(unchecked_bitfield);
		}

		last_index = Some(validator_index);
	}
	bitfields
}

/// Filter out any candidates that have a concluded invalid dispute.
///
/// `scheduled` follows the same naming scheme as provided in the
/// guide: Currently `free` but might become `occupied`.
/// For the filtering here the relevant part is only the current `free`
/// state.
///
/// `candidate_has_concluded_invalid_dispute` must return `true` if the candidate
/// is disputed, false otherwise
///
/// The returned `Vec` is sorted according to the occupied core index.
fn sanitize_backed_candidates<
	T: crate::inclusion::Config,
	F: FnMut(usize, &BackedCandidate<T::Hash>) -> bool,
>(
	relay_parent: T::Hash,
	mut backed_candidates: Vec<BackedCandidate<T::Hash>>,
	mut candidate_has_concluded_invalid_dispute_or_is_invalid: F,
	scheduled: &[CoreAssignment],
) -> Vec<BackedCandidate<T::Hash>> {
	// Remove any candidates that were concluded invalid.
	// This does not assume sorting.
	backed_candidates.indexed_retain(move |idx, backed_candidate| {
		!candidate_has_concluded_invalid_dispute_or_is_invalid(idx, backed_candidate)
	});

	let scheduled_paras_to_core_idx = scheduled
		.into_iter()
		.map(|core_assignment| (core_assignment.para_id, core_assignment.core))
		.collect::<BTreeMap<ParaId, CoreIndex>>();

	// Assure the backed candidate's `ParaId`'s core is free.
	// This holds under the assumption that `Scheduler::schedule` is called _before_.
	// Also checks the candidate references the correct relay parent.

	backed_candidates.retain(|backed_candidate| {
		let desc = backed_candidate.descriptor();
		desc.relay_parent == relay_parent &&
			scheduled_paras_to_core_idx.get(&desc.para_id).is_some()
	});

	// Sort the `Vec` last, once there is a guarantee that these
	// `BackedCandidates` references the expected relay chain parent,
	// but more importantly are scheduled for a free core.
	// This both avoids extra work for obviously invalid candidates,
	// but also allows this to be done in place.
	backed_candidates.sort_by(|x, y| {
		// Never panics, since we filtered all panic arguments out in the previous `fn retain`.
		scheduled_paras_to_core_idx[&x.descriptor().para_id]
			.cmp(&scheduled_paras_to_core_idx[&y.descriptor().para_id])
	});

	backed_candidates
}

/// Derive entropy from babe provided per block randomness.
///
/// In the odd case none is available, uses the `parent_hash` and
/// a const value, while emitting a warning.
fn compute_entropy<T: Config>(parent_hash: T::Hash) -> [u8; 32] {
	const CANDIDATE_SEED_SUBJECT: [u8; 32] = *b"candidate-seed-selection-subject";
	let vrf_random = CurrentBlockRandomness::<T>::random(&CANDIDATE_SEED_SUBJECT[..]).0;
	let mut entropy: [u8; 32] = CANDIDATE_SEED_SUBJECT.clone();
	if let Some(vrf_random) = vrf_random {
		entropy.as_mut().copy_from_slice(vrf_random.as_ref());
	} else {
		// in case there is no vrf randomness present, we utilize the relay parent
		// as seed, it's better than a static value.
		log::warn!(target: LOG_TARGET, "CurrentBlockRandomness did not provide entropy");
		entropy.as_mut().copy_from_slice(parent_hash.as_ref());
	}
	entropy
}

/// Limit disputes in place.
///
/// Returns the unused weight of `remaining_weight`.
fn limit_disputes<T: Config>(
	disputes: &mut MultiDisputeStatementSet,
	remaining_weight: Weight,
	rng: &mut rand_chacha::ChaChaRng,
) -> Weight {
	let mut remaining_weight = remaining_weight;
	let disputes_weight = dispute_statements_weight::<T>(&disputes);
	if disputes_weight > remaining_weight {
		// Sort the dispute statements according to the following prioritization:
		//  1. Prioritize local disputes over remote disputes.
		//  2. Prioritize older disputes over newer disputes.
		disputes.sort_by(|a, b| {
			let a_local_block = T::DisputesHandler::included_state(a.session, a.candidate_hash);
			let b_local_block = T::DisputesHandler::included_state(b.session, b.candidate_hash);
			match (a_local_block, b_local_block) {
				// Prioritize local disputes over remote disputes.
				(None, Some(_)) => Ordering::Greater,
				(Some(_), None) => Ordering::Less,
				// For local disputes, prioritize those that occur at an earlier height.
				(Some(a_height), Some(b_height)) => a_height.cmp(&b_height),
				// Prioritize earlier remote disputes using session as rough proxy.
				(None, None) => a.session.cmp(&b.session),
			}
		});

		// Since the disputes array is sorted, we may use binary search to find the beginning of
		// remote disputes
		let idx = disputes
			.binary_search_by(|probe| {
				if T::DisputesHandler::included_state(probe.session, probe.candidate_hash).is_some()
				{
					Ordering::Greater
				} else {
					Ordering::Less
				}
			})
			// The above predicate will never find an item and therefore we are guaranteed to obtain
			// an error, which we can safely unwrap. QED.
			.unwrap_err();

		// Due to the binary search predicate above, the index computed will constitute the beginning
		// of the remote disputes sub-array
		let remote_disputes = disputes.split_off(idx);

		// Select disputes in-order until the remaining weight is attained
		disputes.retain(|d| {
			let dispute_weight = <<T as Config>::WeightInfo as WeightInfo>::enter_variable_disputes(
				d.statements.len() as u32,
			);
			if remaining_weight >= dispute_weight {
				remaining_weight -= dispute_weight;
				true
			} else {
				false
			}
		});

		// Compute the statements length of all remote disputes
		let d = remote_disputes.iter().map(|d| d.statements.len() as u32).collect::<Vec<u32>>();

		// Select remote disputes at random until the block is full
		let (acc_remote_disputes_weight, indices) = random_sel::<u32, _>(
			rng,
			d,
			vec![],
			|v| <<T as Config>::WeightInfo as WeightInfo>::enter_variable_disputes(*v),
			remaining_weight,
		);

		// Collect all remote disputes
		let mut remote_disputes =
			indices.into_iter().map(|idx| disputes[idx].clone()).collect::<Vec<_>>();

		// Construct the full list of selected disputes
		disputes.append(&mut remote_disputes);

		// Update the remaining weight
		remaining_weight = remaining_weight.saturating_sub(acc_remote_disputes_weight);
	}

	remaining_weight
}
