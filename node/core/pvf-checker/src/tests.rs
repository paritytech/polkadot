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

use ::test_helpers::{dummy_digest, dummy_hash};
use futures::{channel::oneshot, future::BoxFuture, prelude::*};
use polkadot_node_subsystem::{
	jaeger,
	messages::{
		AllMessages, CandidateValidationMessage, PreCheckOutcome, PvfCheckerMessage,
		RuntimeApiMessage, RuntimeApiRequest,
	},
	ActivatedLeaf, ActiveLeavesUpdate, FromOrchestra, LeafStatus, OverseerSignal, RuntimeApiError,
};
use polkadot_node_subsystem_test_helpers::{make_subsystem_context, TestSubsystemContextHandle};
use polkadot_primitives::v2::{
	BlockNumber, Hash, Header, PvfCheckStatement, SessionIndex, ValidationCode, ValidationCodeHash,
	ValidatorId,
};
use sp_application_crypto::AppKey;
use sp_core::testing::TaskExecutor;
use sp_keyring::Sr25519Keyring;
use sp_keystore::SyncCryptoStore;
use sp_runtime::traits::AppVerify;
use std::{collections::HashMap, sync::Arc, time::Duration};

type VirtualOverseer = TestSubsystemContextHandle<PvfCheckerMessage>;

fn dummy_validation_code_hash(descriminator: u8) -> ValidationCodeHash {
	ValidationCode(vec![descriminator]).hash()
}

struct StartsNewSession {
	session_index: SessionIndex,
	validators: Vec<Sr25519Keyring>,
}

#[derive(Debug, Clone)]
struct FakeLeaf {
	block_hash: Hash,
	block_number: BlockNumber,
	pvfs: Vec<ValidationCodeHash>,
}

impl FakeLeaf {
	fn new(parent_hash: Hash, block_number: BlockNumber, pvfs: Vec<ValidationCodeHash>) -> Self {
		let block_header = Header {
			parent_hash,
			number: block_number,
			digest: dummy_digest(),
			state_root: dummy_hash(),
			extrinsics_root: dummy_hash(),
		};
		let block_hash = block_header.hash();
		Self { block_hash, block_number, pvfs }
	}

	fn descendant(&self, pvfs: Vec<ValidationCodeHash>) -> FakeLeaf {
		FakeLeaf::new(self.block_hash, self.block_number + 1, pvfs)
	}
}

struct LeafState {
	/// The session index at which this leaf was activated.
	session_index: SessionIndex,

	/// The list of PVFs that are pending in this leaf.
	pvfs: Vec<ValidationCodeHash>,
}

/// The state we model about a session.
struct SessionState {
	validators: Vec<ValidatorId>,
}

struct TestState {
	leaves: HashMap<Hash, LeafState>,
	sessions: HashMap<SessionIndex, SessionState>,
	last_session_index: SessionIndex,
}

const OUR_VALIDATOR: Sr25519Keyring = Sr25519Keyring::Alice;

fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
	val_ids.iter().map(|v| v.public().into()).collect()
}

impl TestState {
	fn new() -> Self {
		// Initialize the default session 1. No validators are present there.
		let last_session_index = 1;
		let mut sessions = HashMap::new();
		sessions.insert(last_session_index, SessionState { validators: vec![] });

		let mut leaves = HashMap::new();
		leaves.insert(dummy_hash(), LeafState { session_index: last_session_index, pvfs: vec![] });

		Self { leaves, sessions, last_session_index }
	}

	/// A convenience function to receive a message from the overseer and returning `None` if nothing
	/// was received within a reasonable (for local tests anyway) timeout.
	async fn recv_timeout(&mut self, handle: &mut VirtualOverseer) -> Option<AllMessages> {
		futures::select! {
			msg = handle.recv().fuse() => {
				Some(msg)
			}
			_ = futures_timer::Delay::new(Duration::from_millis(500)).fuse() => {
				None
			}
		}
	}

	async fn send_conclude(&mut self, handle: &mut VirtualOverseer) {
		// To ensure that no messages are left in the queue there is no better way to just wait.
		match self.recv_timeout(handle).await {
			Some(msg) => {
				panic!("we supposed to conclude, but received a message: {:#?}", msg);
			},
			None => {
				// No messages are received. We are good.
			},
		}

		handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	}

	/// Convenience function to invoke [`active_leaves_update`] with the new leaf that starts a new
	/// session and there are no deactivated leaves.
	///
	/// Returns the block hash of the newly activated leaf.
	async fn activate_leaf_with_session(
		&mut self,
		handle: &mut VirtualOverseer,
		leaf: FakeLeaf,
		starts_new_session: StartsNewSession,
	) {
		self.active_leaves_update(handle, Some(leaf), Some(starts_new_session), &[])
			.await
	}

	/// Convenience function to invoke [`active_leaves_update`] with a new leaf. The leaf does not
	/// start a new session and there are no deactivated leaves.
	async fn activate_leaf(&mut self, handle: &mut VirtualOverseer, leaf: FakeLeaf) {
		self.active_leaves_update(handle, Some(leaf), None, &[]).await
	}

	async fn deactive_leaves(
		&mut self,
		handle: &mut VirtualOverseer,
		deactivated: impl IntoIterator<Item = &Hash>,
	) {
		self.active_leaves_update(handle, None, None, deactivated).await
	}

	/// Sends an `ActiveLeavesUpdate` message to the overseer and also updates the test state to
	/// record leaves and session changes.
	///
	/// NOTE: This function may stall if there is an unhandled message for the overseer.
	async fn active_leaves_update(
		&mut self,
		handle: &mut VirtualOverseer,
		fake_leaf: Option<FakeLeaf>,
		starts_new_session: Option<StartsNewSession>,
		deactivated: impl IntoIterator<Item = &Hash>,
	) {
		if let Some(new_session) = starts_new_session {
			assert!(fake_leaf.is_some(), "Session can be started only with an activated leaf");
			self.last_session_index = new_session.session_index;
			let prev = self.sessions.insert(
				new_session.session_index,
				SessionState { validators: validator_pubkeys(&new_session.validators) },
			);
			assert!(prev.is_none(), "Session {} already exists", new_session.session_index);
		}

		let activated = if let Some(activated_leaf) = fake_leaf {
			self.leaves.insert(
				activated_leaf.block_hash.clone(),
				LeafState {
					session_index: self.last_session_index,
					pvfs: activated_leaf.pvfs.clone(),
				},
			);

			Some(ActivatedLeaf {
				hash: activated_leaf.block_hash,
				span: Arc::new(jaeger::Span::Disabled),
				number: activated_leaf.block_number,
				status: LeafStatus::Fresh,
			})
		} else {
			None
		};

		handle
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
				activated,
				deactivated: deactivated.into_iter().cloned().collect(),
			})))
			.await;
	}

	/// Expects that the subsystem has sent a `Validators` Runtime API request. Answers with the
	/// mocked validators for the requested leaf.
	async fn expect_validators(&mut self, handle: &mut VirtualOverseer) {
		match self.recv_timeout(handle).await.expect("timeout waiting for a message") {
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::Validators(tx),
			)) => match self.leaves.get(&relay_parent) {
				Some(leaf) => {
					let session_index = leaf.session_index;
					let session = self.sessions.get(&session_index).unwrap();
					tx.send(Ok(session.validators.clone())).unwrap();
				},
				None => {
					panic!("a request to an unknown relay parent has been made");
				},
			},
			msg => panic!("Unexpected message was received: {:#?}", msg),
		}
	}

	/// Expects that the subsystem has sent a `SessionIndexForChild` Runtime API request. Answers
	/// with the mocked session index for the requested leaf.
	async fn expect_session_for_child(&mut self, handle: &mut VirtualOverseer) {
		match self.recv_timeout(handle).await.expect("timeout waiting for a message") {
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SessionIndexForChild(tx),
			)) => match self.leaves.get(&relay_parent) {
				Some(leaf) => {
					tx.send(Ok(leaf.session_index)).unwrap();
				},
				None => {
					panic!("a request to an unknown relay parent has been made");
				},
			},
			msg => panic!("Unexpected message was received: {:#?}", msg),
		}
	}

	/// Expects that the subsystem has sent a `PvfsRequirePrecheck` Runtime API request. Answers
	/// with the mocked PVF set for the requested leaf.
	async fn expect_pvfs_require_precheck(
		&mut self,
		handle: &mut VirtualOverseer,
	) -> ExpectPvfsRequirePrecheck<'_> {
		match self.recv_timeout(handle).await.expect("timeout waiting for a message") {
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::PvfsRequirePrecheck(tx),
			)) => ExpectPvfsRequirePrecheck { test_state: self, relay_parent, tx },
			msg => panic!("Unexpected message was received: {:#?}", msg),
		}
	}

	/// Expects that the subsystem has sent a pre-checking request to candidate-validation. Returns
	/// a mocked handle for the request.
	async fn expect_candidate_precheck(
		&mut self,
		handle: &mut VirtualOverseer,
	) -> ExpectCandidatePrecheck {
		match self.recv_timeout(handle).await.expect("timeout waiting for a message") {
			AllMessages::CandidateValidation(CandidateValidationMessage::PreCheck(
				relay_parent,
				validation_code_hash,
				tx,
			)) => ExpectCandidatePrecheck { relay_parent, validation_code_hash, tx },
			msg => panic!("Unexpected message was received: {:#?}", msg),
		}
	}

	/// Expects that the subsystem has sent a `SubmitPvfCheckStatement` runtime API request. Returns
	/// a mocked handle for the request.
	async fn expect_submit_vote(&mut self, handle: &mut VirtualOverseer) -> ExpectSubmitVote {
		match self.recv_timeout(handle).await.expect("timeout waiting for a message") {
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				relay_parent,
				RuntimeApiRequest::SubmitPvfCheckStatement(stmt, signature, tx),
			)) => {
				let signing_payload = stmt.signing_payload();
				assert!(signature.verify(&signing_payload[..], &OUR_VALIDATOR.public().into()));

				ExpectSubmitVote { relay_parent, stmt, tx }
			},
			msg => panic!("Unexpected message was received: {:#?}", msg),
		}
	}
}

#[must_use]
struct ExpectPvfsRequirePrecheck<'a> {
	test_state: &'a mut TestState,
	relay_parent: Hash,
	tx: oneshot::Sender<Result<Vec<ValidationCodeHash>, RuntimeApiError>>,
}

impl<'a> ExpectPvfsRequirePrecheck<'a> {
	fn reply_mock(self) {
		match self.test_state.leaves.get(&self.relay_parent) {
			Some(leaf) => {
				self.tx.send(Ok(leaf.pvfs.clone())).unwrap();
			},
			None => {
				panic!(
					"a request to an unknown relay parent has been made: {:#?}",
					self.relay_parent
				);
			},
		}
	}

	fn reply_not_supported(self) {
		self.tx
			.send(Err(RuntimeApiError::NotSupported { runtime_api_name: "pvfs_require_precheck" }))
			.unwrap();
	}
}

#[must_use]
struct ExpectCandidatePrecheck {
	relay_parent: Hash,
	validation_code_hash: ValidationCodeHash,
	tx: oneshot::Sender<PreCheckOutcome>,
}

impl ExpectCandidatePrecheck {
	fn reply(self, outcome: PreCheckOutcome) {
		self.tx.send(outcome).unwrap();
	}
}

#[must_use]
struct ExpectSubmitVote {
	relay_parent: Hash,
	stmt: PvfCheckStatement,
	tx: oneshot::Sender<Result<(), RuntimeApiError>>,
}

impl ExpectSubmitVote {
	fn reply_ok(self) {
		self.tx.send(Ok(())).unwrap();
	}
}

fn test_harness(test: impl FnOnce(TestState, VirtualOverseer) -> BoxFuture<'static, ()>) {
	let pool = TaskExecutor::new();
	let (ctx, handle) = make_subsystem_context::<PvfCheckerMessage, _>(pool.clone());
	let keystore = Arc::new(sc_keystore::LocalKeystore::in_memory());

	// Add OUR_VALIDATOR (which is Alice) to the keystore.
	SyncCryptoStore::sr25519_generate_new(
		&*keystore,
		ValidatorId::ID,
		Some(&OUR_VALIDATOR.to_seed()),
	)
	.expect("Generating keys for our node failed");

	let subsystem_task = crate::run(ctx, keystore, crate::Metrics::default()).map(|x| x.unwrap());

	let test_state = TestState::new();
	let test_task = test(test_state, handle);

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn concludes_correctly() {
	test_harness(|mut test_state, mut handle| {
		async move {
			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn reacts_to_new_pvfs_in_heads() {
	test_harness(|mut test_state, mut handle| {
		async move {
			let block = FakeLeaf::new(dummy_hash(), 1, vec![dummy_validation_code_hash(1)]);

			test_state
				.activate_leaf_with_session(
					&mut handle,
					block.clone(),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			let pre_check = test_state.expect_candidate_precheck(&mut handle).await;
			assert_eq!(pre_check.relay_parent, block.block_hash);
			pre_check.reply(PreCheckOutcome::Valid);

			let vote = test_state.expect_submit_vote(&mut handle).await;
			assert_eq!(vote.relay_parent, block.block_hash);
			assert_eq!(vote.stmt.accept, true);
			assert_eq!(vote.stmt.session_index, 2);
			assert_eq!(vote.stmt.validator_index, 0.into());
			assert_eq!(vote.stmt.subject, dummy_validation_code_hash(1));
			vote.reply_ok();

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn no_new_session_no_validators_request() {
	test_harness(|mut test_state, mut handle| {
		async move {
			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 1, vec![]),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			test_state
				.activate_leaf(&mut handle, FakeLeaf::new(dummy_hash(), 2, vec![]))
				.await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn activation_of_descendant_leaves_pvfs_in_view() {
	test_harness(|mut test_state, mut handle| {
		async move {
			let block_1 = FakeLeaf::new(dummy_hash(), 1, vec![dummy_validation_code_hash(1)]);
			let block_2 = block_1.descendant(vec![dummy_validation_code_hash(1)]);

			test_state
				.activate_leaf_with_session(
					&mut handle,
					block_1.clone(),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			test_state
				.expect_candidate_precheck(&mut handle)
				.await
				.reply(PreCheckOutcome::Valid);
			test_state.expect_submit_vote(&mut handle).await.reply_ok();

			// Now we deactivate the first block and activate it's descendant.
			test_state
				.active_leaves_update(
					&mut handle,
					Some(block_2),
					None, // no new session started
					&[block_1.block_hash],
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn reactivating_pvf_leads_to_second_check() {
	test_harness(|mut test_state, mut handle| {
		async move {
			let pvf = dummy_validation_code_hash(1);
			let block_1 = FakeLeaf::new(dummy_hash(), 1, vec![pvf.clone()]);
			let block_2 = block_1.descendant(vec![]);
			let block_3 = block_2.descendant(vec![pvf.clone()]);

			test_state
				.activate_leaf_with_session(
					&mut handle,
					block_1.clone(),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;
			test_state
				.expect_candidate_precheck(&mut handle)
				.await
				.reply(PreCheckOutcome::Valid);
			test_state.expect_submit_vote(&mut handle).await.reply_ok();

			// Now activate a descdedant leaf, where the PVF is not present.
			test_state
				.active_leaves_update(
					&mut handle,
					Some(block_2.clone()),
					None,
					&[block_1.block_hash],
				)
				.await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;

			// Now the third block is activated, where the PVF is present.
			test_state.activate_leaf(&mut handle, block_3).await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state
				.expect_candidate_precheck(&mut handle)
				.await
				.reply(PreCheckOutcome::Valid);

			// We do not vote here, because the PVF was already voted on within this session.

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn dont_double_vote_for_pvfs_in_view() {
	test_harness(|mut test_state, mut handle| {
		async move {
			let pvf = dummy_validation_code_hash(1);
			let block_1_1 = FakeLeaf::new([1; 32].into(), 1, vec![pvf.clone()]);
			let block_2_1 = FakeLeaf::new([2; 32].into(), 1, vec![pvf.clone()]);
			let block_1_2 = block_1_1.descendant(vec![pvf.clone()]);

			test_state
				.activate_leaf_with_session(
					&mut handle,
					block_1_1.clone(),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			// Pre-checking will take quite some time.
			let pre_check = test_state.expect_candidate_precheck(&mut handle).await;

			// Activate a sibiling leaf, has the same PVF.
			test_state.activate_leaf(&mut handle, block_2_1).await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;

			// Now activate a descendant leaf with the same PVF.
			test_state
				.active_leaves_update(
					&mut handle,
					Some(block_1_2.clone()),
					None,
					&[block_1_1.block_hash],
				)
				.await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;

			// Now finish the pre-checking request.
			pre_check.reply(PreCheckOutcome::Valid);
			test_state.expect_submit_vote(&mut handle).await.reply_ok();

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn judgements_come_out_of_order() {
	test_harness(|mut test_state, mut handle| {
		async move {
			let pvf_1 = dummy_validation_code_hash(1);
			let pvf_2 = dummy_validation_code_hash(2);

			let block_1 = FakeLeaf::new([1; 32].into(), 1, vec![pvf_1.clone()]);
			let block_2 = FakeLeaf::new([2; 32].into(), 1, vec![pvf_2.clone()]);

			test_state
				.activate_leaf_with_session(
					&mut handle,
					block_1.clone(),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			let pre_check_1 = test_state.expect_candidate_precheck(&mut handle).await;

			// Activate a sibiling leaf, has the second PVF.
			test_state.activate_leaf(&mut handle, block_2).await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;

			let pre_check_2 = test_state.expect_candidate_precheck(&mut handle).await;

			// Resolve the PVF pre-checks out of order.
			pre_check_2.reply(PreCheckOutcome::Valid);
			pre_check_1.reply(PreCheckOutcome::Invalid);

			// Catch the vote for the second PVF.
			let vote_2 = test_state.expect_submit_vote(&mut handle).await;
			assert_eq!(vote_2.stmt.accept, true);
			assert_eq!(vote_2.stmt.subject, pvf_2.clone());
			vote_2.reply_ok();

			// Catch the vote for the first PVF.
			let vote_1 = test_state.expect_submit_vote(&mut handle).await;
			assert_eq!(vote_1.stmt.accept, false);
			assert_eq!(vote_1.stmt.subject, pvf_1.clone());
			vote_1.reply_ok();

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn dont_vote_until_a_validator() {
	test_harness(|mut test_state, mut handle| {
		async move {
			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 1, vec![dummy_validation_code_hash(1)]),
					StartsNewSession { session_index: 2, validators: vec![Sr25519Keyring::Bob] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			test_state
				.expect_candidate_precheck(&mut handle)
				.await
				.reply(PreCheckOutcome::Invalid);

			// Now a leaf brings a new session. In this session our validator comes into the active
			// set. That means it will cast a vote for each judgement it has.
			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 2, vec![dummy_validation_code_hash(1)]),
					StartsNewSession {
						session_index: 3,
						validators: vec![Sr25519Keyring::Bob, OUR_VALIDATOR],
					},
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;
			let vote = test_state.expect_submit_vote(&mut handle).await;
			assert_eq!(vote.stmt.accept, false);
			assert_eq!(vote.stmt.session_index, 3);
			assert_eq!(vote.stmt.validator_index, 1.into());
			assert_eq!(vote.stmt.subject, dummy_validation_code_hash(1));
			vote.reply_ok();

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn resign_on_session_change() {
	test_harness(|mut test_state, mut handle| {
		async move {
			let pvf_1 = dummy_validation_code_hash(1);
			let pvf_2 = dummy_validation_code_hash(2);

			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 1, vec![pvf_1, pvf_2]),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			let pre_check_1 = test_state.expect_candidate_precheck(&mut handle).await;
			assert_eq!(pre_check_1.validation_code_hash, pvf_1);
			pre_check_1.reply(PreCheckOutcome::Valid);
			let pre_check_2 = test_state.expect_candidate_precheck(&mut handle).await;
			assert_eq!(pre_check_2.validation_code_hash, pvf_2);
			pre_check_2.reply(PreCheckOutcome::Invalid);

			test_state.expect_submit_vote(&mut handle).await.reply_ok();
			test_state.expect_submit_vote(&mut handle).await.reply_ok();

			// So far so good. Now we change the session.
			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 2, vec![pvf_1, pvf_2]),
					StartsNewSession { session_index: 3, validators: vec![OUR_VALIDATOR] },
				)
				.await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			// The votes should be re-signed and re-submitted.
			let mut statements = Vec::new();
			let vote_1 = test_state.expect_submit_vote(&mut handle).await;
			statements.push(vote_1.stmt.clone());
			vote_1.reply_ok();
			let vote_2 = test_state.expect_submit_vote(&mut handle).await;
			statements.push(vote_2.stmt.clone());
			vote_2.reply_ok();

			// Find and check the votes.
			// Unfortunately, the order of revoting is not deterministic so we have to resort to
			// a bit of trickery.
			assert_eq!(statements.iter().find(|s| s.subject == pvf_1).unwrap().accept, true);
			assert_eq!(statements.iter().find(|s| s.subject == pvf_2).unwrap().accept, false);

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn dont_resign_if_not_us() {
	test_harness(|mut test_state, mut handle| {
		async move {
			let pvf_1 = dummy_validation_code_hash(1);
			let pvf_2 = dummy_validation_code_hash(2);

			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 1, vec![pvf_1, pvf_2]),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			let pre_check_1 = test_state.expect_candidate_precheck(&mut handle).await;
			assert_eq!(pre_check_1.validation_code_hash, pvf_1);
			pre_check_1.reply(PreCheckOutcome::Valid);
			let pre_check_2 = test_state.expect_candidate_precheck(&mut handle).await;
			assert_eq!(pre_check_2.validation_code_hash, pvf_2);
			pre_check_2.reply(PreCheckOutcome::Invalid);

			test_state.expect_submit_vote(&mut handle).await.reply_ok();
			test_state.expect_submit_vote(&mut handle).await.reply_ok();

			// So far so good. Now we change the session.
			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 2, vec![pvf_1, pvf_2]),
					StartsNewSession {
						session_index: 3,
						// not us
						validators: vec![Sr25519Keyring::Bob],
					},
				)
				.await;
			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			// We do not expect any votes to be re-signed.

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn api_not_supported() {
	test_harness(|mut test_state, mut handle| {
		async move {
			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 1, vec![dummy_validation_code_hash(1)]),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_not_supported();
			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn not_supported_api_becomes_supported() {
	test_harness(|mut test_state, mut handle| {
		async move {
			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 1, vec![dummy_validation_code_hash(1)]),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_not_supported();

			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 1, vec![dummy_validation_code_hash(1)]),
					StartsNewSession { session_index: 3, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;
			test_state
				.expect_candidate_precheck(&mut handle)
				.await
				.reply(PreCheckOutcome::Valid);
			test_state.expect_submit_vote(&mut handle).await.reply_ok();

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn unexpected_pvf_check_judgement() {
	test_harness(|mut test_state, mut handle| {
		async move {
			let block_1 = FakeLeaf::new(dummy_hash(), 1, vec![dummy_validation_code_hash(1)]);
			test_state
				.activate_leaf_with_session(
					&mut handle,
					block_1.clone(),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			// Catch the pre-check request, but don't reply just yet.
			let pre_check = test_state.expect_candidate_precheck(&mut handle).await;

			// Now deactive the leaf and reply to the precheck request.
			test_state.deactive_leaves(&mut handle, &[block_1.block_hash]).await;
			pre_check.reply(PreCheckOutcome::Invalid);

			// the subsystem must remain silent.

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}

#[test]
fn abstain_for_nondeterministic_pvfcheck_failure() {
	test_harness(|mut test_state, mut handle| {
		async move {
			test_state
				.activate_leaf_with_session(
					&mut handle,
					FakeLeaf::new(dummy_hash(), 1, vec![dummy_validation_code_hash(1)]),
					StartsNewSession { session_index: 2, validators: vec![OUR_VALIDATOR] },
				)
				.await;

			test_state.expect_pvfs_require_precheck(&mut handle).await.reply_mock();
			test_state.expect_session_for_child(&mut handle).await;
			test_state.expect_validators(&mut handle).await;

			test_state
				.expect_candidate_precheck(&mut handle)
				.await
				.reply(PreCheckOutcome::Failed);

			test_state.send_conclude(&mut handle).await;
		}
		.boxed()
	});
}
