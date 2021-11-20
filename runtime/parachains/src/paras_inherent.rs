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
use rand::{Rng, SeedableRng};
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
const SKIP_SIG_VERIFY: bool = false;
pub(crate) const VERIFY_SIGS: bool = true;

pub trait WeightInfo {
	/// Variant over `v`, the count of dispute statements in a dispute statement set. This gives the
	/// weight of a single dispute statement set.
	fn enter_variable_disputes(v: u32) -> Weight;
	/// The weight of one bitfield.
	fn enter_bitfields() -> Weight;
	/// Variant over `v`, the count of validity votes for a backed candidate. This gives the weight
	/// of a single backed candidate.
	fn enter_backed_candidates_variable(v: u32) -> Weight;
	/// The weight of a single backed candidate with a code upgrade.
	fn enter_backed_candidate_code_upgrade() -> Weight;
}

pub struct TestWeightInfo;
// `WeightInfo` impl for unit and integration tests. Based off of the `max_block` weight for the
//  mock.
#[cfg(not(feature = "runtime-benchmarks"))]
impl WeightInfo for TestWeightInfo {
	fn enter_variable_disputes(v: u32) -> Weight {
		// MAX Block Weight should fit 4 disputes
		80_000 * v as Weight + 80_000
	}
	fn enter_bitfields() -> Weight {
		// MAX Block Weight should fit 4 backed candidates
		40_000 as Weight
	}
	fn enter_backed_candidates_variable(v: u32) -> Weight {
		// MAX Block Weight should fit 4 backed candidates
		40_000 * v as Weight + 40_000
	}
	fn enter_backed_candidate_code_upgrade() -> Weight {
		0
	}
}
// To simplify benchmarks running as tests, we set all the weights to 0. `enter` will exit early
// when if the data causes it to be over weight, but we don't want that to block a benchmark from
// running as a test.
#[cfg(feature = "runtime-benchmarks")]
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
	fn enter_backed_candidate_code_upgrade() -> Weight {
		0
	}
}

fn paras_inherent_total_weight<T: Config>(
	backed_candidates: &[BackedCandidate<<T as frame_system::Config>::Hash>],
	bitfields: &[UncheckedSignedAvailabilityBitfield],
	disputes: &[DisputeStatementSet],
) -> Weight {
	backed_candidates_weight::<T>(backed_candidates)
		.saturating_add(signed_bitfields_weight::<T>(bitfields.len()))
		.saturating_add(dispute_statements_weight::<T>(disputes))
}

fn dispute_statements_weight<T: Config>(disputes: &[DisputeStatementSet]) -> Weight {
	disputes
		.iter()
		.map(|d| {
			<<T as Config>::WeightInfo as WeightInfo>::enter_variable_disputes(
				d.statements.len() as u32
			)
		})
		.fold(0, |acc, x| acc.saturating_add(x))
}

fn signed_bitfields_weight<T: Config>(bitfields_len: usize) -> Weight {
	<<T as Config>::WeightInfo as WeightInfo>::enter_bitfields()
		.saturating_mul(bitfields_len as Weight)
}

fn backed_candidate_weight<T: frame_system::Config + Config>(
	candidate: &BackedCandidate<T::Hash>,
) -> Weight {
	if candidate.candidate.commitments.new_validation_code.is_some() {
		<<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidate_code_upgrade()
	} else {
		<<T as Config>::WeightInfo as WeightInfo>::enter_backed_candidates_variable(
			candidate.validity_votes.len() as u32,
		)
	}
}

fn backed_candidates_weight<T: frame_system::Config + Config>(
	candidates: &[BackedCandidate<T::Hash>],
) -> Weight {
	candidates
		.iter()
		.map(|c| backed_candidate_weight::<T>(c))
		.fold(0, |acc, x| acc.saturating_add(x))
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

			let ParachainsInherentData {
				bitfields: mut signed_bitfields,
				mut backed_candidates,
				parent_header,
				mut disputes,
			} = data;

			log::debug!(
				target: LOG_TARGET,
				"[enter] bitfields.len(): {}, backed_candidates.len(): {}, disputes.len() {}",
				signed_bitfields.len(),
				backed_candidates.len(),
				disputes.len()
			);

			// Check that the submitted parent header indeed corresponds to the previous block hash.
			let parent_hash = <frame_system::Pallet<T>>::parent_hash();
			ensure!(
				parent_header.hash().as_ref() == parent_hash.as_ref(),
				Error::<T>::InvalidParentHeader,
			);

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
			let now = <frame_system::Pallet<T>>::block_number();
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
				move |candidate_hash: CandidateHash| -> bool {
					<T>::DisputesHandler::concluded_invalid(current_session, candidate_hash)
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

		let (concluded_invalid_disputes, mut bitfields, scheduled) =
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

				// current concluded invalid disputes, only including the current block's votes
				// TODO why does this say "only including the current block's votes"? This can include
				// remote disputes, right?
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
				let bitfields = sanitize_bitfields::<T, SKIP_SIG_VERIFY>(
					bitfields,
					disputed_bitfield,
					expected_bits,
					parent_hash,
					current_session,
					&validator_public[..],
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
				<scheduler::Pallet<T>>::schedule(freed, <frame_system::Pallet<T>>::block_number());

				let scheduled = <scheduler::Pallet<T>>::scheduled();

				frame_support::storage::TransactionOutcome::Rollback((
					// concluded disputes for backed candidates in this block
					concluded_invalid_disputes,
					// filtered bitfields,
					bitfields,
					// updated schedule
					scheduled,
				))
			});

		let mut backed_candidates = sanitize_backed_candidates::<T, _>(
			parent_hash,
			backed_candidates,
			move |candidate_hash: CandidateHash| -> bool {
				concluded_invalid_disputes.contains(&candidate_hash)
			},
			&scheduled[..],
		);

		let entropy = compute_entropy::<T>(parent_hash);
		let mut rng = rand_chacha::ChaChaRng::from_seed(entropy.into());
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

	while !preferred_indices.is_empty() {
		// randomly pick an index from the preferred set
		let pick = rng.gen_range(0..preferred_indices.len());
		// remove the index from the available set of preferred indices
		let preferred_idx = preferred_indices.swap_remove(pick);

		// preferred indices originate from outside
		if let Some(item) = selectables.get(preferred_idx) {
			let updated = weight_acc + weight_fn(item);
			if updated > weight_limit {
				continue
			}
			weight_acc = updated;
			picked_indices.push(preferred_idx);
		}
	}

	while !indices.is_empty() {
		// randomly pick an index
		let pick = rng.gen_range(0..indices.len());
		// remove the index from the available set of indices
		let idx = indices.swap_remove(pick);

		let item = &selectables[idx];
		let updated = weight_acc + weight_fn(item);

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

/// Considers an upper threshold that the candidates must not exceed.
///
/// If there is sufficient space, all bitfields and candidates will be included.
///
/// Otherwise tries to include all bitfields, and fills in the remaining weight with candidates.
///
/// If even the bitfields are too large to fit into the `max_weight` limit, bitfields are randomly
/// picked and _no_ candidates will be included.
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
		let mut idx = 0_usize;
		candidates.retain(|_backed_candidate| {
			let exists = indices.binary_search(&idx).is_ok();
			idx += 1;
			exists
		});
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

	let mut idx = 0_usize;
	bitfields.retain(|_bitfield| {
		let exists = indices.binary_search(&idx).is_ok();
		idx += 1;
		exists
	});

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
/// `CHECK_SIGS` determines if validator signatures are checked. If true, bitfields that have an
/// invalid signature will be filtered out.
pub(crate) fn sanitize_bitfields<T: crate::inclusion::Config, const CHECK_SIGS: bool>(
	unchecked_bitfields: UncheckedSignedAvailabilityBitfields,
	disputed_bitfield: DisputedBitfield,
	expected_bits: usize,
	parent_hash: T::Hash,
	session_index: SessionIndex,
	validators: &[ValidatorId],
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
				"[CHECK_SIGS: {}] bad bitfield length: {} != {:?}",
				CHECK_SIGS,
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
				"[CHECK_SIGS: {}] bitfield contains disputed cores: {:?}",
				CHECK_SIGS,
				unchecked_bitfield.unchecked_payload().0.clone() & disputed_bitfield.0.clone()
			);
			continue
		}

		let validator_index = unchecked_bitfield.unchecked_validator_index();

		if !last_index.map_or(true, |last_index: ValidatorIndex| last_index < validator_index) {
			log::trace!(
				target: LOG_TARGET,
				"[CHECK_SIGS: {}] bitfield validator index is not greater than last: !({:?} < {})",
				CHECK_SIGS,
				last_index.as_ref().map(|x| x.0),
				validator_index.0
			);
			continue
		}

		if unchecked_bitfield.unchecked_validator_index().0 as usize >= validators.len() {
			log::trace!(
				target: LOG_TARGET,
				"[CHECK_SIGS: {}] bitfield validator index is out of bounds: {} >= {}",
				CHECK_SIGS,
				validator_index.0,
				validators.len(),
			);
			continue
		}

		let validator_public = &validators[validator_index.0 as usize];

		if CHECK_SIGS {
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
fn sanitize_backed_candidates<T: crate::inclusion::Config, F: Fn(CandidateHash) -> bool>(
	relay_parent: T::Hash,
	mut backed_candidates: Vec<BackedCandidate<T::Hash>>,
	candidate_has_concluded_invalid_dispute: F,
	scheduled: &[CoreAssignment],
) -> Vec<BackedCandidate<T::Hash>> {
	// Remove any candidates that were concluded invalid.
	backed_candidates.retain(|backed_candidate| {
		!candidate_has_concluded_invalid_dispute(backed_candidate.candidate.hash())
	});

	// Assure the backed candidate's `ParaId`'s core is free.
	// This holds under the assumption that `Scheduler::schedule` is called _before_.
	// Also checks the candidate references the correct relay parent.
	let scheduled_paras_set = scheduled
		.into_iter()
		.map(|core_assignment| core_assignment.para_id)
		.collect::<BTreeSet<_>>();
	backed_candidates.retain(|backed_candidate| {
		let desc = backed_candidate.descriptor();
		desc.relay_parent == relay_parent && scheduled_paras_set.contains(&desc.para_id)
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

#[cfg(test)]
mod tests {
	use super::*;

	// In order to facilitate benchmarks as tests we have a benchmark feature gated `WeightInfo` impl
	// that uses 0 for all the weights. Because all the weights are 0, the tests that rely on
	// weights for limiting data will fail, so we don't run them when using the benchmark feature.
	#[cfg(not(feature = "runtime-benchmarks"))]
	mod enter {
		use super::*;
		use crate::{
			builder::{Bench, BenchBuilder},
			mock::{new_test_ext, MockGenesisConfig, Test},
		};
		use frame_support::assert_ok;
		use sp_std::collections::btree_map::BTreeMap;

		struct TestConfig {
			dispute_statements: BTreeMap<u32, u32>,
			dispute_sessions: Vec<u32>,
			backed_and_concluding: BTreeMap<u32, u32>,
			num_validators_per_core: u32,
			includes_code_upgrade: Option<u32>,
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
			BenchBuilder::<Test>::new()
				.set_max_validators((dispute_sessions.len() as u32) * num_validators_per_core)
				.set_max_validators_per_core(num_validators_per_core)
				.set_dispute_statements(dispute_statements)
				.build(backed_and_concluding, dispute_sessions.as_slice(), includes_code_upgrade)
		}

		#[test]
		// Validate that if we create 2 backed candidates which are assigned to 2 cores that will be freed via
		// becoming fully available, the backed candidates will not be filtered out in `create_inherent` and
		// will not cause `enter` to early.
		fn include_backed_candidates() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				let dispute_statements = BTreeMap::new();

				let mut backed_and_concluding = BTreeMap::new();
				backed_and_concluding.insert(0, 1);
				backed_and_concluding.insert(1, 1);

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					dispute_sessions: vec![0, 0],
					backed_and_concluding,
					num_validators_per_core: 1,
					includes_code_upgrade: None,
				});

				// We expect the scenario to have cores 0 & 1 with pending availability. The backed
				// candidates are also created for cores 0 & 1, so once the pending available
				// become fully available those cores are marked as free and scheduled for the backed
				// candidates.
				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (2 validators)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 2);
				// * 1 backed candidate per core (2 cores)
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
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
					2
				);
			});
		}

		#[test]
		// Ensure that disputes are filtered out if the session is in the future.
		fn filter_multi_dispute_data() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let dispute_statements = BTreeMap::new();

				let backed_and_concluding = BTreeMap::new();

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					dispute_sessions: vec![
						1, 2, 3, /* Session 3 too new, will get filtered out */
					],
					backed_and_concluding,
					num_validators_per_core: 5,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (5 validators per core, 3 disputes => 3 cores, 15 validators)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 15);
				// * 0 backed candidate per core
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 0);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				let multi_dispute_inherent_data =
					Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
				// Dispute for session that lies too far in the future should be filtered out
				assert!(multi_dispute_inherent_data != expected_para_inherent_data);

				assert_eq!(multi_dispute_inherent_data.disputes.len(), 2);

				// Assert that the first 2 disputes are included
				assert_eq!(
					&multi_dispute_inherent_data.disputes[..2],
					&expected_para_inherent_data.disputes[..2],
				);

				// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
				// alter storage, but just double checking for sanity).
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_eq!(Pallet::<Test>::on_chain_votes(), None);
				// Call enter with our 2 disputes
				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					multi_dispute_inherent_data,
				));

				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know there
					// where no backed candidates included
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
					0
				);
			});
		}

		#[test]
		// Ensure that when dispute data establishes an over weight block that we adequately
		// filter out disputes according to our prioritization rule
		fn limit_dispute_data() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let dispute_statements = BTreeMap::new();
				// No backed and concluding cores, so all cores will be fileld with disputesw
				let backed_and_concluding = BTreeMap::new();

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					dispute_sessions: vec![2, 2, 1], // 3 cores, all disputes
					backed_and_concluding,
					num_validators_per_core: 6,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (6 validators per core, 3 disputes => 18 validators)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 18);
				// * 0 backed candidate per core
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 0);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				let limit_inherent_data =
					Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
				// Expect that inherent data is filtered to include only 2 disputes
				assert!(limit_inherent_data != expected_para_inherent_data);

				// Ensure that the included disputes are sorted by session
				assert_eq!(limit_inherent_data.disputes.len(), 2);
				assert_eq!(limit_inherent_data.disputes[0].session, 1);
				assert_eq!(limit_inherent_data.disputes[1].session, 2);

				// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
				// alter storage, but just double checking for sanity).
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_eq!(Pallet::<Test>::on_chain_votes(), None);
				// Call enter with our 2 disputes
				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					limit_inherent_data,
				));

				assert_eq!(
					// Ensure that our inherent data did not included backed candidates as expected
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
					0
				);
			});
		}

		#[test]
		// Ensure that when dispute data establishes an over weight block that we abort
		// due to an over weight block
		fn limit_dispute_data_overweight() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let dispute_statements = BTreeMap::new();
				// No backed and concluding cores, so all cores will be fileld with disputesw
				let backed_and_concluding = BTreeMap::new();

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					dispute_sessions: vec![2, 2, 1], // 3 cores, all disputes
					backed_and_concluding,
					num_validators_per_core: 6,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (6 validators per core, 3 disputes => 18 validators)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 18);
				// * 0 backed candidate per core
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 0);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					expected_para_inherent_data,
				));
			});
		}

		#[test]
		// Ensure that when a block is over weight due to disputes, but there is still sufficient
		// block weight to include a number of signed bitfields, the inherent data is filtered
		// as expected
		fn limit_dispute_data_ignore_backed_candidates() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let dispute_statements = BTreeMap::new();

				let mut backed_and_concluding = BTreeMap::new();
				// 2 backed candidates shall be scheduled
				backed_and_concluding.insert(0, 2);
				backed_and_concluding.insert(1, 2);

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					// 2 backed candidates + 3 disputes (at sessions 2, 1 and 1)
					dispute_sessions: vec![0, 0, 2, 2, 1],
					backed_and_concluding,
					num_validators_per_core: 4,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (4 validators per core, 2 backed candidates, 3 disputes => 4*5 = 20)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 20);
				// * 2 backed candidates
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				// Nothing is filtered out (including the backed candidates.)
				let limit_inherent_data =
					Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
				assert!(limit_inherent_data != expected_para_inherent_data);

				// Three disputes is over weight (see previous test), so we expect to only see 2 disputes
				assert_eq!(limit_inherent_data.disputes.len(), 2);
				// Ensure disputes are filtered as expected
				assert_eq!(limit_inherent_data.disputes[0].session, 1);
				assert_eq!(limit_inherent_data.disputes[1].session, 2);
				// Ensure all bitfields are included as these are still not over weight
				assert_eq!(
					limit_inherent_data.bitfields.len(),
					expected_para_inherent_data.bitfields.len()
				);
				// Ensure that all backed candidates are filtered out as either would make the block over weight
				assert_eq!(limit_inherent_data.backed_candidates.len(), 0);

				// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
				// alter storage, but just double checking for sanity).
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_eq!(Pallet::<Test>::on_chain_votes(), None);
				// Call enter with our 2 disputes
				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					limit_inherent_data,
				));

				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know
					// all of our candidates got filtered out
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
					0,
				);
			});
		}

		#[test]
		// Ensure that we abort if we encounter an over weight block for disputes + bitfields
		fn limit_dispute_data_ignore_backed_candidates_overweight() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let dispute_statements = BTreeMap::new();

				let mut backed_and_concluding = BTreeMap::new();
				// 2 backed candidates shall be scheduled
				backed_and_concluding.insert(0, 2);
				backed_and_concluding.insert(1, 2);

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					// 2 backed candidates + 3 disputes (at sessions 2, 1 and 1)
					dispute_sessions: vec![0, 0, 2, 2, 1],
					backed_and_concluding,
					num_validators_per_core: 4,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (4 validators per core, 2 backed candidates, 3 disputes => 4*5 = 20)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 20);
				// * 2 backed candidates
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				// Ensure that calling enter with 3 disputes and 2 candidates is over weight
				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					expected_para_inherent_data,
				));

				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know
					// all of our candidates got filtered out
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
					0,
				);
			});
		}

		#[test]
		// Ensure that when a block is over weight due to disputes and bitfields, the bitfields are
		// filtered to accommodate the block size and no backed candidates are included.
		fn limit_bitfields() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let mut dispute_statements = BTreeMap::new();
				// Cap the number of statements per dispute to 20 in order to ensure we have enough
				// space in the block for some (but not all) bitfields
				dispute_statements.insert(2, 20);
				dispute_statements.insert(3, 20);
				dispute_statements.insert(4, 20);

				let mut backed_and_concluding = BTreeMap::new();
				// Schedule 2 backed candidates
				backed_and_concluding.insert(0, 2);
				backed_and_concluding.insert(1, 2);

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					// 2 backed candidates + 3 disputes (at sessions 2, 1 and 1)
					dispute_sessions: vec![0, 0, 2, 2, 1],
					backed_and_concluding,
					num_validators_per_core: 5,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (5 validators per core, 2 backed candidates, 3 disputes => 4*5 = 20),
				assert_eq!(expected_para_inherent_data.bitfields.len(), 25);
				// * 2 backed candidates,
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				// Nothing is filtered out (including the backed candidates.)
				let limit_inherent_data =
					Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
				assert!(limit_inherent_data != expected_para_inherent_data);

				// Three disputes is over weight (see previous test), so we expect to only see 2 disputes
				assert_eq!(limit_inherent_data.disputes.len(), 2);
				// Ensure disputes are filtered as expected
				assert_eq!(limit_inherent_data.disputes[0].session, 1);
				assert_eq!(limit_inherent_data.disputes[1].session, 2);
				// Ensure all bitfields are included as these are still not over weight
				assert_eq!(limit_inherent_data.bitfields.len(), 20,);
				// Ensure that all backed candidates are filtered out as either would make the block over weight
				assert_eq!(limit_inherent_data.backed_candidates.len(), 0);

				// The schedule is still empty prior to calling `enter`. (`create_inherent_inner` should not
				// alter storage, but just double checking for sanity).
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_eq!(Pallet::<Test>::on_chain_votes(), None);
				// Call enter with our 2 disputes
				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					limit_inherent_data,
				));

				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know
					// all of our candidates got filtered out
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
					0,
				);
			});
		}

		#[test]
		// Ensure that when a block is over weight due to disputes and bitfields, we abort
		fn limit_bitfields_overweight() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let mut dispute_statements = BTreeMap::new();
				// Control the number of statements per dispute to ensure we have enough space
				// in the block for some (but not all) bitfields
				dispute_statements.insert(2, 20);
				dispute_statements.insert(3, 20);
				dispute_statements.insert(4, 20);

				let mut backed_and_concluding = BTreeMap::new();
				// 2 backed candidates shall be scheduled
				backed_and_concluding.insert(0, 2);
				backed_and_concluding.insert(1, 2);

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					// 2 backed candidates + 3 disputes (at sessions 2, 1 and 1)
					dispute_sessions: vec![0, 0, 2, 2, 1],
					backed_and_concluding,
					num_validators_per_core: 5,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (5 validators per core, 2 backed candidates, 3 disputes => 5*5 = 25)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 25);
				// * 2 backed candidates
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					expected_para_inherent_data,
				));

				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know
					// all of our candidates got filtered out
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
					0,
				);
			});
		}

		#[test]
		// Ensure that when a block is over weight due to disputes and bitfields, we abort
		fn limit_candidates_over_weight() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let mut dispute_statements = BTreeMap::new();
				// Control the number of statements per dispute to ensure we have enough space
				// in the block for some (but not all) bitfields
				dispute_statements.insert(2, 17);
				dispute_statements.insert(3, 17);
				dispute_statements.insert(4, 17);

				let mut backed_and_concluding = BTreeMap::new();
				// 2 backed candidates shall be scheduled
				backed_and_concluding.insert(0, 16);
				backed_and_concluding.insert(1, 25);

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					dispute_sessions: vec![0, 0, 2, 2, 1], // 2 backed candidates, 3 disputes at sessions 2, 1 and 1 respectively
					backed_and_concluding,
					num_validators_per_core: 5,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (5 validators per core, 2 backed candidates, 3 disputes => 5*5 = 25)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 25);
				// * 2 backed candidates
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);
				let mut inherent_data = InherentData::new();
				inherent_data
					.put_data(PARACHAINS_INHERENT_IDENTIFIER, &expected_para_inherent_data)
					.unwrap();

				let limit_inherent_data =
					Pallet::<Test>::create_inherent_inner(&inherent_data.clone()).unwrap();
				// Expect that inherent data is filtered to include only 1 backed candidate and 2 disputes
				assert!(limit_inherent_data != expected_para_inherent_data);

				// * 1 bitfields
				assert_eq!(limit_inherent_data.bitfields.len(), 25);
				// * 2 backed candidates
				assert_eq!(limit_inherent_data.backed_candidates.len(), 1);
				// * 3 disputes.
				assert_eq!(limit_inherent_data.disputes.len(), 2);

				// The current schedule is empty prior to calling `create_inherent_enter`.
				assert_eq!(<scheduler::Pallet<Test>>::scheduled(), vec![]);

				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					limit_inherent_data,
				));

				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know our 2
					// backed candidates did not get filtered out
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
					1
				);
			});
		}

		#[test]
		// Ensure that when a block is over weight due to disputes and bitfields, we abort
		fn limit_candidates_over_weight_overweight() {
			new_test_ext(MockGenesisConfig::default()).execute_with(|| {
				// Create the inherent data for this block
				let mut dispute_statements = BTreeMap::new();
				// Control the number of statements per dispute to ensure we have enough space
				// in the block for some (but not all) bitfields
				dispute_statements.insert(2, 17);
				dispute_statements.insert(3, 17);
				dispute_statements.insert(4, 17);

				let mut backed_and_concluding = BTreeMap::new();
				// 2 backed candidates shall be scheduled
				backed_and_concluding.insert(0, 16);
				backed_and_concluding.insert(1, 25);

				let scenario = make_inherent_data(TestConfig {
					dispute_statements,
					dispute_sessions: vec![0, 0, 2, 2, 1], // 2 backed candidates, 3 disputes at sessions 2, 1 and 1 respectively
					backed_and_concluding,
					num_validators_per_core: 5,
					includes_code_upgrade: None,
				});

				let expected_para_inherent_data = scenario.data.clone();

				// Check the para inherent data is as expected:
				// * 1 bitfield per validator (5 validators per core, 2 backed candidates, 3 disputes => 5*5 = 25)
				assert_eq!(expected_para_inherent_data.bitfields.len(), 25);
				// * 2 backed candidates
				assert_eq!(expected_para_inherent_data.backed_candidates.len(), 2);
				// * 3 disputes.
				assert_eq!(expected_para_inherent_data.disputes.len(), 3);

				assert_ok!(Pallet::<Test>::enter(
					frame_system::RawOrigin::None.into(),
					expected_para_inherent_data,
				));

				assert_eq!(
					// The length of this vec is equal to the number of candidates, so we know our 2
					// backed candidates did not get filtered out
					Pallet::<Test>::on_chain_votes()
						.unwrap()
						.backing_validators_per_candidate
						.len(),
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
				assert_eq!(
					sanitize_bitfields::<Test, VERIFY_SIGS>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					unchecked_bitfields.clone()
				);
				assert_eq!(
					sanitize_bitfields::<Test, SKIP_SIG_VERIFY>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					),
					unchecked_bitfields.clone()
				);
			}

			// disputed bitfield is non-zero
			{
				let mut disputed_bitfield = DisputedBitfield::zeros(expected_bits);
				// pretend the first core was freed by either a malicious validator
				// or by resolved dispute
				disputed_bitfield.0.set(0, true);

				assert_eq!(
					sanitize_bitfields::<Test, VERIFY_SIGS>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					)
					.len(),
					1
				);
				assert_eq!(
					sanitize_bitfields::<Test, SKIP_SIG_VERIFY>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					)
					.len(),
					1
				);
			}

			// bitfield size mismatch
			{
				assert!(sanitize_bitfields::<Test, VERIFY_SIGS>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits + 1,
					parent_hash,
					session_index,
					&validator_public[..]
				)
				.is_empty());
				assert!(sanitize_bitfields::<Test, SKIP_SIG_VERIFY>(
					unchecked_bitfields.clone(),
					disputed_bitfield.clone(),
					expected_bits + 1,
					parent_hash,
					session_index,
					&validator_public[..]
				)
				.is_empty());
			}

			// remove the last validator
			{
				let shortened = validator_public.len() - 2;
				assert_eq!(
					&sanitize_bitfields::<Test, VERIFY_SIGS>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..shortened]
					)[..],
					&unchecked_bitfields[..shortened]
				);
				assert_eq!(
					&sanitize_bitfields::<Test, SKIP_SIG_VERIFY>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..shortened]
					)[..],
					&unchecked_bitfields[..shortened]
				);
			}

			// switch ordering of bitfields
			{
				let mut unchecked_bitfields = unchecked_bitfields.clone();
				let x = unchecked_bitfields.swap_remove(0);
				unchecked_bitfields.push(x);
				assert_eq!(
					&sanitize_bitfields::<Test, VERIFY_SIGS>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					)[..],
					&unchecked_bitfields[..(unchecked_bitfields.len() - 2)]
				);
				assert_eq!(
					&sanitize_bitfields::<Test, SKIP_SIG_VERIFY>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					)[..],
					&unchecked_bitfields[..(unchecked_bitfields.len() - 2)]
				);
			}

			// check the validators signature
			{
				use primitives::v1::ValidatorSignature;
				let mut unchecked_bitfields = unchecked_bitfields.clone();

				// insert a bad signature for the last bitfield
				let last_bit_idx = unchecked_bitfields.len() - 1;
				unchecked_bitfields
					.get_mut(last_bit_idx)
					.and_then(|u| Some(u.set_signature(ValidatorSignature::default())))
					.expect("we are accessing a valid index");
				assert_eq!(
					&sanitize_bitfields::<Test, VERIFY_SIGS>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					)[..],
					&unchecked_bitfields[..last_bit_idx]
				);
				assert_eq!(
					&sanitize_bitfields::<Test, SKIP_SIG_VERIFY>(
						unchecked_bitfields.clone(),
						disputed_bitfield.clone(),
						expected_bits,
						parent_hash,
						session_index,
						&validator_public[..]
					)[..],
					&unchecked_bitfields[..]
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

			// happy path
			assert_eq!(
				sanitize_backed_candidates::<Test, _>(
					relay_parent,
					backed_candidates.clone(),
					has_concluded_invalid,
					scheduled
				),
				backed_candidates
			);

			// nothing is scheduled, so no paraids match, thus all backed candidates are skipped
			{
				let scheduled = &[][..];
				assert!(sanitize_backed_candidates::<Test, _>(
					relay_parent,
					backed_candidates.clone(),
					has_concluded_invalid,
					scheduled
				)
				.is_empty());
			}

			// relay parent mismatch
			{
				let relay_parent = Hash::repeat_byte(0xFA);
				assert!(sanitize_backed_candidates::<Test, _>(
					relay_parent,
					backed_candidates.clone(),
					has_concluded_invalid,
					scheduled
				)
				.is_empty());
			}

			// candidates that have concluded as invalid are filtered out
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
				assert_eq!(
					sanitize_backed_candidates::<Test, _>(
						relay_parent,
						backed_candidates.clone(),
						has_concluded_invalid,
						scheduled
					)
					.len(),
					backed_candidates.len() / 2
				);
			}
		}
	}
}
