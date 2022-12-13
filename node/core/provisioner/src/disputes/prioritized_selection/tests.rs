// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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

use super::super::{
	super::{tests::common::test_harness, *},
	prioritized_selection::*,
};
use bitvec::prelude::*;
use futures::channel::mpsc;
use polkadot_node_primitives::{CandidateVotes, DisputeStatus, ACTIVE_DURATION_SECS};
use polkadot_node_subsystem::messages::{
	AllMessages, DisputeCoordinatorMessage, RuntimeApiMessage, RuntimeApiRequest,
};
use polkadot_node_subsystem_test_helpers::TestSubsystemSender;
use polkadot_primitives::v2::{
	CandidateHash, DisputeState, InvalidDisputeStatementKind, SessionIndex,
	ValidDisputeStatementKind, ValidatorSignature,
};
use std::sync::Arc;
use test_helpers;

//
// Unit tests for various functions
//
#[test]
fn should_keep_vote_behaves() {
	let onchain_state = DisputeState {
		validators_for: bitvec![u8, Lsb0; 1, 0, 1, 0, 1],
		validators_against: bitvec![u8, Lsb0; 0, 1, 0, 0, 1],
		start: 1,
		concluded_at: None,
	};

	let local_valid_known = (ValidatorIndex(0), ValidDisputeStatementKind::Explicit);
	let local_valid_unknown = (ValidatorIndex(3), ValidDisputeStatementKind::Explicit);

	let local_invalid_known = (ValidatorIndex(1), InvalidDisputeStatementKind::Explicit);
	let local_invalid_unknown = (ValidatorIndex(3), InvalidDisputeStatementKind::Explicit);

	assert_eq!(
		is_vote_worth_to_keep(
			&local_valid_known.0,
			DisputeStatement::Valid(local_valid_known.1),
			&onchain_state
		),
		false
	);
	assert_eq!(
		is_vote_worth_to_keep(
			&local_valid_unknown.0,
			DisputeStatement::Valid(local_valid_unknown.1),
			&onchain_state
		),
		true
	);
	assert_eq!(
		is_vote_worth_to_keep(
			&local_invalid_known.0,
			DisputeStatement::Invalid(local_invalid_known.1),
			&onchain_state
		),
		false
	);
	assert_eq!(
		is_vote_worth_to_keep(
			&local_invalid_unknown.0,
			DisputeStatement::Invalid(local_invalid_unknown.1),
			&onchain_state
		),
		true
	);

	//double voting - onchain knows
	let local_double_vote_onchain_knows =
		(ValidatorIndex(4), InvalidDisputeStatementKind::Explicit);
	assert_eq!(
		is_vote_worth_to_keep(
			&local_double_vote_onchain_knows.0,
			DisputeStatement::Invalid(local_double_vote_onchain_knows.1),
			&onchain_state
		),
		false
	);

	//double voting - onchain doesn't know
	let local_double_vote_onchain_doesnt_knows =
		(ValidatorIndex(0), InvalidDisputeStatementKind::Explicit);
	assert_eq!(
		is_vote_worth_to_keep(
			&local_double_vote_onchain_doesnt_knows.0,
			DisputeStatement::Invalid(local_double_vote_onchain_doesnt_knows.1),
			&onchain_state
		),
		true
	);

	// empty onchain state
	let empty_onchain_state = DisputeState {
		validators_for: BitVec::new(),
		validators_against: BitVec::new(),
		start: 1,
		concluded_at: None,
	};
	assert_eq!(
		is_vote_worth_to_keep(
			&local_double_vote_onchain_doesnt_knows.0,
			DisputeStatement::Invalid(local_double_vote_onchain_doesnt_knows.1),
			&empty_onchain_state
		),
		true
	);
}

#[test]
fn partitioning_happy_case() {
	let mut input = Vec::<(SessionIndex, CandidateHash, DisputeStatus)>::new();
	let mut onchain = HashMap::<(u32, CandidateHash), DisputeState>::new();
	let time_now = secs_since_epoch();

	// Create one dispute for each partition
	let inactive_unknown_onchain = (
		0,
		CandidateHash(Hash::random()),
		DisputeStatus::ConcludedFor(time_now - ACTIVE_DURATION_SECS * 2),
	);
	input.push(inactive_unknown_onchain.clone());

	let inactive_unconcluded_onchain = (
		1,
		CandidateHash(Hash::random()),
		DisputeStatus::ConcludedFor(time_now - ACTIVE_DURATION_SECS * 2),
	);
	input.push(inactive_unconcluded_onchain.clone());
	onchain.insert(
		(inactive_unconcluded_onchain.0, inactive_unconcluded_onchain.1.clone()),
		DisputeState {
			validators_for: bitvec![u8, Lsb0; 1, 1, 1, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, Lsb0; 0, 0, 0, 0, 0, 0, 0, 0, 0],
			start: 1,
			concluded_at: None,
		},
	);

	let active_unknown_onchain = (2, CandidateHash(Hash::random()), DisputeStatus::Active);
	input.push(active_unknown_onchain.clone());

	let active_unconcluded_onchain = (3, CandidateHash(Hash::random()), DisputeStatus::Active);
	input.push(active_unconcluded_onchain.clone());
	onchain.insert(
		(active_unconcluded_onchain.0, active_unconcluded_onchain.1.clone()),
		DisputeState {
			validators_for: bitvec![u8, Lsb0; 1, 1, 1, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, Lsb0; 0, 0, 0, 0, 0, 0, 0, 0, 0],
			start: 1,
			concluded_at: None,
		},
	);

	let active_concluded_onchain = (4, CandidateHash(Hash::random()), DisputeStatus::Active);
	input.push(active_concluded_onchain.clone());
	onchain.insert(
		(active_concluded_onchain.0, active_concluded_onchain.1.clone()),
		DisputeState {
			validators_for: bitvec![u8, Lsb0; 1, 1, 1, 1, 1, 1, 1, 1, 0],
			validators_against: bitvec![u8, Lsb0; 0, 0, 0, 0, 0, 0, 0, 0, 0],
			start: 1,
			concluded_at: Some(3),
		},
	);

	let inactive_concluded_onchain = (
		5,
		CandidateHash(Hash::random()),
		DisputeStatus::ConcludedFor(time_now - ACTIVE_DURATION_SECS * 2),
	);
	input.push(inactive_concluded_onchain.clone());
	onchain.insert(
		(inactive_concluded_onchain.0, inactive_concluded_onchain.1.clone()),
		DisputeState {
			validators_for: bitvec![u8, Lsb0; 1, 1, 1, 1, 1, 1, 1, 0, 0],
			validators_against: bitvec![u8, Lsb0; 0, 0, 0, 0, 0, 0, 0, 0, 0],
			start: 1,
			concluded_at: Some(3),
		},
	);

	let result = partition_recent_disputes(input, &onchain);

	// Check results
	assert_eq!(result.inactive_unknown_onchain.len(), 1);
	assert_eq!(
		result.inactive_unknown_onchain.get(0).unwrap(),
		&(inactive_unknown_onchain.0, inactive_unknown_onchain.1)
	);

	assert_eq!(result.inactive_unconcluded_onchain.len(), 1);
	assert_eq!(
		result.inactive_unconcluded_onchain.get(0).unwrap(),
		&(inactive_unconcluded_onchain.0, inactive_unconcluded_onchain.1)
	);

	assert_eq!(result.active_unknown_onchain.len(), 1);
	assert_eq!(
		result.active_unknown_onchain.get(0).unwrap(),
		&(active_unknown_onchain.0, active_unknown_onchain.1)
	);

	assert_eq!(result.active_unconcluded_onchain.len(), 1);
	assert_eq!(
		result.active_unconcluded_onchain.get(0).unwrap(),
		&(active_unconcluded_onchain.0, active_unconcluded_onchain.1)
	);

	assert_eq!(result.active_concluded_onchain.len(), 1);
	assert_eq!(
		result.active_concluded_onchain.get(0).unwrap(),
		&(active_concluded_onchain.0, active_concluded_onchain.1)
	);

	assert_eq!(result.inactive_concluded_onchain.len(), 1);
	assert_eq!(
		result.inactive_concluded_onchain.get(0).unwrap(),
		&(inactive_concluded_onchain.0, inactive_concluded_onchain.1)
	);
}

// This test verifies the double voting behavior. Currently we don't care if a supermajority is achieved with or
// without the 'help' of a double vote (a validator voting for and against at the same time). This makes the test
// a bit pointless but anyway I'm leaving it here to make this decision explicit and have the test code ready in
// case this behavior needs to be further tested in the future.
// Link to the PR with the discussions: https://github.com/paritytech/polkadot/pull/5567
#[test]
fn partitioning_doubled_onchain_vote() {
	let mut input = Vec::<(SessionIndex, CandidateHash, DisputeStatus)>::new();
	let mut onchain = HashMap::<(u32, CandidateHash), DisputeState>::new();

	// Dispute A relies on a 'double onchain vote' to conclude. Validator with index 0 has voted both `for` and `against`.
	// Despite that this dispute should be considered 'can conclude onchain'.
	let dispute_a = (3, CandidateHash(Hash::random()), DisputeStatus::Active);
	// Dispute B has supermajority + 1 votes, so the doubled onchain vote doesn't affect it. It should be considered
	// as 'can conclude onchain'.
	let dispute_b = (4, CandidateHash(Hash::random()), DisputeStatus::Active);
	input.push(dispute_a.clone());
	input.push(dispute_b.clone());
	onchain.insert(
		(dispute_a.0, dispute_a.1.clone()),
		DisputeState {
			validators_for: bitvec![u8, Lsb0; 1, 1, 1, 1, 1, 1, 1, 0, 0],
			validators_against: bitvec![u8, Lsb0; 1, 0, 0, 0, 0, 0, 0, 0, 0],
			start: 1,
			concluded_at: None,
		},
	);
	onchain.insert(
		(dispute_b.0, dispute_b.1.clone()),
		DisputeState {
			validators_for: bitvec![u8, Lsb0; 1, 1, 1, 1, 1, 1, 1, 1, 0],
			validators_against: bitvec![u8, Lsb0; 1, 0, 0, 0, 0, 0, 0, 0, 0],
			start: 1,
			concluded_at: None,
		},
	);

	let result = partition_recent_disputes(input, &onchain);

	assert_eq!(result.active_unconcluded_onchain.len(), 0);
	assert_eq!(result.active_concluded_onchain.len(), 2);
}

#[test]
fn partitioning_duplicated_dispute() {
	let mut input = Vec::<(SessionIndex, CandidateHash, DisputeStatus)>::new();
	let mut onchain = HashMap::<(u32, CandidateHash), DisputeState>::new();

	let some_dispute = (3, CandidateHash(Hash::random()), DisputeStatus::Active);
	input.push(some_dispute.clone());
	input.push(some_dispute.clone());
	onchain.insert(
		(some_dispute.0, some_dispute.1.clone()),
		DisputeState {
			validators_for: bitvec![u8, Lsb0; 1, 1, 1, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, Lsb0; 0, 0, 0, 0, 0, 0, 0, 0, 0],
			start: 1,
			concluded_at: None,
		},
	);

	let result = partition_recent_disputes(input, &onchain);

	assert_eq!(result.active_unconcluded_onchain.len(), 1);
	assert_eq!(
		result.active_unconcluded_onchain.get(0).unwrap(),
		&(some_dispute.0, some_dispute.1)
	);
}

//
// end-to-end tests for select_disputes()
//

async fn mock_overseer(
	mut receiver: mpsc::UnboundedReceiver<AllMessages>,
	disputes_db: &mut TestDisputes,
	vote_queries_count: &mut usize,
) {
	while let Some(from_job) = receiver.next().await {
		match from_job {
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(
				_,
				RuntimeApiRequest::Disputes(sender),
			)) => {
				let _ = sender.send(Ok(disputes_db
					.onchain_disputes
					.clone()
					.into_iter()
					.map(|(k, v)| (k.0, k.1, v))
					.collect::<Vec<_>>()));
			},
			AllMessages::RuntimeApi(_) => panic!("Unexpected RuntimeApi request"),
			AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::RecentDisputes(sender)) => {
				let _ = sender.send(disputes_db.local_disputes.clone());
			},
			AllMessages::DisputeCoordinator(DisputeCoordinatorMessage::QueryCandidateVotes(
				disputes,
				sender,
			)) => {
				*vote_queries_count += 1;
				let mut res = Vec::new();
				for d in disputes.iter() {
					let v = disputes_db.votes_db.get(d).unwrap().clone();
					res.push((d.0, d.1, v));
				}

				let _ = sender.send(res);
			},
			_ => panic!("Unexpected message: {:?}", from_job),
		}
	}
}

fn leaf() -> ActivatedLeaf {
	ActivatedLeaf {
		hash: Hash::repeat_byte(0xAA),
		number: 0xAA,
		status: LeafStatus::Fresh,
		span: Arc::new(jaeger::Span::Disabled),
	}
}

struct TestDisputes {
	pub local_disputes: Vec<(SessionIndex, CandidateHash, DisputeStatus)>,
	pub votes_db: HashMap<(SessionIndex, CandidateHash), CandidateVotes>,
	pub onchain_disputes: HashMap<(u32, CandidateHash), DisputeState>,
	validators_count: usize,
}

impl TestDisputes {
	pub fn new(validators_count: usize) -> TestDisputes {
		TestDisputes {
			local_disputes: Vec::<(SessionIndex, CandidateHash, DisputeStatus)>::new(),
			votes_db: HashMap::<(SessionIndex, CandidateHash), CandidateVotes>::new(),
			onchain_disputes: HashMap::<(u32, CandidateHash), DisputeState>::new(),
			validators_count,
		}
	}

	// Offchain disputes are on node side
	fn add_offchain_dispute(
		&mut self,
		dispute: (SessionIndex, CandidateHash, DisputeStatus),
		local_votes_count: usize,
		dummy_receipt: CandidateReceipt,
	) {
		self.local_disputes.push(dispute.clone());
		self.votes_db.insert(
			(dispute.0, dispute.1),
			CandidateVotes {
				candidate_receipt: dummy_receipt,
				valid: TestDisputes::generate_local_votes(
					ValidDisputeStatementKind::Explicit,
					0,
					local_votes_count,
				)
				.into_iter()
				.collect(),
				invalid: BTreeMap::new(),
			},
		);
	}

	fn add_onchain_dispute(
		&mut self,
		dispute: (SessionIndex, CandidateHash, DisputeStatus),
		onchain_votes_count: usize,
	) {
		let concluded_at = match dispute.2 {
			DisputeStatus::Active | DisputeStatus::Confirmed => None,
			DisputeStatus::ConcludedAgainst(_) | DisputeStatus::ConcludedFor(_) => Some(1),
		};
		self.onchain_disputes.insert(
			(dispute.0, dispute.1.clone()),
			DisputeState {
				validators_for: TestDisputes::generate_bitvec(
					self.validators_count,
					0,
					onchain_votes_count,
				),
				validators_against: bitvec![u8, Lsb0; 0; self.validators_count],
				start: 1,
				concluded_at,
			},
		);
	}

	pub fn add_unconfirmed_disputes_concluded_onchain(
		&mut self,
		dispute_count: usize,
	) -> (SessionIndex, usize) {
		let local_votes_count = self.validators_count * 90 / 100;
		let onchain_votes_count = self.validators_count * 80 / 100;
		let session_idx = 0;
		let lf = leaf();
		let dummy_receipt = test_helpers::dummy_candidate_receipt(lf.hash.clone());
		for _ in 0..dispute_count {
			let d = (session_idx, CandidateHash(Hash::random()), DisputeStatus::Active);
			self.add_offchain_dispute(d.clone(), local_votes_count, dummy_receipt.clone());
			self.add_onchain_dispute(d, onchain_votes_count);
		}

		(session_idx, (local_votes_count - onchain_votes_count) * dispute_count)
	}

	pub fn add_unconfirmed_disputes_unconcluded_onchain(
		&mut self,
		dispute_count: usize,
	) -> (SessionIndex, usize) {
		let local_votes_count = self.validators_count * 90 / 100;
		let onchain_votes_count = self.validators_count * 40 / 100;
		let session_idx = 1;
		let lf = leaf();
		let dummy_receipt = test_helpers::dummy_candidate_receipt(lf.hash.clone());
		for _ in 0..dispute_count {
			let d = (session_idx, CandidateHash(Hash::random()), DisputeStatus::Active);
			self.add_offchain_dispute(d.clone(), local_votes_count, dummy_receipt.clone());
			self.add_onchain_dispute(d, onchain_votes_count);
		}

		(session_idx, (local_votes_count - onchain_votes_count) * dispute_count)
	}

	pub fn add_confirmed_disputes_unknown_onchain(
		&mut self,
		dispute_count: usize,
	) -> (SessionIndex, usize) {
		let local_votes_count = self.validators_count * 90 / 100;
		let session_idx = 2;
		let lf = leaf();
		let dummy_receipt = test_helpers::dummy_candidate_receipt(lf.hash.clone());
		for _ in 0..dispute_count {
			let d = (session_idx, CandidateHash(Hash::random()), DisputeStatus::Confirmed);
			self.add_offchain_dispute(d.clone(), local_votes_count, dummy_receipt.clone());
		}
		(session_idx, local_votes_count * dispute_count)
	}

	pub fn add_concluded_disputes_known_onchain(
		&mut self,
		dispute_count: usize,
	) -> (SessionIndex, usize) {
		let local_votes_count = self.validators_count * 90 / 100;
		let onchain_votes_count = self.validators_count * 75 / 100;
		let session_idx = 3;
		let lf = leaf();
		let dummy_receipt = test_helpers::dummy_candidate_receipt(lf.hash.clone());
		for _ in 0..dispute_count {
			let d = (session_idx, CandidateHash(Hash::random()), DisputeStatus::ConcludedFor(0));
			self.add_offchain_dispute(d.clone(), local_votes_count, dummy_receipt.clone());
			self.add_onchain_dispute(d, onchain_votes_count);
		}
		(session_idx, (local_votes_count - onchain_votes_count) * dispute_count)
	}

	pub fn add_concluded_disputes_unknown_onchain(
		&mut self,
		dispute_count: usize,
	) -> (SessionIndex, usize) {
		let local_votes_count = self.validators_count * 90 / 100;
		let session_idx = 4;
		let lf = leaf();
		let dummy_receipt = test_helpers::dummy_candidate_receipt(lf.hash.clone());
		for _ in 0..dispute_count {
			let d = (session_idx, CandidateHash(Hash::random()), DisputeStatus::ConcludedFor(0));
			self.add_offchain_dispute(d.clone(), local_votes_count, dummy_receipt.clone());
		}
		(session_idx, local_votes_count * dispute_count)
	}

	pub fn add_unconfirmed_disputes_known_onchain(
		&mut self,
		dispute_count: usize,
	) -> (SessionIndex, usize) {
		let local_votes_count = self.validators_count * 10 / 100;
		let onchain_votes_count = self.validators_count * 10 / 100;
		let session_idx = 5;
		let lf = leaf();
		let dummy_receipt = test_helpers::dummy_candidate_receipt(lf.hash.clone());
		for _ in 0..dispute_count {
			let d = (session_idx, CandidateHash(Hash::random()), DisputeStatus::Active);
			self.add_offchain_dispute(d.clone(), local_votes_count, dummy_receipt.clone());
			self.add_onchain_dispute(d, onchain_votes_count);
		}

		(session_idx, (local_votes_count - onchain_votes_count) * dispute_count)
	}

	pub fn add_unconfirmed_disputes_unknown_onchain(
		&mut self,
		dispute_count: usize,
	) -> (SessionIndex, usize) {
		let local_votes_count = self.validators_count * 10 / 100;
		let session_idx = 6;
		let lf = leaf();
		let dummy_receipt = test_helpers::dummy_candidate_receipt(lf.hash.clone());
		for _ in 0..dispute_count {
			let d = (session_idx, CandidateHash(Hash::random()), DisputeStatus::Active);
			self.add_offchain_dispute(d.clone(), local_votes_count, dummy_receipt.clone());
		}

		(session_idx, local_votes_count * dispute_count)
	}

	fn generate_local_votes<T: Clone>(
		statement_kind: T,
		start_idx: usize,
		count: usize,
	) -> BTreeMap<ValidatorIndex, (T, ValidatorSignature)> {
		assert!(start_idx < count);
		(start_idx..count)
			.map(|idx| {
				(
					ValidatorIndex(idx as u32),
					(statement_kind.clone(), test_helpers::dummy_signature()),
				)
			})
			.collect::<BTreeMap<_, _>>()
	}

	fn generate_bitvec(
		validator_count: usize,
		start_idx: usize,
		count: usize,
	) -> BitVec<u8, bitvec::order::Lsb0> {
		assert!(start_idx < count);
		assert!(start_idx + count < validator_count);
		let mut res = bitvec![u8, Lsb0; 0; validator_count];
		for idx in start_idx..count {
			res.set(idx, true);
		}

		res
	}
}

#[test]
fn normal_flow() {
	const VALIDATOR_COUNT: usize = 10;
	const DISPUTES_PER_BATCH: usize = 2;
	const ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT: usize = 1;

	let mut input = TestDisputes::new(VALIDATOR_COUNT);

	// active, concluded onchain
	let (third_idx, third_votes) =
		input.add_unconfirmed_disputes_concluded_onchain(DISPUTES_PER_BATCH);

	// active unconcluded onchain
	let (first_idx, first_votes) =
		input.add_unconfirmed_disputes_unconcluded_onchain(DISPUTES_PER_BATCH);

	//concluded disputes unknown onchain
	let (fifth_idx, fifth_votes) = input.add_concluded_disputes_unknown_onchain(DISPUTES_PER_BATCH);

	// concluded disputes known onchain - these should be ignored
	let (_, _) = input.add_concluded_disputes_known_onchain(DISPUTES_PER_BATCH);

	// confirmed disputes unknown onchain
	let (second_idx, second_votes) =
		input.add_confirmed_disputes_unknown_onchain(DISPUTES_PER_BATCH);

	let metrics = metrics::Metrics::new_dummy();
	let mut vote_queries: usize = 0;
	test_harness(
		|r| mock_overseer(r, &mut input, &mut vote_queries),
		|mut tx: TestSubsystemSender| async move {
			let lf = leaf();
			let result = select_disputes(&mut tx, &metrics, &lf).await;

			assert!(!result.is_empty());

			assert_eq!(result.len(), 4 * DISPUTES_PER_BATCH);

			// Naive checks that the result is partitioned correctly
			let (first_batch, rest): (Vec<DisputeStatementSet>, Vec<DisputeStatementSet>) =
				result.into_iter().partition(|d| d.session == first_idx);
			assert_eq!(first_batch.len(), DISPUTES_PER_BATCH);

			let (second_batch, rest): (Vec<DisputeStatementSet>, Vec<DisputeStatementSet>) =
				rest.into_iter().partition(|d| d.session == second_idx);
			assert_eq!(second_batch.len(), DISPUTES_PER_BATCH);

			let (third_batch, rest): (Vec<DisputeStatementSet>, Vec<DisputeStatementSet>) =
				rest.into_iter().partition(|d| d.session == third_idx);
			assert_eq!(third_batch.len(), DISPUTES_PER_BATCH);

			let (fifth_batch, rest): (Vec<DisputeStatementSet>, Vec<DisputeStatementSet>) =
				rest.into_iter().partition(|d| d.session == fifth_idx);
			assert_eq!(fifth_batch.len(), DISPUTES_PER_BATCH);

			// Ensure there are no more disputes - fourth_batch should be dropped
			assert_eq!(rest.len(), 0);

			assert_eq!(
				first_batch.iter().map(|d| d.statements.len()).fold(0, |acc, v| acc + v),
				first_votes
			);
			assert_eq!(
				second_batch.iter().map(|d| d.statements.len()).fold(0, |acc, v| acc + v),
				second_votes
			);
			assert_eq!(
				third_batch.iter().map(|d| d.statements.len()).fold(0, |acc, v| acc + v),
				third_votes
			);
			assert_eq!(
				fifth_batch.iter().map(|d| d.statements.len()).fold(0, |acc, v| acc + v),
				fifth_votes
			);
		},
	);
	assert!(vote_queries <= ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT);
}

#[test]
fn many_batches() {
	const VALIDATOR_COUNT: usize = 10;
	const DISPUTES_PER_PARTITION: usize = 10;

	// 10 disputes per partition * 4 partitions = 40 disputes
	// BATCH_SIZE = 11
	// => There should be no more than 40 / 11 queries ( ~4 )
	const ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT: usize = 4;

	let mut input = TestDisputes::new(VALIDATOR_COUNT);

	// active which can conclude onchain
	input.add_unconfirmed_disputes_concluded_onchain(DISPUTES_PER_PARTITION);

	// active which can't conclude onchain
	input.add_unconfirmed_disputes_unconcluded_onchain(DISPUTES_PER_PARTITION);

	//concluded disputes unknown onchain
	input.add_concluded_disputes_unknown_onchain(DISPUTES_PER_PARTITION);

	// concluded disputes known onchain
	input.add_concluded_disputes_known_onchain(DISPUTES_PER_PARTITION);

	// confirmed disputes unknown onchain
	input.add_confirmed_disputes_unknown_onchain(DISPUTES_PER_PARTITION);

	let metrics = metrics::Metrics::new_dummy();
	let mut vote_queries: usize = 0;
	test_harness(
		|r| mock_overseer(r, &mut input, &mut vote_queries),
		|mut tx: TestSubsystemSender| async move {
			let lf = leaf();
			let result = select_disputes(&mut tx, &metrics, &lf).await;

			assert!(!result.is_empty());

			let vote_count = result.iter().map(|d| d.statements.len()).fold(0, |acc, v| acc + v);

			assert!(
				MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME - VALIDATOR_COUNT <= vote_count &&
					vote_count <= MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME,
				"vote_count: {}",
				vote_count
			);
		},
	);

	assert!(
		vote_queries <= ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT,
		"vote_queries: {} ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT: {}",
		vote_queries,
		ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT
	);
}

#[test]
fn votes_above_limit() {
	const VALIDATOR_COUNT: usize = 10;
	const DISPUTES_PER_PARTITION: usize = 50;
	const ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT: usize = 4;

	let mut input = TestDisputes::new(VALIDATOR_COUNT);

	// active which can conclude onchain
	let (_, second_votes) =
		input.add_unconfirmed_disputes_concluded_onchain(DISPUTES_PER_PARTITION);

	// active which can't conclude onchain
	let (_, first_votes) =
		input.add_unconfirmed_disputes_unconcluded_onchain(DISPUTES_PER_PARTITION);

	//concluded disputes unknown onchain
	let (_, third_votes) = input.add_concluded_disputes_unknown_onchain(DISPUTES_PER_PARTITION);

	assert!(
		first_votes + second_votes + third_votes > 3 * MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME,
		"Total relevant votes generated: {}",
		first_votes + second_votes + third_votes
	);

	let metrics = metrics::Metrics::new_dummy();
	let mut vote_queries: usize = 0;
	test_harness(
		|r| mock_overseer(r, &mut input, &mut vote_queries),
		|mut tx: TestSubsystemSender| async move {
			let lf = leaf();
			let result = select_disputes(&mut tx, &metrics, &lf).await;

			assert!(!result.is_empty());

			let vote_count = result.iter().map(|d| d.statements.len()).fold(0, |acc, v| acc + v);

			assert!(
				MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME - VALIDATOR_COUNT <= vote_count &&
					vote_count <= MAX_DISPUTE_VOTES_FORWARDED_TO_RUNTIME,
				"vote_count: {}",
				vote_count
			);
		},
	);

	assert!(
		vote_queries <= ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT,
		"vote_queries: {} ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT: {}",
		vote_queries,
		ACCEPTABLE_RUNTIME_VOTES_QUERIES_COUNT
	);
}

#[test]
fn unconfirmed_are_handled_correctly() {
	const VALIDATOR_COUNT: usize = 10;
	const DISPUTES_PER_PARTITION: usize = 50;

	let mut input = TestDisputes::new(VALIDATOR_COUNT);

	// Add unconfirmed known onchain -> this should be pushed
	let (pushed_idx, _) = input.add_unconfirmed_disputes_known_onchain(DISPUTES_PER_PARTITION);

	// Add unconfirmed unknown onchain -> this should be ignored
	input.add_unconfirmed_disputes_unknown_onchain(DISPUTES_PER_PARTITION);

	let metrics = metrics::Metrics::new_dummy();
	let mut vote_queries: usize = 0;
	test_harness(
		|r| mock_overseer(r, &mut input, &mut vote_queries),
		|mut tx: TestSubsystemSender| async move {
			let lf = leaf();
			let result = select_disputes(&mut tx, &metrics, &lf).await;

			assert!(result.len() == DISPUTES_PER_PARTITION);
			result.iter().for_each(|d| assert!(d.session == pushed_idx));
		},
	);
}
