use super::*;
use ::test_helpers::{dummy_candidate_descriptor, dummy_hash};
use bitvec::bitvec;
use polkadot_primitives::v2::{OccupiedCore, ScheduledCore};

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

pub fn default_bitvec(n_cores: usize) -> CoreAvailability {
	bitvec![u8, bitvec::order::Lsb0; 0; n_cores]
}

pub fn scheduled_core(id: u32) -> ScheduledCore {
	ScheduledCore { para_id: id.into(), collator: None }
}

mod select_availability_bitfields {
	use super::{super::*, default_bitvec, occupied_core};
	use futures::executor::block_on;
	use polkadot_primitives::v2::{ScheduledCore, SigningContext, ValidatorId, ValidatorIndex};
	use sp_application_crypto::AppKey;
	use sp_keystore::{testing::KeyStore, CryptoStore, SyncCryptoStorePtr};
	use std::sync::Arc;

	async fn signed_bitfield(
		keystore: &SyncCryptoStorePtr,
		field: CoreAvailability,
		validator_idx: ValidatorIndex,
	) -> SignedAvailabilityBitfield {
		let public = CryptoStore::sr25519_generate_new(&**keystore, ValidatorId::ID, None)
			.await
			.expect("generated sr25519 key");
		SignedAvailabilityBitfield::sign(
			&keystore,
			field.into(),
			&<SigningContext<Hash>>::default(),
			validator_idx,
			&public.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("Should be signed")
	}

	#[test]
	fn not_more_than_one_per_validator() {
		let keystore: SyncCryptoStorePtr = Arc::new(KeyStore::new());
		let mut bitvec = default_bitvec(2);
		bitvec.set(0, true);
		bitvec.set(1, true);

		let cores = vec![occupied_core(0), occupied_core(1)];

		// we pass in three bitfields with two validators
		// this helps us check the postcondition that we get two bitfields back, for which the validators differ
		let bitfields = vec![
			block_on(signed_bitfield(&keystore, bitvec.clone(), ValidatorIndex(0))),
			block_on(signed_bitfield(&keystore, bitvec.clone(), ValidatorIndex(1))),
			block_on(signed_bitfield(&keystore, bitvec, ValidatorIndex(1))),
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
		let keystore: SyncCryptoStorePtr = Arc::new(KeyStore::new());
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
			block_on(signed_bitfield(&keystore, bitvec0, ValidatorIndex(0))),
			block_on(signed_bitfield(&keystore, bitvec1, ValidatorIndex(1))),
			block_on(signed_bitfield(&keystore, bitvec2.clone(), ValidatorIndex(2))),
		];

		let selected_bitfields =
			select_availability_bitfields(&cores, &bitfields, &Hash::repeat_byte(0));

		// selects only the valid bitfield
		assert_eq!(selected_bitfields.len(), 1);
		assert_eq!(selected_bitfields[0].payload().0, bitvec2);
	}

	#[test]
	fn more_set_bits_win_conflicts() {
		let keystore: SyncCryptoStorePtr = Arc::new(KeyStore::new());
		let mut bitvec = default_bitvec(2);
		bitvec.set(0, true);

		let mut bitvec1 = bitvec.clone();
		bitvec1.set(1, true);

		let cores = vec![occupied_core(0), occupied_core(1)];

		let bitfields = vec![
			block_on(signed_bitfield(&keystore, bitvec, ValidatorIndex(1))),
			block_on(signed_bitfield(&keystore, bitvec1.clone(), ValidatorIndex(1))),
		];

		let selected_bitfields =
			select_availability_bitfields(&cores, &bitfields, &Hash::repeat_byte(0));
		assert_eq!(selected_bitfields.len(), 1);
		assert_eq!(selected_bitfields[0].payload().0, bitvec1.clone());
	}

	#[test]
	fn more_complex_bitfields() {
		let keystore: SyncCryptoStorePtr = Arc::new(KeyStore::new());

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
			block_on(signed_bitfield(&keystore, bitvec2.clone(), ValidatorIndex(3))),
			block_on(signed_bitfield(&keystore, bitvec3.clone(), ValidatorIndex(3))),
			block_on(signed_bitfield(&keystore, bitvec0.clone(), ValidatorIndex(0))),
			block_on(signed_bitfield(&keystore, bitvec2.clone(), ValidatorIndex(2))),
			block_on(signed_bitfield(&keystore, bitvec1.clone(), ValidatorIndex(1))),
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

mod common {
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
		scheduled_core,
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
	use polkadot_primitives::v2::{
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
	) {
		use ChainApiMessage::BlockNumber;
		use RuntimeApiMessage::Request;

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
					_,
					_,
					sender,
				)) => {
					let _ = sender.send(expected.clone());
				},
				_ => panic!("Unexpected message: {:?}", from_job),
			}
		}
	}

	#[test]
	fn can_succeed() {
		test_harness(
			|r| mock_overseer(r, Vec::new()),
			|mut tx: TestSubsystemSender| async move {
				select_candidates(&[], &[], &[], Default::default(), &mut tx).await.unwrap();
			},
		)
	}

	// this tests that only the appropriate candidates get selected.
	// To accomplish this, we supply a candidate list containing one candidate per possible core;
	// the candidate selection algorithm must filter them to the appropriate set
	#[test]
	fn selects_correct_candidates() {
		let mock_cores = mock_availability_cores();
		let n_cores = mock_cores.len();

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

		let expected_backed = expected_candidates
			.iter()
			.map(|c| BackedCandidate {
				candidate: CommittedCandidateReceipt {
					descriptor: c.descriptor.clone(),
					commitments: Default::default(),
				},
				validity_votes: Vec::new(),
				validator_indices: default_bitvec(n_cores),
			})
			.collect();

		test_harness(
			|r| mock_overseer(r, expected_backed),
			|mut tx: TestSubsystemSender| async move {
				let result =
					select_candidates(&mock_cores, &[], &candidates, Default::default(), &mut tx)
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
		let n_cores = mock_cores.len();

		let empty_hash = PersistedValidationData::<Hash, BlockNumber>::default().hash();

		// why those particular indices? see the comments on mock_availability_cores()
		// the first candidate with code is included out of [1, 4, 7, 8, 10].
		let cores = [1, 7, 10];
		let cores_with_code = [1, 4, 8];

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

		let candidates: Vec<_> = committed_receipts.iter().map(|r| r.to_plain()).collect();

		let expected_candidates: Vec<_> =
			cores.iter().map(|&idx| candidates[idx].clone()).collect();

		let expected_backed: Vec<_> = cores
			.iter()
			.map(|&idx| BackedCandidate {
				candidate: committed_receipts[idx].clone(),
				validity_votes: Vec::new(),
				validator_indices: default_bitvec(n_cores),
			})
			.collect();

		test_harness(
			|r| mock_overseer(r, expected_backed),
			|mut tx: TestSubsystemSender| async move {
				let result =
					select_candidates(&mock_cores, &[], &candidates, Default::default(), &mut tx)
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

mod select_disputes {
	use super::{super::*, common::test_harness};
	use futures::channel::mpsc;
	use polkadot_node_subsystem::{
		messages::{AllMessages, DisputeCoordinatorMessage, RuntimeApiMessage, RuntimeApiRequest},
		RuntimeApiError,
	};
	use polkadot_node_subsystem_test_helpers::TestSubsystemSender;
	use polkadot_primitives::v2::DisputeState;
	use std::sync::Arc;
	use test_helpers;

	// Global Test Data
	fn recent_disputes(len: usize) -> Vec<(SessionIndex, CandidateHash)> {
		let mut res = Vec::with_capacity(len);
		for _ in 0..len {
			res.push((0, CandidateHash(Hash::random())));
		}

		res
	}

	// same as recent_disputes() but with SessionIndex set to 1
	fn active_disputes(len: usize) -> Vec<(SessionIndex, CandidateHash)> {
		let mut res = Vec::with_capacity(len);
		for _ in 0..len {
			res.push((1, CandidateHash(Hash::random())));
		}

		res
	}

	fn leaf() -> ActivatedLeaf {
		ActivatedLeaf {
			hash: Hash::repeat_byte(0xAA),
			number: 0xAA,
			status: LeafStatus::Fresh,
			span: Arc::new(jaeger::Span::Disabled),
		}
	}

	async fn mock_overseer(
		leaf: ActivatedLeaf,
		mut receiver: mpsc::UnboundedReceiver<AllMessages>,
		onchain_disputes: Result<Vec<(SessionIndex, CandidateHash, DisputeState)>, RuntimeApiError>,
		recent_disputes: Vec<(SessionIndex, CandidateHash)>,
		active_disputes: Vec<(SessionIndex, CandidateHash)>,
	) {
		while let Some(from_job) = receiver.next().await {
			match from_job {
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_,
					RuntimeApiRequest::StagingDisputes(sender),
				)) => {
					let _ = sender.send(onchain_disputes.clone());
				},
				AllMessages::RuntimeApi(_) => panic!("Unexpected RuntimeApi request"),
				AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::RecentDisputes(
					sender,
				)) => {
					let _ = sender.send(recent_disputes.clone());
				},
				AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::ActiveDisputes(
					sender,
				)) => {
					let _ = sender.send(active_disputes.clone());
				},
				AllMessages::DisputeCoordinator(
					DisputeCoordinatorMessage::QueryCandidateVotes(disputes, sender),
				) => {
					let mut res = Vec::new();
					let v = CandidateVotes {
						candidate_receipt: test_helpers::dummy_candidate_receipt(leaf.hash.clone()),
						valid: vec![],
						invalid: vec![],
					};
					for r in disputes.iter() {
						res.push((r.0, r.1, v.clone()));
					}

					let _ = sender.send(res);
				},
				_ => panic!("Unexpected message: {:?}", from_job),
			}
		}
	}

	#[test]
	fn recent_disputes_are_withing_onchain_limit() {
		const RECENT_DISPUTES_SIZE: usize = 10;
		let metrics = metrics::Metrics::new_dummy();
		let onchain_disputes = Ok(Vec::new());
		let active_disputes = Vec::new();
		let recent_disputes = recent_disputes(RECENT_DISPUTES_SIZE);

		let recent_disputes_overseer = recent_disputes.clone();
		test_harness(
			|r| {
				mock_overseer(
					leaf(),
					r,
					onchain_disputes,
					recent_disputes_overseer,
					active_disputes,
				)
			},
			|mut tx: TestSubsystemSender| async move {
				let lf = leaf();
				let disputes = select_disputes(&mut tx, &metrics, &lf).await.unwrap();

				assert!(!disputes.is_empty());

				let result = disputes.iter().zip(recent_disputes.iter());
				// We should get all recent disputes.
				for (d, r) in result {
					assert_eq!(d.session, r.0);
					assert_eq!(d.candidate_hash, r.1);
				}
			},
		)
	}

	#[test]
	fn recent_disputes_are_too_much_but_active_are_within_limit() {
		const RECENT_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME + 10;
		const ACTIVE_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME;
		let metrics = metrics::Metrics::new_dummy();
		let onchain_disputes = Ok(Vec::new());
		let recent_disputes = recent_disputes(RECENT_DISPUTES_SIZE);
		let active_disputes = active_disputes(ACTIVE_DISPUTES_SIZE);

		let active_disputes_overseer = active_disputes.clone();
		test_harness(
			|r| {
				mock_overseer(
					leaf(),
					r,
					onchain_disputes,
					recent_disputes,
					active_disputes_overseer,
				)
			},
			|mut tx: TestSubsystemSender| async move {
				let lf = leaf();
				let disputes = select_disputes(&mut tx, &metrics, &lf).await.unwrap();

				assert!(!disputes.is_empty());

				let result = disputes.iter().zip(active_disputes.iter());
				// We should get all active disputes.
				for (d, r) in result {
					assert_eq!(d.session, r.0);
					assert_eq!(d.candidate_hash, r.1);
				}
			},
		)
	}

	#[test]
	fn recent_disputes_are_too_much_but_active_are_less_than_the_limit() {
		// In this case all active disputes + a random set of recent disputes should be returned
		const RECENT_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME + 10;
		const ACTIVE_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME - 10;
		let metrics = metrics::Metrics::new_dummy();
		let onchain_disputes = Ok(Vec::new());
		let recent_disputes = recent_disputes(RECENT_DISPUTES_SIZE);
		let active_disputes = active_disputes(ACTIVE_DISPUTES_SIZE);

		let active_disputes_overseer = active_disputes.clone();
		test_harness(
			|r| {
				mock_overseer(
					leaf(),
					r,
					onchain_disputes,
					recent_disputes,
					active_disputes_overseer,
				)
			},
			|mut tx: TestSubsystemSender| async move {
				let lf = leaf();
				let disputes = select_disputes(&mut tx, &metrics, &lf).await.unwrap();

				assert!(!disputes.is_empty());

				// Recent disputes are generated with `SessionIndex` = 0
				let (res_recent, res_active): (Vec<DisputeStatementSet>, Vec<DisputeStatementSet>) =
					disputes.into_iter().partition(|d| d.session == 0);

				// It should be good enough the count the number of active disputes and not compare them one by one. Checking the exact values is already covered by the previous tests.
				assert_eq!(res_active.len(), active_disputes.len()); // We have got all active disputes
				assert_eq!(res_active.len() + res_recent.len(), MAX_DISPUTES_FORWARDED_TO_RUNTIME);
				// And some recent ones.
			},
		)
	}

	//These tests rely on staging Runtime functions so they are separated and compiled conditionally.
	#[cfg(feature = "staging-client")]
	mod staging_tests {
		use super::*;

		fn dummy_dispute_state() -> DisputeState {
			DisputeState {
				validators_for: BitVec::new(),
				validators_against: BitVec::new(),
				start: 0,
				concluded_at: None,
			}
		}

		#[test]
		fn recent_disputes_are_too_much_active_fits_test_recent_prioritisation() {
			// In this case recent disputes are above `MAX_DISPUTES_FORWARDED_TO_RUNTIME` limit and the active ones are below it.
			// The expected behaviour is to send all active disputes and extend the set with recent ones. During the extension the disputes unknown for the Runtime are added with priority.
			const RECENT_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME + 10;
			const ACTIVE_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME - 10;
			const ONCHAIN_DISPUTE_SIZE: usize = RECENT_DISPUTES_SIZE - 9;
			let metrics = metrics::Metrics::new_dummy();
			let recent_disputes = recent_disputes(RECENT_DISPUTES_SIZE);
			let active_disputes = active_disputes(ACTIVE_DISPUTES_SIZE);
			let onchain_disputes: Result<
				Vec<(SessionIndex, CandidateHash, DisputeState)>,
				RuntimeApiError,
			> = Ok(Vec::from(&recent_disputes[0..ONCHAIN_DISPUTE_SIZE])
				.iter()
				.map(|(session_index, candidate_hash)| {
					(*session_index, candidate_hash.clone(), dummy_dispute_state())
				})
				.collect());
			let active_disputes_overseer = active_disputes.clone();
			let recent_disputes_overseer = recent_disputes.clone();
			test_harness(
				|r| {
					mock_overseer(
						leaf(),
						r,
						onchain_disputes,
						recent_disputes_overseer,
						active_disputes_overseer,
					)
				},
				|mut tx: TestSubsystemSender| async move {
					let lf = leaf();
					let disputes = select_disputes(&mut tx, &metrics, &lf).await.unwrap();

					assert!(!disputes.is_empty());

					// Recent disputes are generated with `SessionIndex` = 0
					let (res_recent, res_active): (
						Vec<DisputeStatementSet>,
						Vec<DisputeStatementSet>,
					) = disputes.into_iter().partition(|d| d.session == 0);

					// It should be good enough the count the number of the disputes and not compare them one by one as this was already covered in other tests.
					assert_eq!(res_active.len(), active_disputes.len()); // We've got all active disputes.
					assert_eq!(
						res_recent.len(),
						MAX_DISPUTES_FORWARDED_TO_RUNTIME - active_disputes.len()
					); // And some recent ones.

					// Check if the recent disputes were unknown for the Runtime.
					let expected_recent_disputes =
						Vec::from(&recent_disputes[ONCHAIN_DISPUTE_SIZE..]);
					let res_recent_set: HashSet<(SessionIndex, CandidateHash)> = HashSet::from_iter(
						res_recent.iter().map(|d| (d.session, d.candidate_hash)),
					);

					// Explicitly check that all unseen disputes are sent to the Runtime.
					for d in &expected_recent_disputes {
						assert_eq!(res_recent_set.contains(d), true);
					}
				},
			)
		}

		#[test]
		fn active_disputes_are_too_much_test_active_prioritisation() {
			// In this case the active disputes are above the `MAX_DISPUTES_FORWARDED_TO_RUNTIME` limit so the unseen ones should be prioritised.
			const RECENT_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME + 10;
			const ACTIVE_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME + 10;
			const ONCHAIN_DISPUTE_SIZE: usize = ACTIVE_DISPUTES_SIZE - 9;

			let metrics = metrics::Metrics::new_dummy();
			let recent_disputes = recent_disputes(RECENT_DISPUTES_SIZE);
			let active_disputes = active_disputes(ACTIVE_DISPUTES_SIZE);
			let onchain_disputes: Result<
				Vec<(SessionIndex, CandidateHash, DisputeState)>,
				RuntimeApiError,
			> = Ok(Vec::from(&active_disputes[0..ONCHAIN_DISPUTE_SIZE])
				.iter()
				.map(|(session_index, candidate_hash)| {
					(*session_index, candidate_hash.clone(), dummy_dispute_state())
				})
				.collect());
			let active_disputes_overseer = active_disputes.clone();
			let recent_disputes_overseer = recent_disputes.clone();
			test_harness(
				|r| {
					mock_overseer(
						leaf(),
						r,
						onchain_disputes,
						recent_disputes_overseer,
						active_disputes_overseer,
					)
				},
				|mut tx: TestSubsystemSender| async move {
					let lf = leaf();
					let disputes = select_disputes(&mut tx, &metrics, &lf).await.unwrap();

					assert!(!disputes.is_empty());

					// Recent disputes are generated with `SessionIndex` = 0
					let (res_recent, res_active): (
						Vec<DisputeStatementSet>,
						Vec<DisputeStatementSet>,
					) = disputes.into_iter().partition(|d| d.session == 0);

					// It should be good enough the count the number of the disputes and not compare them one by one
					assert_eq!(res_recent.len(), 0); // We expect no recent disputes
					assert_eq!(res_active.len(), MAX_DISPUTES_FORWARDED_TO_RUNTIME);

					let expected_active_disputes =
						Vec::from(&active_disputes[ONCHAIN_DISPUTE_SIZE..]);
					let res_active_set: HashSet<(SessionIndex, CandidateHash)> = HashSet::from_iter(
						res_active.iter().map(|d| (d.session, d.candidate_hash)),
					);

					// Explicitly check that the unseen disputes are delivered to the Runtime.
					for d in &expected_active_disputes {
						assert_eq!(res_active_set.contains(d), true);
					}
				},
			)
		}

		#[test]
		fn active_disputes_are_too_much_and_are_all_unseen() {
			// In this case there are a lot of active disputes unseen by the Runtime. The focus of the test is to verify that in such cases known disputes are NOT sent to the Runtime.
			const RECENT_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME + 10;
			const ACTIVE_DISPUTES_SIZE: usize = MAX_DISPUTES_FORWARDED_TO_RUNTIME + 5;
			const ONCHAIN_DISPUTE_SIZE: usize = 5;

			let metrics = metrics::Metrics::new_dummy();
			let recent_disputes = recent_disputes(RECENT_DISPUTES_SIZE);
			let active_disputes = active_disputes(ACTIVE_DISPUTES_SIZE);
			let onchain_disputes: Result<
				Vec<(SessionIndex, CandidateHash, DisputeState)>,
				RuntimeApiError,
			> = Ok(Vec::from(&active_disputes[0..ONCHAIN_DISPUTE_SIZE])
				.iter()
				.map(|(session_index, candidate_hash)| {
					(*session_index, candidate_hash.clone(), dummy_dispute_state())
				})
				.collect());
			let active_disputes_overseer = active_disputes.clone();
			let recent_disputes_overseer = recent_disputes.clone();
			test_harness(
				|r| {
					mock_overseer(
						leaf(),
						r,
						onchain_disputes,
						recent_disputes_overseer,
						active_disputes_overseer,
					)
				},
				|mut tx: TestSubsystemSender| async move {
					let lf = leaf();
					let disputes = select_disputes(&mut tx, &metrics, &lf).await.unwrap();
					assert!(!disputes.is_empty());

					// Recent disputes are generated with `SessionIndex` = 0
					let (res_recent, res_active): (
						Vec<DisputeStatementSet>,
						Vec<DisputeStatementSet>,
					) = disputes.into_iter().partition(|d| d.session == 0);

					// It should be good enough the count the number of the disputes and not compare them one by one
					assert_eq!(res_recent.len(), 0);
					assert_eq!(res_active.len(), MAX_DISPUTES_FORWARDED_TO_RUNTIME);

					// For sure we don't want to see any of this disputes in the result
					let unexpected_active_disputes =
						Vec::from(&active_disputes[0..ONCHAIN_DISPUTE_SIZE]);
					let res_active_set: HashSet<(SessionIndex, CandidateHash)> = HashSet::from_iter(
						res_active.iter().map(|d| (d.session, d.candidate_hash)),
					);

					// Verify that the result DOESN'T contain known disputes (because there is an excessive number of unknown onces).
					for d in &unexpected_active_disputes {
						assert_eq!(res_active_set.contains(d), false);
					}
				},
			)
		}
	}
}
