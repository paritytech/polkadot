use super::*;
use bitvec::bitvec;
use polkadot_primitives::v1::{OccupiedCore, ScheduledCore};

pub fn occupied_core(para_id: u32) -> CoreState {
	CoreState::Occupied(OccupiedCore {
		para_id: para_id.into(),
		group_responsible: para_id.into(),
		next_up_on_available: None,
		occupied_since: 100_u32,
		time_out_at: 200_u32,
		next_up_on_time_out: None,
		availability: default_bitvec(),
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

pub fn default_bitvec() -> CoreAvailability {
	bitvec![bitvec::order::Lsb0, u8; 0; 32]
}

pub fn scheduled_core(id: u32) -> ScheduledCore {
	ScheduledCore {
		para_id: id.into(),
		..Default::default()
	}
}

mod select_availability_bitfields {
	use super::super::*;
	use super::{default_bitvec, occupied_core};
	use futures::executor::block_on;
	use std::sync::Arc;
	use polkadot_primitives::v1::{SigningContext, ValidatorIndex, ValidatorId};
	use sp_application_crypto::AppKey;
	use sp_keystore::{CryptoStore, SyncCryptoStorePtr};
	use sc_keystore::LocalKeystore;

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
		).await.expect("Should be signed")
	}

	#[test]
	fn not_more_than_one_per_validator() {
		// Configure filesystem-based keystore as generating keys without seed
		// would trigger the key to be generated on the filesystem.
		let keystore_path = tempfile::tempdir().expect("Creates keystore path");
		let keystore : SyncCryptoStorePtr = Arc::new(LocalKeystore::open(keystore_path.path(), None)
			.expect("Creates keystore"));
		let bitvec = default_bitvec();

		let cores = vec![occupied_core(0), occupied_core(1)];

		// we pass in three bitfields with two validators
		// this helps us check the postcondition that we get two bitfields back, for which the validators differ
		let bitfields = vec![
			block_on(signed_bitfield(&keystore, bitvec.clone(), 0)),
			block_on(signed_bitfield(&keystore, bitvec.clone(), 1)),
			block_on(signed_bitfield(&keystore, bitvec, 1)),
		];

		let mut selected_bitfields = select_availability_bitfields(&cores, &bitfields);
		selected_bitfields.sort_by_key(|bitfield| bitfield.validator_index());

		assert_eq!(selected_bitfields.len(), 2);
		assert_eq!(selected_bitfields[0], bitfields[0]);
		// we don't know which of the (otherwise equal) bitfields will be selected
		assert!(selected_bitfields[1] == bitfields[1] || selected_bitfields[1] == bitfields[2]);
	}

	#[test]
	fn each_corresponds_to_an_occupied_core() {
		// Configure filesystem-based keystore as generating keys without seed
		// would trigger the key to be generated on the filesystem.
		let keystore_path = tempfile::tempdir().expect("Creates keystore path");
		let keystore : SyncCryptoStorePtr = Arc::new(LocalKeystore::open(keystore_path.path(), None)
			.expect("Creates keystore"));
		let bitvec = default_bitvec();

		let cores = vec![CoreState::Free, CoreState::Scheduled(Default::default())];

		let bitfields = vec![
			block_on(signed_bitfield(&keystore, bitvec.clone(), 0)),
			block_on(signed_bitfield(&keystore, bitvec.clone(), 1)),
			block_on(signed_bitfield(&keystore, bitvec, 1)),
		];

		let mut selected_bitfields = select_availability_bitfields(&cores, &bitfields);
		selected_bitfields.sort_by_key(|bitfield| bitfield.validator_index());

		// bitfields not corresponding to occupied cores are not selected
		assert!(selected_bitfields.is_empty());
	}

	#[test]
	fn more_set_bits_win_conflicts() {
		// Configure filesystem-based keystore as generating keys without seed
		// would trigger the key to be generated on the filesystem.
		let keystore_path = tempfile::tempdir().expect("Creates keystore path");
		let keystore : SyncCryptoStorePtr = Arc::new(LocalKeystore::open(keystore_path.path(), None)
			.expect("Creates keystore"));
		let bitvec_zero = default_bitvec();
		let bitvec_one = {
			let mut bitvec = bitvec_zero.clone();
			bitvec.set(0, true);
			bitvec
		};

		let cores = vec![occupied_core(0)];

		let bitfields = vec![
			block_on(signed_bitfield(&keystore, bitvec_zero, 0)),
			block_on(signed_bitfield(&keystore, bitvec_one.clone(), 0)),
		];

		// this test is probablistic: chances are excellent that it does what it claims to.
		// it cannot fail unless things are broken.
		// however, there is a (very small) chance that it passes when things are broken.
		for _ in 0..64 {
			let selected_bitfields = select_availability_bitfields(&cores, &bitfields);
			assert_eq!(selected_bitfields.len(), 1);
			assert_eq!(selected_bitfields[0].payload().0, bitvec_one);
		}
	}
}

mod select_candidates {
	use futures_timer::Delay;
	use super::super::*;
	use super::{build_occupied_core, default_bitvec, occupied_core, scheduled_core};
	use polkadot_node_subsystem::messages::RuntimeApiRequest::{
		AvailabilityCores, PersistedValidationData as PersistedValidationDataReq,
	};
	use polkadot_primitives::v1::{
		BlockNumber, CandidateDescriptor, CommittedCandidateReceipt, PersistedValidationData,
	};
	use FromJob::{ChainApi, Runtime};

	const BLOCK_UNDER_PRODUCTION: BlockNumber = 128;

	fn test_harness<OverseerFactory, Overseer, TestFactory, Test>(
		overseer_factory: OverseerFactory,
		test_factory: TestFactory,
	) where
		OverseerFactory: FnOnce(mpsc::Receiver<FromJob>) -> Overseer,
		Overseer: Future<Output = ()>,
		TestFactory: FnOnce(mpsc::Sender<FromJob>) -> Test,
		Test: Future<Output = ()>,
	{
		let (tx, rx) = mpsc::channel(64);
		let overseer = overseer_factory(rx);
		let test = test_factory(tx);

		futures::pin_mut!(overseer, test);

		let _ = futures::executor::block_on(future::select(overseer, test));
	}

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

	async fn mock_overseer(mut receiver: mpsc::Receiver<FromJob>) {
		use ChainApiMessage::BlockNumber;
		use RuntimeApiMessage::Request;

		while let Some(from_job) = receiver.next().await {
			match from_job {
				ChainApi(BlockNumber(_relay_parent, tx)) => {
					tx.send(Ok(Some(BLOCK_UNDER_PRODUCTION - 1))).unwrap()
				}
				Runtime(Request(
					_parent_hash,
					PersistedValidationDataReq(_para_id, _assumption, tx),
				)) => tx.send(Ok(Some(Default::default()))).unwrap(),
				Runtime(Request(_parent_hash, AvailabilityCores(tx))) => {
					tx.send(Ok(mock_availability_cores())).unwrap()
				}
				// non-exhaustive matches are fine for testing
				_ => unimplemented!(),
			}
		}
	}

	#[test]
	fn handles_overseer_failure() {
		let overseer = |rx: mpsc::Receiver<FromJob>| async move {
			// drop the receiver so it closes and the sender can't send, then just sleep long enough that
			// this is almost certainly not the first of the two futures to complete
			std::mem::drop(rx);
			Delay::new(std::time::Duration::from_secs(1)).await;
		};

		let test = |mut tx: mpsc::Sender<FromJob>| async move {
			// wait so that the overseer can drop the rx before we attempt to send
			Delay::new(std::time::Duration::from_millis(50)).await;
			let result = select_candidates(&[], &[], &[], Default::default(), &mut tx).await;
			println!("{:?}", result);
			assert!(std::matches!(result, Err(Error::ChainApiMessageSend(_))));
		};

		test_harness(overseer, test);
	}

	#[test]
	fn can_succeed() {
		test_harness(mock_overseer, |mut tx: mpsc::Sender<FromJob>| async move {
			let result = select_candidates(&[], &[], &[], Default::default(), &mut tx).await;
			println!("{:?}", result);
			assert!(result.is_ok());
		})
	}

	// this tests that only the appropriate candidates get selected.
	// To accomplish this, we supply a candidate list containing one candidate per possible core;
	// the candidate selection algorithm must filter them to the appropriate set
	#[test]
	fn selects_correct_candidates() {
		let mock_cores = mock_availability_cores();

		let empty_hash = PersistedValidationData::<BlockNumber>::default().hash();

		let candidate_template = BackedCandidate {
			candidate: CommittedCandidateReceipt {
				descriptor: CandidateDescriptor {
					persisted_validation_data_hash: empty_hash,
					..Default::default()
				},
				..Default::default()
			},
			validity_votes: Vec::new(),
			validator_indices: default_bitvec(),
		};

		let candidates: Vec<_> = std::iter::repeat(candidate_template)
			.take(mock_cores.len())
			.enumerate()
			.map(|(idx, mut candidate)| {
				candidate.candidate.descriptor.para_id = idx.into();
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
					candidate.candidate.descriptor.persisted_validation_data_hash
						= Default::default();
					candidate
				} else {
					// third go-around: right hash, wrong para_id
					candidate.candidate.descriptor.para_id = idx.into();
					candidate
				}
			})
			.collect();

		// why those particular indices? see the comments on mock_availability_cores()
		let expected_candidates: Vec<_> = [1, 4, 7, 8, 10]
			.iter()
			.map(|&idx| candidates[idx].clone())
			.collect();

		test_harness(mock_overseer, |mut tx: mpsc::Sender<FromJob>| async move {
			let result =
				select_candidates(&mock_cores, &[], &candidates, Default::default(), &mut tx)
					.await;

			if result.is_err() {
				println!("{:?}", result);
			}
			assert_eq!(result.unwrap(), expected_candidates);
		})
	}
}
