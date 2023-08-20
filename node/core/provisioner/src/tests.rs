// Copyright (C) Parity Technologies (UK) Ltd.
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
use ::test_helpers::{dummy_candidate_descriptor, dummy_hash};
use bitvec::bitvec;
use polkadot_primitives::{OccupiedCore, ScheduledCore};

const MOCK_GROUP_SIZE: usize = 5;

pub fn occupied_core(para_id: u32) -> CoreState {
	CoreState::Occupied(OccupiedCore {
		group_responsible: para_id.into(),
		next_up_on_available: None,
		occupied_since: 100_u32,
		time_out_at: 200_u32,
		next_up_on_time_out: None,
		availability: bitvec![u8, bitvec::order::Lsb0; 0; 32],
		candidate_descriptor: dummy_candidate_descriptor(dummy_hash()),
		candidate_hash: Default::default(),
	})
}

pub fn build_occupied_core<Builder>(para_id: u32, builder: Builder) -> CoreState
where
	Builder: FnOnce(&mut OccupiedCore),
{
	let mut core = match occupied_core(para_id) {
		CoreState::Occupied(core) => core,
		_ => unreachable!(),
	};

	builder(&mut core);

	CoreState::Occupied(core)
}

pub fn default_bitvec(size: usize) -> CoreAvailability {
	bitvec![u8, bitvec::order::Lsb0; 0; size]
}

pub fn scheduled_core(id: u32) -> ScheduledCore {
	ScheduledCore { para_id: id.into(), collator: None }
}

mod select_availability_bitfields {
	use super::{super::*, default_bitvec, occupied_core};
	use polkadot_primitives::{ScheduledCore, SigningContext, ValidatorId, ValidatorIndex};
	use sp_application_crypto::AppCrypto;
	use sp_keystore::{testing::MemoryKeystore, Keystore, KeystorePtr};
	use std::sync::Arc;

	fn signed_bitfield(
		keystore: &KeystorePtr,
		field: CoreAvailability,
		validator_idx: ValidatorIndex,
	) -> SignedAvailabilityBitfield {
		let public = Keystore::sr25519_generate_new(&**keystore, ValidatorId::ID, None)
			.expect("generated sr25519 key");
		SignedAvailabilityBitfield::sign(
			&keystore,
			field.into(),
			&<SigningContext<Hash>>::default(),
			validator_idx,
			&public.into(),
		)
		.ok()
		.flatten()
		.expect("Should be signed")
	}

	#[test]
	fn not_more_than_one_per_validator() {
		let keystore: KeystorePtr = Arc::new(MemoryKeystore::new());
		let mut bitvec = default_bitvec(2);
		bitvec.set(0, true);
		bitvec.set(1, true);

		let cores = vec![occupied_core(0), occupied_core(1)];

		// we pass in three bitfields with two validators
		// this helps us check the postcondition that we get two bitfields back, for which the
		// validators differ
		let bitfields = vec![
			signed_bitfield(&keystore, bitvec.clone(), ValidatorIndex(0)),
			signed_bitfield(&keystore, bitvec.clone(), ValidatorIndex(1)),
			signed_bitfield(&keystore, bitvec, ValidatorIndex(1)),
		];

		let mut selected_bitfields =
			select_availability_bitfields(&cores, &bitfields, &Hash::repeat_byte(0));
		selected_bitfields.sort_by_key(|bitfield| bitfield.validator_index());

		assert_eq!(selected_bitfields.len(), 2);
		assert_eq!(selected_bitfields[0], bitfields[0]);
		// we don't know which of the (otherwise equal) bitfields will be selected
		assert!(selected_bitfields[1] == bitfields[1] || selected_bitfields[1] == bitfields[2]);
	}

	#[test]
	fn each_corresponds_to_an_occupied_core() {
		let keystore: KeystorePtr = Arc::new(MemoryKeystore::new());
		let bitvec = default_bitvec(3);

		// invalid: bit on free core
		let mut bitvec0 = bitvec.clone();
		bitvec0.set(0, true);

		// invalid: bit on scheduled core
		let mut bitvec1 = bitvec.clone();
		bitvec1.set(1, true);

		// valid: bit on occupied core.
		let mut bitvec2 = bitvec.clone();
		bitvec2.set(2, true);

		let cores = vec![
			CoreState::Free,
			CoreState::Scheduled(ScheduledCore { para_id: Default::default(), collator: None }),
			occupied_core(2),
		];

		let bitfields = vec![
			signed_bitfield(&keystore, bitvec0, ValidatorIndex(0)),
			signed_bitfield(&keystore, bitvec1, ValidatorIndex(1)),
			signed_bitfield(&keystore, bitvec2.clone(), ValidatorIndex(2)),
		];

		let selected_bitfields =
			select_availability_bitfields(&cores, &bitfields, &Hash::repeat_byte(0));

		// selects only the valid bitfield
		assert_eq!(selected_bitfields.len(), 1);
		assert_eq!(selected_bitfields[0].payload().0, bitvec2);
	}

	#[test]
	fn more_set_bits_win_conflicts() {
		let keystore: KeystorePtr = Arc::new(MemoryKeystore::new());
		let mut bitvec = default_bitvec(2);
		bitvec.set(0, true);

		let mut bitvec1 = bitvec.clone();
		bitvec1.set(1, true);

		let cores = vec![occupied_core(0), occupied_core(1)];

		let bitfields = vec![
			signed_bitfield(&keystore, bitvec, ValidatorIndex(1)),
			signed_bitfield(&keystore, bitvec1.clone(), ValidatorIndex(1)),
		];

		let selected_bitfields =
			select_availability_bitfields(&cores, &bitfields, &Hash::repeat_byte(0));
		assert_eq!(selected_bitfields.len(), 1);
		assert_eq!(selected_bitfields[0].payload().0, bitvec1.clone());
	}

	#[test]
	fn more_complex_bitfields() {
		let keystore: KeystorePtr = Arc::new(MemoryKeystore::new());

		let cores = vec![occupied_core(0), occupied_core(1), occupied_core(2), occupied_core(3)];

		let mut bitvec0 = default_bitvec(4);
		bitvec0.set(0, true);
		bitvec0.set(2, true);

		let mut bitvec1 = default_bitvec(4);
		bitvec1.set(1, true);

		let mut bitvec2 = default_bitvec(4);
		bitvec2.set(2, true);

		let mut bitvec3 = default_bitvec(4);
		bitvec3.set(0, true);
		bitvec3.set(1, true);
		bitvec3.set(2, true);
		bitvec3.set(3, true);

		// these are out of order but will be selected in order. The better
		// bitfield for 3 will be selected.
		let bitfields = vec![
			signed_bitfield(&keystore, bitvec2.clone(), ValidatorIndex(3)),
			signed_bitfield(&keystore, bitvec3.clone(), ValidatorIndex(3)),
			signed_bitfield(&keystore, bitvec0.clone(), ValidatorIndex(0)),
			signed_bitfield(&keystore, bitvec2.clone(), ValidatorIndex(2)),
			signed_bitfield(&keystore, bitvec1.clone(), ValidatorIndex(1)),
		];

		let selected_bitfields =
			select_availability_bitfields(&cores, &bitfields, &Hash::repeat_byte(0));
		assert_eq!(selected_bitfields.len(), 4);
		assert_eq!(selected_bitfields[0].payload().0, bitvec0);
		assert_eq!(selected_bitfields[1].payload().0, bitvec1);
		assert_eq!(selected_bitfields[2].payload().0, bitvec2);
		assert_eq!(selected_bitfields[3].payload().0, bitvec3);
	}
}

pub(crate) mod common {
	use super::super::*;
	use futures::channel::mpsc;
	use polkadot_node_subsystem::messages::AllMessages;
	use polkadot_node_subsystem_test_helpers::TestSubsystemSender;

	pub fn test_harness<OverseerFactory, Overseer, TestFactory, Test>(
		overseer_factory: OverseerFactory,
		test_factory: TestFactory,
	) where
		OverseerFactory: FnOnce(mpsc::UnboundedReceiver<AllMessages>) -> Overseer,
		Overseer: Future<Output = ()>,
		TestFactory: FnOnce(TestSubsystemSender) -> Test,
		Test: Future<Output = ()>,
	{
		let (tx, rx) = polkadot_node_subsystem_test_helpers::sender_receiver();
		let overseer = overseer_factory(rx);
		let test = test_factory(tx);

		futures::pin_mut!(overseer, test);

		let _ = futures::executor::block_on(future::join(overseer, test));
	}
}

mod select_candidates {
	use super::{
		super::*, build_occupied_core, common::test_harness, default_bitvec, occupied_core,
		scheduled_core, MOCK_GROUP_SIZE,
	};
	use ::test_helpers::{dummy_candidate_descriptor, dummy_hash};
	use futures::channel::mpsc;
	use polkadot_node_subsystem::messages::{
		AllMessages, RuntimeApiMessage,
		RuntimeApiRequest::{
			AvailabilityCores, PersistedValidationData as PersistedValidationDataReq,
		},
	};
	use polkadot_node_subsystem_test_helpers::TestSubsystemSender;
	use polkadot_node_subsystem_util::runtime::ProspectiveParachainsMode;
	use polkadot_primitives::{
		BlockNumber, CandidateCommitments, CommittedCandidateReceipt, PersistedValidationData,
	};

	const BLOCK_UNDER_PRODUCTION: BlockNumber = 128;

	// For test purposes, we always return this set of availability cores:
	//
	//   [
	//      0: Free,
	//      1: Scheduled(default),
	//      2: Occupied(no next_up set),
	//      3: Occupied(next_up_on_available set but not available),
	//      4: Occupied(next_up_on_available set and available),
	//      5: Occupied(next_up_on_time_out set but not timeout),
	//      6: Occupied(next_up_on_time_out set and timeout but available),
	//      7: Occupied(next_up_on_time_out set and timeout and not available),
	//      8: Occupied(both next_up set, available),
	//      9: Occupied(both next_up set, not available, no timeout),
	//     10: Occupied(both next_up set, not available, timeout),
	//     11: Occupied(next_up_on_available and available, but different successor para_id)
	//   ]
	fn mock_availability_cores() -> Vec<CoreState> {
		use std::ops::Not;
		use CoreState::{Free, Scheduled};

		vec![
			// 0: Free,
			Free,
			// 1: Scheduled(default),
			Scheduled(scheduled_core(1)),
			// 2: Occupied(no next_up set),
			occupied_core(2),
			// 3: Occupied(next_up_on_available set but not available),
			build_occupied_core(3, |core| {
				core.next_up_on_available = Some(scheduled_core(3));
			}),
			// 4: Occupied(next_up_on_available set and available),
			build_occupied_core(4, |core| {
				core.next_up_on_available = Some(scheduled_core(4));
				core.availability = core.availability.clone().not();
			}),
			// 5: Occupied(next_up_on_time_out set but not timeout),
			build_occupied_core(5, |core| {
				core.next_up_on_time_out = Some(scheduled_core(5));
			}),
			// 6: Occupied(next_up_on_time_out set and timeout but available),
			build_occupied_core(6, |core| {
				core.next_up_on_time_out = Some(scheduled_core(6));
				core.time_out_at = BLOCK_UNDER_PRODUCTION;
				core.availability = core.availability.clone().not();
			}),
			// 7: Occupied(next_up_on_time_out set and timeout and not available),
			build_occupied_core(7, |core| {
				core.next_up_on_time_out = Some(scheduled_core(7));
				core.time_out_at = BLOCK_UNDER_PRODUCTION;
			}),
			// 8: Occupied(both next_up set, available),
			build_occupied_core(8, |core| {
				core.next_up_on_available = Some(scheduled_core(8));
				core.next_up_on_time_out = Some(scheduled_core(8));
				core.availability = core.availability.clone().not();
			}),
			// 9: Occupied(both next_up set, not available, no timeout),
			build_occupied_core(9, |core| {
				core.next_up_on_available = Some(scheduled_core(9));
				core.next_up_on_time_out = Some(scheduled_core(9));
			}),
			// 10: Occupied(both next_up set, not available, timeout),
			build_occupied_core(10, |core| {
				core.next_up_on_available = Some(scheduled_core(10));
				core.next_up_on_time_out = Some(scheduled_core(10));
				core.time_out_at = BLOCK_UNDER_PRODUCTION;
			}),
			// 11: Occupied(next_up_on_available and available, but different successor para_id)
			build_occupied_core(11, |core| {
				core.next_up_on_available = Some(scheduled_core(12));
				core.availability = core.availability.clone().not();
			}),
		]
	}

	async fn mock_overseer(
		mut receiver: mpsc::UnboundedReceiver<AllMessages>,
		expected: Vec<BackedCandidate>,
		prospective_parachains_mode: ProspectiveParachainsMode,
	) {
		use ChainApiMessage::BlockNumber;
		use RuntimeApiMessage::Request;

		let mut candidates_iter = expected
			.iter()
			.map(|candidate| (candidate.hash(), candidate.descriptor().relay_parent));

		let mut backed_iter = expected.clone().into_iter();

		while let Some(from_job) = receiver.next().await {
			match from_job {
				AllMessages::ChainApi(BlockNumber(_relay_parent, tx)) =>
					tx.send(Ok(Some(BLOCK_UNDER_PRODUCTION - 1))).unwrap(),
				AllMessages::RuntimeApi(Request(
					_parent_hash,
					PersistedValidationDataReq(_para_id, _assumption, tx),
				)) => tx.send(Ok(Some(Default::default()))).unwrap(),
				AllMessages::RuntimeApi(Request(_parent_hash, AvailabilityCores(tx))) =>
					tx.send(Ok(mock_availability_cores())).unwrap(),
				AllMessages::CandidateBacking(CandidateBackingMessage::GetBackedCandidates(
					hashes,
					sender,
				)) => {
					let response: Vec<BackedCandidate> =
						backed_iter.by_ref().take(hashes.len()).collect();
					let expected_hashes: Vec<(CandidateHash, Hash)> = response
						.iter()
						.map(|candidate| (candidate.hash(), candidate.descriptor().relay_parent))
						.collect();

					assert_eq!(expected_hashes, hashes);

					let _ = sender.send(response);
				},
				AllMessages::ProspectiveParachains(
					ProspectiveParachainsMessage::GetBackableCandidate(.., tx),
				) => match prospective_parachains_mode {
					ProspectiveParachainsMode::Enabled { .. } => {
						let _ = tx.send(candidates_iter.next());
					},
					ProspectiveParachainsMode::Disabled =>
						panic!("unexpected prospective parachains request"),
				},
				_ => panic!("Unexpected message: {:?}", from_job),
			}
		}
	}

	#[test]
	fn can_succeed() {
		test_harness(
			|r| mock_overseer(r, Vec::new(), ProspectiveParachainsMode::Disabled),
			|mut tx: TestSubsystemSender| async move {
				let prospective_parachains_mode = ProspectiveParachainsMode::Disabled;
				select_candidates(
					&[],
					&[],
					&[],
					prospective_parachains_mode,
					Default::default(),
					&mut tx,
				)
				.await
				.unwrap();
			},
		)
	}

	// this tests that only the appropriate candidates get selected.
	// To accomplish this, we supply a candidate list containing one candidate per possible core;
	// the candidate selection algorithm must filter them to the appropriate set
	#[test]
	fn selects_correct_candidates() {
		let mock_cores = mock_availability_cores();

		let empty_hash = PersistedValidationData::<Hash, BlockNumber>::default().hash();

		let mut descriptor_template = dummy_candidate_descriptor(dummy_hash());
		descriptor_template.persisted_validation_data_hash = empty_hash;
		let candidate_template = CandidateReceipt {
			descriptor: descriptor_template,
			commitments_hash: CandidateCommitments::default().hash(),
		};

		let candidates: Vec<_> = std::iter::repeat(candidate_template)
			.take(mock_cores.len())
			.enumerate()
			.map(|(idx, mut candidate)| {
				candidate.descriptor.para_id = idx.into();
				candidate
			})
			.cycle()
			.take(mock_cores.len() * 3)
			.enumerate()
			.map(|(idx, mut candidate)| {
				if idx < mock_cores.len() {
					// first go-around: use candidates which should work
					candidate
				} else if idx < mock_cores.len() * 2 {
					// for the second repetition of the candidates, give them the wrong hash
					candidate.descriptor.persisted_validation_data_hash = Default::default();
					candidate
				} else {
					// third go-around: right hash, wrong para_id
					candidate.descriptor.para_id = idx.into();
					candidate
				}
			})
			.collect();

		// why those particular indices? see the comments on mock_availability_cores()
		let expected_candidates: Vec<_> =
			[1, 4, 7, 8, 10].iter().map(|&idx| candidates[idx].clone()).collect();
		let prospective_parachains_mode = ProspectiveParachainsMode::Disabled;

		let expected_backed = expected_candidates
			.iter()
			.map(|c| BackedCandidate {
				candidate: CommittedCandidateReceipt {
					descriptor: c.descriptor.clone(),
					commitments: Default::default(),
				},
				validity_votes: Vec::new(),
				validator_indices: default_bitvec(MOCK_GROUP_SIZE),
			})
			.collect();

		test_harness(
			|r| mock_overseer(r, expected_backed, prospective_parachains_mode),
			|mut tx: TestSubsystemSender| async move {
				let result = select_candidates(
					&mock_cores,
					&[],
					&candidates,
					prospective_parachains_mode,
					Default::default(),
					&mut tx,
				)
				.await
				.unwrap();

				result.into_iter().for_each(|c| {
					assert!(
						expected_candidates.iter().any(|c2| c.candidate.corresponds_to(c2)),
						"Failed to find candidate: {:?}",
						c,
					)
				});
			},
		)
	}

	#[test]
	fn selects_max_one_code_upgrade() {
		let mock_cores = mock_availability_cores();

		let empty_hash = PersistedValidationData::<Hash, BlockNumber>::default().hash();

		// why those particular indices? see the comments on mock_availability_cores()
		// the first candidate with code is included out of [1, 4, 7, 8, 10].
		let cores = [1, 4, 7, 8, 10];
		let cores_with_code = [1, 4, 8];

		let expected_cores = [1, 7, 10];

		let committed_receipts: Vec<_> = (0..mock_cores.len())
			.map(|i| {
				let mut descriptor = dummy_candidate_descriptor(dummy_hash());
				descriptor.para_id = i.into();
				descriptor.persisted_validation_data_hash = empty_hash;
				CommittedCandidateReceipt {
					descriptor,
					commitments: CandidateCommitments {
						new_validation_code: if cores_with_code.contains(&i) {
							Some(vec![].into())
						} else {
							None
						},
						..Default::default()
					},
				}
			})
			.collect();

		// Input to select_candidates
		let candidates: Vec<_> = committed_receipts.iter().map(|r| r.to_plain()).collect();
		// Build possible outputs from select_candidates
		let backed_candidates: Vec<_> = committed_receipts
			.iter()
			.map(|committed_receipt| BackedCandidate {
				candidate: committed_receipt.clone(),
				validity_votes: Vec::new(),
				validator_indices: default_bitvec(MOCK_GROUP_SIZE),
			})
			.collect();

		// First, provisioner will request backable candidates for each scheduled core.
		// Then, some of them get filtered due to new validation code rule.
		let expected_backed: Vec<_> =
			cores.iter().map(|&idx| backed_candidates[idx].clone()).collect();
		let expected_backed_filtered: Vec<_> =
			expected_cores.iter().map(|&idx| candidates[idx].clone()).collect();

		let prospective_parachains_mode = ProspectiveParachainsMode::Disabled;

		test_harness(
			|r| mock_overseer(r, expected_backed, prospective_parachains_mode),
			|mut tx: TestSubsystemSender| async move {
				let result = select_candidates(
					&mock_cores,
					&[],
					&candidates,
					prospective_parachains_mode,
					Default::default(),
					&mut tx,
				)
				.await
				.unwrap();

				assert_eq!(result.len(), 3);

				result.into_iter().for_each(|c| {
					assert!(
						expected_backed_filtered.iter().any(|c2| c.candidate.corresponds_to(c2)),
						"Failed to find candidate: {:?}",
						c,
					)
				});
			},
		)
	}

	#[test]
	fn request_from_prospective_parachains() {
		let mock_cores = mock_availability_cores();
		let empty_hash = PersistedValidationData::<Hash, BlockNumber>::default().hash();

		let mut descriptor_template = dummy_candidate_descriptor(dummy_hash());
		descriptor_template.persisted_validation_data_hash = empty_hash;
		let candidate_template = CandidateReceipt {
			descriptor: descriptor_template,
			commitments_hash: CandidateCommitments::default().hash(),
		};

		let candidates: Vec<_> = std::iter::repeat(candidate_template)
			.take(mock_cores.len())
			.enumerate()
			.map(|(idx, mut candidate)| {
				candidate.descriptor.para_id = idx.into();
				candidate
			})
			.collect();

		// why those particular indices? see the comments on mock_availability_cores()
		let expected_candidates: Vec<_> =
			[1, 4, 7, 8, 10].iter().map(|&idx| candidates[idx].clone()).collect();
		// Expect prospective parachains subsystem requests.
		let prospective_parachains_mode =
			ProspectiveParachainsMode::Enabled { max_candidate_depth: 0, allowed_ancestry_len: 0 };

		let expected_backed = expected_candidates
			.iter()
			.map(|c| BackedCandidate {
				candidate: CommittedCandidateReceipt {
					descriptor: c.descriptor.clone(),
					commitments: Default::default(),
				},
				validity_votes: Vec::new(),
				validator_indices: default_bitvec(MOCK_GROUP_SIZE),
			})
			.collect();

		test_harness(
			|r| mock_overseer(r, expected_backed, prospective_parachains_mode),
			|mut tx: TestSubsystemSender| async move {
				let result = select_candidates(
					&mock_cores,
					&[],
					&[],
					prospective_parachains_mode,
					Default::default(),
					&mut tx,
				)
				.await
				.unwrap();

				result.into_iter().for_each(|c| {
					assert!(
						expected_candidates.iter().any(|c2| c.candidate.corresponds_to(c2)),
						"Failed to find candidate: {:?}",
						c,
					)
				});
			},
		)
	}

	#[test]
	fn request_receipts_based_on_relay_parent() {
		let mock_cores = mock_availability_cores();
		let empty_hash = PersistedValidationData::<Hash, BlockNumber>::default().hash();

		let mut descriptor_template = dummy_candidate_descriptor(dummy_hash());
		descriptor_template.persisted_validation_data_hash = empty_hash;
		let candidate_template = CandidateReceipt {
			descriptor: descriptor_template,
			commitments_hash: CandidateCommitments::default().hash(),
		};

		let candidates: Vec<_> = std::iter::repeat(candidate_template)
			.take(mock_cores.len())
			.enumerate()
			.map(|(idx, mut candidate)| {
				candidate.descriptor.para_id = idx.into();
				candidate.descriptor.relay_parent = Hash::repeat_byte(idx as u8);
				candidate
			})
			.collect();

		// why those particular indices? see the comments on mock_availability_cores()
		let expected_candidates: Vec<_> =
			[1, 4, 7, 8, 10].iter().map(|&idx| candidates[idx].clone()).collect();
		// Expect prospective parachains subsystem requests.
		let prospective_parachains_mode =
			ProspectiveParachainsMode::Enabled { max_candidate_depth: 0, allowed_ancestry_len: 0 };

		let expected_backed = expected_candidates
			.iter()
			.map(|c| BackedCandidate {
				candidate: CommittedCandidateReceipt {
					descriptor: c.descriptor.clone(),
					commitments: Default::default(),
				},
				validity_votes: Vec::new(),
				validator_indices: default_bitvec(MOCK_GROUP_SIZE),
			})
			.collect();

		test_harness(
			|r| mock_overseer(r, expected_backed, prospective_parachains_mode),
			|mut tx: TestSubsystemSender| async move {
				let result = select_candidates(
					&mock_cores,
					&[],
					&[],
					prospective_parachains_mode,
					Default::default(),
					&mut tx,
				)
				.await
				.unwrap();

				result.into_iter().for_each(|c| {
					assert!(
						expected_candidates.iter().any(|c2| c.candidate.corresponds_to(c2)),
						"Failed to find candidate: {:?}",
						c,
					)
				});
			},
		)
	}
}
