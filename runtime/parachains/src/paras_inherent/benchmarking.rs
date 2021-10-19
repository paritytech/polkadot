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

use super::*;
use crate::{configuration, inclusion, initializer, paras, scheduler, session_info, shared};
use bitvec::{order::Lsb0 as BitOrderLsb0, vec::BitVec};
use frame_benchmarking::{account, benchmarks, impl_benchmark_test_suite};
use frame_system::RawOrigin;
use primitives::v1::{
	byzantine_threshold, collator_signature_payload, AvailabilityBitfield, CandidateCommitments,
	CandidateDescriptor, CandidateHash, CollatorId, CommittedCandidateReceipt, CompactStatement,
	CoreIndex, CoreOccupied, DisputeStatement, DisputeStatementSet, GroupIndex, HeadData,
	Id as ParaId, InvalidDisputeStatementKind, PersistedValidationData, SigningContext,
	UncheckedSigned, ValidDisputeStatementKind, ValidatorId, ValidatorIndex, ValidityAttestation,
};
use sp_core::H256;
use sp_runtime::{
	generic::Digest,
	traits::{One, Zero},
	RuntimeAppPublic,
};
use sp_std::convert::TryInto;

fn byte32_slice_from(n: u32) -> [u8; 32] {
	let mut slice = [0u8; 32];
	slice[31] = (n % (1 << 8)) as u8;
	slice[30] = ((n >> 8) % (1 << 8)) as u8;
	slice[29] = ((n >> 16) % (1 << 8)) as u8;
	slice[28] = ((n >> 24) % (1 << 8)) as u8;

	slice
}

// Brainstorming worst case aspects:
//
// - there are many fresh disputes, where the disputes have just been initiated.
// - create a new `DisputeState` with blank bitfields.
// - make sure spam slots is maxed out without being cleared
// - force one side to have a super majority, so we enable slashing <-- TODO
//
/// Paras inherent `enter` benchmark scenario builder.
struct BenchBuilder<T: Config> {
	validators: Option<Vec<ValidatorId>>,
	_phantom: sp_std::marker::PhantomData<T>,
}

/// Paras inherent `enter` benchmark scenario.
struct Bench<T: Config> {
	data: ParachainsInherentData<T::Header>,
}

impl<T: Config> BenchBuilder<T> {
	fn new() -> Self {
		BenchBuilder {
			validators: None,
			_phantom: sp_std::marker::PhantomData::<T>,
		}
	}

	/// Mock header for block #1.
	fn header() -> T::Header {
		T::Header::new(
			One::one(),         // number
			Default::default(), // extrinsics_root,
			Default::default(), // storage_root,
			Default::default(), // parent_hash,
			Default::default(), // digest,
		)
	}

	fn create_indexes(seed: u32) -> (ParaId, CoreIndex, GroupIndex) {
		let para_id = ParaId::from(seed);
		let core_idx = CoreIndex(seed);
		let group_idx =
			scheduler::Pallet::<T>::group_assigned_to_core(core_idx, 2u32.into()).unwrap();

		(para_id, core_idx, group_idx)
	}

	fn candidate_availability_mock(
		group_idx: GroupIndex,
		core_idx: CoreIndex,
		candidate_hash: CandidateHash,
		availability_votes: BitVec<BitOrderLsb0, u8>,
	) -> inclusion::CandidatePendingAvailability<T::Hash, T::BlockNumber> {
		inclusion::CandidatePendingAvailability::<T::Hash, T::BlockNumber>::new(
			core_idx,
			candidate_hash,
			Default::default(),
			availability_votes,
			Default::default(),
			Zero::zero(),
			One::one(),
			group_idx,
		)
	}

	fn add_availability(
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
		// TODO notes: commitments does not include any data that would lead to heavy code
		// paths in `enact_candidate`. But enact_candidates does return a weight so maybe
		// that should be used. (Relevant for when bitfields indicate a candidate is available)
		let commitments = CandidateCommitments::<u32>::default();
		inclusion::PendingAvailability::<T>::insert(para_id, candidate_availability);
		inclusion::PendingAvailabilityCommitments::<T>::insert(&para_id, commitments);
	}

	fn availability_bitvec(concluding: Vec<u32>) -> AvailabilityBitfield {
		let mut bitfields = bitvec::bitvec![bitvec::order::Lsb0, u8; 0; 0];
		for i in 0..Self::cores() {
			// the first `availability` cores are marked as available
			if concluding.contains(&(i as u32)) {
				bitfields.push(true);
			} else {
				bitfields.push(false)
			}

			scheduler::AvailabilityCores::<T>::mutate(|cores| {
				cores[i as usize] = Some(CoreOccupied::Parachain)
			});
		}

		bitfields.into()
	}

	fn run_to_block(to: u32) {
		let to = to.into();
		while frame_system::Pallet::<T>::block_number() < to {
			let b = frame_system::Pallet::<T>::block_number();
			initializer::Pallet::<T>::on_finalize(b);

			let b = b + One::one();
			frame_system::Pallet::<T>::set_block_number(b);
			initializer::Pallet::<T>::on_initialize(b);
		}
	}

	/// Insert para ids into `paras::Parachains`.
	fn setup_para_ids(cores: u32) {
		// make sure parachains exist prior to session change.
		for i in 0..cores {
			let para_id = ParaId::from(i as u32);
			paras::Pallet::<T>::parachains_append(para_id);
		}
	}

	/// Generate validator key pairs and account ids.
	fn generate_validator_pairs(validator_count: u32) -> Vec<(T::AccountId, ValidatorId)> {
		(0..validator_count)
			.map(|i| {
				let public = ValidatorId::generate_pair(None);

				// this account is not actually used anywhere, just necessary to fulfill expected type
				// `validators` param of `test_trigger_on_new_session`.
				let account: T::AccountId = account("validator", i, i);
				(account, public)
			})
			.collect()
	}

	fn signing_context() -> SigningContext<T::Hash> {
		SigningContext { parent_hash: Self::header().hash(), session_index: 1 }
	}

	fn max_validators() -> u32 {
		let config_max = configuration::Pallet::<T>::config().max_validators.unwrap_or(200);
		config_max
	}

	fn max_validators_per_core() -> u32 {
		configuration::Pallet::<T>::config().max_validators_per_core.unwrap_or(5)
	}

	fn cores() -> u32 {
		Self::max_validators() / Self::max_validators_per_core()
	}

	fn max_statements() -> u32 {
		Self::max_validators()
	}

	/// Byzantine statement spam threshold.
	fn statement_spam_thresh() -> u32 {
		byzantine_threshold(Self::max_statements() as usize) as u32
	}

	fn validator_availability_votes_yes() -> BitVec<bitvec::order::Lsb0, u8> {
		// every validator confirms availability.
		bitvec::bitvec![bitvec::order::Lsb0, u8; 1; Self::max_validators() as usize]
	}

	/// Setup session 1 and create `self.validators_map` and `self.validators`.
	fn setup_session_1(mut self, validators: Vec<(T::AccountId, ValidatorId)>) -> Self {
		// initialize session 1.
		initializer::Pallet::<T>::test_trigger_on_new_session(
			true, // indicate the validator set has changed
			1,    // session index
			validators.clone().iter().map(|(a, v)| (a, v.clone())), // validators
			None, // queued - when this is None validators are considered queued
		);

		Self::run_to_block(2);

		// we use this because we want to make sure `frame_system::ParentHash` is set. The storage
		// item itself is private.
		frame_system::Pallet::<T>::initialize(
			&2u32.into(),
			&Self::header().hash(),
			&Digest::<T::Hash> { logs: Vec::new() },
			Default::default(),
		);

		// confirm setup at session change.
		// assert_eq!(scheduler::AvailabilityCores::<T>::get().len(), Self::cores() as usize);
		assert_eq!(scheduler::ValidatorGroups::<T>::get().len(), Self::cores() as usize);
		assert_eq!(<shared::Pallet<T>>::session_index(), 1);

				log::info!(target: LOG_TARGET, "b");

		// get validators from session info. We need to refetch them since they have been shuffled.
		let validators_shuffled: Vec<_> =
			session_info::Pallet::<T>::session_info(1)
				.unwrap()
				.validators
				.clone()
				.into_iter()
				.enumerate()
				.map(|(val_idx, public)| {
					// TODO we don't actually need to map here anymore, can just to a for loop to
					// sanity check things.
					{
						// sanity check that the validator keys line up as expected.
						let active_val_keys = shared::Pallet::<T>::active_validator_keys();
						let public_check = active_val_keys.get(val_idx).unwrap();
						assert_eq!(public, *public_check);
					}

					public
				})
				.collect();

		self.validators = Some(validators_shuffled);

		self
	}

	fn create_fully_available_and_new_backed(
		&self,
		first: u32,
		last: u32,
	) -> (Vec<BackedCandidate<T::Hash>>, Vec<UncheckedSigned<AvailabilityBitfield>>) {
		let validators =
			self.validators.as_ref().expect("must have some validators prior to calling");
		let config = configuration::Pallet::<T>::config();

		let backed_rng = first..last;

		let concluding_cores: Vec<_> = backed_rng.clone().map(|i| i as u32).collect();
		let availability_bitvec = Self::availability_bitvec(concluding_cores);

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

		log::info!(target: LOG_TARGET, "c");

		for seed in backed_rng.clone() {
			// make sure the candidates that are concluding by becoming available are marked as
			// pending availability.
			let (para_id, core_idx, group_idx) = Self::create_indexes(seed);
			Self::add_availability(
				para_id,
				core_idx,
				group_idx,
				Self::validator_availability_votes_yes(),
				CandidateHash(H256::from(byte32_slice_from(seed))),
			);

			scheduler::AvailabilityCores::<T>::mutate(|cores| {
				cores[seed as usize] = Some(CoreOccupied::Parachain)
			});
		}

		log::info!(target: LOG_TARGET, "d");


		let backed_candidates: Vec<BackedCandidate<T::Hash>> = backed_rng
			.clone()
			.map(|seed| {
				let (para_id, _core_idx, group_idx) = Self::create_indexes(seed);

				// generate a pair and add it to the keystore.
				let collator_public = CollatorId::generate_pair(None);

				let relay_parent = Self::header().hash();
				let head_data: HeadData = Default::default();
				let persisted_validation_data_hash = PersistedValidationData::<H256> {
					parent_head: head_data.clone(), // dummy parent_head
					relay_parent_number: (*Self::header().number())
						.try_into()
						.map_err(|_| ())
						.expect("header.number is valid"),
					relay_parent_storage_root: Default::default(), // equivalent to header.storage_root,
					max_pov_size: config.max_pov_size,
				}
				.hash();

				let pov_hash = Default::default();
				let validation_code_hash = Default::default();
				let payload = collator_signature_payload(
					&relay_parent,
					&para_id,
					&persisted_validation_data_hash,
					&pov_hash,
					&validation_code_hash,
				);
				let signature = collator_public.sign(&payload).unwrap();

				// set the head data so it can be used while validating the signatures on the candidate
				// receipt.
				paras::Pallet::<T>::heads_insert(&para_id, head_data.clone());

				let mut past_code_meta = paras::ParaPastCodeMeta::<T::BlockNumber>::default();
				past_code_meta.note_replacement(0u32.into(), 0u32.into());

				// TODO explain
				paras::Pallet::<T>::past_code_meta_insert(&para_id, past_code_meta);
				paras::Pallet::<T>::current_code_hash_insert(
					&para_id,
					validation_code_hash.clone(),
				);

				let group_validators = scheduler::Pallet::<T>::group_validators(group_idx).unwrap();

				let candidate = CommittedCandidateReceipt::<T::Hash> {
					descriptor: CandidateDescriptor::<T::Hash> {
						para_id,
						relay_parent,
						// collator: collator_pair.public(),
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
						new_validation_code: None,
						head_data, // HeadData
						processed_downward_messages: 0,
						hrmp_watermark: 1u32,
					},
				};

				let candidate_hash = candidate.hash();

				let validity_votes: Vec<_> = group_validators
					.iter()
					.map(|val_idx| {
						let public = validators.get(val_idx.0 as usize).unwrap();

						// let pair = validator_map.get(public).unwrap();
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

				BackedCandidate::<T::Hash> {
					candidate,
					validity_votes,
					validator_indices: bitvec::bitvec![bitvec::order::Lsb0, u8; 1; group_validators.len()],
				}
			})
			.collect();

			log::info!(target: LOG_TARGET, "e");

		(backed_candidates, bitfields)
	}

	fn create_disputes_with_some_spam(&self, start: u32, last: u32) -> Vec<DisputeStatementSet> {
		let validators =
			self.validators.as_ref().expect("must have some validators prior to calling");
		let config = configuration::Pallet::<T>::config();


		log::info!(target: LOG_TARGET, "f");
		log::info!(target: LOG_TARGET, "disputes with spam start {}, last {}", start, last);

		let mut spam_count = 0;
		(start..last)
			.map(|seed| {
				// fill corresponding storage items for inclusion that will be `taken` when `collect_disputed`
				// is called.
				let (para_id, core_idx, group_idx) = Self::create_indexes(seed);
				let candidate_hash = CandidateHash(H256::from(byte32_slice_from(seed)));

				Self::add_availability(
					para_id,
					core_idx,
					group_idx,
					Self::validator_availability_votes_yes(), // TODO
					candidate_hash,
				);

				// create the set of statements to dispute the above candidate hash.
				let statement_range = if spam_count < config.dispute_max_spam_slots {
					// if we have not hit the spam dispute statement limit, only make up to the byzantine
					// threshold number of statements.

					// TODO: we could max the amount of spam even more by  taking 3 1/3 chunks of
					// validator set and having them each attest to different statements. Right now we
					// just use 1 1/3 chunk.
					0..Self::statement_spam_thresh()
				} else {
					// otherwise, make the maximum number of statements, which is over the byzantine
					// threshold and thus these statements will not be counted as potential spam.
					0..Self::max_statements()
				};
				log::info!(target: LOG_TARGET, "g");

				let statements = statement_range
					.map(|validator_index| {
						let validator_public = &validators.get(validator_index as usize).unwrap();

						// we need dispute statements on each side.
						let dispute_statement = if validator_index % 2 == 0 {
							DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit)
						} else {
							DisputeStatement::Valid(ValidDisputeStatementKind::Explicit)
						};
						let data = dispute_statement
							.payload_data(candidate_hash.clone(), 1);

						// let debug_res = validator_pair.public().sign(&data);
						// println!("debug res validator sign {}", debug_res);
						let statement_sig = validator_public.sign(&data).unwrap();

						(dispute_statement, ValidatorIndex(validator_index), statement_sig)
					})
					.collect();

				if spam_count < config.dispute_max_spam_slots {
					spam_count += 1;
				}

				// return dispute statements with metadata.
				DisputeStatementSet {
					candidate_hash: candidate_hash.clone(),
					session: 1,
					statements,
				}
			})
			.collect()
	}

	fn build(self, backed_and_concluding: u32, disputed: u32) -> Bench<T> {
		// make sure relevant storage is cleared. TODO this is just to get the asserts to work when
		// running tests because it seems the storage is not cleared in between.
		inclusion::PendingAvailabilityCommitments::<T>::remove_all(None);
		inclusion::PendingAvailability::<T>::remove_all(None);

		Self::setup_para_ids(Self::cores());

		let validator_ids = Self::generate_validator_pairs(Self::max_validators());
		let builder = self.setup_session_1(validator_ids);

		let (backed_candidates, bitfields) =
			builder.create_fully_available_and_new_backed(0, backed_and_concluding);

		let last_disputed = backed_and_concluding + disputed;
		assert!(last_disputed <= Self::cores());
		let disputes = builder.create_disputes_with_some_spam(backed_and_concluding, last_disputed);
		log::info!(target: LOG_TARGET, "i");

		// spam slots are empty prior.
		// TODO
		// assert_eq!(disputes::Pallet::<T>::spam_slots(&builder.current_session), None);
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			(disputed + backed_and_concluding) as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			(disputed + backed_and_concluding) as usize
		);

		log::info!(target: LOG_TARGET, "k");


		Bench::<T> {
			data: ParachainsInherentData {
				bitfields,
				backed_candidates,
				disputes, // Vec<DisputeStatementSet>
				parent_header: Self::header(),
			},
		}
	}
}

// Variant over `d`, the number of cores with a disputed candidate. Remainder of cores are concluding
// and backed candidates.
benchmarks! {
	enter_dispute_dominant {
		let d in 0..BenchBuilder::<T>::cores();

		let backed_and_concluding = BenchBuilder::<T>::cores() - d;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
			.build(backed_and_concluding, d);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// check that the disputes storage has updated as expected.

		// TODO
		// if d > 1 {
		// 	let spam_slots = disputes::Pallet::<T>::spam_slots(&scenario.current_session).unwrap();
		// 	assert!(
		// 		// we expect the first 1/3rd of validators to have maxed out spam slots. Sub 1 for when
		// 		// there is an odd number of validators.
		// 		// TODO
		// 		&spam_slots[..(BenchBuilder::<T>::statement_spam_thresh() - 1) as usize]
		// 		.iter()
		// 		.all(|n| *n == config.dispute_max_spam_slots)
		// 	);
		// 	assert!(
		// 		&spam_slots[BenchBuilder::<T>::statement_spam_thresh() as usize ..]
		// 		.iter()
		// 		.all(|n| *n == 0)
		// 	);
		// }

		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			backed_and_concluding as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			backed_and_concluding as usize
		);

		// max possible number of cores have been scheduled.
		assert_eq!(scheduler::Scheduled::<T>::get().len(), d as usize);

		// all cores are occupied by a parachain.
		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}

	enter_disputes_only {
		let d in 0..BenchBuilder::<T>::cores();

		log::info!(target: LOG_TARGET, "a");
		let backed_and_concluding = 0;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
			.build(backed_and_concluding, d);
		log::info!(target: LOG_TARGET, "x");

	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		log::warn!(target: LOG_TARGET, "y");
		// check that the disputes storage has updated as expected.

		// TODO
		// if d > 1 {
		// 	let spam_slots = disputes::Pallet::<T>::spam_slots(&scenario.current_session).unwrap();
		// 	assert!(
		// 		// we expect the first 1/3rd of validators to have maxed out spam slots. Sub 1 for when
		// 		// there is an odd number of validators.
		// 		// TODO
		// 		&spam_slots[..(BenchBuilder::<T>::statement_spam_thresh() - 1) as usize]
		// 		.iter()
		// 		.all(|n| *n == config.dispute_max_spam_slots)
		// 	);
		// 	assert!(
		// 		&spam_slots[BenchBuilder::<T>::statement_spam_thresh() as usize ..]
		// 		.iter()
		// 		.all(|n| *n == 0)
		// 	);
		// }


		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			backed_and_concluding as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			backed_and_concluding as usize
		);

		// max possible number of cores have been scheduled.
		assert_eq!(scheduler::Scheduled::<T>::get().len(), d as usize);

		// all cores are occupied by a parachain.
		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}

	// Variant of over `b`, the number of cores concluding and immediately receiving a new
	// backed candidate. Remainder of cores are occupied by disputes.
	enter_backed_dominant {
		let b in 0..BenchBuilder::<T>::cores();

		let disputed = BenchBuilder::<T>::cores() - b;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
			.build(b, disputed);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			b as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			b as usize
		);

		// exactly the disputed cores are schedule since they where freed.
		assert_eq!(scheduler::Scheduled::<T>::get().len(), disputed as usize);

		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}

	enter_backed_only {
		let b in 0..BenchBuilder::<T>::cores();

		let disputed = 0;

		let config = configuration::Pallet::<T>::config();
		let scenario = BenchBuilder::<T>::new()
			.build(b, disputed);
	}: enter(RawOrigin::None, scenario.data.clone())
	verify {
		// pending availability data is removed when disputes are collected.
		assert_eq!(
			inclusion::PendingAvailabilityCommitments::<T>::iter().count(),
			b as usize
		);
		assert_eq!(
			inclusion::PendingAvailability::<T>::iter().count(),
			b as usize
		);

		// exactly the disputed cores are schedule since they where freed.
		assert_eq!(scheduler::Scheduled::<T>::get().len(), disputed as usize);

		assert_eq!(
			scheduler::AvailabilityCores::<T>::get().len(), BenchBuilder::<T>::cores() as usize
		);
	}
}

// - no spam scenario
// - max backed candidates scenario

impl_benchmark_test_suite!(
	Pallet,
	crate::mock::new_test_ext(Default::default()),
	crate::mock::Test
);
