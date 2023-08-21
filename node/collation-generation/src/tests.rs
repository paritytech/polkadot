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
use assert_matches::assert_matches;
use futures::{
	lock::Mutex,
	task::{Context as FuturesContext, Poll},
	Future,
};
use polkadot_node_primitives::{BlockData, Collation, CollationResult, MaybeCompressedPoV, PoV};
use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest},
};
use polkadot_node_subsystem_test_helpers::{subsystem_test_harness, TestSubsystemContextHandle};
use polkadot_node_subsystem_util::TimeoutExt;
use polkadot_primitives::{
	CollatorPair, HeadData, Id as ParaId, PersistedValidationData, ScheduledCore, ValidationCode,
};
use sp_keyring::sr25519::Keyring as Sr25519Keyring;
use std::pin::Pin;
use test_helpers::{dummy_hash, dummy_head_data, dummy_validator};

type VirtualOverseer = TestSubsystemContextHandle<CollationGenerationMessage>;

fn test_harness<T: Future<Output = VirtualOverseer>>(test: impl FnOnce(VirtualOverseer) -> T) {
	let pool = sp_core::testing::TaskExecutor::new();
	let (context, virtual_overseer) =
		polkadot_node_subsystem_test_helpers::make_subsystem_context(pool);
	let subsystem = async move {
		let subsystem = crate::CollationGenerationSubsystem::new(Metrics::default());

		subsystem.run(context).await;
	};

	let test_fut = test(virtual_overseer);

	futures::pin_mut!(test_fut);
	futures::executor::block_on(futures::future::join(
		async move {
			let mut virtual_overseer = test_fut.await;
			// Ensure we have handled all responses.
			if let Ok(Some(msg)) = virtual_overseer.rx.try_next() {
				panic!("Did not handle all responses: {:?}", msg);
			}
			// Conclude.
			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
		},
		subsystem,
	));
}

fn test_collation() -> Collation {
	Collation {
		upward_messages: Default::default(),
		horizontal_messages: Default::default(),
		new_validation_code: None,
		head_data: dummy_head_data(),
		proof_of_validity: MaybeCompressedPoV::Raw(PoV { block_data: BlockData(Vec::new()) }),
		processed_downward_messages: 0_u32,
		hrmp_watermark: 0_u32.into(),
	}
}

fn test_collation_compressed() -> Collation {
	let mut collation = test_collation();
	let compressed = collation.proof_of_validity.clone().into_compressed();
	collation.proof_of_validity = MaybeCompressedPoV::Compressed(compressed);
	collation
}

fn test_validation_data() -> PersistedValidationData {
	let mut persisted_validation_data = PersistedValidationData::default();
	persisted_validation_data.max_pov_size = 1024;
	persisted_validation_data
}

// Box<dyn Future<Output = Collation> + Unpin + Send
struct TestCollator;

impl Future for TestCollator {
	type Output = Option<CollationResult>;

	fn poll(self: Pin<&mut Self>, _cx: &mut FuturesContext) -> Poll<Self::Output> {
		Poll::Ready(Some(CollationResult { collation: test_collation(), result_sender: None }))
	}
}

impl Unpin for TestCollator {}

async fn overseer_recv(overseer: &mut VirtualOverseer) -> AllMessages {
	const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(2000);

	overseer
		.recv()
		.timeout(TIMEOUT)
		.await
		.expect(&format!("{:?} is long enough to receive messages", TIMEOUT))
}

fn test_config<Id: Into<ParaId>>(para_id: Id) -> CollationGenerationConfig {
	CollationGenerationConfig {
		key: CollatorPair::generate().0,
		collator: Some(Box::new(|_: Hash, _vd: &PersistedValidationData| TestCollator.boxed())),
		para_id: para_id.into(),
	}
}

fn test_config_no_collator<Id: Into<ParaId>>(para_id: Id) -> CollationGenerationConfig {
	CollationGenerationConfig {
		key: CollatorPair::generate().0,
		collator: None,
		para_id: para_id.into(),
	}
}

fn scheduled_core_for<Id: Into<ParaId>>(para_id: Id) -> ScheduledCore {
	ScheduledCore { para_id: para_id.into(), collator: None }
}

#[test]
fn requests_availability_per_relay_parent() {
	let activated_hashes: Vec<Hash> =
		vec![[1; 32].into(), [4; 32].into(), [9; 32].into(), [16; 32].into()];

	let requested_availability_cores = Arc::new(Mutex::new(Vec::new()));

	let overseer_requested_availability_cores = requested_availability_cores.clone();
	let overseer = |mut handle: TestSubsystemContextHandle<CollationGenerationMessage>| async move {
		loop {
			match handle.try_recv().await {
				None => break,
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(hash, RuntimeApiRequest::AvailabilityCores(tx)))) => {
					overseer_requested_availability_cores.lock().await.push(hash);
					tx.send(Ok(vec![])).unwrap();
				}
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(_hash, RuntimeApiRequest::Validators(tx)))) => {
					tx.send(Ok(vec![dummy_validator(); 3])).unwrap();
				}
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::StagingAsyncBackingParams(
						tx,
					),
				))) => {
					tx.send(Err(RuntimeApiError::NotSupported { runtime_api_name: "doesnt_matter" })).unwrap();
				},
				Some(msg) => panic!("didn't expect any other overseer requests given no availability cores; got {:?}", msg),
			}
		}
	};

	let subsystem_activated_hashes = activated_hashes.clone();
	subsystem_test_harness(overseer, |mut ctx| async move {
		handle_new_activations(
			Arc::new(test_config(123u32)),
			subsystem_activated_hashes,
			&mut ctx,
			Metrics(None),
		)
		.await
		.unwrap();
	});

	let mut requested_availability_cores = Arc::try_unwrap(requested_availability_cores)
		.expect("overseer should have shut down by now")
		.into_inner();
	requested_availability_cores.sort();

	assert_eq!(requested_availability_cores, activated_hashes);
}

#[test]
fn requests_validation_data_for_scheduled_matches() {
	let activated_hashes: Vec<Hash> = vec![
		Hash::repeat_byte(1),
		Hash::repeat_byte(4),
		Hash::repeat_byte(9),
		Hash::repeat_byte(16),
	];

	let requested_validation_data = Arc::new(Mutex::new(Vec::new()));

	let overseer_requested_validation_data = requested_validation_data.clone();
	let overseer = |mut handle: TestSubsystemContextHandle<CollationGenerationMessage>| async move {
		loop {
			match handle.try_recv().await {
				None => break,
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					hash,
					RuntimeApiRequest::AvailabilityCores(tx),
				))) => {
					tx.send(Ok(vec![
						CoreState::Free,
						// this is weird, see explanation below
						CoreState::Scheduled(scheduled_core_for(
							(hash.as_fixed_bytes()[0] * 4) as u32,
						)),
						CoreState::Scheduled(scheduled_core_for(
							(hash.as_fixed_bytes()[0] * 5) as u32,
						)),
					]))
					.unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					hash,
					RuntimeApiRequest::PersistedValidationData(
						_para_id,
						_occupied_core_assumption,
						tx,
					),
				))) => {
					overseer_requested_validation_data.lock().await.push(hash);
					tx.send(Ok(None)).unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::Validators(tx),
				))) => {
					tx.send(Ok(vec![dummy_validator(); 3])).unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::StagingAsyncBackingParams(tx),
				))) => {
					tx.send(Err(RuntimeApiError::NotSupported {
						runtime_api_name: "doesnt_matter",
					}))
					.unwrap();
				},
				Some(msg) => {
					panic!("didn't expect any other overseer requests; got {:?}", msg)
				},
			}
		}
	};

	subsystem_test_harness(overseer, |mut ctx| async move {
		handle_new_activations(
			Arc::new(test_config(16)),
			activated_hashes,
			&mut ctx,
			Metrics(None),
		)
		.await
		.unwrap();
	});

	let requested_validation_data = Arc::try_unwrap(requested_validation_data)
		.expect("overseer should have shut down by now")
		.into_inner();

	// the only activated hash should be from the 4 hash:
	// each activated hash generates two scheduled cores: one with its value * 4, one with its value
	// * 5 given that the test configuration has a `para_id` of 16, there's only one way to get that
	// value: with the 4 hash.
	assert_eq!(requested_validation_data, vec![[4; 32].into()]);
}

#[test]
fn sends_distribute_collation_message() {
	let activated_hashes: Vec<Hash> = vec![
		Hash::repeat_byte(1),
		Hash::repeat_byte(4),
		Hash::repeat_byte(9),
		Hash::repeat_byte(16),
	];

	// empty vec doesn't allocate on the heap, so it's ok we throw it away
	let to_collator_protocol = Arc::new(Mutex::new(Vec::new()));
	let inner_to_collator_protocol = to_collator_protocol.clone();

	let overseer = |mut handle: TestSubsystemContextHandle<CollationGenerationMessage>| async move {
		loop {
			match handle.try_recv().await {
				None => break,
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					hash,
					RuntimeApiRequest::AvailabilityCores(tx),
				))) => {
					tx.send(Ok(vec![
						CoreState::Free,
						// this is weird, see explanation below
						CoreState::Scheduled(scheduled_core_for(
							(hash.as_fixed_bytes()[0] * 4) as u32,
						)),
						CoreState::Scheduled(scheduled_core_for(
							(hash.as_fixed_bytes()[0] * 5) as u32,
						)),
					]))
					.unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::PersistedValidationData(
						_para_id,
						_occupied_core_assumption,
						tx,
					),
				))) => {
					tx.send(Ok(Some(test_validation_data()))).unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::Validators(tx),
				))) => {
					tx.send(Ok(vec![dummy_validator(); 3])).unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::ValidationCodeHash(
						_para_id,
						OccupiedCoreAssumption::Free,
						tx,
					),
				))) => {
					tx.send(Ok(Some(ValidationCode(vec![1, 2, 3]).hash()))).unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::StagingAsyncBackingParams(tx),
				))) => {
					tx.send(Err(RuntimeApiError::NotSupported {
						runtime_api_name: "doesnt_matter",
					}))
					.unwrap();
				},
				Some(msg @ AllMessages::CollatorProtocol(_)) => {
					inner_to_collator_protocol.lock().await.push(msg);
				},
				Some(msg) => {
					panic!("didn't expect any other overseer requests; got {:?}", msg)
				},
			}
		}
	};

	let config = Arc::new(test_config(16));
	let subsystem_config = config.clone();

	subsystem_test_harness(overseer, |mut ctx| async move {
		handle_new_activations(subsystem_config, activated_hashes, &mut ctx, Metrics(None))
			.await
			.unwrap();
	});

	let mut to_collator_protocol = Arc::try_unwrap(to_collator_protocol)
		.expect("subsystem should have shut down by now")
		.into_inner();

	// we expect a single message to be sent, containing a candidate receipt.
	// we don't care too much about the `commitments_hash` right now, but let's ensure that we've
	// calculated the correct descriptor
	let expect_pov_hash = test_collation_compressed().proof_of_validity.into_compressed().hash();
	let expect_validation_data_hash = test_validation_data().hash();
	let expect_relay_parent = Hash::repeat_byte(4);
	let expect_validation_code_hash = ValidationCode(vec![1, 2, 3]).hash();
	let expect_payload = collator_signature_payload(
		&expect_relay_parent,
		&config.para_id,
		&expect_validation_data_hash,
		&expect_pov_hash,
		&expect_validation_code_hash,
	);
	let expect_descriptor = CandidateDescriptor {
		signature: config.key.sign(&expect_payload),
		para_id: config.para_id,
		relay_parent: expect_relay_parent,
		collator: config.key.public(),
		persisted_validation_data_hash: expect_validation_data_hash,
		pov_hash: expect_pov_hash,
		erasure_root: dummy_hash(), // this isn't something we're checking right now
		para_head: test_collation().head_data.hash(),
		validation_code_hash: expect_validation_code_hash,
	};

	assert_eq!(to_collator_protocol.len(), 1);
	match AllMessages::from(to_collator_protocol.pop().unwrap()) {
		AllMessages::CollatorProtocol(CollatorProtocolMessage::DistributeCollation(
			CandidateReceipt { descriptor, .. },
			_pov,
			..,
		)) => {
			// signature generation is non-deterministic, so we can't just assert that the
			// expected descriptor is correct. What we can do is validate that the produced
			// descriptor has a valid signature, then just copy in the generated signature
			// and check the rest of the fields for equality.
			assert!(CollatorPair::verify(
				&descriptor.signature,
				&collator_signature_payload(
					&descriptor.relay_parent,
					&descriptor.para_id,
					&descriptor.persisted_validation_data_hash,
					&descriptor.pov_hash,
					&descriptor.validation_code_hash,
				)
				.as_ref(),
				&descriptor.collator,
			));
			let expect_descriptor = {
				let mut expect_descriptor = expect_descriptor;
				expect_descriptor.signature = descriptor.signature.clone();
				expect_descriptor.erasure_root = descriptor.erasure_root;
				expect_descriptor
			};
			assert_eq!(descriptor, expect_descriptor);
		},
		_ => panic!("received wrong message type"),
	}
}

#[test]
fn fallback_when_no_validation_code_hash_api() {
	// This is a variant of the above test, but with the validation code hash API disabled.

	let activated_hashes: Vec<Hash> = vec![
		Hash::repeat_byte(1),
		Hash::repeat_byte(4),
		Hash::repeat_byte(9),
		Hash::repeat_byte(16),
	];

	// empty vec doesn't allocate on the heap, so it's ok we throw it away
	let to_collator_protocol = Arc::new(Mutex::new(Vec::new()));
	let inner_to_collator_protocol = to_collator_protocol.clone();

	let overseer = |mut handle: TestSubsystemContextHandle<CollationGenerationMessage>| async move {
		loop {
			match handle.try_recv().await {
				None => break,
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					hash,
					RuntimeApiRequest::AvailabilityCores(tx),
				))) => {
					tx.send(Ok(vec![
						CoreState::Free,
						CoreState::Scheduled(scheduled_core_for(
							(hash.as_fixed_bytes()[0] * 4) as u32,
						)),
						CoreState::Scheduled(scheduled_core_for(
							(hash.as_fixed_bytes()[0] * 5) as u32,
						)),
					]))
					.unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::PersistedValidationData(
						_para_id,
						_occupied_core_assumption,
						tx,
					),
				))) => {
					tx.send(Ok(Some(test_validation_data()))).unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::Validators(tx),
				))) => {
					tx.send(Ok(vec![dummy_validator(); 3])).unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::ValidationCodeHash(
						_para_id,
						OccupiedCoreAssumption::Free,
						tx,
					),
				))) => {
					tx.send(Err(RuntimeApiError::NotSupported {
						runtime_api_name: "validation_code_hash",
					}))
					.unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::ValidationCode(_para_id, OccupiedCoreAssumption::Free, tx),
				))) => {
					tx.send(Ok(Some(ValidationCode(vec![1, 2, 3])))).unwrap();
				},
				Some(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_hash,
					RuntimeApiRequest::StagingAsyncBackingParams(tx),
				))) => {
					tx.send(Err(RuntimeApiError::NotSupported {
						runtime_api_name: "doesnt_matter",
					}))
					.unwrap();
				},
				Some(msg @ AllMessages::CollatorProtocol(_)) => {
					inner_to_collator_protocol.lock().await.push(msg);
				},
				Some(msg) => {
					panic!("didn't expect any other overseer requests; got {:?}", msg)
				},
			}
		}
	};

	let config = Arc::new(test_config(16u32));
	let subsystem_config = config.clone();

	// empty vec doesn't allocate on the heap, so it's ok we throw it away
	subsystem_test_harness(overseer, |mut ctx| async move {
		handle_new_activations(subsystem_config, activated_hashes, &mut ctx, Metrics(None))
			.await
			.unwrap();
	});

	let to_collator_protocol = Arc::try_unwrap(to_collator_protocol)
		.expect("subsystem should have shut down by now")
		.into_inner();

	let expect_validation_code_hash = ValidationCode(vec![1, 2, 3]).hash();

	assert_eq!(to_collator_protocol.len(), 1);
	match &to_collator_protocol[0] {
		AllMessages::CollatorProtocol(CollatorProtocolMessage::DistributeCollation(
			CandidateReceipt { descriptor, .. },
			_pov,
			..,
		)) => {
			assert_eq!(expect_validation_code_hash, descriptor.validation_code_hash);
		},
		_ => panic!("received wrong message type"),
	}
}

#[test]
fn submit_collation_is_no_op_before_initialization() {
	test_harness(|mut virtual_overseer| async move {
		virtual_overseer
			.send(FromOrchestra::Communication {
				msg: CollationGenerationMessage::SubmitCollation(SubmitCollationParams {
					relay_parent: Hash::repeat_byte(0),
					collation: test_collation(),
					parent_head: vec![1, 2, 3].into(),
					validation_code_hash: Hash::repeat_byte(1).into(),
					result_sender: None,
				}),
			})
			.await;

		virtual_overseer
	});
}

#[test]
fn submit_collation_leads_to_distribution() {
	let relay_parent = Hash::repeat_byte(0);
	let validation_code_hash = ValidationCodeHash::from(Hash::repeat_byte(42));
	let parent_head = HeadData::from(vec![1, 2, 3]);
	let para_id = ParaId::from(5);
	let expected_pvd = PersistedValidationData {
		parent_head: parent_head.clone(),
		relay_parent_number: 10,
		relay_parent_storage_root: Hash::repeat_byte(1),
		max_pov_size: 1024,
	};

	test_harness(|mut virtual_overseer| async move {
		virtual_overseer
			.send(FromOrchestra::Communication {
				msg: CollationGenerationMessage::Initialize(test_config_no_collator(para_id)),
			})
			.await;

		virtual_overseer
			.send(FromOrchestra::Communication {
				msg: CollationGenerationMessage::SubmitCollation(SubmitCollationParams {
					relay_parent,
					collation: test_collation(),
					parent_head: vec![1, 2, 3].into(),
					validation_code_hash,
					result_sender: None,
				}),
			})
			.await;

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(rp, RuntimeApiRequest::Validators(tx))) => {
				assert_eq!(rp, relay_parent);
				let _ = tx.send(Ok(vec![
					Sr25519Keyring::Alice.public().into(),
					Sr25519Keyring::Bob.public().into(),
					Sr25519Keyring::Charlie.public().into(),
				]));
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(rp, RuntimeApiRequest::PersistedValidationData(id, a, tx))) => {
				assert_eq!(rp, relay_parent);
				assert_eq!(id, para_id);
				assert_eq!(a, OccupiedCoreAssumption::TimedOut);

				// Candidate receipt should be constructed with the real parent head.
				let mut pvd = expected_pvd.clone();
				pvd.parent_head = vec![4, 5, 6].into();
				let _ = tx.send(Ok(Some(pvd)));
			}
		);

		assert_matches!(
			overseer_recv(&mut virtual_overseer).await,
			AllMessages::CollatorProtocol(CollatorProtocolMessage::DistributeCollation(
				ccr,
				parent_head_data_hash,
				..
			)) => {
				assert_eq!(parent_head_data_hash, parent_head.hash());
				assert_eq!(ccr.descriptor().persisted_validation_data_hash, expected_pvd.hash());
				assert_eq!(ccr.descriptor().para_head, dummy_head_data().hash());
				assert_eq!(ccr.descriptor().validation_code_hash, validation_code_hash);
			}
		);

		virtual_overseer
	});
}
