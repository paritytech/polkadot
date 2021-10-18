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

use std::collections::HashSet;

use futures::channel::{mpsc, oneshot};
use futures::FutureExt;

use polkadot_node_primitives::APPROVAL_EXECUTION_TIMEOUT;
use polkadot_node_subsystem::messages::{CandidateValidationMessage, RuntimeApiMessage, RuntimeApiRequest};
use polkadot_node_subsystem::{ActiveLeavesUpdate, RecoveryError, SubsystemContext, SubsystemSender, messages::AvailabilityRecoveryMessage};
use polkadot_primitives::v1::{BlockNumber, CandidateHash, CandidateReceipt, Hash, SessionIndex};


use crate::real::LOG_TARGET;

use super::{error::Result, ordering::CandidateComparator};

mod queues;
use queues::Queues;
pub use queues::ParticipationRequest;

/// How many participation processes do we want to run in parallel the most.
///
/// This should be a relatively low value, while we might have a speedup once we fetched the data,
/// due to multi core architectures, but the fetching itself can not be improved by parallel
/// requests. This means that higher numbers make it harder for a single dispute to resolve fast.
const MAX_PARALLEL_PARTICIPATIONS: usize = 3;

/// Keep track of disputes we need to participate in.
///
/// - Prioritize and queue participations
/// - Dequeue participation requests in order and launch participation worker.
struct Participation {
	/// Participations currently being processed.
	running_participations: HashSet<CandidateHash>,
	/// Priority and best effort queues.
	queue: Queues,
	/// Sender to be passed to worker tasks.
	worker_sender: mpsc::Sender<WorkerMessage>,
	/// Some recent block for retrieving validation code from chain.
	recent_block: Option<(BlockNumber, Hash)>,
}

/// Message from worker tasks.
struct WorkerMessage {
	/// Relevant session.
	session: SessionIndex,
	/// The candidate the worker has been spawned for.
	candidate_hash: CandidateHash,
	/// Used receipt.
	candidate_receipt: CandidateReceipt,
	/// Actual result.
	validation_result: ValidationResult,
}

enum ValidationResult {
	/// Candidate was found to be valid.
	Valid,
	/// Candidate was found to be invalid.
	Invalid,
	/// Candidate was found to be unavailable.
	Unavailable,
	/// Something went wrong (bug), details can be found in the logs.
	Error,
}

impl WorkerMessage {
	fn from_request(req: ParticipationRequest, validation_result: ValidationResult) -> Self {
		let session = req.session();
		let (candidate_hash, candidate_receipt) = req.into_candidate_info();
		Self {
			session,
			candidate_hash,
			candidate_receipt,
			validation_result,
		}
	}
}

impl Participation {
	/// Get ready for managing dispute participation requests.
	///
	/// The passed in sender will be used by background workers to communicate back their results.
	/// The calling context should make sure to call `Participation::on_worker_message()` for the
	/// received messages.
	pub fn new(sender: mpsc::Sender<WorkerMessage>) -> Self {
		Self {
			running_participations: HashSet::new(),
			queue: Queues::new(),
			worker_sender: sender,
			recent_block: None,
		}
	}

	/// Queue a dispute for the node to participate in.
	///
	/// If capacity is available right now and we already got some relay chain head via
	/// `on_active_leaves_update`, the participation will be launched right away.
	///
	/// Returns: false, if queues are already full.
	pub async fn queue_participation<Context: SubsystemContext>(&mut self, ctx: &mut Context, comparator: Option<CandidateComparator>, req: ParticipationRequest) -> Result<bool>  {
		// Participation already running - we can ignore that request:
		if self.running_participations.contains(req.candidate_hash()) { 
			return Ok(true)
		}
		// Available capacity - participate right away (if we already have a recent block):
		if let Some((_, h)) = self.recent_block {
			if self.running_participations.len() < MAX_PARALLEL_PARTICIPATIONS  {
				self.fork_participation(ctx, req, h)?;
				return Ok(true)
			}
		}
		// Out of capacity - queue:
		Ok(self.queue.queue(comparator, req))
	}

	/// Message from a worker task was received.
	///
	/// This message has to be called for each received worker message, in order to make sure
	/// enough participation processes are running at any given time.
	pub async fn on_worker_message<Context: SubsystemContext>(&mut self, ctx: &mut Context, msg: WorkerMessage) {}

	/// Process active leaves update.
	pub async fn on_active_leaves_update<Context: SubsystemContext>(&mut self, ctx: &mut Context, update: &ActiveLeavesUpdate) -> Result<()> {
		if let Some(activated) = &update.activated {
			match self.recent_block {
				None => {
					self.recent_block = Some((activated.number, activated.hash));
					// Work got potentially unblocked:
					self.dequeue_until_capacity(ctx, activated.hash).await?;
				}
				Some((number, hash)) if activated.number > number => {
					self.recent_block = Some((activated.number, activated.hash));
				}
				Some(_) => {}
			}
		}
		Ok(())
	}

	/// Dequeue until `MAX_PARALLEL_PARTICIPATIONS` is reached.
	async fn dequeue_until_capacity<Context: SubsystemContext>(&mut self, ctx: &mut Context, recent_head: Hash) -> Result<()> {
		while self.running_participations.len() < MAX_PARALLEL_PARTICIPATIONS {
			if let Some(req) = self.queue.dequeue() {
				self.fork_participation(ctx, req, recent_head)?;
			} else {
				break
			}
		}
		Ok(())
	}

	/// Fork a participation task in the background.
	fn fork_participation<Context: SubsystemContext>(&mut self, ctx: &mut Context, req: ParticipationRequest, recent_head: Hash) -> Result<()> {
		if self.running_participations.insert(req.candidate_hash().clone()) {
			ctx.spawn("participation-worker", participate(ctx.sender().clone(), recent_head, req).boxed())?;
		}
		Ok(())
	}
}


async fn participate(
	sender: impl SubsystemSender,
	block_hash: Hash,
	req: ParticipationRequest,
) -> Result<WorkerMessage> {

	// in order to validate a candidate we need to start by recovering the
	// available data
	let (recover_available_data_tx, recover_available_data_rx) = oneshot::channel();
	sender.send_message(AvailabilityRecoveryMessage::RecoverAvailableData(
		req.candidate_receipt().clone(),
		req.session(),
		None,
		recover_available_data_tx,
	))
	.await;

	let available_data = match recover_available_data_rx.await? {
		Ok(data) => data,
		Err(RecoveryError::Invalid) => {
			// the available data was recovered but it is invalid, therefore we'll
			// vote negatively for the candidate dispute
			return Ok(WorkerMessage::from_request(req, ValidationResult::Invalid))
		},
		Err(RecoveryError::Unavailable) =>
			return Ok(WorkerMessage::from_request(req, ValidationResult::Unavailable))
	};

	// we also need to fetch the validation code which we can reference by its
	// hash as taken from the candidate descriptor
	let (code_tx, code_rx) = oneshot::channel();
	sender.send_message(RuntimeApiMessage::Request(
		block_hash,
		RuntimeApiRequest::ValidationCodeByHash(
			req.candidate_receipt().descriptor.validation_code_hash,
			code_tx,
		),
	))
	.await;

	let validation_code = match code_rx.await?? {
		Some(code) => code,
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"Validation code unavailable for code hash {:?} in the state of block {:?}",
				req.candidate_receipt().descriptor.validation_code_hash,
				block_hash,
			);

			return Ok(WorkerMessage::from_request(req, ValidationResult::Error))
		},
	};

	// we dispatch a request to store the available data for the candidate. we
	// want to maximize data availability for other potential checkers involved
	// in the dispute
	let (store_available_data_tx, store_available_data_rx) = oneshot::channel();
	ctx.send_message(AvailabilityStoreMessage::StoreAvailableData {
		candidate_hash: req.candidate_hash(),
		n_validators: req.n_validators(),
		available_data: available_data.clone(),
		tx: store_available_data_tx,
	})
	.await;

	match store_available_data_rx.await? {
		Err(err) => {
			tracing::warn!(
				target: LOG_TARGET,
				?err,
				"Failed to store available data for candidate {:?}",
				req.candidate_hash(),
			);
		},
		Ok(()) => {},
	}

	// we issue a request to validate the candidate with the provided exhaustive
	// parameters
	//
	// We use the approval execution timeout because this is intended to
	// be run outside of backing and therefore should be subject to the
	// same level of leeway.
	let (validation_tx, validation_rx) = oneshot::channel();
	sender.send_message(CandidateValidationMessage::ValidateFromExhaustive(
		available_data.validation_data,
		validation_code,
		req.candidate_receipt().descriptor.clone(),
		available_data.pov,
		APPROVAL_EXECUTION_TIMEOUT,
		validation_tx,
	))
	.await;

	// we cast votes (either positive or negative) depending on the outcome of
	// the validation and if valid, whether the commitments hash matches
	match validation_rx.await? {
		Err(err) => {
			tracing::warn!(
				target: LOG_TARGET,
				"Candidate {:?} validation failed with: {:?}",
				req.candidate_hash(),
				err,
			);

			Ok(WorkerMessage::from_request(req, ValidationResult::Invalid))
		},
		Ok(ValidationResult::Invalid(invalid)) => {
			tracing::warn!(
				target: LOG_TARGET,
				"Candidate {:?} considered invalid: {:?}",
				req.candidate_hash(),
				invalid,
			);

			Ok(WorkerMessage::from_request(req, ValidationResult::Invalid))
		},
		Ok(ValidationResult::Valid(commitments, _)) => {
			if commitments.hash() != candidate_receipt.commitments_hash {
				tracing::warn!(
					target: LOG_TARGET,
					expected = ?candidate_receipt.commitments_hash,
					got = ?commitments.hash(),
					"Candidate is valid but commitments hash doesn't match",
				);

				Ok(WorkerMessage::from_request(req, ValidationResult::Invalid))
			} else {
				Ok(WorkerMessage::from_request(req, ValidationResult::Valid))
			}
		},
	}
}
