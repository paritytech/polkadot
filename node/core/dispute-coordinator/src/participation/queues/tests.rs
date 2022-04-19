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

use ::test_helpers::{dummy_candidate_receipt, dummy_hash};
use assert_matches::assert_matches;
use polkadot_primitives::v2::{BlockNumber, Hash};

use super::{CandidateComparator, ParticipationRequest, QueueError, Queues};

/// Make a `ParticipationRequest` based on the given commitments hash.
fn make_participation_request(hash: Hash) -> ParticipationRequest {
	let mut receipt = dummy_candidate_receipt(dummy_hash());
	// make it differ:
	receipt.commitments_hash = hash;
	ParticipationRequest::new(receipt, 1, 100)
}

/// Make dummy comparator for request, based on the given block number.
fn make_dummy_comparator(
	req: &ParticipationRequest,
	relay_parent: BlockNumber,
) -> CandidateComparator {
	CandidateComparator::new_dummy(relay_parent, *req.candidate_hash())
}

/// Check that dequeuing acknowledges order.
///
/// Any priority item will be dequeued before any best effort items, priority items will be
/// processed in order. Best effort items, based on how often they have been added.
#[test]
fn ordering_works_as_expected() {
	let mut queue = Queues::new();
	let req1 = make_participation_request(Hash::repeat_byte(0x01));
	let req_prio = make_participation_request(Hash::repeat_byte(0x02));
	let req3 = make_participation_request(Hash::repeat_byte(0x03));
	let req_prio_2 = make_participation_request(Hash::repeat_byte(0x04));
	let req5 = make_participation_request(Hash::repeat_byte(0x05));
	let req_full = make_participation_request(Hash::repeat_byte(0x06));
	let req_prio_full = make_participation_request(Hash::repeat_byte(0x07));
	queue.queue_with_comparator(None, req1.clone()).unwrap();
	queue
		.queue_with_comparator(Some(make_dummy_comparator(&req_prio, 1)), req_prio.clone())
		.unwrap();
	queue.queue_with_comparator(None, req3.clone()).unwrap();
	queue
		.queue_with_comparator(Some(make_dummy_comparator(&req_prio_2, 2)), req_prio_2.clone())
		.unwrap();
	queue.queue_with_comparator(None, req3.clone()).unwrap();
	queue.queue_with_comparator(None, req5.clone()).unwrap();
	assert_matches!(
		queue.queue_with_comparator(Some(make_dummy_comparator(&req_prio_full, 3)), req_prio_full),
		Err(QueueError::PriorityFull)
	);
	assert_matches!(queue.queue_with_comparator(None, req_full), Err(QueueError::BestEffortFull));

	assert_eq!(queue.dequeue(), Some(req_prio));
	assert_eq!(queue.dequeue(), Some(req_prio_2));
	assert_eq!(queue.dequeue(), Some(req3));
	assert_matches!(
		queue.dequeue(),
		Some(r) => { assert!(r == req1 || r == req5) }
	);
	assert_matches!(
		queue.dequeue(),
		Some(r) => { assert!(r == req1 || r == req5) }
	);
	assert_matches!(queue.dequeue(), None);
}

/// No matter how often a candidate gets queued, it should only ever get dequeued once.
#[test]
fn candidate_is_only_dequeued_once() {
	let mut queue = Queues::new();
	let req1 = make_participation_request(Hash::repeat_byte(0x01));
	let req_prio = make_participation_request(Hash::repeat_byte(0x02));
	let req_best_effort_then_prio = make_participation_request(Hash::repeat_byte(0x03));
	let req_prio_then_best_effort = make_participation_request(Hash::repeat_byte(0x04));

	queue.queue_with_comparator(None, req1.clone()).unwrap();
	queue
		.queue_with_comparator(Some(make_dummy_comparator(&req_prio, 1)), req_prio.clone())
		.unwrap();
	// Insert same best effort again:
	queue.queue_with_comparator(None, req1.clone()).unwrap();
	// insert same prio again:
	queue
		.queue_with_comparator(Some(make_dummy_comparator(&req_prio, 1)), req_prio.clone())
		.unwrap();

	// Insert first as best effort:
	queue.queue_with_comparator(None, req_best_effort_then_prio.clone()).unwrap();
	// Then as prio:
	queue
		.queue_with_comparator(
			Some(make_dummy_comparator(&req_best_effort_then_prio, 2)),
			req_best_effort_then_prio.clone(),
		)
		.unwrap();

	// Make space in prio:
	assert_eq!(queue.dequeue(), Some(req_prio));

	// Insert first as prio:
	queue
		.queue_with_comparator(
			Some(make_dummy_comparator(&req_prio_then_best_effort, 3)),
			req_prio_then_best_effort.clone(),
		)
		.unwrap();
	// Then as best effort:
	queue.queue_with_comparator(None, req_prio_then_best_effort.clone()).unwrap();

	assert_eq!(queue.dequeue(), Some(req_best_effort_then_prio));
	assert_eq!(queue.dequeue(), Some(req_prio_then_best_effort));
	assert_eq!(queue.dequeue(), Some(req1));
	assert_eq!(queue.dequeue(), None);
}
