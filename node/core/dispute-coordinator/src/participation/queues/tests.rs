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

use crate::ParticipationPriority;
use ::test_helpers::{dummy_candidate_receipt, dummy_hash};
use assert_matches::assert_matches;
use polkadot_primitives::v2::{BlockNumber, Hash};

use super::{CandidateComparator, ParticipationRequest, QueueError, Queues};

/// Make a `ParticipationRequest` based on the given commitments hash.
fn make_participation_request(hash: Hash) -> ParticipationRequest {
	let mut receipt = dummy_candidate_receipt(dummy_hash());
	// make it differ:
	receipt.commitments_hash = hash;
	ParticipationRequest::new(receipt, 1)
}

/// Make dummy comparator for request, based on the given block number.
fn make_dummy_comparator(
	req: &ParticipationRequest,
	relay_parent: Option<BlockNumber>,
) -> CandidateComparator {
	CandidateComparator::new_dummy(relay_parent, *req.candidate_hash())
}

/// Check that dequeuing acknowledges order.
///
/// Any priority item will be dequeued before any best effort items, priority and best effort with
/// known parent block number items will be processed in order. Best effort items without known parent
/// block number should be treated with lowest priority.
#[test]
fn ordering_works_as_expected() {
	let mut queue = Queues::new();
	let req1 = make_participation_request(Hash::repeat_byte(0x01));
	let req_prio = make_participation_request(Hash::repeat_byte(0x02));
	let req3 = make_participation_request(Hash::repeat_byte(0x03));
	let req_prio_2 = make_participation_request(Hash::repeat_byte(0x04));
	let req5_unknown_parent = make_participation_request(Hash::repeat_byte(0x05));
	let req_full = make_participation_request(Hash::repeat_byte(0x06));
	let req_prio_full = make_participation_request(Hash::repeat_byte(0x07));
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req1, Some(1)),
			ParticipationPriority::BestEffort,
			req1.clone(),
		)
		.unwrap();
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req_prio, Some(1)),
			ParticipationPriority::Priority,
			req_prio.clone(),
		)
		.unwrap();
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req3, Some(2)),
			ParticipationPriority::BestEffort,
			req3.clone(),
		)
		.unwrap();
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req_prio_2, Some(2)),
			ParticipationPriority::Priority,
			req_prio_2.clone(),
		)
		.unwrap();
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req5_unknown_parent, None),
			ParticipationPriority::BestEffort,
			req5_unknown_parent.clone(),
		)
		.unwrap();
	assert_matches!(
		queue.queue_with_comparator(
			make_dummy_comparator(&req_prio_full, Some(3)),
			ParticipationPriority::Priority,
			req_prio_full
		),
		Err(QueueError::PriorityFull)
	);
	assert_matches!(
		queue.queue_with_comparator(
			make_dummy_comparator(&req_full, Some(3)),
			ParticipationPriority::BestEffort,
			req_full
		),
		Err(QueueError::BestEffortFull)
	);

	// Prioritized queue is ordered correctly
	assert_eq!(queue.dequeue(), Some(req_prio));
	assert_eq!(queue.dequeue(), Some(req_prio_2));
	// So is the best-effort
	assert_eq!(queue.dequeue(), Some(req1));
	assert_eq!(queue.dequeue(), Some(req3));
	assert_eq!(queue.dequeue(), Some(req5_unknown_parent));

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

	queue
		.queue_with_comparator(
			make_dummy_comparator(&req1, None),
			ParticipationPriority::BestEffort,
			req1.clone(),
		)
		.unwrap();
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req_prio, Some(1)),
			ParticipationPriority::Priority,
			req_prio.clone(),
		)
		.unwrap();
	// Insert same best effort again:
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req1, None),
			ParticipationPriority::BestEffort,
			req1.clone(),
		)
		.unwrap();
	// insert same prio again:
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req_prio, Some(1)),
			ParticipationPriority::Priority,
			req_prio.clone(),
		)
		.unwrap();

	// Insert first as best effort:
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req_best_effort_then_prio, Some(2)),
			ParticipationPriority::BestEffort,
			req_best_effort_then_prio.clone(),
		)
		.unwrap();
	// Then as prio:
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req_best_effort_then_prio, Some(2)),
			ParticipationPriority::Priority,
			req_best_effort_then_prio.clone(),
		)
		.unwrap();

	// Make space in prio:
	assert_eq!(queue.dequeue(), Some(req_prio));

	// Insert first as prio:
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req_prio_then_best_effort, Some(3)),
			ParticipationPriority::Priority,
			req_prio_then_best_effort.clone(),
		)
		.unwrap();
	// Then as best effort:
	queue
		.queue_with_comparator(
			make_dummy_comparator(&req_prio_then_best_effort, Some(3)),
			ParticipationPriority::BestEffort,
			req_prio_then_best_effort.clone(),
		)
		.unwrap();

	assert_eq!(queue.dequeue(), Some(req_best_effort_then_prio));
	assert_eq!(queue.dequeue(), Some(req_prio_then_best_effort));
	assert_eq!(queue.dequeue(), Some(req1));
	assert_eq!(queue.dequeue(), None);
}
