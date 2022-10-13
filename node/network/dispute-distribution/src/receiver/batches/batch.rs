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

use std::{collections::HashMap, time::Instant};

use gum::CandidateHash;
use polkadot_node_network_protocol::{
	request_response::{incoming::OutgoingResponseSender, v1::DisputeRequest},
	PeerId,
};
use polkadot_node_primitives::SignedDisputeStatement;
use polkadot_primitives::v2::{CandidateReceipt, ValidatorIndex};

use crate::receiver::{BATCH_COLLECTING_INTERVAL, MIN_KEEP_BATCH_ALIVE_VOTES};

use super::MAX_BATCH_LIFETIME;

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

	/// Cache of `CandidateHash` (candidate_receipt.hash()).
	candidate_hash: CandidateHash,

	/// All valid votes received in this batch so far.
	///
	/// We differentiate between valid and invalid votes, so we can detect (and drop) duplicates,
	/// while still allowing validators to equivocate.
	///
	/// Detecting and rejecting duplicates is crucial in order to effectively enforce
	/// `MIN_KEEP_BATCH_ALIVE_VOTES` per `BATCH_COLLECTING_INTERVAL`. If we would count duplicates
	/// here, the mechanism would be broken.
	valid_votes: HashMap<ValidatorIndex, SignedDisputeStatement>,

	/// All invalid votes received in this batch so far.
	invalid_votes: HashMap<ValidatorIndex, SignedDisputeStatement>,

	/// How many votes have been batched since the last tick/creation.
	votes_batched_since_last_tick: u32,

	/// Expiry time for the batch.
	///
	/// By this time the latest this batch will get flushed.
	best_before: Instant,

	/// Requesters waiting for a response.
	requesters: Vec<(PeerId, OutgoingResponseSender<DisputeRequest>)>,
}

/// Result of checking a batch every `BATCH_COLLECTING_INTERVAL`.
pub(super) enum TickResult {
	/// Batch is still alive, please call `tick` again at the given `Instant`.
	Alive(Batch, Instant),
	/// Batch is done, ready for import!
	Done(PreparedImport),
}

/// Ready for import.
pub struct PreparedImport {
	pub candidate_receipt: CandidateReceipt,
	pub statements: Vec<(SignedDisputeStatement, ValidatorIndex)>,
	/// Information about original requesters.
	pub requesters: Vec<(PeerId, OutgoingResponseSender<DisputeRequest>)>,
}

impl From<Batch> for PreparedImport {
	fn from(batch: Batch) -> Self {
		let Batch {
			candidate_receipt,
			valid_votes,
			invalid_votes,
			requesters: pending_responses,
			..
		} = batch;

		let statements = valid_votes
			.into_iter()
			.chain(invalid_votes.into_iter())
			.map(|(index, statement)| (statement, index))
			.collect();

		Self { candidate_receipt, statements, requesters: pending_responses }
	}
}

impl Batch {
	/// Create a new empty batch based on the given `CandidateReceipt`.
	///
	/// To create a `Batch` use Batches::find_batch`.
	///
	/// Arguments:
	///
	/// * `candidate_receipt` - The candidate this batch is meant to track votes for.
	/// * `now` - current time stamp for calculating the first tick.
	///
	/// Returns: A batch and the first `Instant` you are supposed to call `tick`.
	pub(super) fn new(candidate_receipt: CandidateReceipt, now: Instant) -> (Self, Instant) {
		let s = Self {
			candidate_hash: candidate_receipt.hash(),
			candidate_receipt,
			valid_votes: HashMap::new(),
			invalid_votes: HashMap::new(),
			votes_batched_since_last_tick: 0,
			best_before: Instant::now() + MAX_BATCH_LIFETIME,
			requesters: Vec::new(),
		};
		let next_tick = s.calculate_next_tick(now);
		(s, next_tick)
	}

	/// Receipt of the candidate this batch is batching votes for.
	pub fn candidate_receipt(&self) -> &CandidateReceipt {
		&self.candidate_receipt
	}

	/// Add votes from a validator into the batch.
	///
	/// The statements are supposed to be the valid and invalid statements received in a
	/// `DisputeRequest`.
	///
	/// The given `pending_response` is the corresponding response sender for responding to `peer`.
	/// If at least one of the votes is new as far as this batch is concerned we record the
	/// pending_response, for later use. In case both votes are known already, we return the
	/// response sender as an `Err` value.
	pub fn add_votes(
		&mut self,
		valid_vote: (SignedDisputeStatement, ValidatorIndex),
		invalid_vote: (SignedDisputeStatement, ValidatorIndex),
		peer: PeerId,
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
			self.requesters.push((peer, pending_response));
			Ok(())
		}
	}

	/// Check batch for liveness.
	///
	/// This function is supposed to be called at instants given at construction and as returned as
	/// part of `TickResult`.
	pub(super) fn tick(mut self, now: Instant) -> TickResult {
		if self.votes_batched_since_last_tick >= MIN_KEEP_BATCH_ALIVE_VOTES &&
			now < self.best_before
		{
			// Still good:
			let next_tick = self.calculate_next_tick(now);
			// Reset counter:
			self.votes_batched_since_last_tick = 0;
			TickResult::Alive(self, next_tick)
		} else {
			TickResult::Done(PreparedImport::from(self))
		}
	}

	/// Calculate when the next tick should happen.
	///
	/// This will usually return `now + BATCH_COLLECTING_INTERVAL`, except if the lifetime of this batch
	/// would exceed `MAX_BATCH_LIFETIME`.
	///
	/// # Arguments
	///
	/// * `now` - The current time.
	fn calculate_next_tick(&self, now: Instant) -> Instant {
		let next_tick = now + BATCH_COLLECTING_INTERVAL;
		if next_tick < self.best_before {
			next_tick
		} else {
			self.best_before
		}
	}
}
