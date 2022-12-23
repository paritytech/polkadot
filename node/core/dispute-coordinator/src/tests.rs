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

use std::{
	collections::HashMap,
	sync::{
		atomic::{AtomicU64, Ordering as AtomicOrdering},
		Arc,
	},
	time::Duration,
};

use assert_matches::assert_matches;
use futures::{
	channel::oneshot,
	future::{self, BoxFuture},
};

use polkadot_node_subsystem_util::database::Database;

use polkadot_node_primitives::{
	DisputeStatus, SignedDisputeStatement, SignedFullStatement, Statement,
};
use polkadot_node_subsystem::{
	messages::{
		ApprovalVotingMessage, ChainApiMessage, DisputeCoordinatorMessage,
		DisputeDistributionMessage, ImportStatementsResult,
	},
	overseer::FromOrchestra,
	OverseerSignal,
};

use polkadot_node_subsystem_util::TimeoutExt;
use sc_keystore::LocalKeystore;
use sp_application_crypto::AppKey;
use sp_core::{sr25519::Pair, testing::TaskExecutor, Pair as PairT};
use sp_keyring::Sr25519Keyring;
use sp_keystore::{SyncCryptoStore, SyncCryptoStorePtr};

use ::test_helpers::{dummy_candidate_receipt_bad_sig, dummy_digest, dummy_hash};
use polkadot_node_primitives::{Timestamp, ACTIVE_DURATION_SECS};
use polkadot_node_subsystem::{
	jaeger,
	messages::{AllMessages, BlockDescription, RuntimeApiMessage, RuntimeApiRequest},
	ActivatedLeaf, ActiveLeavesUpdate, LeafStatus,
};
use polkadot_node_subsystem_test_helpers::{
	make_buffered_subsystem_context, TestSubsystemContextHandle,
};
use polkadot_primitives::v2::{
	ApprovalVote, BlockNumber, CandidateCommitments, CandidateEvent, CandidateHash,
	CandidateReceipt, CoreIndex, DisputeStatement, GroupIndex, Hash, HeadData, Header, IndexedVec,
	MultiDisputeStatementSet, ScrapedOnChainVotes, SessionIndex, SessionInfo, SigningContext,
	ValidDisputeStatementKind, ValidatorId, ValidatorIndex, ValidatorSignature,
};

use crate::{
	backend::Backend,
	metrics::Metrics,
	participation::{participation_full_happy_path, participation_missing_availability},
	status::Clock,
	Config, DisputeCoordinatorSubsystem,
};

use super::db::v1::DbBackend;

const TEST_TIMEOUT: Duration = Duration::from_secs(2);

// sets up a keystore with the given keyring accounts.
fn make_keystore(seeds: impl Iterator<Item = String>) -> LocalKeystore {
	let store = LocalKeystore::in_memory();

	for s in seeds {
		store
			.sr25519_generate_new(polkadot_primitives::v2::PARACHAIN_KEY_TYPE_ID, Some(&s))
			.unwrap();
	}

	store
}

type VirtualOverseer = TestSubsystemContextHandle<DisputeCoordinatorMessage>;

const OVERSEER_RECEIVE_TIMEOUT: Duration = Duration::from_secs(2);

async fn overseer_recv(virtual_overseer: &mut VirtualOverseer) -> AllMessages {
	virtual_overseer
		.recv()
		.timeout(OVERSEER_RECEIVE_TIMEOUT)
		.await
		.expect("overseer `recv` timed out")
}

enum VoteType {
	Backing,
	Explicit,
}

/// Helper to condense repeated code that creates vote pairs, one valid and one
/// invalid. Optionally the valid vote of the pair can be made a backing vote.
async fn generate_opposing_votes_pair(
	test_state: &TestState,
	valid_voter_idx: ValidatorIndex,
	invalid_voter_idx: ValidatorIndex,
	candidate_hash: CandidateHash,
	session: SessionIndex,
	valid_vote_type: VoteType,
) -> (SignedDisputeStatement, SignedDisputeStatement) {
	let valid_vote = match valid_vote_type {
		VoteType::Backing =>
			test_state
				.issue_backing_statement_with_index(valid_voter_idx, candidate_hash, session)
				.await,
		VoteType::Explicit =>
			test_state
				.issue_explicit_statement_with_index(valid_voter_idx, candidate_hash, session, true)
				.await,
	};
	let invalid_vote = test_state
		.issue_explicit_statement_with_index(invalid_voter_idx, candidate_hash, session, false)
		.await;

	(valid_vote, invalid_vote)
}

#[derive(Clone)]
struct MockClock {
	time: Arc<AtomicU64>,
}

impl Default for MockClock {
	fn default() -> Self {
		MockClock { time: Arc::new(AtomicU64::default()) }
	}
}

impl Clock for MockClock {
	fn now(&self) -> Timestamp {
		self.time.load(AtomicOrdering::SeqCst)
	}
}

impl MockClock {
	fn set(&self, to: Timestamp) {
		self.time.store(to, AtomicOrdering::SeqCst)
	}
}

struct TestState {
	validators: Vec<Pair>,
	validator_public: IndexedVec<ValidatorIndex, ValidatorId>,
	validator_groups: IndexedVec<GroupIndex, Vec<ValidatorIndex>>,
	master_keystore: Arc<sc_keystore::LocalKeystore>,
	subsystem_keystore: Arc<sc_keystore::LocalKeystore>,
	db: Arc<dyn Database>,
	config: Config,
	clock: MockClock,
	headers: HashMap<Hash, Header>,
	last_block: Hash,
	// last session the subsystem knows about.
	known_session: Option<SessionIndex>,
}

impl Default for TestState {
	fn default() -> TestState {
		let p1 = Pair::from_string("//Polka", None).unwrap();
		let p2 = Pair::from_string("//Dot", None).unwrap();
		let p3 = Pair::from_string("//Kusama", None).unwrap();
		let validators = vec![
			(Sr25519Keyring::Alice.pair(), Sr25519Keyring::Alice.to_seed()),
			(Sr25519Keyring::Bob.pair(), Sr25519Keyring::Bob.to_seed()),
			(Sr25519Keyring::Charlie.pair(), Sr25519Keyring::Charlie.to_seed()),
			(Sr25519Keyring::Dave.pair(), Sr25519Keyring::Dave.to_seed()),
			(Sr25519Keyring::Eve.pair(), Sr25519Keyring::Eve.to_seed()),
			(Sr25519Keyring::One.pair(), Sr25519Keyring::One.to_seed()),
			(Sr25519Keyring::Ferdie.pair(), Sr25519Keyring::Ferdie.to_seed()),
			// Two more keys needed so disputes are not confirmed already with only 3 statements.
			(p1, "//Polka".into()),
			(p2, "//Dot".into()),
			(p3, "//Kusama".into()),
		];

		let validator_public = validators
			.clone()
			.into_iter()
			.map(|k| ValidatorId::from(k.0.public()))
			.collect();

		let validator_groups = IndexedVec::<GroupIndex, Vec<ValidatorIndex>>::from(vec![
			vec![ValidatorIndex(0), ValidatorIndex(1)],
			vec![ValidatorIndex(2), ValidatorIndex(3)],
			vec![ValidatorIndex(4), ValidatorIndex(5), ValidatorIndex(6)],
		]);

		let master_keystore = make_keystore(validators.iter().map(|v| v.1.clone())).into();
		let subsystem_keystore =
			make_keystore(vec![Sr25519Keyring::Alice.to_seed()].into_iter()).into();

		let db = kvdb_memorydb::create(1);
		let db = polkadot_node_subsystem_util::database::kvdb_impl::DbAdapter::new(db, &[]);
		let db = Arc::new(db);
		let config = Config { col_dispute_data: 0, col_session_data: 1 };

		let genesis_header = Header {
			parent_hash: Hash::zero(),
			number: 0,
			digest: dummy_digest(),
			state_root: dummy_hash(),
			extrinsics_root: dummy_hash(),
		};
		let last_block = genesis_header.hash();

		let mut headers = HashMap::new();
		let _ = headers.insert(last_block, genesis_header.clone());

		TestState {
			validators: validators.into_iter().map(|(pair, _)| pair).collect(),
			validator_public,
			validator_groups,
			master_keystore,
			subsystem_keystore,
			db,
			config,
			clock: MockClock::default(),
			headers,
			last_block,
			known_session: None,
		}
	}
}

impl TestState {
	async fn activate_leaf_at_session(
		&mut self,
		virtual_overseer: &mut VirtualOverseer,
		session: SessionIndex,
		block_number: BlockNumber,
		candidate_events: Vec<CandidateEvent>,
	) {
		assert!(block_number > 0);

		let block_header = Header {
			parent_hash: self.last_block,
			number: block_number,
			digest: dummy_digest(),
			state_root: dummy_hash(),
			extrinsics_root: dummy_hash(),
		};
		let block_hash = block_header.hash();

		let _ = self.headers.insert(block_hash, block_header.clone());
		self.last_block = block_hash;

		gum::debug!(?block_number, "Activating block in activate_leaf_at_session.");
		virtual_overseer
			.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
				ActiveLeavesUpdate::start_work(ActivatedLeaf {
					hash: block_hash,
					span: Arc::new(jaeger::Span::Disabled),
					number: block_number,
					status: LeafStatus::Fresh,
				}),
			)))
			.await;

		self.handle_sync_queries(
			virtual_overseer,
			block_hash,
			block_number,
			session,
			candidate_events,
		)
		.await;
	}

	async fn handle_sync_queries(
		&mut self,
		virtual_overseer: &mut VirtualOverseer,
		block_hash: Hash,
		block_number: BlockNumber,
		session: SessionIndex,
		candidate_events: Vec<CandidateEvent>,
	) {
		// Order of messages is not fixed (different on initializing):
		#[derive(Debug)]
		struct FinishedSteps {
			got_session_information: bool,
			got_scraping_information: bool,
		}

		impl FinishedSteps {
			fn new() -> Self {
				Self { got_session_information: false, got_scraping_information: false }
			}
			fn is_done(&self) -> bool {
				self.got_session_information && self.got_scraping_information
			}
		}

		let mut finished_steps = FinishedSteps::new();

		while !finished_steps.is_done() {
			let recv = overseer_recv(virtual_overseer).await;
			match recv {
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(tx),
				)) => {
					assert!(
						!finished_steps.got_session_information,
						"session infos already retrieved"
					);
					finished_steps.got_session_information = true;
					assert_eq!(h, block_hash);
					let _ = tx.send(Ok(session));

					// Queries for fetching earliest unfinalized block session. See `RollingSessionWindow`.
					if self.known_session.is_none() {
						assert_matches!(
							overseer_recv(virtual_overseer).await,
							AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(
								s_tx,
							)) => {
								let _ = s_tx.send(Ok(block_number));
							}
						);

						assert_matches!(
							overseer_recv(virtual_overseer).await,
							AllMessages::ChainApi(ChainApiMessage::FinalizedBlockHash(
								number,
								s_tx,
							)) => {
								assert_eq!(block_number, number);
								let _ = s_tx.send(Ok(Some(block_hash)));
							}
						);

						assert_matches!(
							overseer_recv(virtual_overseer).await,
							AllMessages::RuntimeApi(RuntimeApiMessage::Request(
								h,
								RuntimeApiRequest::SessionIndexForChild(s_tx),
							)) => {
								assert_eq!(h, block_hash);
								let _ = s_tx.send(Ok(session));
							}
						);
					}

					// No queries, if subsystem knows about this session already.
					if self.known_session == Some(session) {
						continue
					}
					self.known_session = Some(session);

					loop {
						// answer session info queries until the current session is reached.
						assert_matches!(
						overseer_recv(virtual_overseer).await,
						AllMessages::RuntimeApi(RuntimeApiMessage::Request(
								h,
								RuntimeApiRequest::SessionInfo(session_index, tx),
								)) => {
							assert_eq!(h, block_hash);

							let _ = tx.send(Ok(Some(self.session_info())));
							if session_index == session { break }
						}
						);
					}
				},
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(tx)) => {
					assert!(
						!finished_steps.got_scraping_information,
						"Scraping info was already retrieved!"
					);
					finished_steps.got_scraping_information = true;
					tx.send(Ok(0)).unwrap();
					assert_matches!(
					overseer_recv(virtual_overseer).await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
							_new_leaf,
							RuntimeApiRequest::CandidateEvents(tx),
							)) => {
						tx.send(Ok(candidate_events.clone())).unwrap();
					}
					);
					gum::trace!("After answering runtime api request");
					assert_matches!(
					overseer_recv(virtual_overseer).await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
							_new_leaf,
							RuntimeApiRequest::FetchOnChainVotes(tx),
							)) => {
						//add some `BackedCandidates` or resolved disputes here as needed
						tx.send(Ok(Some(ScrapedOnChainVotes {
							session,
							backing_validators_per_candidate: Vec::default(),
							disputes: MultiDisputeStatementSet::default(),
						}))).unwrap();
					}
					);
					gum::trace!("After answering runtime API request (votes)");
				},
				AllMessages::ChainApi(ChainApiMessage::BlockNumber(hash, tx)) => {
					let block_num = self.headers.get(&hash).map(|header| header.number);
					tx.send(Ok(block_num)).unwrap();
				},
				msg => {
					panic!("Received unexpected message in `handle_sync_queries`: {:?}", msg);
				},
			}
		}
	}

	async fn handle_resume_sync(
		&mut self,
		virtual_overseer: &mut VirtualOverseer,
		session: SessionIndex,
	) {
		let leaves: Vec<Hash> = self.headers.keys().cloned().collect();
		for (n, leaf) in leaves.iter().enumerate() {
			gum::debug!(
				block_number= ?n,
				"Activating block in handle resume sync."
			);
			virtual_overseer
				.send(FromOrchestra::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: *leaf,
						number: n as u32,
						span: Arc::new(jaeger::Span::Disabled),
						status: LeafStatus::Fresh,
					}),
				)))
				.await;

			self.handle_sync_queries(
				virtual_overseer,
				*leaf,
				n as BlockNumber,
				session,
				Vec::new(),
			)
			.await;
		}
	}

	fn session_info(&self) -> SessionInfo {
		let discovery_keys = self.validators.iter().map(|k| <_>::from(k.public())).collect();

		let assignment_keys = self.validators.iter().map(|k| <_>::from(k.public())).collect();

		SessionInfo {
			validators: self.validator_public.clone(),
			discovery_keys,
			assignment_keys,
			validator_groups: self.validator_groups.clone(),
			n_cores: self.validator_groups.len() as _,
			zeroth_delay_tranche_width: 0,
			relay_vrf_modulo_samples: 1,
			n_delay_tranches: 100,
			no_show_slots: 1,
			needed_approvals: 10,
			active_validator_indices: Vec::new(),
			dispute_period: 6,
			random_seed: [0u8; 32],
		}
	}

	async fn issue_explicit_statement_with_index(
		&self,
		index: ValidatorIndex,
		candidate_hash: CandidateHash,
		session: SessionIndex,
		valid: bool,
	) -> SignedDisputeStatement {
		let public = self.validator_public.get(index).unwrap().clone();

		let keystore = self.master_keystore.clone() as SyncCryptoStorePtr;

		SignedDisputeStatement::sign_explicit(&keystore, valid, candidate_hash, session, public)
			.await
			.unwrap()
			.unwrap()
	}

	async fn issue_backing_statement_with_index(
		&self,
		index: ValidatorIndex,
		candidate_hash: CandidateHash,
		session: SessionIndex,
	) -> SignedDisputeStatement {
		let keystore = self.master_keystore.clone() as SyncCryptoStorePtr;
		let validator_id = self.validators[index.0 as usize].public().into();
		let context =
			SigningContext { session_index: session, parent_hash: Hash::repeat_byte(0xac) };

		let statement = SignedFullStatement::sign(
			&keystore,
			Statement::Valid(candidate_hash),
			&context,
			index,
			&validator_id,
		)
		.await
		.unwrap()
		.unwrap()
		.into_unchecked();

		SignedDisputeStatement::from_backing_statement(&statement, context, validator_id).unwrap()
	}

	fn issue_approval_vote_with_index(
		&self,
		index: ValidatorIndex,
		candidate_hash: CandidateHash,
		session: SessionIndex,
	) -> SignedDisputeStatement {
		let keystore = self.master_keystore.clone() as SyncCryptoStorePtr;
		let validator_id = self.validators[index.0 as usize].public();

		let payload = ApprovalVote(candidate_hash).signing_payload(session);
		let signature = SyncCryptoStore::sign_with(
			&*keystore,
			ValidatorId::ID,
			&validator_id.into(),
			&payload[..],
		)
		.ok()
		.flatten()
		.unwrap();

		SignedDisputeStatement::new_unchecked_from_trusted_source(
			DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking),
			candidate_hash,
			session,
			validator_id.into(),
			signature.try_into().unwrap(),
		)
	}

	fn resume<F>(mut self, test: F) -> Self
	where
		F: FnOnce(TestState, VirtualOverseer) -> BoxFuture<'static, TestState>,
	{
		self.known_session = None;
		let (ctx, ctx_handle) = make_buffered_subsystem_context(TaskExecutor::new(), 1);
		let subsystem = DisputeCoordinatorSubsystem::new(
			self.db.clone(),
			self.config.clone(),
			self.subsystem_keystore.clone(),
			Metrics::default(),
		);
		let backend =
			DbBackend::new(self.db.clone(), self.config.column_config(), Metrics::default());
		let subsystem_task = subsystem.run(ctx, backend, Box::new(self.clock.clone()));
		let test_task = test(self, ctx_handle);

		let (_, state) = futures::executor::block_on(future::join(subsystem_task, test_task));
		state
	}
}

fn test_harness<F>(test: F) -> TestState
where
	F: FnOnce(TestState, VirtualOverseer) -> BoxFuture<'static, TestState>,
{
	TestState::default().resume(test)
}

/// Handle participation messages.
async fn participation_with_distribution(
	virtual_overseer: &mut VirtualOverseer,
	candidate_hash: &CandidateHash,
	expected_commitments_hash: Hash,
) {
	participation_full_happy_path(virtual_overseer, expected_commitments_hash).await;
	assert_matches!(
		overseer_recv(virtual_overseer).await,
		AllMessages::DisputeDistribution(
			DisputeDistributionMessage::SendDispute(msg)
		) => {
			assert_eq!(&msg.candidate_receipt().hash(), candidate_hash);
		}
	);
}

fn make_valid_candidate_receipt() -> CandidateReceipt {
	let mut candidate_receipt = dummy_candidate_receipt_bad_sig(dummy_hash(), dummy_hash());
	candidate_receipt.commitments_hash = CandidateCommitments::default().hash();
	candidate_receipt
}

fn make_invalid_candidate_receipt() -> CandidateReceipt {
	dummy_candidate_receipt_bad_sig(Default::default(), Some(Default::default()))
}

// Generate a `CandidateBacked` event from a `CandidateReceipt`. The rest is dummy data.
fn make_candidate_backed_event(candidate_receipt: CandidateReceipt) -> CandidateEvent {
	CandidateEvent::CandidateBacked(
		candidate_receipt,
		HeadData(Vec::new()),
		CoreIndex(0),
		GroupIndex(0),
	)
}

// Generate a `CandidateIncluded` event from a `CandidateReceipt`. The rest is dummy data.
fn make_candidate_included_event(candidate_receipt: CandidateReceipt) -> CandidateEvent {
	CandidateEvent::CandidateIncluded(
		candidate_receipt,
		HeadData(Vec::new()),
		CoreIndex(0),
		GroupIndex(0),
	)
}

/// Handle request for approval votes:
pub async fn handle_approval_vote_request(
	ctx_handle: &mut VirtualOverseer,
	expected_hash: &CandidateHash,
	votes_to_send: HashMap<ValidatorIndex, ValidatorSignature>,
) {
	assert_matches!(
		ctx_handle.recv().await,
		AllMessages::ApprovalVoting(
			ApprovalVotingMessage::GetApprovalSignaturesForCandidate(hash, tx)
		) => {
			assert_eq!(&hash, expected_hash);
			tx.send(votes_to_send).unwrap();
		},
		"overseer did not receive `GetApprovalSignaturesForCandidate` message.",
	);
}

#[test]
fn too_many_unconfirmed_statements_are_considered_spam() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt1 = make_valid_candidate_receipt();
			let candidate_hash1 = candidate_receipt1.hash();
			let candidate_receipt2 = make_invalid_candidate_receipt();
			let candidate_hash2 = candidate_receipt2.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let (valid_vote1, invalid_vote1) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(3),
				ValidatorIndex(1),
				candidate_hash1,
				session,
				VoteType::Backing,
			)
			.await;

			let (valid_vote2, invalid_vote2) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(3),
				ValidatorIndex(1),
				candidate_hash2,
				session,
				VoteType::Backing,
			)
			.await;

			gum::trace!("Before sending `ImportStatements`");
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt1.clone(),
						session,
						statements: vec![
							(valid_vote1, ValidatorIndex(3)),
							(invalid_vote1, ValidatorIndex(1)),
						],
						pending_confirmation: None,
					},
				})
				.await;
			gum::trace!("After sending `ImportStatements`");

			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash1, HashMap::new())
				.await;

			// Participation has to fail here, otherwise the dispute will be confirmed. However
			// participation won't happen at all because the dispute is neither backed, not confirmed
			// nor the candidate is included. Or in other words - we'll refrain from participation.

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(
					rx.await.unwrap(),
					vec![(session, candidate_hash1, DisputeStatus::Active)]
				);

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash1)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 1);
				assert_eq!(votes.invalid.len(), 1);
			}

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt2.clone(),
						session,
						statements: vec![
							(valid_vote2, ValidatorIndex(3)),
							(invalid_vote2, ValidatorIndex(1)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;

			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash2, HashMap::new())
				.await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash2)],
							tx,
						),
					})
					.await;

				assert_matches!(rx.await.unwrap().get(0), None);
			}

			// Result should be invalid, because it should be considered spam.
			assert_matches!(confirmation_rx.await, Ok(ImportStatementsResult::InvalidImport));

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// No more messages expected:
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn approval_vote_import_works() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt1 = make_valid_candidate_receipt();
			let candidate_hash1 = candidate_receipt1.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let (valid_vote1, invalid_vote1) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(3),
				ValidatorIndex(1),
				candidate_hash1,
				session,
				VoteType::Backing,
			)
			.await;

			let approval_vote = test_state.issue_approval_vote_with_index(
				ValidatorIndex(4),
				candidate_hash1,
				session,
			);

			gum::trace!("Before sending `ImportStatements`");
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt1.clone(),
						session,
						statements: vec![
							(valid_vote1, ValidatorIndex(3)),
							(invalid_vote1, ValidatorIndex(1)),
						],
						pending_confirmation: None,
					},
				})
				.await;
			gum::trace!("After sending `ImportStatements`");

			let approval_votes = [(ValidatorIndex(4), approval_vote.into_validator_signature())]
				.into_iter()
				.collect();

			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash1, approval_votes)
				.await;

			// Participation won't happen here because the dispute is neither backed, not confirmed
			// nor the candidate is included. Or in other words - we'll refrain from participation.

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(
					rx.await.unwrap(),
					vec![(session, candidate_hash1, DisputeStatus::Active)]
				);

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash1)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 2);
				assert!(
					votes.valid.raw().get(&ValidatorIndex(4)).is_some(),
					"Approval vote is missing!"
				);
				assert_eq!(votes.invalid.len(), 1);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// No more messages expected:
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn dispute_gets_confirmed_via_participation() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt1 = make_valid_candidate_receipt();
			let candidate_hash1 = candidate_receipt1.hash();
			let candidate_receipt2 = make_invalid_candidate_receipt();
			let candidate_hash2 = candidate_receipt2.hash();

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![
						make_candidate_backed_event(candidate_receipt1.clone()),
						make_candidate_backed_event(candidate_receipt2.clone()),
					],
				)
				.await;

			let (valid_vote1, invalid_vote1) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(3),
				ValidatorIndex(1),
				candidate_hash1,
				session,
				VoteType::Explicit,
			)
			.await;

			let (valid_vote2, invalid_vote2) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(3),
				ValidatorIndex(1),
				candidate_hash2,
				session,
				VoteType::Explicit,
			)
			.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt1.clone(),
						session,
						statements: vec![
							(valid_vote1, ValidatorIndex(3)),
							(invalid_vote1, ValidatorIndex(1)),
						],
						pending_confirmation: None,
					},
				})
				.await;
			gum::debug!("After First import!");
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash1, HashMap::new())
				.await;

			participation_with_distribution(
				&mut virtual_overseer,
				&candidate_hash1,
				candidate_receipt1.commitments_hash,
			)
			.await;
			gum::debug!("After Participation!");

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(
					rx.await.unwrap(),
					vec![(session, candidate_hash1, DisputeStatus::Active)]
				);

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash1)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 2);
				assert_eq!(votes.invalid.len(), 1);
			}

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt2.clone(),
						session,
						statements: vec![
							(valid_vote2, ValidatorIndex(3)),
							(invalid_vote2, ValidatorIndex(1)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash2, HashMap::new())
				.await;

			participation_missing_availability(&mut virtual_overseer).await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash2)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 1);
				assert_eq!(votes.invalid.len(), 1);
			}

			// Result should be valid, because our node participated, so spam slots are cleared:
			assert_matches!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// No more messages expected:
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn dispute_gets_confirmed_at_byzantine_threshold() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt1 = make_valid_candidate_receipt();
			let candidate_hash1 = candidate_receipt1.hash();
			let candidate_receipt2 = make_invalid_candidate_receipt();
			let candidate_hash2 = candidate_receipt2.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let (valid_vote1, invalid_vote1) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(3),
				ValidatorIndex(1),
				candidate_hash1,
				session,
				VoteType::Explicit,
			)
			.await;

			let (valid_vote1a, invalid_vote1a) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(4),
				ValidatorIndex(5),
				candidate_hash1,
				session,
				VoteType::Explicit,
			)
			.await;

			let (valid_vote2, invalid_vote2) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(3),
				ValidatorIndex(1),
				candidate_hash2,
				session,
				VoteType::Explicit,
			)
			.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt1.clone(),
						session,
						statements: vec![
							(valid_vote1, ValidatorIndex(3)),
							(invalid_vote1, ValidatorIndex(1)),
							(valid_vote1a, ValidatorIndex(4)),
							(invalid_vote1a, ValidatorIndex(5)),
						],
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash1, HashMap::new())
				.await;

			// Participation won't happen here because the dispute is neither backed, not confirmed
			// nor the candidate is included. Or in other words - we'll refrain from participation.

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(
					rx.await.unwrap(),
					vec![(session, candidate_hash1, DisputeStatus::Confirmed)]
				);

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash1)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 2);
				assert_eq!(votes.invalid.len(), 2);
			}

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt2.clone(),
						session,
						statements: vec![
							(valid_vote2, ValidatorIndex(3)),
							(invalid_vote2, ValidatorIndex(1)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash2, HashMap::new())
				.await;

			participation_missing_availability(&mut virtual_overseer).await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash2)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 1);
				assert_eq!(votes.invalid.len(), 1);
			}

			// Result should be valid, because byzantine threshold has been reached in first
			// import, so spam slots are cleared:
			assert_matches!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// No more messages expected:
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn backing_statements_import_works_and_no_spam() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let valid_vote1 = test_state
				.issue_backing_statement_with_index(ValidatorIndex(3), candidate_hash, session)
				.await;

			let valid_vote2 = test_state
				.issue_backing_statement_with_index(ValidatorIndex(4), candidate_hash, session)
				.await;

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote1, ValidatorIndex(3)),
							(valid_vote2, ValidatorIndex(4)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;
			assert_matches!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			{
				// Just backing votes - we should not have any active disputes now.
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert!(rx.await.unwrap().is_empty());

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 2);
				assert_eq!(votes.invalid.len(), 0);
			}

			let candidate_receipt = make_invalid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			let valid_vote1 = test_state
				.issue_backing_statement_with_index(ValidatorIndex(3), candidate_hash, session)
				.await;

			let valid_vote2 = test_state
				.issue_backing_statement_with_index(ValidatorIndex(4), candidate_hash, session)
				.await;

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_backed_event(candidate_receipt.clone())],
				)
				.await;

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			// Backing vote import should not have accounted to spam slots, so this should succeed
			// as well:
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote1, ValidatorIndex(3)),
							(valid_vote2, ValidatorIndex(4)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;

			// Result should be valid, because our node participated, so spam slots are cleared:
			assert_matches!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// No more messages expected:
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn conflicting_votes_lead_to_dispute_participation() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_backed_event(candidate_receipt.clone())],
				)
				.await;

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(3),
				ValidatorIndex(1),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			let invalid_vote_2 = test_state
				.issue_explicit_statement_with_index(
					ValidatorIndex(2),
					candidate_hash,
					session,
					false,
				)
				.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(3)),
							(invalid_vote, ValidatorIndex(1)),
						],
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			participation_with_distribution(
				&mut virtual_overseer,
				&candidate_hash,
				candidate_receipt.commitments_hash,
			)
			.await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(
					rx.await.unwrap(),
					vec![(session, candidate_hash, DisputeStatus::Active)]
				);

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 2);
				assert_eq!(votes.invalid.len(), 1);
			}

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![(invalid_vote_2, ValidatorIndex(2))],
						pending_confirmation: None,
					},
				})
				.await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 2);
				assert_eq!(votes.invalid.len(), 2);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// This confirms that the second vote doesn't lead to participation again.
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn positive_votes_dont_trigger_participation() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_backed_event(candidate_receipt.clone())],
				)
				.await;

			let valid_vote = test_state
				.issue_explicit_statement_with_index(
					ValidatorIndex(2),
					candidate_hash,
					session,
					true,
				)
				.await;

			let valid_vote_2 = test_state
				.issue_explicit_statement_with_index(
					ValidatorIndex(1),
					candidate_hash,
					session,
					true,
				)
				.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![(valid_vote, ValidatorIndex(2))],
						pending_confirmation: None,
					},
				})
				.await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert!(rx.await.unwrap().is_empty());

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 1);
				assert!(votes.invalid.is_empty());
			}

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![(valid_vote_2, ValidatorIndex(1))],
						pending_confirmation: None,
					},
				})
				.await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert!(rx.await.unwrap().is_empty());

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 2);
				assert!(votes.invalid.is_empty());
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// This confirms that no participation request is made.
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn wrong_validator_index_is_ignored() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(2),
				ValidatorIndex(1),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(1)),
							(invalid_vote, ValidatorIndex(2)),
						],
						pending_confirmation: None,
					},
				})
				.await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert!(rx.await.unwrap().is_empty());

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash)],
							tx,
						),
					})
					.await;

				assert_matches!(rx.await.unwrap().get(0), None);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// This confirms that no participation request is made.
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn finality_votes_ignore_disputed_candidates() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_backed_event(candidate_receipt.clone())],
				)
				.await;

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(2),
				ValidatorIndex(1),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(2)),
							(invalid_vote, ValidatorIndex(1)),
						],
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			participation_with_distribution(
				&mut virtual_overseer,
				&candidate_hash,
				candidate_receipt.commitments_hash,
			)
			.await;

			{
				let (tx, rx) = oneshot::channel();

				let base_block = Hash::repeat_byte(0x0f);
				let block_hash_a = Hash::repeat_byte(0x0a);
				let block_hash_b = Hash::repeat_byte(0x0b);

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::DetermineUndisputedChain {
							base: (10, base_block),
							block_descriptions: vec![BlockDescription {
								block_hash: block_hash_a,
								session,
								candidates: vec![candidate_hash],
							}],
							tx,
						},
					})
					.await;

				assert_eq!(rx.await.unwrap(), (10, base_block));

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::DetermineUndisputedChain {
							base: (10, base_block),
							block_descriptions: vec![
								BlockDescription {
									block_hash: block_hash_a,
									session,
									candidates: vec![],
								},
								BlockDescription {
									block_hash: block_hash_b,
									session,
									candidates: vec![candidate_hash],
								},
							],
							tx,
						},
					})
					.await;

				assert_eq!(rx.await.unwrap(), (11, block_hash_a));
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn supermajority_valid_dispute_may_be_finalized() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_backed_event(candidate_receipt.clone())],
				)
				.await;

			let supermajority_threshold =
				polkadot_primitives::v2::supermajority_threshold(test_state.validators.len());

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(2),
				ValidatorIndex(1),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(2)),
							(invalid_vote, ValidatorIndex(1)),
						],
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			participation_with_distribution(
				&mut virtual_overseer,
				&candidate_hash,
				candidate_receipt.commitments_hash,
			)
			.await;

			let mut statements = Vec::new();
			for i in (0_u32..supermajority_threshold as u32 - 1).map(|i| i + 3) {
				let vote = test_state
					.issue_explicit_statement_with_index(
						ValidatorIndex(i),
						candidate_hash,
						session,
						true,
					)
					.await;

				statements.push((vote, ValidatorIndex(i as _)));
			}

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements,
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			{
				let (tx, rx) = oneshot::channel();

				let base_hash = Hash::repeat_byte(0x0f);
				let block_hash_a = Hash::repeat_byte(0x0a);
				let block_hash_b = Hash::repeat_byte(0x0b);

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::DetermineUndisputedChain {
							base: (10, base_hash),
							block_descriptions: vec![BlockDescription {
								block_hash: block_hash_a,
								session,
								candidates: vec![candidate_hash],
							}],
							tx,
						},
					})
					.await;

				assert_eq!(rx.await.unwrap(), (11, block_hash_a));

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::DetermineUndisputedChain {
							base: (10, base_hash),
							block_descriptions: vec![
								BlockDescription {
									block_hash: block_hash_a,
									session,
									candidates: vec![],
								},
								BlockDescription {
									block_hash: block_hash_b,
									session,
									candidates: vec![candidate_hash],
								},
							],
							tx,
						},
					})
					.await;

				assert_eq!(rx.await.unwrap(), (12, block_hash_b));
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn concluded_supermajority_for_non_active_after_time() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_backed_event(candidate_receipt.clone())],
				)
				.await;

			let supermajority_threshold =
				polkadot_primitives::v2::supermajority_threshold(test_state.validators.len());

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(2),
				ValidatorIndex(1),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(2)),
							(invalid_vote, ValidatorIndex(1)),
						],
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			participation_with_distribution(
				&mut virtual_overseer,
				&candidate_hash,
				candidate_receipt.commitments_hash,
			)
			.await;

			let mut statements = Vec::new();
			// -2: 1 for already imported vote and one for local vote (which is valid).
			for i in (0_u32..supermajority_threshold as u32 - 2).map(|i| i + 3) {
				let vote = test_state
					.issue_explicit_statement_with_index(
						ValidatorIndex(i),
						candidate_hash,
						session,
						true,
					)
					.await;

				statements.push((vote, ValidatorIndex(i as _)));
			}

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements,
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			test_state.clock.set(ACTIVE_DURATION_SECS + 1);

			{
				let (tx, rx) = oneshot::channel();

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert!(rx.await.unwrap().is_empty());

				let (tx, rx) = oneshot::channel();

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::RecentDisputes(tx),
					})
					.await;

				assert_eq!(rx.await.unwrap().len(), 1);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn concluded_supermajority_against_non_active_after_time() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_invalid_candidate_receipt();

			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_backed_event(candidate_receipt.clone())],
				)
				.await;

			let supermajority_threshold =
				polkadot_primitives::v2::supermajority_threshold(test_state.validators.len());

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(2),
				ValidatorIndex(1),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(2)),
							(invalid_vote, ValidatorIndex(1)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;
			assert_matches!(confirmation_rx.await.unwrap(),
				ImportStatementsResult::ValidImport => {}
			);

			// Use a different expected commitments hash to ensure the candidate validation returns invalid.
			participation_with_distribution(
				&mut virtual_overseer,
				&candidate_hash,
				CandidateCommitments::default().hash(),
			)
			.await;

			let mut statements = Vec::new();
			// minus 2, because of local vote and one previously imported invalid vote.
			for i in (0_u32..supermajority_threshold as u32 - 2).map(|i| i + 3) {
				let vote = test_state
					.issue_explicit_statement_with_index(
						ValidatorIndex(i),
						candidate_hash,
						session,
						false,
					)
					.await;

				statements.push((vote, ValidatorIndex(i as _)));
			}

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements,
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			test_state.clock.set(ACTIVE_DURATION_SECS + 1);

			{
				let (tx, rx) = oneshot::channel();

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert!(rx.await.unwrap().is_empty());
				let (tx, rx) = oneshot::channel();

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::RecentDisputes(tx),
					})
					.await;

				assert_eq!(rx.await.unwrap().len(), 1);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert_matches!(
				virtual_overseer.try_recv().await,
				None => {}
			);

			test_state
		})
	});
}

#[test]
fn resume_dispute_without_local_statement() {
	let session = 1;

	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(1),
				ValidatorIndex(2),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(1)),
							(invalid_vote, ValidatorIndex(2)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			// Missing availability -> No local vote.
			// Participation won't happen here because the dispute is neither backed, not confirmed
			// nor the candidate is included. Or in other words - we'll refrain from participation.

			assert_eq!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			{
				let (tx, rx) = oneshot::channel();

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(rx.await.unwrap().len(), 1);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	})
	// Alice should send a DisputeParticiationMessage::Participate on restart since she has no
	// local statement for the active dispute.
	.resume(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			participation_with_distribution(
				&mut virtual_overseer,
				&candidate_hash,
				candidate_receipt.commitments_hash,
			)
			.await;

			let mut statements = Vec::new();
			// Getting votes for supermajority. Should already have two valid votes.
			for i in vec![3, 4, 5, 6, 7] {
				let vote = test_state
					.issue_explicit_statement_with_index(
						ValidatorIndex(i),
						candidate_hash,
						session,
						true,
					)
					.await;

				statements.push((vote, ValidatorIndex(i as _)));
			}

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements,
						pending_confirmation: None,
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			// Advance the clock far enough so that the concluded dispute will be omitted from an
			// ActiveDisputes query.
			test_state.clock.set(test_state.clock.now() + ACTIVE_DURATION_SECS + 1);

			{
				let (tx, rx) = oneshot::channel();

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert!(rx.await.unwrap().is_empty());
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn resume_dispute_with_local_statement() {
	let session = 1;

	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_backed_event(candidate_receipt.clone())],
				)
				.await;

			let local_valid_vote = test_state
				.issue_explicit_statement_with_index(
					ValidatorIndex(0),
					candidate_hash,
					session,
					true,
				)
				.await;

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(1),
				ValidatorIndex(2),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(local_valid_vote, ValidatorIndex(0)),
							(valid_vote, ValidatorIndex(1)),
							(invalid_vote, ValidatorIndex(2)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;

			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			assert_eq!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			{
				let (tx, rx) = oneshot::channel();

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(rx.await.unwrap().len(), 1);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	})
	// Alice should not send a DisputeParticiationMessage::Participate on restart since she has a
	// local statement for the active dispute.
	.resume(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			// Assert that subsystem is not sending Participation messages because we issued a local statement
			assert!(virtual_overseer.recv().timeout(TEST_TIMEOUT).await.is_none());

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn resume_dispute_without_local_statement_or_local_key() {
	let session = 1;
	let mut test_state = TestState::default();
	test_state.subsystem_keystore =
		make_keystore(vec![Sr25519Keyring::Two.to_seed()].into_iter()).into();
	test_state
		.resume(|mut test_state, mut virtual_overseer| {
			Box::pin(async move {
				test_state.handle_resume_sync(&mut virtual_overseer, session).await;

				let candidate_receipt = make_valid_candidate_receipt();
				let candidate_hash = candidate_receipt.hash();

				test_state
					.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
					.await;

				let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
					&test_state,
					ValidatorIndex(1),
					ValidatorIndex(2),
					candidate_hash,
					session,
					VoteType::Explicit,
				)
				.await;

				let (pending_confirmation, confirmation_rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ImportStatements {
							candidate_receipt: candidate_receipt.clone(),
							session,
							statements: vec![
								(valid_vote, ValidatorIndex(1)),
								(invalid_vote, ValidatorIndex(2)),
							],
							pending_confirmation: Some(pending_confirmation),
						},
					})
					.await;
				handle_approval_vote_request(
					&mut virtual_overseer,
					&candidate_hash,
					HashMap::new(),
				)
				.await;

				assert_eq!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

				{
					let (tx, rx) = oneshot::channel();

					virtual_overseer
						.send(FromOrchestra::Communication {
							msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
						})
						.await;

					assert_eq!(rx.await.unwrap().len(), 1);
				}

				virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
				assert_matches!(
					virtual_overseer.try_recv().await,
					None => {}
				);

				test_state
			})
		})
		// Two should not send a DisputeParticiationMessage::Participate on restart since she is no
		// validator in that dispute.
		.resume(|mut test_state, mut virtual_overseer| {
			Box::pin(async move {
				test_state.handle_resume_sync(&mut virtual_overseer, session).await;

				// Assert that subsystem is not sending Participation messages because we issued a local statement
				assert!(virtual_overseer.recv().timeout(TEST_TIMEOUT).await.is_none());

				virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
				assert!(virtual_overseer.try_recv().await.is_none());

				test_state
			})
		});
}

#[test]
fn resume_dispute_with_local_statement_without_local_key() {
	let session = 1;

	let test_state = TestState::default();
	let mut test_state = test_state.resume(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let local_valid_vote = test_state
				.issue_explicit_statement_with_index(
					ValidatorIndex(0),
					candidate_hash,
					session,
					true,
				)
				.await;

			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(1),
				ValidatorIndex(2),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(local_valid_vote, ValidatorIndex(0)),
							(valid_vote, ValidatorIndex(1)),
							(invalid_vote, ValidatorIndex(2)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			assert_eq!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			{
				let (tx, rx) = oneshot::channel();

				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(rx.await.unwrap().len(), 1);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});

	// No keys:
	test_state.subsystem_keystore =
		make_keystore(vec![Sr25519Keyring::Two.to_seed()].into_iter()).into();
	// Two should not send a DisputeParticiationMessage::Participate on restart since we gave
	// her a non existing key.
	test_state.resume(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			// Assert that subsystem is not sending Participation messages because we don't
			// have a key.
			assert!(virtual_overseer.recv().timeout(TEST_TIMEOUT).await.is_none());

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn issue_valid_local_statement_does_cause_distribution_but_not_duplicate_participation() {
	issue_local_statement_does_cause_distribution_but_not_duplicate_participation(true);
}

#[test]
fn issue_invalid_local_statement_does_cause_distribution_but_not_duplicate_participation() {
	issue_local_statement_does_cause_distribution_but_not_duplicate_participation(false);
}

fn issue_local_statement_does_cause_distribution_but_not_duplicate_participation(validity: bool) {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let other_vote = test_state
				.issue_explicit_statement_with_index(
					ValidatorIndex(1),
					candidate_hash,
					session,
					!validity,
				)
				.await;

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![(other_vote, ValidatorIndex(1))],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;

			assert_eq!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			// Initiate dispute locally:
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::IssueLocalStatement(
						session,
						candidate_hash,
						candidate_receipt.clone(),
						validity,
					),
				})
				.await;

			// Dispute distribution should get notified now:
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::DisputeDistribution(
					DisputeDistributionMessage::SendDispute(msg)
				) => {
					assert_eq!(msg.session_index(), session);
					assert_eq!(msg.candidate_receipt(), &candidate_receipt);
				}
			);

			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			// Make sure we won't participate:
			assert!(virtual_overseer.recv().timeout(TEST_TIMEOUT).await.is_none());

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn own_approval_vote_gets_distributed_on_dispute() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let statement = test_state.issue_approval_vote_with_index(
				ValidatorIndex(0),
				candidate_hash,
				session,
			);

			// Import our approval vote:
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![(statement, ValidatorIndex(0))],
						pending_confirmation: None,
					},
				})
				.await;

			// Trigger dispute:
			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(2),
				ValidatorIndex(1),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(invalid_vote, ValidatorIndex(1)),
							(valid_vote, ValidatorIndex(2)),
						],
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;
			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			assert_eq!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			// Dispute distribution should get notified now (without participation, as we already
			// have an approval vote):
			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::DisputeDistribution(
					DisputeDistributionMessage::SendDispute(msg)
				) => {
					assert_eq!(msg.session_index(), session);
					assert_eq!(msg.candidate_receipt(), &candidate_receipt);
				}
			);

			// No participation should occur:
			assert_matches!(virtual_overseer.recv().timeout(TEST_TIMEOUT).await, None);

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn negative_issue_local_statement_only_triggers_import() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_invalid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::IssueLocalStatement(
						session,
						candidate_hash,
						candidate_receipt.clone(),
						false,
					),
				})
				.await;

			// Assert that subsystem is not participating.
			assert!(virtual_overseer.recv().timeout(TEST_TIMEOUT).await.is_none());

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			let backend = DbBackend::new(
				test_state.db.clone(),
				test_state.config.column_config(),
				Metrics::default(),
			);

			let votes = backend.load_candidate_votes(session, &candidate_hash).unwrap().unwrap();
			assert_eq!(votes.invalid.len(), 1);
			assert_eq!(votes.valid.len(), 0);

			let disputes = backend.load_recent_disputes().unwrap();
			assert_eq!(disputes, None);

			test_state
		})
	});
}

#[test]
fn redundant_votes_ignored() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let valid_vote = test_state
				.issue_backing_statement_with_index(ValidatorIndex(1), candidate_hash, session)
				.await;

			let valid_vote_2 = test_state
				.issue_backing_statement_with_index(ValidatorIndex(1), candidate_hash, session)
				.await;

			assert!(valid_vote.validator_signature() != valid_vote_2.validator_signature());

			let (tx, rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![(valid_vote.clone(), ValidatorIndex(1))],
						pending_confirmation: Some(tx),
					},
				})
				.await;

			rx.await.unwrap();

			let (tx, rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![(valid_vote_2, ValidatorIndex(1))],
						pending_confirmation: Some(tx),
					},
				})
				.await;

			rx.await.unwrap();

			let backend = DbBackend::new(
				test_state.db.clone(),
				test_state.config.column_config(),
				Metrics::default(),
			);

			let votes = backend.load_candidate_votes(session, &candidate_hash).unwrap().unwrap();
			assert_eq!(votes.invalid.len(), 0);
			assert_eq!(votes.valid.len(), 1);
			assert_eq!(&votes.valid[0].2, valid_vote.validator_signature());

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
/// Make sure no disputes are recorded when there are no opposing votes, even if we reached supermajority.
fn no_onesided_disputes() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();
			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			let mut statements = Vec::new();
			for index in 1..10 {
				statements.push((
					test_state
						.issue_backing_statement_with_index(
							ValidatorIndex(index),
							candidate_hash,
							session,
						)
						.await,
					ValidatorIndex(index),
				));
			}

			let (pending_confirmation, confirmation_rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements,
						pending_confirmation: Some(pending_confirmation),
					},
				})
				.await;
			assert_matches!(confirmation_rx.await, Ok(ImportStatementsResult::ValidImport));

			// We should not have any active disputes now.
			let (tx, rx) = oneshot::channel();
			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
				})
				.await;

			assert!(rx.await.unwrap().is_empty());

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// No more messages expected:
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

#[test]
fn refrain_from_participation() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			// activate leaf - no backing/included event
			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			// generate two votes
			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(1),
				ValidatorIndex(2),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(1)),
							(invalid_vote, ValidatorIndex(2)),
						],
						pending_confirmation: None,
					},
				})
				.await;

			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(rx.await.unwrap().len(), 1);

				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 1);
				assert_eq!(votes.invalid.len(), 1);
			}

			// activate leaf - no backing event
			test_state
				.activate_leaf_at_session(&mut virtual_overseer, session, 1, Vec::new())
				.await;

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			// confirm that no participation request is made.
			assert!(virtual_overseer.try_recv().await.is_none());

			test_state
		})
	});
}

/// We have got no `participation_for_backed_candidates` test because most of the other tests (e.g.
/// `dispute_gets_confirmed_via_participation`, `backing_statements_import_works_and_no_spam`) use
/// candidate backing event to trigger participation. If they pass - that case works.
#[test]
fn participation_for_included_candidates() {
	test_harness(|mut test_state, mut virtual_overseer| {
		Box::pin(async move {
			let session = 1;

			test_state.handle_resume_sync(&mut virtual_overseer, session).await;

			let candidate_receipt = make_valid_candidate_receipt();
			let candidate_hash = candidate_receipt.hash();

			// activate leaf - with candidate included event
			test_state
				.activate_leaf_at_session(
					&mut virtual_overseer,
					session,
					1,
					vec![make_candidate_included_event(candidate_receipt.clone())],
				)
				.await;

			// generate two votes
			let (valid_vote, invalid_vote) = generate_opposing_votes_pair(
				&test_state,
				ValidatorIndex(1),
				ValidatorIndex(2),
				candidate_hash,
				session,
				VoteType::Explicit,
			)
			.await;

			virtual_overseer
				.send(FromOrchestra::Communication {
					msg: DisputeCoordinatorMessage::ImportStatements {
						candidate_receipt: candidate_receipt.clone(),
						session,
						statements: vec![
							(valid_vote, ValidatorIndex(1)),
							(invalid_vote, ValidatorIndex(2)),
						],
						pending_confirmation: None,
					},
				})
				.await;

			handle_approval_vote_request(&mut virtual_overseer, &candidate_hash, HashMap::new())
				.await;

			participation_with_distribution(
				&mut virtual_overseer,
				&candidate_hash,
				candidate_receipt.commitments_hash,
			)
			.await;

			{
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::ActiveDisputes(tx),
					})
					.await;

				assert_eq!(rx.await.unwrap().len(), 1);

				// check if we have participated (casted a vote)
				let (tx, rx) = oneshot::channel();
				virtual_overseer
					.send(FromOrchestra::Communication {
						msg: DisputeCoordinatorMessage::QueryCandidateVotes(
							vec![(session, candidate_hash)],
							tx,
						),
					})
					.await;

				let (_, _, votes) = rx.await.unwrap().get(0).unwrap().clone();
				assert_eq!(votes.valid.raw().len(), 2); // 2 => we have participated
				assert_eq!(votes.invalid.len(), 1);
			}

			virtual_overseer.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;

			test_state
		})
	});
}
