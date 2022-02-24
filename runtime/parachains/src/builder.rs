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

use crate::{
	configuration, inclusion, paras,
	paras_inherent::{self},
	runner::PalletRunner,
	scheduler,
};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use frame_support::pallet_prelude::*;
use primitives::v1::{
	supermajority_threshold, AvailabilityBitfield, BackedCandidate, CandidateCommitments,
	CandidateDescriptor, CandidateHash, CollatorId, CollatorSignature, CoreIndex, CoreOccupied,
	DisputeStatementSet, GroupIndex, HeadData, Id as ParaId,
	InherentData as ParachainsInherentData, SessionIndex, ValidationCode, ValidatorId,
};
use sp_core::{sr25519, H256};
use sp_runtime::{
	traits::{Header as HeaderT, One, TrailingZeroInput, Zero},
	RuntimeAppPublic,
};
use sp_std::{collections::btree_map::BTreeMap, prelude::Vec, vec};

fn mock_validation_code() -> ValidationCode {
	ValidationCode(vec![1, 2, 3])
}

/// Grab an account, seeded by a name and index.
///
/// This is directly from frame-benchmarking. Copy/pasted so we can use it when not compiling with
/// "features = runtime-benchmarks".
fn account<AccountId: Decode>(name: &'static str, index: u32, seed: u32) -> AccountId {
	let entropy = (name, index, seed).using_encoded(sp_io::hashing::blake2_256);
	AccountId::decode(&mut TrailingZeroInput::new(&entropy[..]))
		.expect("infinite input; no invalid input; qed")
}

/// Create a 32 byte slice based on the given number.
fn byte32_slice_from(n: u32) -> [u8; 32] {
	let mut slice = [0u8; 32];
	slice[31] = (n % (1 << 8)) as u8;
	slice[30] = ((n >> 8) % (1 << 8)) as u8;
	slice[29] = ((n >> 16) % (1 << 8)) as u8;
	slice[28] = ((n >> 24) % (1 << 8)) as u8;

	slice
}

/// Paras inherent `enter` benchmark scenario builder.
pub(crate) struct BenchBuilder<T: paras_inherent::Config> {
	/// Active validators. Validators should be declared prior to all other setup.
	validators: Option<Vec<ValidatorId>>,
	/// Starting block number; we expect it to get incremented on session setup.
	block_number: T::BlockNumber,
	/// Starting session; we expect it to get incremented on session setup.
	session: SessionIndex,
	/// Session we want the scenario to take place in. We will roll to this session.
	target_session: u32,
	/// Optionally set the max validators per core; otherwise uses the configuration value.
	max_validators_per_core: Option<u32>,
	/// Optionally set the max validators; otherwise uses the configuration value.
	max_validators: Option<u32>,
	/// Optionally set the number of dispute statements for each candidate.
	dispute_statements: BTreeMap<u32, u32>,
	/// Session index of for each dispute. Index of slice corresponds to a core,
	/// which is offset by the number of entries for `backed_and_concluding_cores`. I.E. if
	/// `backed_and_concluding_cores` has 3 entries, the first index of `dispute_sessions`
	/// will correspond to core index 3. There must be one entry for each core with a dispute
	/// statement set.
	dispute_sessions: Vec<u32>,
	/// Map from core seed to number of validity votes.
	backed_and_concluding_cores: BTreeMap<u32, u32>,
	/// Make every candidate include a code upgrade by setting this to `Some` where the interior
	/// value is the byte length of the new code.
	code_upgrade: Option<u32>,
	_phantom: sp_std::marker::PhantomData<T>,
}

/// Paras inherent `enter` benchmark scenario.
#[cfg(any(feature = "runtime-benchmarks", test))]
pub(crate) struct Bench<T: paras_inherent::Config> {
	pub(crate) data: ParachainsInherentData<T::Header>,
	pub(crate) _session: u32,
	pub(crate) _block_number: T::BlockNumber,
}

impl<T: paras_inherent::Config> BenchBuilder<T> {
	/// Create a new `BenchBuilder` with some opinionated values that should work with the rest
	/// of the functions in this implementation.
	pub(crate) fn new() -> Self {
		BenchBuilder {
			validators: None,
			block_number: Zero::zero(),
			session: SessionIndex::from(0u32),
			target_session: 2u32,
			max_validators_per_core: None,
			max_validators: None,
			dispute_statements: BTreeMap::new(),
			dispute_sessions: Default::default(),
			backed_and_concluding_cores: Default::default(),
			code_upgrade: None,
			_phantom: sp_std::marker::PhantomData::<T>,
		}
	}

	/// Set the session index for each dispute statement set (in other words, set the session the
	/// the dispute statement set's relay chain block is from). Indexes of `dispute_sessions`
	/// correspond to a core, which is offset by the number of entries for
	/// `backed_and_concluding_cores`. I.E. if `backed_and_concluding_cores` cores has 3 entries,
	/// the first index of `dispute_sessions` will correspond to core index 3.
	///
	/// Note that there must be an entry for each core with a dispute statement set.
	pub(crate) fn set_dispute_sessions(mut self, dispute_sessions: impl AsRef<[u32]>) -> Self {
		self.dispute_sessions = dispute_sessions.as_ref().to_vec();
		self
	}

	/// Set a map from core/para id seed to number of validity votes.
	pub(crate) fn set_backed_and_concluding_cores(
		mut self,
		backed_and_concluding_cores: BTreeMap<u32, u32>,
	) -> Self {
		self.backed_and_concluding_cores = backed_and_concluding_cores;
		self
	}

	/// Set to include a code upgrade for all backed candidates. The value will be the byte length
	/// of the code.
	pub(crate) fn set_code_upgrade(mut self, code_upgrade: impl Into<Option<u32>>) -> Self {
		self.code_upgrade = code_upgrade.into();
		self
	}

	/// Mock header.
	pub(crate) fn header(block_number: T::BlockNumber) -> T::Header {
		T::Header::new(
			block_number,       // `block_number`,
			Default::default(), // `extrinsics_root`,
			Default::default(), // `storage_root`,
			Default::default(), // `parent_hash`,
			Default::default(), // digest,
		)
	}

	/// Maximum number of validators that may be part of a validator group.
	pub(crate) fn fallback_max_validators() -> u32 {
		configuration::Pallet::<T>::config().max_validators.unwrap_or(200)
	}

	/// Maximum number of validators participating in parachains consensus (a.k.a. active validators).
	fn max_validators(&self) -> u32 {
		self.max_validators.unwrap_or(Self::fallback_max_validators())
	}

	/// Set the maximum number of active validators.
	#[cfg(not(feature = "runtime-benchmarks"))]
	pub(crate) fn set_max_validators(mut self, n: u32) -> Self {
		self.max_validators = Some(n);
		self
	}

	/// Maximum number of validators per core (a.k.a. max validators per group). This value is used if none is
	/// explicitly set on the builder.
	pub(crate) fn fallback_max_validators_per_core() -> u32 {
		configuration::Pallet::<T>::config().max_validators_per_core.unwrap_or(5)
	}

	/// Specify a mapping of core index/ para id to the number of dispute statements for the
	/// corresponding dispute statement set. Note that if the number of disputes is not specified
	/// it fallbacks to having a dispute per every validator. Additionally, an entry is not
	/// guaranteed to have a dispute - it must line up with the cores marked as disputed as defined
	/// in `Self::Build`.
	#[cfg(not(feature = "runtime-benchmarks"))]
	pub(crate) fn set_dispute_statements(mut self, m: BTreeMap<u32, u32>) -> Self {
		self.dispute_statements = m;
		self
	}

	/// Get the maximum number of validators per core.
	fn max_validators_per_core(&self) -> u32 {
		self.max_validators_per_core.unwrap_or(Self::fallback_max_validators_per_core())
	}

	/// Set maximum number of validators per core.
	#[cfg(not(feature = "runtime-benchmarks"))]
	pub(crate) fn set_max_validators_per_core(mut self, n: u32) -> Self {
		self.max_validators_per_core = Some(n);
		self
	}

	/// Get the maximum number of cores we expect from this configuration.
	pub(crate) fn max_cores(&self) -> u32 {
		self.max_validators() / self.max_validators_per_core()
	}

	/// Get the minimum number of validity votes in order for a backed candidate to be included.
	#[cfg(feature = "runtime-benchmarks")]
	pub(crate) fn fallback_min_validity_votes() -> u32 {
		(Self::fallback_max_validators() / 2) + 1
	}

	/// Create para id, core index, and grab the associated group index from the scheduler pallet.
	fn create_indexes(&self, seed: u32) -> (ParaId, CoreIndex, GroupIndex) {
		let para_id = ParaId::from(seed);
		let core_idx = CoreIndex(seed);
		let group_idx =
			scheduler::Pallet::<T>::group_assigned_to_core(core_idx, self.block_number).unwrap();

		(para_id, core_idx, group_idx)
	}

	pub(crate) fn mock_head_data() -> HeadData {
		let max_head_size = configuration::Pallet::<T>::config().max_head_data_size;
		HeadData(vec![0xFF; max_head_size as usize])
	}

	fn candidate_descriptor_mock() -> CandidateDescriptor<T::Hash> {
		CandidateDescriptor::<T::Hash> {
			para_id: 0.into(),
			relay_parent: Default::default(),
			collator: CollatorId::from(sr25519::Public::from_raw([42u8; 32])),
			persisted_validation_data_hash: Default::default(),
			pov_hash: Default::default(),
			erasure_root: Default::default(),
			signature: CollatorSignature::from(sr25519::Signature([42u8; 64])),
			para_head: Default::default(),
			validation_code_hash: mock_validation_code().hash(),
		}
	}

	/// Create a mock of `CandidatePendingAvailability`.
	fn candidate_availability_mock(
		group_idx: GroupIndex,
		core_idx: CoreIndex,
		candidate_hash: CandidateHash,
		availability_votes: BitVec<BitOrderLsb0, u8>,
	) -> inclusion::CandidatePendingAvailability<T::Hash, T::BlockNumber> {
		inclusion::CandidatePendingAvailability::<T::Hash, T::BlockNumber>::new(
			core_idx,                          // core
			candidate_hash,                    // hash
			Self::candidate_descriptor_mock(), // candidate descriptor
			availability_votes,                // availability votes
			Default::default(),                // backers
			Zero::zero(),                      // relay parent
			One::one(),                        // relay chain block this was backed in
			group_idx,                         // backing group
		)
	}

	/// Add `CandidatePendingAvailability` and `CandidateCommitments` to the relevant storage items.
	///
	/// NOTE: the default `CandidateCommitments` used does not include any data that would lead to
	/// heavy code paths in `enact_candidate`. But enact_candidates does return a weight which will
	/// get taken into account.
	pub(crate) fn add_availability(
		para_id: ParaId,
		core_idx: CoreIndex,
		group_idx: GroupIndex,
		availability_votes: BitVec<BitOrderLsb0, u8>,
		candidate_hash: CandidateHash,
	) {
		let candidate_availability = Self::candidate_availability_mock(
			group_idx,
			core_idx,
			candidate_hash,
			availability_votes,
		);
		let commitments = CandidateCommitments::<u32> {
			upward_messages: vec![],
			horizontal_messages: vec![],
			new_validation_code: None,
			head_data: Self::mock_head_data(),
			processed_downward_messages: 0,
			hrmp_watermark: 0u32.into(),
		};
		inclusion::PendingAvailability::<T>::insert(para_id, candidate_availability);
		inclusion::PendingAvailabilityCommitments::<T>::insert(&para_id, commitments);
	}

	/// Create an `AvailabilityBitfield` where `concluding` is a map where each key is a core index
	/// that is concluding and `cores` is the total number of cores in the system.
	pub(crate) fn availability_bitvec(
		concluding: &BTreeMap<u32, u32>,
		cores: u32,
	) -> AvailabilityBitfield {
		let mut bitfields = bitvec::bitvec![bitvec::order::Lsb0, u8; 0; 0];
		for i in 0..cores {
			if concluding.get(&(i as u32)).is_some() {
				bitfields.push(true);
			} else {
				bitfields.push(false)
			}
		}

		bitfields.into()
	}

	/// Register `cores` count of parachains.
	///
	/// Note that this must be called at least 2 sessions before the target session as there is a
	/// n+2 session delay for the scheduled actions to take effect.
	fn setup_para_ids(cores: u32) {
		// make sure parachains exist prior to session change.
		for i in 0..cores {
			let para_id = ParaId::from(i as u32);

			paras::Pallet::<T>::schedule_para_initialize(
				para_id,
				paras::ParaGenesisArgs {
					genesis_head: Self::mock_head_data(),
					validation_code: mock_validation_code(),
					parachain: true,
				},
			)
			.unwrap();
		}
	}

	/// Generate validator key pairs and account ids.
	fn generate_validator_pairs(validator_count: u32) -> Vec<(T::AccountId, ValidatorId)> {
		(0..validator_count)
			.map(|i| {
				let public = ValidatorId::generate_pair(None);

				// The account Id is not actually used anywhere, just necessary to fulfill the
				// expected type of the `validators` param of `test_trigger_on_new_session`.
				let account: T::AccountId = account("validator", i, i);
				(account, public)
			})
			.collect()
	}

	/// Create a bitvec of `validators` length with all yes votes.
	pub(crate) fn validator_availability_votes_yes(
		validators: usize,
	) -> BitVec<bitvec::order::Lsb0, u8> {
		// every validator confirms availability.
		bitvec::bitvec![bitvec::order::Lsb0, u8; 1; validators as usize]
	}

	/// Create backed candidates for `cores_with_backed_candidates`. You need these cores to be
	/// scheduled _within_ paras inherent, which requires marking the available bitfields as fully
	/// available.
	/// - `cores_with_backed_candidates` Mapping of `para_id`/`core_idx`/`group_idx` seed to number of
	/// validity votes.
	fn create_backed_candidates(
		&self,
		cores_with_backed_candidates: &BTreeMap<u32, u32>,
		includes_code_upgrade: Option<u32>,
	) -> Vec<BackedCandidate<T::Hash>> {
		cores_with_backed_candidates
			.iter()
			.map(|(seed, num_votes)| {
				PalletRunner::<T>::create_backed_candidate(seed, num_votes, includes_code_upgrade)
			})
			.collect()
	}

	/// Fill cores `start..last` with dispute statement sets. The statement sets will have 3/4th of
	/// votes be valid, and 1/4th of votes be invalid.
	fn create_disputes_with_no_spam(
		&self,
		start: u32,
		last: u32,
		dispute_sessions: impl AsRef<[u32]>,
	) -> Vec<DisputeStatementSet> {
		let validators =
			self.validators.as_ref().expect("must have some validators prior to calling");

		let dispute_sessions = dispute_sessions.as_ref();
		(start..last)
			.map(|seed| {
				let dispute_session_idx = (seed - start) as usize;
				let dispute_session = dispute_sessions
					.get(dispute_session_idx)
					.cloned()
					.unwrap_or(self.target_session);

				let (para_id, core_idx, group_idx) = self.create_indexes(seed);
				let candidate_hash = CandidateHash(H256::from(byte32_slice_from(seed)));

				Self::add_availability(
					para_id,
					core_idx,
					group_idx,
					Self::validator_availability_votes_yes(validators.len()),
					candidate_hash,
				);

				let statements_len =
					self.dispute_statements.get(&seed).cloned().unwrap_or(validators.len() as u32);
				// For every statement we generate dispute statement set
				// TODO: investigate if changing 0 % 4 to supermajority_threshold affects anything
				let validators_for = supermajority_threshold(statements_len as usize) as u32;
				let validators_against = statements_len - validators_for;
				let statements = PalletRunner::<T>::create_dispute_statements(
					candidate_hash,
					validators_for,
					validators_against,
					dispute_session,
				);

				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: dispute_session,
					statements,
				}
			})
			.collect()
	}

	/// Build a scenario for testing or benchmarks.
	///
	/// Note that this API only allows building scenarios where the `backed_and_concluding_cores`
	/// are mutually exclusive with the cores for disputes. So
	/// `backed_and_concluding_cores.len() + dispute_sessions.len()` must be less than the max
	/// number of cores.
	pub(crate) fn build(mut self) -> Bench<T> {
		PalletRunner::<T>::init();

		// We don't allow a core to have both disputes and be marked fully available at this block.
		// We determine how many validators can be per core
		let cores = self.max_cores();
		let used_cores =
			(self.dispute_sessions.len() + self.backed_and_concluding_cores.len()) as u32;
		assert!(used_cores <= cores);

		// NOTE: there is an n+2 session delay for these actions to take effect.
		// We are currently in Session 0, so these changes will take effect in Session 2.
		Self::setup_para_ids(used_cores);

		let validator_ids = Self::generate_validator_pairs(self.max_validators());
		let target_session = SessionIndex::from(self.target_session);

		// This is former self.setup_session();
		// Setup session 1 and create `self.validators_map` and `self.validators`.
		let res =
			PalletRunner::<T>::run_to_session(target_session, validator_ids, used_cores, true);
		self.validators = Some(res.0);
		self.block_number = res.1;
		self.session = res.2;

		let bitfields = PalletRunner::<T>::create_availability_bitfields_for_session(
			PalletRunner::<T>::current_session_index(),
			&self.backed_and_concluding_cores,
			used_cores,
		);

		let backed_candidates =
			self.create_backed_candidates(&self.backed_and_concluding_cores, self.code_upgrade);

		// That's where we create disputes
		let disputes = self.create_disputes_with_no_spam(
			self.backed_and_concluding_cores.len() as u32,
			used_cores,
			self.dispute_sessions.as_slice(),
		);

		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			used_cores as usize,
		);
		assert_eq!(inclusion::PendingAvailability::<T>::iter().count(), used_cores as usize,);

		// Mark all the used cores as occupied. We expect that their are `backed_and_concluding_cores`
		// that are pending availability and that there are `used_cores - backed_and_concluding_cores `
		// which are about to be disputed.
		scheduler::AvailabilityCores::<T>::set(vec![
			Some(CoreOccupied::Parachain);
			used_cores as usize
		]);

		Bench::<T> {
			data: ParachainsInherentData {
				bitfields,
				backed_candidates,
				disputes,
				parent_header: Self::header(self.block_number.clone()),
			},
			_session: target_session,
			_block_number: self.block_number,
		}
	}
}
