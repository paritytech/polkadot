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
	collections::{hash_map, HashMap},
	time::{Duration, Instant},
};

use futures::future::pending;

use polkadot_node_network_protocol::request_response::DISPUTE_REQUEST_TIMEOUT;
use polkadot_primitives::v2::{CandidateHash, CandidateReceipt};

use crate::{
	receiver::batches::{batch::TickResult, waiting_queue::PendingWake},
	LOG_TARGET,
};

pub use self::batch::{Batch, PreparedImport};
use self::waiting_queue::WaitingQueue;

use super::{
	error::{JfyiError, JfyiResult},
	BATCH_COLLECTING_INTERVAL,
};

/// A single batch (per candidate) as managed by `Batches`.
mod batch;

/// Queue events in time and wait for them to become ready.
mod waiting_queue;

/// Safe-guard in case votes trickle in real slow.
///
/// If the batch life time exceeded the time the sender is willing to wait for a confirmation, we
/// would trigger pointless re-sends.
const MAX_BATCH_LIFETIME: Duration = DISPUTE_REQUEST_TIMEOUT.saturating_sub(Duration::from_secs(2));

/// Limit the number of batches that can be alive at any given time.
///
/// Reasoning for this number, see guide.
pub const MAX_BATCHES: usize = 1000;

/// Manage batches.
///
/// - Batches can be found via `find_batch()` in order to add votes to them/check they exist.
/// - Batches can be checked for being ready for flushing in order to import contained votes.
pub struct Batches {
	/// The batches we manage.
	///
	/// Kept invariants:
	/// For each entry in `batches`, there exists an entry in `waiting_queue` as well - we wait on
	/// all batches!
	batches: HashMap<CandidateHash, Batch>,
	/// Waiting queue for waiting for batches to become ready for `tick`.
	///
	/// Kept invariants by `Batches`:
	/// For each entry in the `waiting_queue` there exists a corresponding entry in `batches`.
	waiting_queue: WaitingQueue<CandidateHash>,
}

/// A found batch is either really found or got created so it can be found.
pub enum FoundBatch<'a> {
	/// Batch just got created.
	Created(&'a mut Batch),
	/// Batch already existed.
	Found(&'a mut Batch),
}

impl Batches {
	/// Create new empty `Batches`.
	pub fn new() -> Self {
		debug_assert!(
			MAX_BATCH_LIFETIME > BATCH_COLLECTING_INTERVAL,
			"Unexpectedly low `MAX_BATCH_LIFETIME`, please check parameters."
		);
		Self { batches: HashMap::new(), waiting_queue: WaitingQueue::new() }
	}

	/// Find a particular batch.
	///
	/// That is either find it, or we create it as reflected by the result `FoundBatch`.
	pub fn find_batch(
		&mut self,
		candidate_hash: CandidateHash,
		candidate_receipt: CandidateReceipt,
	) -> JfyiResult<FoundBatch> {
		if self.batches.len() >= MAX_BATCHES {
			return Err(JfyiError::MaxBatchLimitReached)
		}
		debug_assert!(candidate_hash == candidate_receipt.hash());
		let result = match self.batches.entry(candidate_hash) {
			hash_map::Entry::Vacant(vacant) => {
				let now = Instant::now();
				let (created, ready_at) = Batch::new(candidate_receipt, now);
				let pending_wake = PendingWake { payload: candidate_hash, ready_at };
				self.waiting_queue.push(pending_wake);
				FoundBatch::Created(vacant.insert(created))
			},
			hash_map::Entry::Occupied(occupied) => FoundBatch::Found(occupied.into_mut()),
		};
		Ok(result)
	}

	/// Wait for the next `tick` to check for ready batches.
	///
	/// This function blocks (returns `Poll::Pending`) until at least one batch can be
	/// checked for readiness meaning that `BATCH_COLLECTING_INTERVAL` has passed since the last
	/// check for that batch or it reached end of life.
	///
	/// If this `Batches` instance is empty (does not actually contain any batches), then this
	/// function will always return `Poll::Pending`.
	///
	/// Returns: A `Vec` of all `PreparedImport`s from batches that became ready.
	pub async fn check_batches(&mut self) -> Vec<PreparedImport> {
		let now = Instant::now();

		let mut imports = Vec::new();

		// Wait for at least one batch to become ready:
		self.waiting_queue.wait_ready(now).await;

		// Process all ready entries:
		while let Some(wake) = self.waiting_queue.pop_ready(now) {
			let batch = self.batches.remove(&wake.payload);
			debug_assert!(
				batch.is_some(),
				"Entries referenced in `waiting_queue` are supposed to exist!"
			);
			let batch = match batch {
				None => return pending().await,
				Some(batch) => batch,
			};
			match batch.tick(now) {
				TickResult::Done(import) => {
					gum::trace!(
						target: LOG_TARGET,
						candidate_hash = ?wake.payload,
						"Batch became ready."
					);
					imports.push(import);
				},
				TickResult::Alive(old_batch, next_tick) => {
					gum::trace!(
						target: LOG_TARGET,
						candidate_hash = ?wake.payload,
						"Batch found to be still alive on check."
					);
					let pending_wake = PendingWake { payload: wake.payload, ready_at: next_tick };
					self.waiting_queue.push(pending_wake);
					self.batches.insert(wake.payload, old_batch);
				},
			}
		}
		imports
	}
}
