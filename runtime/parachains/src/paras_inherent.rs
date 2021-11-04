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
	inclusion, initializer,
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
use pallet_babe::{self, CurrentBlockRandomness};
use primitives::v1::{
	BackedCandidate, CandidateHash, CoreIndex, DisputeStatementSet,
	InherentData as ParachainsInherentData, MultiDisputeStatementSet,
	ScrapedOnChainVotes, SessionIndex, SigningContext, UncheckedSignedAvailabilityBitfield,
	UncheckedSignedAvailabilityBitfields, ValidatorId, PARACHAINS_INHERENT_IDENTIFIER,
};
use rand::Rng;
use scale_info::TypeInfo;
use sp_runtime::traits::Header as HeaderT;
use sp_std::{
	cmp::Ordering,
	collections::{btree_map::BTreeMap, btree_set::BTreeSet},
	prelude::*,
	vec::Vec,
};
#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

const LOG_TARGET: &str = "runtime::inclusion-inherent";

pub trait WeightInfo {
	/// Variant over `v`, the count of dispute statements in a dispute statement set. This gives the
	/// weight of a single dispute statement set.
	fn enter_variable_disputes(v: u32) -> Weight;
	/// The weight of one bitfield.
	fn enter_bitfields() -> Weight;
	/// Variant over `v`, the count of validity votes for a backed candidate. This gives the weight
	/// of a single backed candidate.
	fn enter_backed_candidates_variable(v: u32) -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn enter_variable_disputes(_v: u32) -> Weight {
		0
	}
	fn enter_bitfields() -> Weight {
		0
	}
	fn enter_backed_candidates_variable(_v: u32) -> Weight {
		0
	}
}

fn paras_inherent_total_weight<T: Config>(
	backed_candidates: &[BackedCandidate<<T as frame_system::Config>::Hash>],
	bitfields: &[UncheckedSignedAvailabilityBitfield],
	disputes: &[DisputeStatementSet],
) -> Weight {
	backed_candidates_weight::<T>(backed_candidates) +
	signed_bitfields_weight::<T>(bitfields.len()) +
	dispute_statements_weight::<T>(disputes)
}

fn minimal_inherent_weight<T: Config>() -> Weight {
	// We just take the min of all our options. This can be changed in the future.
	<T as Config>::WeightInfo::enter_bitfields()
		.min(<T as Config>::WeightInfo::enter_variable_disputes(0))
		.min(<T as Config>::WeightInfo::enter_backed_candidates_variable(0))
}

fn dispute_statements_weight<T: Config>(disputes: &[DisputeStatementSet]) -> Weight {
    disputes.iter().map(|d| <<T as Config>::WeightInfo as WeightInfo>::enter_variable_disputes(d.statements.len() as u32)).sum()
}

fn signed_bitfields_weight<T: Config>(bitfields_len: usize) -> Weight {
    <<T as Config>::WeightInfo as WeightInfo>::enter_bitfields() * bitfields_len as Weight
}

fn backed_candidates_weight<T: frame_system::Config + Config>(
    candidate: &[BackedCandidate<T::Hash>],
) -> Weight {
    candidate.iter().map(|v| <<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidates_variable(v.validity_votes.len() as u32)).sum()
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
					}
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
			let ParachainsInherentData {
				bitfields: signed_bitfields,
				backed_candidates,
				parent_header,
				disputes,
			} = data;
			let total_weight = paras_inherent_total_weight::<T>(
				&backed_candidates,
				&signed_bitfields,
				&disputes,
			);

			if total_weight > <T as frame_system::Config>::BlockWeights::get().max_block {
				return Ok(Some(minimal_inherent_weight::<T>()).into());
			}

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
					return Ok(Some(minimal_inherent_weight::<T>()).into());
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
				false,
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
			<scheduler::Pallet<T>>::schedule(freed, now);

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
			// this is max config.ump_service_total_weight
			let _ump_weight = <ump::Pallet<T>>::process_pending_upward_messages();

			// And track that we've finished processing the inherent for this block.
			Included::<T>::set(Some(()));

			Ok(Some(total_weight).into())
		}
	}
}

impl<T: Config> Pallet<T> {
	/// Create the `ParachainsInherentData` that gets passed to `[`Self::enter`] in [`Self::create_inherent`].
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
				return None;
			}
		};

		let parent_hash = <frame_system::Pallet<T>>::parent_hash();
		if parent_hash != parent_header.hash() {
			log::warn!(
				target: LOG_TARGET,
				"ParachainsInherentData references a different parent header hash than frame"
			);
			return None;
		}

		let current_session = <shared::Pallet<T>>::session_index();
		let expected_bits = <scheduler::Pallet<T>>::availability_cores().len();
		let validator_public = shared::Pallet::<T>::active_validator_keys();

		T::DisputesHandler::filter_multi_dispute_data(&mut disputes);

		let (concluded_invalid_disputes, disputed_bitfield, scheduled) =
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

				// concluded invalid disputes, only including the _current block's_ votes
				let current_concluded_invalid_disputes = disputes
					.iter()
					.filter(|dss| dss.session == current_session)
					.map(|dss| (dss.session, dss.candidate_hash))
					.filter(|(session, candidate)| {
						<T>::DisputesHandler::concluded_invalid(*session, *candidate)
					})
					.map(|(_session, candidate)| candidate)
					.collect::<BTreeSet<CandidateHash>>();

				// all concluded invalid disputes, that are relevant for the set of candidates
				// the inherent provided
				let concluded_invalid_disputes = backed_candidates
					.iter()
					.map(|backed_candidate| backed_candidate.hash())
					.filter(|candidate| {
						<T>::DisputesHandler::concluded_invalid(current_session, *candidate)
					})
					.collect::<BTreeSet<CandidateHash>>();

				let dispute_freed_cores =
					<inclusion::Pallet<T>>::collect_disputed(&current_concluded_invalid_disputes);
				let disputed_bitfield =
					create_disputed_bitfield(expected_bits, dispute_freed_cores.iter());

				// Below Operations:
				// * free disputed cores if any exist
				// * get cores that have become free from processing fully bitfields
				// * get cores that have become free from timing out
				// * create  one collection of all the freed cores so we can schedule them
				// * schedule freed cores based on collection of freed cores

				{
					let mut freed_disputed: Vec<_> = <inclusion::Pallet<T>>::collect_disputed(
						&current_concluded_invalid_disputes,
					)
					.into_iter()
					.map(|core| (core, FreedReason::Concluded))
					.collect();

					if !freed_disputed.is_empty() {
						// There are freed disputed cores, so sort them and free them against the scheduler.

						// unstable sort is fine, because core indices are unique
						// i.e. the same candidate can't occupy 2 cores at once.
						freed_disputed.sort_unstable_by_key(|pair| pair.0); // sort by core index

						<scheduler::Pallet<T>>::free_cores(freed_disputed);
					}
				}

				// Get cores that have become free from processing fully bitfields
				let freed_concluded = <inclusion::Pallet<T>>::process_bitfields(
					expected_bits,
					bitfields.clone(),
					disputed_bitfield.clone(),
					<scheduler::Pallet<T>>::core_para,
					true,
				)
				.unwrap_or_else(|err| {
					log::error!(
						target: LOG_TARGET,
						"bitfields could not be processed while creating inherent: {:?}",
						err,
					);
					Vec::new()
				});

				// Get cores that have become free from timing out
				let availability_pred = <scheduler::Pallet<T>>::availability_timeout_predicate();
				let freed_timeout = if let Some(pred) = availability_pred {
					<inclusion::Pallet<T>>::collect_pending(pred)
				} else {
					Vec::new()
				};

				// Create one collection of cores freed from timeouts and availability conclusion ..
				let timeout_and_concluded_freed_cores = freed_concluded
					.into_iter()
					.map(|(c, _hash)| (c, FreedReason::Concluded))
					.chain(freed_timeout.into_iter().map(|c| (c, FreedReason::TimedOut)))
					.collect::<BTreeMap<CoreIndex, FreedReason>>();

				<scheduler::Pallet<T>>::clear();
				// .. so we can schedule them.
				<scheduler::Pallet<T>>::schedule(
					timeout_and_concluded_freed_cores,
					<frame_system::Pallet<T>>::block_number(),
				);

				// Return
				frame_support::storage::TransactionOutcome::Rollback((
					// * concluded disputes for backed candidates in this block,
					concluded_invalid_disputes,
					// * bitfield marking disputed cores,
					disputed_bitfield,
					// * and the newly created core schedule.
					<scheduler::Pallet<T>>::scheduled(),
				))
			});

		// TODO this is wasteful since its also called in `process_bitfield`. We should probably
		// bubble up the bitfields from `process_bitfields` so this does not need to be called here.
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
				log::warn!(target: LOG_TARGET, "CurrentBlockRandomness did not provide entropy");
				entropy.as_mut().copy_from_slice(parent_hash.as_ref());
			}
			entropy
		};

		let remaining_weight = limit_disputes::<T>(&mut disputes);

		let (_backed_candidates_weight, backed_candidates, bitfields) =
			apply_weight_limit::<T>(backed_candidates, bitfields, entropy, remaining_weight);

		Some(ParachainsInherentData::<T::Header> {
			bitfields,
			backed_candidates,
			disputes,
			parent_header,
		})
	}
}

/// Assures the `$condition` is `true`, and raises
/// an error if `$action` is `true`.
/// If `$action` is `false`, executes `$alt` if present.
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
	let total_bitfields_weight = (bitfields.len() as Weight).saturating_mul(signed_bitfields_weight::<T>(bitfields.len()));

	let total_candidates_weight = (candidates.len() as Weight)
		.saturating_mul(backed_candidates_weight::<T>(candidates.as_slice()));

	let total = total_bitfields_weight.saturating_add(total_candidates_weight);

	// everything fits into the block
	if max_weight < total {
		return (total, candidates, bitfields);
	}

	use rand_chacha::rand_core::SeedableRng;
	let mut rng = rand_chacha::ChaChaRng::from_seed(entropy.into());

	fn random_sel<X, F: Fn(&X) -> Weight>(
		rng: &mut rand_chacha::ChaChaRng,
		selectables: Vec<X>,
		weight_fn: F,
		weight_limit: Weight,
	) -> (Weight, Vec<usize>) {
		let mut indices = (0..selectables.len()).into_iter().collect::<Vec<_>>();
		let mut picked_indices = Vec::with_capacity(selectables.len().saturating_sub(1));

		let mut weight_acc = 0 as Weight;
		if !selectables.is_empty() {
			while weight_acc < weight_limit && !indices.is_empty() {
				// randomly pick an index
				let pick = rng.gen_range(0..indices.len());
				// remove the index from the available set of indices
				let idx = indices.swap_remove(pick);

				let item = &selectables[idx];

				picked_indices.push(idx);
				weight_acc = weight_fn(item);
			}
		}

		// sorting indices, so the ordering is retained
		// unstable sorting is fine, since there are no duplicates
		picked_indices.sort_unstable();
		(weight_acc, picked_indices)
	}

	// There is weight remaining to be consumed by a subset of candidates
	// which are going to be picked now.
	if let Some(remaining_weight) = max_weight.checked_sub(total_bitfields_weight) {
		let c = candidates.iter().map(|c| c.validity_votes.len() as u32).collect::<Vec<u32>>();

		let (acc_candidate_weight, indices) = random_sel::<u32, _>(
			&mut rng,
			c,
			|v| <<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidates_variable(*v),
			remaining_weight,
		);
		let candidates =
			indices.into_iter().map(move |idx| candidates[idx].clone()).collect::<Vec<_>>();
		// pick all bitfields, and
		// fill the remaining space with candidates
		let total = acc_candidate_weight.saturating_add(total_bitfields_weight);
		return (total, candidates, bitfields);
	}

	// insufficient space for even the bitfields alone, so only try to fit as many of those
	// into the block and skip the candidates entirely
	let (total, indices) = random_sel::<UncheckedSignedAvailabilityBitfield, _>(
		&mut rng,
		bitfields.clone(),
    	|_| <<T as Config>::WeightInfo as WeightInfo>::enter_bitfields(),
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
			unchecked_bitfield.unchecked_payload().0.clone() & disputed_bitfield.0.clone()
				== all_zeros,
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

/// Filter out any candidates that have a concluded invalid dispute.
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
		desc.relay_parent == relay_parent
			&& scheduled.iter().any(|core| core.para_id == desc.para_id)
	});
	ensure2!(backed_candidates.len() == n, Error::<T>::CandidateConcludedInvalid, EARLY_RETURN);

	// Limit weight, to avoid overweight block.
	Ok(backed_candidates)
}

fn limit_disputes<T: Config>(
	disputes: &mut MultiDisputeStatementSet,
) -> Weight {
	let mut remaining_weight = <T as frame_system::Config>::BlockWeights::get().max_block;
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

		disputes.retain(|d| {
			let dispute_weight = <<T as Config>::WeightInfo as WeightInfo>::enter_variable_disputes(d.statements.len() as u32);
			let inclusion_weight = if remaining_weight > dispute_weight {
				dispute_weight
			} else {
				0 as Weight
			};

			if inclusion_weight > 0 {
				remaining_weight -= dispute_weight;
			}

			inclusion_weight > 0
		});
	}

	remaining_weight
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::mock::{new_test_ext, MockGenesisConfig, Test};
	use crate::builder::{Bench, BenchBuilder};
	use sp_std::collections::btree_map::BTreeMap;
	use frame_support::{assert_ok, assert_err};

	mod limit_backed_candidates {
		use super::*;

		struct TestConfig {
			dispute_statements: BTreeMap<u32, u32>,
			dispute_sessions: Vec<u32>,
			backed_and_concluding: BTreeMap<u32, u32>,
			num_validators_per_core: u32,
			includes_code_upgrade: bool,
		}

		fn make_inherent_data(
			TestConfig {
				dispute_statements,
				dispute_sessions,
				backed_and_concluding,
				num_validators_per_core,
				includes_code_upgrade,
			}: TestConfig,
		) -> Bench<Test> {
			let scenario = BenchBuilder::<Test>::new()
				.set_max_validators((dispute_sessions.len() as u32) * num_validators_per_core)
				.set_max_validators_per_core(num_validators_per_core) // 5 validators, 1 validator per core => 5 cores.
				.set_dispute_statements(dispute_statements)
				.build(backed_and_concluding, dispute_sessions.as_slice()); // build backed candidates for cores 0 & 1.

			for c in scenario.data.backed_candidates.iter() {
				println!("NUMBER OF VALIDITY VOTES {:?}", c.validity_votes.len());
			}
			println!("SCENARIO {:?}", scenario.data.disputes.len());
			for d in scenario.data.disputes.iter() {
				println!("Amount of statements: {:?}", d.statements.len());
				println!("SESSION {:?}", d.session);
			}

			scenario
		}

		#[test]
		fn include_backed_candidates() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let dispute_statements = BTreeMap::new();

				let mut backed_and_concluding = BTreeMap::new();
				backed_and_concluding.insert(0, 1);
				backed_and_concluding.insert(1, 1);

				let scenario = make_inherent_data(
					TestConfig {
						dispute_statements,
						dispute_sessions: vec![0, 0,],
						backed_and_concluding,
						num_validators_per_core: 1,
						includes_code_upgrade: false
					}
				);

				// We expect the scenario to have cores 0 & 1 with pending availability. The backed
				// candidates are also created for cores 0 & 1, so once the pending available
				// become fully available those cores are marked as free and scheduled for the backed
				// candidates.
				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator
				assert_eq!(expected_para_inherent_data.bitfields.len(), 2);
				// * 1 backed candidate per core
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
				// * 0 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 0);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				// Nothing is filtered out (including the backed candidates.)
				assert_eq!(
					Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap(),
					expected_para_inherent_data
				);

				// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
				// alter storage, but just double checking for sanity).
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_eq!(Pallet::<Test>::on_chain_votes(), None);
				// Call enter with our 2 backed candidates
				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					expected_para_inherent_data
				));
				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know our 2
					// backed candidates did not get filtered out
					Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
					2
				);
			});
		}

		#[test]
		fn limit_paras_inherent() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let dispute_statements = BTreeMap::new();

				let backed_and_concluding = BTreeMap::new();

				let scenario = make_inherent_data(
					TestConfig {
						dispute_statements,
						dispute_sessions: vec![3, 2, 1],
						backed_and_concluding,
						num_validators_per_core: 5,
						includes_code_upgrade: false
					}
				);

				// We expect the scenario to have cores 0 & 1 with pending availability. The backed
				// candidates are also created for cores 0 & 1, so once the pending available
				// become fully available those cores are marked as free and scheduled for the backed
				// candidates.
				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator
				assert_eq!(expected_para_inherent_data.bitfields.len(), 15);
				// * 1 backed candidate per core
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 0);
				// * 0 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				// Nothing is filtered out (including the backed candidates.)
				let limit_para_inherent_data = Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
				assert!(limit_para_inherent_data != expected_para_inherent_data);

				assert_eq!(
					limit_para_inherent_data.disputes.len(),
					2usize,
				);

				assert_eq!(
					expected_para_inherent_data.disputes.len(),
					3usize,
				);

				assert_eq!(
					expected_para_inherent_data.disputes[2].session,
					1,
				);

				// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
				// alter storage, but just double checking for sanity).
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_eq!(Pallet::<Test>::on_chain_votes(), None);
				// Call enter with our 2 backed candidates
				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					limit_para_inherent_data
				));

				assert_err!(
					Pallet::<Test>::enter(
						frame_system::RawOrigin::None.into(),
						expected_para_inherent_data,
					),
					DispatchError::from(Error::<Test>::TooManyInclusionInherents),
				);

				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know our 2
					// backed candidates did not get filtered out
					Pallet::<Test>::on_chain_votes().unwrap().backing_validators_per_candidate.len(),
					0
				);
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
}
