// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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
use ::test_helpers::{
	dummy_candidate_receipt_bad_sig, dummy_collator, dummy_collator_signature,
	dummy_committed_candidate_receipt, dummy_hash, dummy_validation_code,
};
use assert_matches::assert_matches;
use futures::{future, Future};
use polkadot_node_primitives::{BlockData, InvalidCandidate};
use polkadot_node_subsystem::{
	messages::{
		AllMessages, CollatorProtocolMessage, RuntimeApiMessage, RuntimeApiRequest,
		ValidationFailed,
	},
	ActivatedLeaf, ActiveLeavesUpdate, FromOrchestra, LeafStatus, OverseerSignal,
};
use polkadot_node_subsystem_test_helpers as test_helpers;
use polkadot_primitives::v2::{
	CandidateDescriptor, CollatorId, GroupRotationInfo, HeadData, PersistedValidationData,
	ScheduledCore,
};
use sp_application_crypto::AppKey;
use sp_keyring::Sr25519Keyring;
use sp_keystore::{CryptoStore, SyncCryptoStore};
use sp_tracing as _;
use statement_table::v2::Misbehavior;
use std::collections::HashMap;

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

fn table_statement_to_primitive(statement: TableStatement) -> Statement {
	match statement {
		TableStatement::Seconded(committed_candidate_receipt) =>
			Statement::Seconded(committed_candidate_receipt),
		TableStatement::Valid(candidate_hash) => Statement::Valid(candidate_hash),
	}
}

struct TestState {
	chain_ids: Vec<ParaId>,
	keystore: SyncCryptoStorePtr,
	validators: Vec<Sr25519Keyring>,
	validator_public: Vec<ValidatorId>,
	validation_data: PersistedValidationData,
	validator_groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
	availability_cores: Vec<CoreState>,
	head_data: HashMap<ParaId, HeadData>,
	signing_context: SigningContext,
	relay_parent: Hash,
}

impl Default for TestState {
	fn default() -> Self {
		let chain_a = ParaId::from(1);
		let chain_b = ParaId::from(2);
		let thread_a = ParaId::from(3);

		let chain_ids = vec![chain_a, chain_b, thread_a];

		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
			Sr25519Keyring::One,
		];

		let keystore = Arc::new(sc_keystore::LocalKeystore::in_memory());
		// Make sure `Alice` key is in the keystore, so this mocked node will be a parachain validator.
		SyncCryptoStore::sr25519_generate_new(
			&*keystore,
			ValidatorId::ID,
			Some(&validators[0].to_seed()),
		)
		.expect("Insert key into keystore");

		let validator_public = validator_pubkeys(&validators);

		let validator_groups = vec![vec![2, 0, 3, 5], vec![1], vec![4]]
			.into_iter()
			.map(|g| g.into_iter().map(ValidatorIndex).collect())
			.collect();
		let group_rotation_info =
			GroupRotationInfo { session_start_block: 0, group_rotation_frequency: 100, now: 1 };

		let thread_collator: CollatorId = Sr25519Keyring::Two.public().into();
		let availability_cores = vec![
			CoreState::Scheduled(ScheduledCore { para_id: chain_a, collator: None }),
			CoreState::Scheduled(ScheduledCore { para_id: chain_b, collator: None }),
			CoreState::Scheduled(ScheduledCore {
				para_id: thread_a,
				collator: Some(thread_collator.clone()),
			}),
		];

		let mut head_data = HashMap::new();
		head_data.insert(chain_a, HeadData(vec![4, 5, 6]));

		let relay_parent = Hash::repeat_byte(5);

		let signing_context = SigningContext { session_index: 1, parent_hash: relay_parent };

		let validation_data = PersistedValidationData {
			parent_head: HeadData(vec![7, 8, 9]),
			relay_parent_number: 0_u32.into(),
			max_pov_size: 1024,
			relay_parent_storage_root: dummy_hash(),
		};

		Self {
			chain_ids,
			keystore,
			validators,
			validator_public,
			validator_groups: (validator_groups, group_rotation_info),
			availability_cores,
			head_data,
			validation_data,
			signing_context,
			relay_parent,
		}
	}
}

type VirtualOverseer = test_helpers::TestSubsystemContextHandle<CandidateBackingMessage>;

fn test_harness<T: Future<Output = VirtualOverseer>>(
	keystore: SyncCryptoStorePtr,
	test: impl FnOnce(VirtualOverseer) -> T,
) {
	let pool = sp_core::testing::TaskExecutor::new();

	let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

	let subsystem = async move {
		if let Err(e) = super::run(context, keystore, Metrics(None)).await {
			panic!("{:?}", e);
		}
	};

	let test_fut = test(virtual_overseer);

	futures::pin_mut!(test_fut);
	futures::pin_mut!(subsystem);
	futures::executor::block_on(future::join(
		async move {
			let mut virtual_overseer = test_fut.await;
			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
		},
		subsystem,
	));
}

fn make_erasure_root(test: &TestState, pov: PoV) -> Hash {
	let available_data =
		AvailableData { validation_data: test.validation_data.clone(), pov: Arc::new(pov) };

	let chunks = erasure_coding::obtain_chunks_v1(test.validators.len(), &available_data).unwrap();
	erasure_coding::branches(&chunks).root()
}

#[derive(Default)]
struct TestCandidateBuilder {
	para_id: ParaId,
	head_data: HeadData,
	pov_hash: Hash,
	relay_parent: Hash,
	erasure_root: Hash,
}

impl TestCandidateBuilder {
	fn build(self) -> CommittedCandidateReceipt {
		CommittedCandidateReceipt {
			descriptor: CandidateDescriptor {
				para_id: self.para_id,
				pov_hash: self.pov_hash,
				relay_parent: self.relay_parent,
				erasure_root: self.erasure_root,
				collator: dummy_collator(),
				signature: dummy_collator_signature(),
				para_head: dummy_hash(),
				validation_code_hash: dummy_validation_code().hash(),
				persisted_validation_data_hash: dummy_hash(),
			},
			commitments: CandidateCommitments {
				head_data: self.head_data,
				upward_messages: vec![],
				horizontal_messages: vec![],
				new_validation_code: None,
				processed_downward_messages: 0,
				hrmp_watermark: 0_u32,
			},
		}
	}
}

// Tests that the subsystem performs actions that are required on startup.
async fn test_startup(virtual_overseer: &mut VirtualOverseer, test_state: &TestState) {
	// Start work on some new parent.
	virtual_overseer
		.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(
			ActivatedLeaf {
				hash: test_state.relay_parent,
				number: 1,
				status: LeafStatus::Fresh,
				span: Arc::new(jaeger::Span::Disabled),
			},
		))))
		.await;

	// Check that subsystem job issues a request for a validator set.
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::Validators(tx))
		) if parent == test_state.relay_parent => {
			tx.send(Ok(test_state.validator_public.clone())).unwrap();
		}
	);

	// Check that subsystem job issues a request for the validator groups.
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::ValidatorGroups(tx))
		) if parent == test_state.relay_parent => {
			tx.send(Ok(test_state.validator_groups.clone())).unwrap();
		}
	);

	// Check that subsystem job issues a request for the session index for child.
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::SessionIndexForChild(tx))
		) if parent == test_state.relay_parent => {
			tx.send(Ok(test_state.signing_context.session_index)).unwrap();
		}
	);

	// Check that subsystem job issues a request for the availability cores.
	assert_matches!(
		virtual_overseer.recv().await,
		AllMessages::RuntimeApi(
			RuntimeApiMessage::Request(parent, RuntimeApiRequest::AvailabilityCores(tx))
		) if parent == test_state.relay_parent => {
			tx.send(Ok(test_state.availability_cores.clone())).unwrap();
		}
	);
}

// Test that a `CandidateBackingMessage::Second` issues validation work
// and in case validation is successful issues a `StatementDistributionMessage`.
#[test]
fn backing_second_works() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };

		let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

		let pov_hash = pov.hash();
		let candidate = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let second = CandidateBackingMessage::Second(
			test_state.relay_parent,
			candidate.to_plain(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					candidate_receipt,
					pov,
					timeout,
					tx,
				)
			) if pov == pov && &candidate_receipt.descriptor == candidate.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT &&  candidate.commitments.hash() == candidate_receipt.commitments_hash => {
				tx.send(Ok(
					ValidationResult::Valid(CandidateCommitments {
						head_data: expected_head_data.clone(),
						horizontal_messages: Vec::new(),
						upward_messages: Vec::new(),
						new_validation_code: None,
						processed_downward_messages: 0,
						hrmp_watermark: 0,
					}, test_state.validation_data.clone()),
				)).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::StoreAvailableData { candidate_hash, tx, .. }
			) if candidate_hash == candidate.hash() => {
				tx.send(Ok(())).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(
					parent_hash,
					_signed_statement,
				)
			) if parent_hash == test_state.relay_parent => {}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(CollatorProtocolMessage::Seconded(hash, statement)) => {
				assert_eq!(test_state.relay_parent, hash);
				assert_matches!(statement.payload(), Statement::Seconded(_));
			}
		);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(test_state.relay_parent),
			)))
			.await;
		virtual_overseer
	});
}

// Test that the candidate reaches quorum successfully.
#[test]
fn backing_works() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![1, 2, 3]) };

		let pov_hash = pov.hash();

		let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

		let candidate_a = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let candidate_a_hash = candidate_a.hash();
		let candidate_a_commitments_hash = candidate_a.commitments.hash();

		let public1 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[5].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let public2 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.await
		.expect("Insert key into keystore");

		let signed_a = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Seconded(candidate_a.clone()),
			&test_state.signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let signed_b = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Valid(candidate_a_hash),
			&test_state.signing_context,
			ValidatorIndex(5),
			&public1.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_a.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// Sending a `Statement::Seconded` for our assignment will start
		// validation process. The first thing requested is the PoV.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::FetchPoV {
					relay_parent,
					tx,
					..
				}
			) if relay_parent == test_state.relay_parent => {
				tx.send(pov.clone()).unwrap();
			}
		);

		// The next step is the actual request to Validation subsystem
		// to validate the `Seconded` candidate.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					c,
					pov,
					timeout,
					tx,
				)
			) if pov == pov && c.descriptor() == candidate_a.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT && c.commitments_hash == candidate_a_commitments_hash=> {
				tx.send(Ok(
					ValidationResult::Valid(CandidateCommitments {
						head_data: expected_head_data.clone(),
						upward_messages: Vec::new(),
						horizontal_messages: Vec::new(),
						new_validation_code: None,
						processed_downward_messages: 0,
						hrmp_watermark: 0,
					}, test_state.validation_data.clone()),
				)).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::StoreAvailableData { candidate_hash, tx, .. }
			) if candidate_hash == candidate_a.hash() => {
				tx.send(Ok(())).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::Provisioner(
				ProvisionerMessage::ProvisionableData(
					_,
					ProvisionableData::BackedCandidate(candidate_receipt)
				)
			) => {
				assert_eq!(candidate_receipt, candidate_a.to_plain());
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(hash, _stmt)
			) => {
				assert_eq!(test_state.relay_parent, hash);
			}
		);

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_b.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(test_state.relay_parent),
			)))
			.await;
		virtual_overseer
	});
}

#[test]
fn backing_works_while_validation_ongoing() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![1, 2, 3]) };

		let pov_hash = pov.hash();

		let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

		let candidate_a = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let candidate_a_hash = candidate_a.hash();
		let candidate_a_commitments_hash = candidate_a.commitments.hash();

		let public1 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[5].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let public2 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let public3 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[3].to_seed()),
		)
		.await
		.expect("Insert key into keystore");

		let signed_a = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Seconded(candidate_a.clone()),
			&test_state.signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let signed_b = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Valid(candidate_a_hash),
			&test_state.signing_context,
			ValidatorIndex(5),
			&public1.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let signed_c = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Valid(candidate_a_hash),
			&test_state.signing_context,
			ValidatorIndex(3),
			&public3.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_a.clone());
		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// Sending a `Statement::Seconded` for our assignment will start
		// validation process. The first thing requested is PoV from the
		// `PoVDistribution`.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::FetchPoV {
					relay_parent,
					tx,
					..
				}
			) if relay_parent == test_state.relay_parent => {
				tx.send(pov.clone()).unwrap();
			}
		);

		// The next step is the actual request to Validation subsystem
		// to validate the `Seconded` candidate.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					c,
					pov,
					timeout,
					tx,
				)
			) if pov == pov && c.descriptor() == candidate_a.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT && candidate_a_commitments_hash == c.commitments_hash => {
				// we never validate the candidate. our local node
				// shouldn't issue any statements.
				std::mem::forget(tx);
			}
		);

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_b.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// Candidate gets backed entirely by other votes.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::Provisioner(
				ProvisionerMessage::ProvisionableData(
					_,
					ProvisionableData::BackedCandidate(CandidateReceipt {
						descriptor,
						..
					})
				)
			) if descriptor == candidate_a.descriptor
		);

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_c.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		let (tx, rx) = oneshot::channel();
		let msg = CandidateBackingMessage::GetBackedCandidates(
			test_state.relay_parent,
			vec![candidate_a.hash()],
			tx,
		);

		virtual_overseer.send(FromOrchestra::Communication { msg }).await;

		let candidates = rx.await.unwrap();
		assert_eq!(1, candidates.len());
		assert_eq!(candidates[0].validity_votes.len(), 3);

		assert!(candidates[0]
			.validity_votes
			.contains(&ValidityAttestation::Implicit(signed_a.signature().clone())));
		assert!(candidates[0]
			.validity_votes
			.contains(&ValidityAttestation::Explicit(signed_b.signature().clone())));
		assert!(candidates[0]
			.validity_votes
			.contains(&ValidityAttestation::Explicit(signed_c.signature().clone())));
		assert_eq!(
			candidates[0].validator_indices,
			bitvec::bitvec![u8, bitvec::order::Lsb0; 1, 0, 1, 1],
		);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(test_state.relay_parent),
			)))
			.await;
		virtual_overseer
	});
}

// Issuing conflicting statements on the same candidate should
// be a misbehavior.
#[test]
fn backing_misbehavior_works() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![1, 2, 3]) };

		let pov_hash = pov.hash();

		let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

		let candidate_a = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			head_data: expected_head_data.clone(),
			..Default::default()
		}
		.build();

		let candidate_a_hash = candidate_a.hash();
		let candidate_a_commitments_hash = candidate_a.commitments.hash();

		let public2 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let seconded_2 = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Seconded(candidate_a.clone()),
			&test_state.signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let valid_2 = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Valid(candidate_a_hash),
			&test_state.signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, seconded_2.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::FetchPoV {
					relay_parent,
					tx,
					..
				}
			) if relay_parent == test_state.relay_parent => {
				tx.send(pov.clone()).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					c,
					pov,
					timeout,
					tx,
				)
			) if pov == pov && c.descriptor() == candidate_a.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT && candidate_a_commitments_hash == c.commitments_hash => {
				tx.send(Ok(
					ValidationResult::Valid(CandidateCommitments {
						head_data: expected_head_data.clone(),
						upward_messages: Vec::new(),
						horizontal_messages: Vec::new(),
						new_validation_code: None,
						processed_downward_messages: 0,
						hrmp_watermark: 0,
					}, test_state.validation_data.clone()),
				)).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::StoreAvailableData { candidate_hash, tx, .. }
			) if candidate_hash == candidate_a.hash() => {
					tx.send(Ok(())).unwrap();
				}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::Provisioner(
				ProvisionerMessage::ProvisionableData(
					_,
					ProvisionableData::BackedCandidate(CandidateReceipt {
						descriptor,
						..
					})
				)
			) if descriptor == candidate_a.descriptor
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(
					relay_parent,
					signed_statement,
				)
			) if relay_parent == test_state.relay_parent => {
				assert_eq!(*signed_statement.payload(), Statement::Valid(candidate_a_hash));
			}
		);

		// This `Valid` statement is redundant after the `Seconded` statement already sent.
		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, valid_2.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::Provisioner(
				ProvisionerMessage::ProvisionableData(
					_,
					ProvisionableData::MisbehaviorReport(
						relay_parent,
						validator_index,
						Misbehavior::ValidityDoubleVote(vdv),
					)
				)
			) if relay_parent == test_state.relay_parent => {
				let ((t1, s1), (t2, s2)) = vdv.deconstruct::<TableContext>();
				let t1 = table_statement_to_primitive(t1);
				let t2 = table_statement_to_primitive(t2);

				SignedFullStatement::new(
					t1,
					validator_index,
					s1,
					&test_state.signing_context,
					&test_state.validator_public[validator_index.0 as usize],
				).expect("signature must be valid");

				SignedFullStatement::new(
					t2,
					validator_index,
					s2,
					&test_state.signing_context,
					&test_state.validator_public[validator_index.0 as usize],
				).expect("signature must be valid");
			}
		);
		virtual_overseer
	});
}

// Test that if we are asked to second an invalid candidate we
// can still second a valid one afterwards.
#[test]
fn backing_dont_second_invalid() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov_block_a = PoV { block_data: BlockData(vec![42, 43, 44]) };

		let pov_block_b = PoV { block_data: BlockData(vec![45, 46, 47]) };

		let pov_hash_a = pov_block_a.hash();
		let pov_hash_b = pov_block_b.hash();

		let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

		let candidate_a = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash: pov_hash_a,
			erasure_root: make_erasure_root(&test_state, pov_block_a.clone()),
			..Default::default()
		}
		.build();

		let candidate_b = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash: pov_hash_b,
			erasure_root: make_erasure_root(&test_state, pov_block_b.clone()),
			head_data: expected_head_data.clone(),
			..Default::default()
		}
		.build();

		let second = CandidateBackingMessage::Second(
			test_state.relay_parent,
			candidate_a.to_plain(),
			pov_block_a.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					c,
					pov,
					timeout,
					tx,
				)
			) if pov == pov && c.descriptor() == candidate_a.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT => {
				tx.send(Ok(ValidationResult::Invalid(InvalidCandidate::BadReturn))).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(
				CollatorProtocolMessage::Invalid(parent_hash, c)
			) if parent_hash == test_state.relay_parent && c == candidate_a.to_plain()
		);

		let second = CandidateBackingMessage::Second(
			test_state.relay_parent,
			candidate_b.to_plain(),
			pov_block_b.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					c,
					pov,
					timeout,
					tx,
				)
			) if pov == pov && c.descriptor() == candidate_b.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT => {
				tx.send(Ok(
					ValidationResult::Valid(CandidateCommitments {
						head_data: expected_head_data.clone(),
						upward_messages: Vec::new(),
						horizontal_messages: Vec::new(),
						new_validation_code: None,
						processed_downward_messages: 0,
						hrmp_watermark: 0,
					}, test_state.validation_data.clone()),
				)).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::StoreAvailableData { candidate_hash, tx, .. }
			) if candidate_hash == candidate_b.hash() => {
				tx.send(Ok(())).unwrap();
			}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::StatementDistribution(
				StatementDistributionMessage::Share(
					parent_hash,
					signed_statement,
				)
			) if parent_hash == test_state.relay_parent => {
				assert_eq!(*signed_statement.payload(), Statement::Seconded(candidate_b));
			}
		);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(test_state.relay_parent),
			)))
			.await;
		virtual_overseer
	});
}

// Test that if we have already issued a statement (in this case `Invalid`) about a
// candidate we will not be issuing a `Seconded` statement on it.
#[test]
fn backing_second_after_first_fails_works() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };

		let pov_hash = pov.hash();

		let candidate = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let validator2 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.await
		.expect("Insert key into keystore");

		let signed_a = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Seconded(candidate.clone()),
			&test_state.signing_context,
			ValidatorIndex(2),
			&validator2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		// Send in a `Statement` with a candidate.
		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_a.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// Subsystem requests PoV and requests validation.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::FetchPoV {
					relay_parent,
					tx,
					..
				}
			) if relay_parent == test_state.relay_parent => {
				tx.send(pov.clone()).unwrap();
			}
		);

		// Tell subsystem that this candidate is invalid.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					c,
					pov,
					timeout,
					tx,
				)
			) if pov == pov && c.descriptor() == candidate.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT && c.commitments_hash == candidate.commitments.hash() => {
				tx.send(Ok(ValidationResult::Invalid(InvalidCandidate::BadReturn))).unwrap();
			}
		);

		// Ask subsystem to `Second` a candidate that already has a statement issued about.
		// This should emit no actions from subsystem.
		let second = CandidateBackingMessage::Second(
			test_state.relay_parent,
			candidate.to_plain(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		let pov_to_second = PoV { block_data: BlockData(vec![3, 2, 1]) };

		let pov_hash = pov_to_second.hash();

		let candidate_to_second = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			erasure_root: make_erasure_root(&test_state, pov_to_second.clone()),
			..Default::default()
		}
		.build();

		let second = CandidateBackingMessage::Second(
			test_state.relay_parent,
			candidate_to_second.to_plain(),
			pov_to_second.clone(),
		);

		// In order to trigger _some_ actions from subsystem ask it to second another
		// candidate. The only reason to do so is to make sure that no actions were
		// triggered on the prev step.
		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					_,
					pov,
					_,
					_,
				)
			) => {
				assert_eq!(&*pov, &pov_to_second);
			}
		);
		virtual_overseer
	});
}

// That that if the validation of the candidate has failed this does not stop
// the work of this subsystem and so it is not fatal to the node.
#[test]
fn backing_works_after_failed_validation() {
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };

		let pov_hash = pov.hash();

		let candidate = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let public2 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let signed_a = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Seconded(candidate.clone()),
			&test_state.signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		// Send in a `Statement` with a candidate.
		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_a.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// Subsystem requests PoV and requests validation.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::FetchPoV {
					relay_parent,
					tx,
					..
				}
			) if relay_parent == test_state.relay_parent => {
				tx.send(pov.clone()).unwrap();
			}
		);

		// Tell subsystem that this candidate is invalid.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					c,
					pov,
					timeout,
					tx,
				)
			) if pov == pov && c.descriptor() == candidate.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT && c.commitments_hash == candidate.commitments.hash() => {
				tx.send(Err(ValidationFailed("Internal test error".into()))).unwrap();
			}
		);

		// Try to get a set of backable candidates to trigger _some_ action in the subsystem
		// and check that it is still alive.
		let (tx, rx) = oneshot::channel();
		let msg = CandidateBackingMessage::GetBackedCandidates(
			test_state.relay_parent,
			vec![candidate.hash()],
			tx,
		);

		virtual_overseer.send(FromOrchestra::Communication { msg }).await;
		assert_eq!(rx.await.unwrap().len(), 0);
		virtual_overseer
	});
}

// Test that a `CandidateBackingMessage::Second` issues validation work
// and in case validation is successful issues a `StatementDistributionMessage`.
#[test]
fn backing_doesnt_second_wrong_collator() {
	let mut test_state = TestState::default();
	test_state.availability_cores[0] = CoreState::Scheduled(ScheduledCore {
		para_id: ParaId::from(1),
		collator: Some(Sr25519Keyring::Bob.public().into()),
	});

	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };

		let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

		let pov_hash = pov.hash();
		let candidate = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let second = CandidateBackingMessage::Second(
			test_state.relay_parent,
			candidate.to_plain(),
			pov.clone(),
		);

		virtual_overseer.send(FromOrchestra::Communication { msg: second }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CollatorProtocol(
				CollatorProtocolMessage::Invalid(parent, c)
			) if parent == test_state.relay_parent && c == candidate.to_plain() => {
			}
		);

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(test_state.relay_parent),
			)))
			.await;
		virtual_overseer
	});
}

#[test]
fn validation_work_ignores_wrong_collator() {
	let mut test_state = TestState::default();
	test_state.availability_cores[0] = CoreState::Scheduled(ScheduledCore {
		para_id: ParaId::from(1),
		collator: Some(Sr25519Keyring::Bob.public().into()),
	});

	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![1, 2, 3]) };

		let pov_hash = pov.hash();

		let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

		let candidate_a = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let public2 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let seconding = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Seconded(candidate_a.clone()),
			&test_state.signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, seconding.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// The statement will be ignored because it has the wrong collator.
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(test_state.relay_parent),
			)))
			.await;
		virtual_overseer
	});
}

#[test]
fn candidate_backing_reorders_votes() {
	use sp_core::Encode;

	let para_id = ParaId::from(10);
	let validators = vec![
		Sr25519Keyring::Alice,
		Sr25519Keyring::Bob,
		Sr25519Keyring::Charlie,
		Sr25519Keyring::Dave,
		Sr25519Keyring::Ferdie,
		Sr25519Keyring::One,
	];

	let validator_public = validator_pubkeys(&validators);
	let validator_groups = {
		let mut validator_groups = HashMap::new();
		validator_groups
			.insert(para_id, vec![0, 1, 2, 3, 4, 5].into_iter().map(ValidatorIndex).collect());
		validator_groups
	};

	let table_context = TableContext {
		validator: None,
		groups: validator_groups,
		validators: validator_public.clone(),
	};

	let fake_attestation = |idx: u32| {
		let candidate =
			dummy_candidate_receipt_bad_sig(Default::default(), Some(Default::default()));
		let hash = candidate.hash();
		let mut data = vec![0; 64];
		data[0..32].copy_from_slice(hash.0.as_bytes());
		data[32..36].copy_from_slice(idx.encode().as_slice());

		let sig = ValidatorSignature::try_from(data).unwrap();
		statement_table::generic::ValidityAttestation::Implicit(sig)
	};

	let attested = TableAttestedCandidate {
		candidate: dummy_committed_candidate_receipt(dummy_hash()),
		validity_votes: vec![
			(ValidatorIndex(5), fake_attestation(5)),
			(ValidatorIndex(3), fake_attestation(3)),
			(ValidatorIndex(1), fake_attestation(1)),
		],
		group_id: para_id,
	};

	let backed = table_attested_to_backed(attested, &table_context).unwrap();

	let expected_bitvec = {
		let mut validator_indices = BitVec::<u8, bitvec::order::Lsb0>::with_capacity(6);
		validator_indices.resize(6, false);

		validator_indices.set(1, true);
		validator_indices.set(3, true);
		validator_indices.set(5, true);

		validator_indices
	};

	// Should be in bitfield order, which is opposite to the order provided to the function.
	let expected_attestations =
		vec![fake_attestation(1).into(), fake_attestation(3).into(), fake_attestation(5).into()];

	assert_eq!(backed.validator_indices, expected_bitvec);
	assert_eq!(backed.validity_votes, expected_attestations);
}

// Test whether we retry on failed PoV fetching.
#[test]
fn retry_works() {
	// sp_tracing::try_init_simple();
	let test_state = TestState::default();
	test_harness(test_state.keystore.clone(), |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![42, 43, 44]) };

		let pov_hash = pov.hash();

		let candidate = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let public2 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let public3 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[3].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let public5 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[5].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let signed_a = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Seconded(candidate.clone()),
			&test_state.signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");
		let signed_b = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Valid(candidate.hash()),
			&test_state.signing_context,
			ValidatorIndex(3),
			&public3.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");
		let signed_c = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Valid(candidate.hash()),
			&test_state.signing_context,
			ValidatorIndex(5),
			&public5.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		// Send in a `Statement` with a candidate.
		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_a.clone());
		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// Subsystem requests PoV and requests validation.
		// We cancel - should mean retry on next backing statement.
		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::FetchPoV {
					relay_parent,
					tx,
					..
				}
			) if relay_parent == test_state.relay_parent => {
				std::mem::drop(tx);
			}
		);

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_b.clone());
		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		// Not deterministic which message comes first:
		for _ in 0u32..2 {
			match virtual_overseer.recv().await {
				AllMessages::Provisioner(ProvisionerMessage::ProvisionableData(
					_,
					ProvisionableData::BackedCandidate(CandidateReceipt { descriptor, .. }),
				)) => {
					assert_eq!(descriptor, candidate.descriptor);
				},
				AllMessages::AvailabilityDistribution(
					AvailabilityDistributionMessage::FetchPoV { relay_parent, tx, .. },
				) if relay_parent == test_state.relay_parent => {
					std::mem::drop(tx);
				},
				msg => {
					assert!(false, "Unexpected message: {:?}", msg);
				},
			}
		}

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_c.clone());
		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::AvailabilityDistribution(
				AvailabilityDistributionMessage::FetchPoV {
					relay_parent,
					tx,
					..
				}
				// Subsystem requests PoV and requests validation.
				// Now we pass.
				) if relay_parent == test_state.relay_parent => {
					tx.send(pov.clone()).unwrap();
				}
		);

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::CandidateValidation(
				CandidateValidationMessage::ValidateFromChainState(
					c,
					pov,
					timeout,
					_tx,
				)
			) if pov == pov && c.descriptor() == candidate.descriptor() && timeout == BACKING_EXECUTION_TIMEOUT && c.commitments_hash == candidate.commitments.hash()
		);
		virtual_overseer
	});
}

#[test]
fn observes_backing_even_if_not_validator() {
	let test_state = TestState::default();
	let empty_keystore = Arc::new(sc_keystore::LocalKeystore::in_memory());
	test_harness(empty_keystore, |mut virtual_overseer| async move {
		test_startup(&mut virtual_overseer, &test_state).await;

		let pov = PoV { block_data: BlockData(vec![1, 2, 3]) };

		let pov_hash = pov.hash();

		let expected_head_data = test_state.head_data.get(&test_state.chain_ids[0]).unwrap();

		let candidate_a = TestCandidateBuilder {
			para_id: test_state.chain_ids[0],
			relay_parent: test_state.relay_parent,
			pov_hash,
			head_data: expected_head_data.clone(),
			erasure_root: make_erasure_root(&test_state, pov.clone()),
			..Default::default()
		}
		.build();

		let candidate_a_hash = candidate_a.hash();
		let public0 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[0].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let public1 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[5].to_seed()),
		)
		.await
		.expect("Insert key into keystore");
		let public2 = CryptoStore::sr25519_generate_new(
			&*test_state.keystore,
			ValidatorId::ID,
			Some(&test_state.validators[2].to_seed()),
		)
		.await
		.expect("Insert key into keystore");

		// Produce a 3-of-5 quorum on the candidate.

		let signed_a = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Seconded(candidate_a.clone()),
			&test_state.signing_context,
			ValidatorIndex(0),
			&public0.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let signed_b = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Valid(candidate_a_hash),
			&test_state.signing_context,
			ValidatorIndex(5),
			&public1.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let signed_c = SignedFullStatement::sign(
			&test_state.keystore,
			Statement::Valid(candidate_a_hash),
			&test_state.signing_context,
			ValidatorIndex(2),
			&public2.into(),
		)
		.await
		.ok()
		.flatten()
		.expect("should be signed");

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_a.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_b.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		assert_matches!(
			virtual_overseer.recv().await,
			AllMessages::Provisioner(
				ProvisionerMessage::ProvisionableData(
					_,
					ProvisionableData::BackedCandidate(candidate_receipt)
				)
			) => {
				assert_eq!(candidate_receipt, candidate_a.to_plain());
			}
		);

		let statement =
			CandidateBackingMessage::Statement(test_state.relay_parent, signed_c.clone());

		virtual_overseer.send(FromOrchestra::Communication { msg: statement }).await;

		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::stop_work(test_state.relay_parent),
			)))
			.await;
		virtual_overseer
	});
}
