use super::*;
use crate::{builder::BenchBuilder, initializer, paras, paras_inherent, session_info};
use frame_support::pallet_prelude::*;
use primitives::{
	v0::ValidatorSignature,
	v1::{
		collator_signature_payload, supermajority_threshold, AvailabilityBitfield, BackedCandidate,
		CandidateCommitments, CandidateDescriptor, CandidateHash, CollatorId,
		CommittedCandidateReceipt, CompactStatement, CoreIndex, DisputeStatement,
		DisputeStatementSet, GroupIndex, Id as ParaId, InvalidDisputeStatementKind,
		PersistedValidationData, SessionIndex, SigningContext, UncheckedSigned,
		ValidDisputeStatementKind, ValidationCode, ValidatorId, ValidatorIndex,
		ValidityAttestation,
	},
};
use sp_core::H256;
use sp_runtime::{
	generic::Digest,
	traits::{Header as HeaderT, One, TrailingZeroInput},
	RuntimeAppPublic,
};
use sp_std::{collections::btree_map::BTreeMap, convert::TryInto, prelude::Vec, vec};

/// Create a 32 byte slice based on the given number.
fn byte32_slice_from(n: u32) -> [u8; 32] {
	let mut slice = [0u8; 32];
	slice[31] = (n % (1 << 8)) as u8;
	slice[30] = ((n >> 8) % (1 << 8)) as u8;
	slice[29] = ((n >> 16) % (1 << 8)) as u8;
	slice[28] = ((n >> 24) % (1 << 8)) as u8;

	slice
}

fn mock_validation_code() -> ValidationCode {
	ValidationCode(vec![1, 2, 3])
}

/// To use in tests only
#[cfg(any(feature = "runtime-benchmarks", test))]
pub struct PalletRunner<T> {
	pallet_config: PhantomData<T>,
}

/// Note: initializer config aggregates a lot of different pallets configs, as well
/// as `frame_system::Config`
#[cfg(test)]
impl<C: initializer::pallet::Config + paras_inherent::pallet::Config> PalletRunner<C> {
	pub fn init() {
		// Make sure relevant storage is cleared. This is just to get the asserts to work when
		// running tests because it seems the storage is not cleared in between.
		inclusion::PendingAvailabilityCommitments::<C>::remove_all(None);
		inclusion::PendingAvailability::<C>::remove_all(None);
	}

	/// Grab an account, seeded by a name and index.
	///
	/// This is directly from frame-benchmarking. Copy/pasted so we can use it when not compiling with
	/// "features = runtime-benchmarks".
	fn create_account_id<AccountId: Decode>(
		name: &'static str,
		index: u32,
		seed: u32,
	) -> AccountId {
		let entropy = (name, index, seed).using_encoded(sp_io::hashing::blake2_256);
		AccountId::decode(&mut TrailingZeroInput::new(&entropy[..]))
			.expect("infinite input; no invalid input; qed")
	}

	/// This is used to trigger new session; Account id isn't used anywhere in the new session
	/// hook, so dummy value can be used
	fn create_dummy_account_id() -> C::AccountId {
		Self::create_account_id("validator", 0, 0)
	}

	/// Same as `BenchBuilder::setup_session`, but based off the current block
	/// to make it possible to trigger multiple sessions during the test
	/// Skipping inherent hooks is needed for the bench builder to work.
	/// TODO: need to explore how not skipping those hooks would affect benchmarks
	pub fn run_to_session(
		target_session: SessionIndex,
		validators: Vec<(C::AccountId, ValidatorId)>,
		total_cores: u32,
		skip_inherent_hooks: bool,
	) -> (Vec<ValidatorId>, C::BlockNumber, SessionIndex) {
		let current_session_index = Self::current_session_index();
		for session_index in current_session_index..=target_session {
			// Triggers new session in the pallet - this is not implemented in the mock,
			// so needs to be called manually here
			initializer::Pallet::<C>::test_trigger_on_new_session(
				false,
				session_index,
				validators.iter().map(|(a, v)| (a, v.clone())),
				None,
			);
			Self::run_to_next_block(skip_inherent_hooks);
		}

		let block_number = Self::current_block_number();
		let header = BenchBuilder::<C>::header(block_number.clone());

		// Starts the execution of a particular block
		frame_system::Pallet::<C>::initialize(
			&header.number(),
			&header.hash(),
			&Digest { logs: Vec::new() },
		);

		assert_eq!(<shared::Pallet<C>>::session_index(), target_session);

		// We need to refetch validators since they have been shuffled.
		let validators_shuffled: Vec<_> = session_info::Pallet::<C>::session_info(target_session)
			.unwrap()
			.validators
			.clone();

		// self.validators = Some(validators_shuffled);
		// self.block_number = block_number;
		// self.session = target_session;
		assert_eq!(paras::Pallet::<C>::parachains().len(), total_cores as usize);
		(validators_shuffled, block_number, target_session)
	}

	/// Triggers `initializer::on_new_session`
	pub fn trigger_new_session() {
		// First argument doesn't do anything at the time of writing
		let new_session_index = Self::current_session_index() + 1;
		let validators: Vec<(C::AccountId, ValidatorId)> =
			Self::active_validators_for_session(Self::current_session_index())
				.iter()
				.map(|v| (Self::create_dummy_account_id(), v.clone()))
				.collect();
		initializer::Pallet::<C>::test_trigger_on_new_session(
			true,
			new_session_index,
			validators.iter().map(|(a, v)| (a, v.clone())),
			None,
		);
	}

	pub fn active_validators_for_session(session_index: SessionIndex) -> Vec<ValidatorId> {
		session_info::Pallet::<C>::session_info(session_index)
			.unwrap()
			.validators
			.clone()
	}

	/// Calls finalization hooks for the current block and
	/// initialization hooks for the next block.
	/// Skipping inherent hooks is needed for the bench builder to work.
	///  TODO: need to explore how not skipping those hooks would affect benchmarks
	pub fn run_to_next_block(skip_inherent_hooks: bool) {
		// Finalizing current block
		// NOTE: this has to be specifically the initializer, no frame_system,
		// otherwise initializer doesn't get triggered
		initializer::Pallet::<C>::on_finalize(Self::current_block_number());

		// skipping inherent hooks is needed for the bench builder to work.
		// need to explore how not skipping those hooks would affect benchmarks
		if !skip_inherent_hooks {
			// Finalizing inherent
			paras_inherent::Pallet::<C>::on_finalize(Self::current_block_number());
		}

		let next_block_number = Self::current_block_number() + One::one();
		frame_system::Pallet::<C>::set_block_number(next_block_number.clone());
		// Initializing next block
		initializer::Pallet::<C>::on_initialize(next_block_number.clone());

		if !skip_inherent_hooks {
			// Initializing inherent
			paras_inherent::Pallet::<C>::on_initialize(next_block_number.clone());
		}
	}

	/// Number of the relay parent block.
	fn relay_parent_number() -> u32 {
		(Self::current_block_number() - One::one())
			.try_into()
			.map_err(|_| ())
			.expect("self.block_number is u32")
	}

	pub fn create_backed_candidate(
		seed: &u32,
		num_votes: &u32,
		includes_code_upgrade: Option<u32>,
	) -> BackedCandidate<C::Hash> {
		let validators = Self::active_validators_for_session(Self::current_session_index());
		let config = configuration::Pallet::<C>::config();
		assert!(*num_votes <= validators.len() as u32);
		let (para_id, _core_idx, group_idx) = Self::create_indexes(seed.clone());

		// This generates a pair and adds it to the keystore, returning just the public.
		let collator_public = CollatorId::generate_pair(None);
		let relay_parent = <frame_system::Pallet<C>>::parent_hash();
		let head_data = BenchBuilder::<C>::mock_head_data();
		let persisted_validation_data_hash = PersistedValidationData::<H256> {
			parent_head: head_data.clone(),
			relay_parent_number: Self::relay_parent_number(),
			relay_parent_storage_root: Default::default(),
			max_pov_size: config.max_pov_size,
		}
		.hash();

		let pov_hash = Default::default();
		let validation_code_hash = mock_validation_code().hash();
		let payload = collator_signature_payload(
			&relay_parent,
			&para_id,
			&persisted_validation_data_hash,
			&pov_hash,
			&validation_code_hash,
		);
		let signature = collator_public.sign(&payload).unwrap();

		// Set the head data so it can be used while validating the signatures on the
		// candidate receipt.
		paras::Pallet::<C>::heads_insert(&para_id, head_data.clone());

		let mut past_code_meta = paras::ParaPastCodeMeta::<C::BlockNumber>::default();
		past_code_meta.note_replacement(0u32.into(), 0u32.into());

		let group_validators = scheduler::Pallet::<C>::group_validators(group_idx).unwrap();

		let candidate = CommittedCandidateReceipt::<C::Hash> {
			descriptor: CandidateDescriptor::<C::Hash> {
				para_id,
				relay_parent,
				collator: collator_public,
				persisted_validation_data_hash,
				pov_hash,
				erasure_root: Default::default(),
				signature,
				para_head: head_data.hash(),
				validation_code_hash,
			},
			commitments: CandidateCommitments::<u32> {
				upward_messages: Vec::new(),
				horizontal_messages: Vec::new(),
				new_validation_code: includes_code_upgrade
					.map(|v| ValidationCode(vec![42u8; v as usize])),
				head_data,
				processed_downward_messages: 0,
				hrmp_watermark: Self::relay_parent_number(),
			},
		};

		let candidate_hash = candidate.hash();

		let validity_votes: Vec<_> = group_validators
			.iter()
			.take(*num_votes as usize)
			.map(|val_idx| {
				let public = validators.get(val_idx.0 as usize).unwrap();
				let sig = UncheckedSigned::<CompactStatement>::benchmark_sign(
					public,
					CompactStatement::Valid(candidate_hash.clone()),
					&Self::signing_context(),
					*val_idx,
				)
				.benchmark_signature();

				ValidityAttestation::Explicit(sig.clone())
			})
			.collect();

		BackedCandidate::<C::Hash> {
			candidate,
			validity_votes,
			validator_indices: bitvec::bitvec![bitvec::order::Lsb0, u8; 1; group_validators.len()],
		}
	}

	/// Creates and signs disputes statements for the given candidate hash
	pub fn create_dispute_statements(
		candidate_hash: CandidateHash,
		validators_for: u32,
		validators_against: u32,
		dispute_session: u32,
	) -> Vec<(DisputeStatement, ValidatorIndex, ValidatorSignature)> {
		let statements_for = (0..validators_for).map(|validator_index| {
			Self::sign_dispute_statement(
				validator_index,
				candidate_hash,
				DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
				dispute_session,
			)
		});

		let statements_against =
			(validators_for..validators_for + validators_against).map(|validator_index| {
				Self::sign_dispute_statement(
					validator_index,
					candidate_hash,
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					dispute_session,
				)
			});

		statements_for.chain(statements_against).collect()
	}

	/// Signs the validator dispute statement for the validator_index in the active validators
	/// set for the current session
	fn sign_dispute_statement(
		validator_index: u32,
		candidate_hash: CandidateHash,
		dispute_statement: DisputeStatement,
		dispute_session: u32,
	) -> (DisputeStatement, ValidatorIndex, ValidatorSignature) {
		let validators = Self::active_validators_for_session(Self::current_session_index());
		let validator_public = &validators.get(validator_index as usize).unwrap();
		let data = dispute_statement.payload_data(candidate_hash.clone(), dispute_session);
		let statement_sig = validator_public.sign(&data).unwrap();

		(dispute_statement, ValidatorIndex(validator_index), statement_sig)
	}

	fn signing_context() -> SigningContext<C::Hash> {
		let relay_parent = <frame_system::Pallet<C>>::parent_hash();

		SigningContext { parent_hash: relay_parent, session_index: Self::current_session_index() }
	}

	/// Create a `UncheckedSigned<AvailabilityBitfield> for each validator where each core in
	/// `concluding_cores` is fully available. Additionally set up storage such that each
	/// `concluding_cores`is pending becoming fully available so the generated bitfields will be
	///  to the cores successfully being freed from the candidates being marked as available.
	pub fn create_availability_bitfields_for_session(
		session_index: SessionIndex,
		concluding_cores: &BTreeMap<u32, u32>,
		total_cores: u32,
	) -> Vec<UncheckedSigned<AvailabilityBitfield>> {
		let validators = Self::active_validators_for_session(session_index);

		let availability_bitvec =
			BenchBuilder::<C>::availability_bitvec(concluding_cores, total_cores);

		let bitfields: Vec<UncheckedSigned<AvailabilityBitfield>> = validators
			.iter()
			.enumerate()
			.map(|(i, public)| {
				let unchecked_signed = UncheckedSigned::<AvailabilityBitfield>::benchmark_sign(
					public,
					availability_bitvec.clone(),
					&Self::signing_context(),
					ValidatorIndex(i as u32),
				);

				unchecked_signed
			})
			.collect();

		for (seed, _) in concluding_cores.iter() {
			// make sure the candidates that will be concluding are marked as pending availability.
			let (para_id, core_idx, group_idx) = Self::create_indexes(seed.clone());
			BenchBuilder::<C>::add_availability(
				para_id,
				core_idx,
				group_idx,
				BenchBuilder::<C>::validator_availability_votes_yes(validators.len()),
				CandidateHash(H256::from(byte32_slice_from(seed.clone()))),
			);
		}

		bitfields
	}

	/// Create para id, core index, and grab the associated group index from the scheduler pallet.
	fn create_indexes(seed: u32) -> (ParaId, CoreIndex, GroupIndex) {
		let para_id = ParaId::from(seed);
		let core_idx = CoreIndex(seed);

		let group_idx =
			scheduler::Pallet::<C>::group_assigned_to_core(core_idx, Self::current_block_number())
				.unwrap();

		(para_id, core_idx, group_idx)
	}

	/// Creates a header for the current on top of the current chain
	pub fn create_parent_header() -> C::Header {
		let parent_height = Self::current_block_number() - One::one();

		BenchBuilder::<C>::header(parent_height)
	}

	pub fn create_dispute_against_block(block_hash: CandidateHash) -> DisputeStatementSet {
		let validators = Self::active_validators_for_session(Self::current_session_index());
		let statements_len = validators.len() as u32;

		// Supermajority votes against the block
		let validators_against = supermajority_threshold(statements_len as usize) as u32;
		let validators_for = statements_len - validators_against;

		let statements = Self::create_dispute_statements(
			block_hash.clone(),
			validators_for,
			validators_against,
			Self::current_session_index(),
		);

		DisputeStatementSet {
			candidate_hash: block_hash.clone(),
			session: Self::current_session_index(),
			statements,
		}
	}

	pub fn create_unresolved_dispute(block_hash: CandidateHash) -> DisputeStatementSet {
		let validators_against = 1;
		let validators_for = 1;

		let statements = Self::create_dispute_statements(
			block_hash,
			validators_for,
			validators_against,
			Self::current_session_index(),
		);

		DisputeStatementSet {
			candidate_hash: block_hash,
			session: Self::current_session_index(),
			statements,
		}
	}

	/// Returns the current block number
	pub fn current_block_number() -> C::BlockNumber {
		frame_system::Pallet::<C>::block_number()
	}

	pub fn current_session_index() -> SessionIndex {
		<shared::Pallet<C>>::session_index()
	}

	pub fn assert_last_events(events: Vec<<C as frame_system::Config>::Event>) {
		let has_enough_events = frame_system::Pallet::<C>::events().len() >= events.len();
		assert!(has_enough_events);
		let all_events: Vec<<C as frame_system::Config>::Event> =
			frame_system::Pallet::<C>::events().iter().map(|e| e.event.clone()).collect();
		let last_events = &all_events[all_events.len() - events.len()..];
		assert_eq!(last_events, events.as_slice());
	}

	pub fn candidate_hash_from_seed(seed: u32) -> CandidateHash {
		CandidateHash(H256::from(byte32_slice_from(seed)))
	}
}
