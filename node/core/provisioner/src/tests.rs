// Copyright 2020 Parity Technologies (UK) Ltd.
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
use futures::{channel::mpsc, future};

fn default_bitvec(n_cores: usize) -> bitvec::vec::BitVec<bitvec::order::Lsb0, u8> {
	bitvec::vec::BitVec::repeat(false, n_cores)
}

use crate::collect_backed_candidates;
use polkadot_node_subsystem::messages::AllMessages;
use polkadot_node_subsystem_test_helpers::TestSubsystemSender;
use polkadot_primitives::v1::{
	BlockNumber, CandidateCommitments, CandidateDescriptor, CommittedCandidateReceipt, Hash,
	PersistedValidationData,
};

fn test_harness<OverseerFactory, Overseer, TestFactory, Test>(
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

async fn mock_overseer(
	mut receiver: mpsc::UnboundedReceiver<AllMessages>,
	expected: Vec<BackedCandidate>,
) {
	while let Some(from_job) = receiver.next().await {
		match from_job {
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
			collect_backed_candidates(Vec::new(), Default::default(), &mut tx)
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
	let empty_hash = PersistedValidationData::<Hash, BlockNumber>::default().hash();

	let candidate_template = CandidateReceipt {
		descriptor: CandidateDescriptor {
			persisted_validation_data_hash: empty_hash,
			..Default::default()
		},
		commitments_hash: CandidateCommitments::default().hash(),
	};
	let n_cores = 5;
	let candidate_receipts: Vec<_> = std::iter::repeat(candidate_template)
		.take(n_cores)
		.enumerate()
		.map(|(idx, mut candidate)| {
			candidate.descriptor.para_id = idx.into();
			candidate
		})
		.cycle()
		.take(n_cores * 4)
		.enumerate()
		.map(|(idx, mut candidate_receipt)| {
			if idx < n_cores {
				// first go-around: use candidates which should work
				candidate_receipt
			} else if idx < n_cores * 2 {
				// for the second repetition of the candidates, give them the wrong hash
				candidate_receipt.descriptor.persisted_validation_data_hash = Default::default();
				candidate_receipt
			} else if idx < n_cores * 3 {
				// third go-around: right hash, wrong para_id
				candidate_receipt.descriptor.para_id = idx.into();
				candidate_receipt
			} else {
				// fourth go-around: wrong relay parent, this is the only thing that is checked
				candidate_receipt.descriptor.relay_parent = Hash::repeat_byte(0xFF);
				candidate_receipt
			}
		})
		.collect();
	// candidates now contains 1/3 valid canidates, and 2/3 invalid
	// but we don't check them in them here, so they should be passed alright

	let expected_candidate_receipts =
		candidate_receipts.iter().take(n_cores * 3).cloned().collect::<Vec<_>>();

	let expected_backed = expected_candidate_receipts
		.iter()
		.map(|candidate_receipt| BackedCandidate {
			candidate: CommittedCandidateReceipt {
				descriptor: candidate_receipt.descriptor.clone(),
				..Default::default()
			},
			validity_votes: Vec::new(),
			validator_indices: default_bitvec(n_cores),
		})
		.collect();

	test_harness(
		|r| mock_overseer(r, expected_backed),
		|mut tx: TestSubsystemSender| async move {
			let result = collect_backed_candidates(candidate_receipts, Default::default(), &mut tx)
				.await
				.unwrap();

			result.into_iter().for_each(|c| {
				assert!(
					expected_candidate_receipts.iter().any(|c2| c.candidate.corresponds_to(c2)),
					"Failed to find candidate: {:?}",
					c,
				)
			});
		},
	)
}
