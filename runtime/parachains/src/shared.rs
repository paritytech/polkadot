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

//! A pallet for any shared state that other pallets may want access to.
//!
//! To avoid cyclic dependencies, it is important that this pallet is not
//! dependent on any of the other pallets.

use frame_support::pallet_prelude::*;
use primitives::v1::{CandidateHash, CoreIndex, SessionIndex, ValidatorId, ValidatorIndex};
use sp_consensus_babe::digests::{CompatibleDigestItem, PreDigest};
use sp_runtime::traits::{One, Zero};
use sp_std::vec::Vec;

use rand::{seq::SliceRandom, SeedableRng};
use rand_chacha::ChaCha20Rng;

use crate::configuration::HostConfiguration;

pub use pallet::*;

// `SESSION_DELAY` is used to delay any changes to Paras registration or configurations.
// Wait until the session index is 2 larger then the current index to apply any changes,
// which guarantees that at least one full session has passed before any changes are applied.
pub(crate) const SESSION_DELAY: SessionIndex = 2;

// `MAX_CANDIDATES_TO_PRUNE` is used to upper bound the number of ancient candidates that can be
// pruned when a new block is included. This is used to distribute the cost of pruning over
// multiple blocks rather than pruning all historical candidates upon starting a new session.
pub(crate) const MAX_CANDIDATES_TO_PRUNE: usize = 200;

#[frame_support::pallet]
pub mod pallet {
	use super::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	pub struct Pallet<T>(_);

	#[pallet::config]
	pub trait Config: frame_system::Config {}

	/// The current session index.
	#[pallet::storage]
	#[pallet::getter(fn session_index)]
	pub(super) type CurrentSessionIndex<T: Config> = StorageValue<_, SessionIndex, ValueQuery>;

	/// All the validators actively participating in parachain consensus.
	/// Indices are into the broader validator set.
	#[pallet::storage]
	#[pallet::getter(fn active_validator_indices)]
	pub(super) type ActiveValidatorIndices<T: Config> =
		StorageValue<_, Vec<ValidatorIndex>, ValueQuery>;

	/// The parachain attestation keys of the validators actively participating in parachain consensus.
	/// This should be the same length as `ActiveValidatorIndices`.
	#[pallet::storage]
	#[pallet::getter(fn active_validator_keys)]
	pub(super) type ActiveValidatorKeys<T: Config> = StorageValue<_, Vec<ValidatorId>, ValueQuery>;

	/// All included blocks on the chain, mapped to the core index the candidate was assigned to
	/// and the block number in this chain that should be reverted back to if the candidate is
	/// disputed and determined to be invalid.
	#[pallet::storage]
	#[pallet::getter(fn included_candidates)]
	pub(super) type IncludedCandidates<T: Config> = StorageDoubleMap<
		_,
		Twox64Concat,
		SessionIndex,
		Blake2_128Concat,
		CandidateHash,
		(T::BlockNumber, CoreIndex),
	>;

	/// The set of all past sessions that have yet to fully pruned. Sessions are added to this set
	/// upon moving to new current session, and removed after all of their included candidates
	/// have been removed. This set is tracked independently so that the cost of removing included
	/// candidates can be amortized over multiple blocks rather than removing all candidates upon
	/// transitioning to a new session.
	#[pallet::storage]
	#[pallet::getter(fn pruneable_sessions)]
	pub(super) type PruneableSessions<T: Config> = StorageMap<_, Twox64Concat, SessionIndex, ()>;

	#[pallet::storage]
	#[pallet::getter(fn historical_babe_vrfs)]
	pub(super) type HistoricalBabeVrfs<T: Config> = StorageDoubleMap<
		_,
		Blake2_128Concat,
		SessionIndex,
		Twox64Concat,
		T::BlockNumber,
		Vec<Vec<u8>>,
	>;

	#[pallet::call]
	impl<T: Config> Pallet<T> {}
}

impl<T: Config> Pallet<T> {
	/// Called by the initializer to initialize the configuration pallet.
	pub(crate) fn initializer_initialize(_now: T::BlockNumber) -> Weight {
		0
	}

	/// Called by the initializer to finalize the configuration pallet.
	pub(crate) fn initializer_finalize() {}

	/// Called by the initializer to note that a new session has started.
	///
	/// Returns the list of outgoing paras from the actions queue.
	pub(crate) fn initializer_on_new_session(
		session_index: SessionIndex,
		random_seed: [u8; 32],
		new_config: &HostConfiguration<T::BlockNumber>,
		all_validators: Vec<ValidatorId>,
	) -> Vec<ValidatorId> {
		CurrentSessionIndex::<T>::set(session_index);
		let mut rng: ChaCha20Rng = SeedableRng::from_seed(random_seed);

		let mut shuffled_indices: Vec<_> = (0..all_validators.len())
			.enumerate()
			.map(|(i, _)| ValidatorIndex(i as _))
			.collect();

		shuffled_indices.shuffle(&mut rng);

		if let Some(max) = new_config.max_validators {
			shuffled_indices.truncate(max as usize);
		}

		let active_validator_keys =
			crate::util::take_active_subset(&shuffled_indices, &all_validators);

		ActiveValidatorIndices::<T>::set(shuffled_indices);
		ActiveValidatorKeys::<T>::set(active_validator_keys.clone());

		active_validator_keys
	}

	/// Return the session index that should be used for any future scheduled changes.
	pub fn scheduled_session() -> SessionIndex {
		Self::session_index().saturating_add(SESSION_DELAY)
	}

	/// Adds a session that is no longer in the dispute window as pruneable. Subsequent block
	/// inclusions will incrementally remove old candidates to avoid taking the performance hit of
	/// removing all candidates at once.
	pub(crate) fn mark_session_pruneable(session: SessionIndex) {
		PruneableSessions::<T>::insert(session, ());
	}

	/// Prunes up to `max_candidates_to_prune` candidates from `IncludedCandidates` that belong to
	/// non-active sessions.
	pub(crate) fn prune_ancient_sessions(max_candidates_to_prune: usize) {
		let mut to_prune_candidates = Vec::new();
		let mut to_prune_blocks = Vec::new();

		let mut n_candidates = 0;

		let mut incomplete_session = None;
		for session in PruneableSessions::<T>::iter_keys() {
			let mut hashes = Vec::new();
			for candidate_hash in IncludedCandidates::<T>::iter_key_prefix(session) {
				// Exit condition when this session still has more candidates to prune; mark the
				// session as incomplete so the remaining candidates can be pruned in a subsequent
				// invocation.
				if n_candidates >= max_candidates_to_prune {
					incomplete_session = Some(session);
					break
				}
				hashes.push(candidate_hash);
				n_candidates += 1;
			}

			for block_number in HistoricalBabeVrfs::<T>::iter_key_prefix(session) {
				to_prune_blocks.push((session, block_number));
				if incomplete_session.is_some() {
					break
				}
			}

			to_prune_candidates.push((session, hashes));
			// Exit condition when all candidates from this session were selected for pruning.
			if n_candidates >= max_candidates_to_prune {
				break
			}
		}

		for (session, block_number) in to_prune_blocks {
			HistoricalBabeVrfs::<T>::remove(session, block_number);
		}

		for (session, candidate_hashes) in to_prune_candidates {
			for candidate_hash in candidate_hashes {
				IncludedCandidates::<T>::remove(session, candidate_hash);
			}

			// Prune the session only if it was not marked as incomplete.
			match incomplete_session {
				Some(incomplete_session) if incomplete_session == session => {},
				_ => PruneableSessions::<T>::remove(session),
			}
		}
	}

	/// Records an included candidate, returning the block height that should be reverted to if the
	/// block is found to be invalid. This method will return `None` if and only if `included_in`
	/// is zero.
	pub(crate) fn note_included_candidate(
		session: SessionIndex,
		candidate_hash: CandidateHash,
		included_in: T::BlockNumber,
		core_index: CoreIndex,
	) -> Option<T::BlockNumber> {
		if included_in.is_zero() {
			return None
		}

		let revert_to = included_in - One::one();
		<IncludedCandidates<T>>::insert(&session, &candidate_hash, (revert_to, core_index));

		Some(revert_to)
	}

	/// Returns true if a candidate hash exists for the given session and candidate hash.
	pub(crate) fn is_included_candidate(
		session: &SessionIndex,
		candidate_hash: &CandidateHash,
	) -> bool {
		<IncludedCandidates<T>>::contains_key(session, candidate_hash)
	}

	/// Returns the revert-to height and core index for a given candidate, if one exists.
	pub(crate) fn get_included_candidate(
		session: &SessionIndex,
		candidate_hash: &CandidateHash,
	) -> Option<(T::BlockNumber, CoreIndex)> {
		<IncludedCandidates<T>>::get(session, candidate_hash)
	}

	#[cfg(test)]
	pub(crate) fn is_pruneable_session(session: &SessionIndex) -> bool {
		<PruneableSessions<T>>::contains_key(session)
	}

	#[cfg(test)]
	pub(crate) fn included_candidates_iter_prefix(
		session: SessionIndex,
	) -> impl Iterator<Item = (CandidateHash, (T::BlockNumber, CoreIndex))> {
		<IncludedCandidates<T>>::iter_prefix(session)
	}

	/// Records an included historical BABE VRF
	pub fn extract_vrfs() -> Vec<Vec<u8>> {
		<frame_system::Pallet<T>>::digest()
			.logs
			.into_iter()
			.filter_map(|d| {
				d.as_babe_pre_digest()
					.map(|p| match p {
						PreDigest::Primary(primary) => Some(primary.vrf_output.encode()),
						PreDigest::SecondaryVRF(secondary) => Some(secondary.vrf_output.encode()),
						PreDigest::SecondaryPlain(_) => None,
					})
					.flatten()
			})
			.collect()
	}

	pub fn note_vrfs(
		session: SessionIndex,
		block_number: T::BlockNumber,
		vrfs: Vec<Vec<u8>>,
	) -> bool {
		if HistoricalBabeVrfs::<T>::get(session, block_number).is_none() {
			HistoricalBabeVrfs::<T>::insert(session, block_number, vrfs);
			true
		} else {
			false
		}
	}

	pub fn historical_vrfs(session: SessionIndex, block_number: T::BlockNumber) -> Vec<Vec<u8>> {
		HistoricalBabeVrfs::<T>::get(session, block_number).unwrap_or_default()
	}

	/// Test function for setting the current session index.
	#[cfg(any(feature = "std", feature = "runtime-benchmarks", test))]
	pub fn set_session_index(index: SessionIndex) {
		CurrentSessionIndex::<T>::set(index);
	}

	#[cfg(test)]
	pub(crate) fn set_active_validators_ascending(active: Vec<ValidatorId>) {
		ActiveValidatorIndices::<T>::set(
			(0..active.len()).map(|i| ValidatorIndex(i as _)).collect(),
		);
		ActiveValidatorKeys::<T>::set(active);
	}

	#[cfg(test)]
	pub(crate) fn set_active_validators_with_indices(
		indices: Vec<ValidatorIndex>,
		keys: Vec<ValidatorId>,
	) {
		assert_eq!(indices.len(), keys.len());
		ActiveValidatorIndices::<T>::set(indices);
		ActiveValidatorKeys::<T>::set(keys);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		configuration::HostConfiguration,
		mock::{new_test_ext, MockGenesisConfig, ParasShared},
	};
	use keyring::Sr25519Keyring;

	fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
		val_ids.iter().map(|v| v.public().into()).collect()
	}

	#[test]
	fn sets_and_shuffles_validators() {
		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		];

		let mut config = HostConfiguration::default();
		config.max_validators = None;

		let pubkeys = validator_pubkeys(&validators);

		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let validators = ParasShared::initializer_on_new_session(1, [1; 32], &config, pubkeys);

			assert_eq!(
				validators,
				validator_pubkeys(&[
					Sr25519Keyring::Ferdie,
					Sr25519Keyring::Bob,
					Sr25519Keyring::Charlie,
					Sr25519Keyring::Dave,
					Sr25519Keyring::Alice,
				])
			);

			assert_eq!(ParasShared::active_validator_keys(), validators);

			assert_eq!(
				ParasShared::active_validator_indices(),
				vec![
					ValidatorIndex(4),
					ValidatorIndex(1),
					ValidatorIndex(2),
					ValidatorIndex(3),
					ValidatorIndex(0),
				]
			);
		});
	}

	#[test]
	fn sets_truncates_and_shuffles_validators() {
		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		];

		let mut config = HostConfiguration::default();
		config.max_validators = Some(2);

		let pubkeys = validator_pubkeys(&validators);

		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let validators = ParasShared::initializer_on_new_session(1, [1; 32], &config, pubkeys);

			assert_eq!(
				validators,
				validator_pubkeys(&[Sr25519Keyring::Ferdie, Sr25519Keyring::Bob,])
			);

			assert_eq!(ParasShared::active_validator_keys(), validators);

			assert_eq!(
				ParasShared::active_validator_indices(),
				vec![ValidatorIndex(4), ValidatorIndex(1),]
			);
		});
	}

	#[test]
	fn note_included_candidate_ignores_block_zero() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let session = 1;
			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
			assert!(ParasShared::note_included_candidate(session, candidate_hash, 0, CoreIndex(0))
				.is_none());
			assert!(!ParasShared::is_included_candidate(&session, &candidate_hash));
			assert!(ParasShared::get_included_candidate(&session, &candidate_hash).is_none());
		});
	}

	#[test]
	fn note_included_candidate_stores_block_non_zero_with_revert_to_minus_one() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let session = 1;
			let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
			let block_number = 1;
			let core_index = CoreIndex(2);
			assert_eq!(
				ParasShared::note_included_candidate(
					session,
					candidate_hash,
					block_number,
					core_index
				),
				Some(block_number - 1),
			);
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash));
			assert_eq!(
				ParasShared::get_included_candidate(&session, &candidate_hash),
				Some((block_number - 1, core_index)),
			);
		});
	}

	#[test]
	fn prune_ancient_sessions_no_incomplete_session() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let session = 1;
			let candidate_hash1 = CandidateHash(sp_core::H256::repeat_byte(1));
			let candidate_hash2 = CandidateHash(sp_core::H256::repeat_byte(2));
			let block_number = 1;
			let core_index = CoreIndex(0);

			assert_eq!(
				ParasShared::note_included_candidate(
					session,
					candidate_hash1,
					block_number,
					core_index,
				),
				Some(block_number - 1),
			);
			assert_eq!(
				ParasShared::note_included_candidate(
					session,
					candidate_hash2,
					block_number,
					core_index,
				),
				Some(block_number - 1),
			);
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash1));
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash2));

			// Prune before any sessions are marked pruneable.
			ParasShared::prune_ancient_sessions(2);

			// Both candidates should still exist.
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash1));
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash2));

			// Mark the candidates' session as pruneable.
			ParasShared::mark_session_pruneable(session);
			assert!(ParasShared::is_pruneable_session(&session));

			// Prune the sessions, which should remove both candidates. The session should not be
			// marked as incomplete since there are exactly two candidates.
			ParasShared::prune_ancient_sessions(2);

			assert!(!ParasShared::is_pruneable_session(&session));
			assert!(!ParasShared::is_included_candidate(&session, &candidate_hash1));
			assert!(!ParasShared::is_included_candidate(&session, &candidate_hash2));
		})
	}

	#[test]
	fn prune_ancient_sessions_incomplete_session() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let session = 1;
			let candidate_hash1 = CandidateHash(sp_core::H256::repeat_byte(1));
			let candidate_hash2 = CandidateHash(sp_core::H256::repeat_byte(2));
			let block_number = 1;
			let core_index = CoreIndex(0);

			assert_eq!(
				ParasShared::note_included_candidate(
					session,
					candidate_hash1,
					block_number,
					core_index,
				),
				Some(block_number - 1),
			);
			assert_eq!(
				ParasShared::note_included_candidate(
					session,
					candidate_hash2,
					block_number,
					core_index,
				),
				Some(block_number - 1),
			);
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash1));
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash2));

			// Prune before any sessions are marked pruneable.
			ParasShared::prune_ancient_sessions(1);

			// Both candidates should still exist.
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash1));
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash2));

			// Mark the candidates' session as pruneable.
			ParasShared::mark_session_pruneable(session);
			assert!(ParasShared::is_pruneable_session(&session));

			// Prune the sessions, which should remove one of the candidates. The session will be
			// marked as incomplete so the session should remain unpruned.
			ParasShared::prune_ancient_sessions(1);

			assert!(ParasShared::is_pruneable_session(&session));
			assert!(!ParasShared::is_included_candidate(&session, &candidate_hash1));
			assert!(ParasShared::is_included_candidate(&session, &candidate_hash2));
		})
	}

	#[test]
	fn prune_ancient_sessions_complete_and_incomplete_sessions() {
		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let session1 = 1;
			let session2 = 2;
			let candidate_hash1 = CandidateHash(sp_core::H256::repeat_byte(1));
			let candidate_hash2 = CandidateHash(sp_core::H256::repeat_byte(2));
			let candidate_hash3 = CandidateHash(sp_core::H256::repeat_byte(3));
			let block_number1 = 1;
			let block_number2 = 2;
			let core_index = CoreIndex(0);

			assert_eq!(
				ParasShared::note_included_candidate(
					session1,
					candidate_hash1,
					block_number1,
					core_index,
				),
				Some(block_number1 - 1),
			);
			assert_eq!(
				ParasShared::note_included_candidate(
					session2,
					candidate_hash2,
					block_number2,
					core_index,
				),
				Some(block_number2 - 1),
			);
			assert_eq!(
				ParasShared::note_included_candidate(
					session2,
					candidate_hash3,
					block_number2,
					core_index,
				),
				Some(block_number2 - 1),
			);
			assert!(ParasShared::is_included_candidate(&session1, &candidate_hash1));
			assert!(ParasShared::is_included_candidate(&session2, &candidate_hash2));
			assert!(ParasShared::is_included_candidate(&session2, &candidate_hash3));

			// Prune before any sessions are marked pruneable.
			ParasShared::prune_ancient_sessions(2);

			// Both candidates should still exist.
			assert!(ParasShared::is_included_candidate(&session1, &candidate_hash1));
			assert!(ParasShared::is_included_candidate(&session2, &candidate_hash2));
			assert!(ParasShared::is_included_candidate(&session2, &candidate_hash3));

			// Mark the candidates' session as pruneable.
			ParasShared::mark_session_pruneable(session1);
			ParasShared::mark_session_pruneable(session2);
			assert!(ParasShared::is_pruneable_session(&session1));
			assert!(ParasShared::is_pruneable_session(&session2));

			// Prune the sessions, which should remove one candidate from each session. The first
			// session should be pruned while the second session will be marked as incomplete, and
			// so should remain in the set of pruneable sessions.
			ParasShared::prune_ancient_sessions(2);

			assert!(!ParasShared::is_pruneable_session(&session1));
			assert!(ParasShared::is_pruneable_session(&session2));
			assert!(!ParasShared::is_included_candidate(&session1, &candidate_hash1));
			assert!(ParasShared::is_included_candidate(&session2, &candidate_hash2));
			assert!(!ParasShared::is_included_candidate(&session2, &candidate_hash3));
		})
	}
}
