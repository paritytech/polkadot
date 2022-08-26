// Copyright 2022 Parity Technologies (UK) Ltd.
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
	cmp::{Ord, Ordering},
	collections::{hash_map, BTreeMap, BinaryHeap, HashMap, HashSet},
	time::Instant,
};

use futures_timer::Delay;
use polkadot_node_network_protocol::request_response::{
	incoming::OutgoingResponseSender, v1::DisputeRequest,
};
use polkadot_node_primitives::SignedDisputeStatement;
use polkadot_primitives::v2::{CandidateHash, CandidateReceipt, ValidatorIndex};

use super::BATCH_COLLECTING_INTERVAL;

/// TODO: Limit number of batches

// - Batches can be added very rate limit timeout.
// - They have to be checked every BATCH_COLLECTING_INTERVAL.
// - We can get the earliest next wakeup - keep ordered list of wakeups! Then we always know when
// the next one comes - only needs to get updated on insert. - Tada!
struct Batches {
	batches: HashMap<CandidateHash, Batch>,
	check_waker: Option<Delay>,
	pending_wakes: BinaryHeap<PendingWake>,
}

/// Represents some batch waiting for its next tick to happen at `next_tick`.
///
/// This is an internal type meant to be used in the `pending_wakes` `BinaryHeap` field of
/// `Batches`. It provides an `Ord` instance, that sorts descending with regard to `Instant` (so we
/// get a `min-heap` with the earliest `Instant` at the top.
#[derive(Eq, PartialEq)]
struct PendingWake {
	candidate_hash: CandidateHash,
	next_tick: Instant,
}

/// A found batch is either really found or got created so it can be found.
enum FoundBatch<'a> {
	/// Batch just got created.
	Created(&'a mut Batch),
	/// Batch already existed.
	Found(&'a mut Batch),
}

impl Batches {
	/// Create new empty `Batches`.
	pub fn new() -> Self {
		Self { batches: HashMap::new(), check_waker: None, pending_wakes: BinaryHeap::new() }
	}

	/// Find a particular batch.
	///
	/// That is either find it, or we create it as reflected by the result `FoundBatch`.
	pub fn find_batch(
		&mut self,
		candidate_hash: CandidateHash,
		candidate_receipt: CandidateReceipt,
	) -> FoundBatch {
		debug_assert!(candidate_hash == candidate_receipt.hash());
		match self.batches.entry(candidate_hash) {
			hash_map::Entry::Vacant(vacant) =>
				FoundBatch::Created(vacant.insert(Batch::new(candidate_receipt))),
			hash_map::Entry::Occupied(occupied) => FoundBatch::Found(occupied.get_mut()),
		}
	}
	// Next steps:
	//
	// - Make sure binary heap above stays current.
	// - Use head of binary heap to schedule next wakeup.
	// - Provide funtion that provides imports delayed by the wakeup future (similar to rate
	// limiting).
	// - Important: Direct updating of last_tick of `Batch` has to be forbidden as this would break
	// our binary heap. Instead all updates have to go through `Batches`
}

/// A batch of votes to be imported into the `dispute-coordinator`.
///
/// Vote imports are way more efficient when performed in batches, hence we batch together incoming
/// votes until the rate of incoming votes falls below a threshold, then we import into the dispute
/// coordinator.
///
/// A `Batch` keeps track of the votes to be imported and the current incoming rate, on rate update
/// it will "flush" in case the incoming rate dropped too low, preparing the import.
pub struct Batch {
	/// The actual candidate this batch is concerned with.
	candidate_receipt: CandidateReceipt,

	/// Cache `CandidateHash` to do efficient sanity checks.
	candidate_hash: CandidateHash,

	/// All valid votes received in this batch so far.
	///
	/// We differentiate between valid and invalid votes, so we can detect (and drop) duplicates,
	/// while still allowing validators to equivocate.
	///
	/// Detecting and rejecting duplicats is crucial in order to effectively enforce
	/// `MIN_KEEP_BATCH_ALIVE_VOTES` per `BATCH_COLLECTING_INTERVAL`. If we would count duplicates
	/// here, the mechanism would be broken.
	valid_votes: HashMap<ValidatorIndex, SignedDisputeStatement>,

	/// All invalid votes received in this batch so far.
	invalid_votes: HashMap<ValidatorIndex, SignedDisputeStatement>,

	/// How many votes have been batched in the last `BATCH_COLLECTING_INTERVAL`?
	votes_batched_since_last_tick: u32,

	/// Timestamp of creation or last time we checked incoming rate.
	last_tick: Instant,

	/// Requesters waiting for a response.
	pending_responses: Vec<OutgoingResponseSender<DisputeRequest>>,
}

impl Batch {
	/// Create a new empty batch based on the given `CandidateReceipt`.
	///
	/// To create a `Batch` use Batches::find_batch`.
	fn new(candidate_receipt: CandidateReceipt) -> Self {
		Self {
			candidate_hash: candidate_receipt.hash(),
			candidate_receipt,
			valid_votes: HashMap::new(),
			invalid_votes: HashMap::new(),
			votes_batched_since_last_tick: 0,
			last_tick: Instant::now(),
			pending_responses: Vec::new(),
		}
	}

	/// Add votes from a validator into the batch.
	///
	/// The statements are supposed to be the valid and invalid statements received in a
	/// `DisputeRequest`.
	///
	/// The given `pending_response` is the corresponding response sender. If at least one of the
	/// votes is new as far as this batch is concerned we record the pending_response, for later
	/// use. In case both votes are known already, we return the response sender as an `Err` value.
	pub fn add_votes(
		&mut self,
		valid_vote: (SignedDisputeStatement, ValidatorIndex),
		invalid_vote: (SignedDisputeStatement, ValidatorIndex),
		pending_response: OutgoingResponseSender<DisputeRequest>,
	) -> Result<(), OutgoingResponseSender<DisputeRequest>> {
		debug_assert!(valid_vote.0.candidate_hash() == invalid_vote.0.candidate_hash());
		debug_assert!(valid_vote.0.candidate_hash() == &self.candidate_hash);

		let mut duplicate = true;

		if self.valid_votes.insert(valid_vote.1, valid_vote.0).is_none() {
			self.votes_batched_since_last_tick += 1;
			duplicate = false;
		}
		if self.invalid_votes.insert(invalid_vote.1, invalid_vote.0).is_none() {
			self.votes_batched_since_last_tick += 1;
			duplicate = false;
		}

		if duplicate {
			Err(pending_response)
		} else {
			Ok(())
		}
	}

	/// When the next "tick" is supposed to happen.
	fn time_next_tick(&self) -> Instant {
		self.last_tick + BATCH_COLLECTING_INTERVAL
	}
}

impl PartialOrd<PendingWake> for PendingWake {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}
impl Ord for PendingWake {
	fn cmp(&self, other: &Self) -> Ordering {
		// Reverse order for min-heap:
		match other.next_tick.cmp(&self.next_tick) {
			Ordering::Equal => other.candidate_hash.cmp(&self.candidate_hash),
			o => o,
		}
	}
}
