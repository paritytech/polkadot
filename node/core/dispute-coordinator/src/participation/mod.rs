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
#[cfg(test)]
use std::time::Duration;

use futures::{
	channel::{mpsc, oneshot},
	FutureExt, SinkExt,
};
#[cfg(test)]
use futures_timer::Delay;

use polkadot_node_primitives::{ValidationResult, APPROVAL_EXECUTION_TIMEOUT};
use polkadot_node_subsystem::{
	messages::{AvailabilityRecoveryMessage, CandidateValidationMessage},
	overseer, ActiveLeavesUpdate, RecoveryError,
};
use polkadot_node_subsystem_util::runtime::get_validation_code_by_hash;
use polkadot_primitives::v2::{BlockNumber, CandidateHash, CandidateReceipt, Hash, SessionIndex};

use crate::LOG_TARGET;

use crate::error::{FatalError, FatalResult, Result};

#[cfg(test)]
mod tests;
#[cfg(test)]
pub use tests::{participation_full_happy_path, participation_missing_availability};

mod queues;
use queues::Queues;
pub use queues::{ParticipationPriority, ParticipationRequest, QueueError};

/// How many participation processes do we want to run in parallel the most.
///
/// This should be a relatively low value, while we might have a speedup once we fetched the data,
/// due to multi-core architectures, but the fetching itself can not be improved by parallel
/// requests. This means that higher numbers make it harder for a single dispute to resolve fast.
const MAX_PARALLEL_PARTICIPATIONS: usize = 3;

/// Keep track of disputes we need to participate in.
///
/// - Prioritize and queue participations
/// - Dequeue participation requests in order and launch participation worker.
pub struct Participation {
	/// Participations currently being processed.
	running_participations: HashSet<CandidateHash>,
	/// Priority and best effort queues.
	queue: Queues,
	/// Sender to be passed to worker tasks.
	worker_sender: WorkerMessageSender,
	/// Some recent block for retrieving validation code from chain.
	recent_block: Option<(BlockNumber, Hash)>,
}

/// Message from worker tasks.
#[derive(Debug)]
pub struct WorkerMessage(ParticipationStatement);

/// Sender use by worker tasks.
pub type WorkerMessageSender = mpsc::Sender<WorkerMessage>;

/// Receiver to receive messages from worker tasks.
pub type WorkerMessageReceiver = mpsc::Receiver<WorkerMessage>;

/// Statement as result of the validation process.
#[derive(Debug)]
pub struct ParticipationStatement {
	/// Relevant session.
	pub session: SessionIndex,
	/// The candidate the worker has been spawned for.
	pub candidate_hash: CandidateHash,
	/// Used receipt.
	pub candidate_receipt: CandidateReceipt,
	/// Actual result.
	pub outcome: ParticipationOutcome,
}

/// Outcome of the validation process.
#[derive(Copy, Clone, Debug)]
pub enum ParticipationOutcome {
	/// Candidate was found to be valid.
	Valid,
	/// Candidate was found to be invalid.
	Invalid,
	/// Candidate was found to be unavailable.
	Unavailable,
	/// Something went wrong (bug), details can be found in the logs.
	Error,
}

impl ParticipationOutcome {
	/// If validation was successful, get whether the candidate was valid or invalid.
	pub fn validity(self) -> Option<bool> {
		match self {
			Self::Valid => Some(true),
			Self::Invalid => Some(false),
			Self::Unavailable | Self::Error => None,
		}
	}
}

impl WorkerMessage {
	fn from_request(req: ParticipationRequest, outcome: ParticipationOutcome) -> Self {
		let session = req.session();
		let (candidate_hash, candidate_receipt) = req.into_candidate_info();
		Self(ParticipationStatement { session, candidate_hash, candidate_receipt, outcome })
	}
}

#[overseer::contextbounds(DisputeCoordinator, prefix = self::overseer)]
impl Participation {
	/// Get ready for managing dispute participation requests.
	///
	/// The passed in sender will be used by background workers to communicate back their results.
	/// The calling context should make sure to call `Participation::on_worker_message()` for the
	/// received messages.
	pub fn new(sender: WorkerMessageSender) -> Self {
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
	pub async fn queue_participation<Context>(
		&mut self,
		ctx: &mut Context,
		priority: ParticipationPriority,
		req: ParticipationRequest,
	) -> Result<()> {
		// Participation already running - we can ignore that request:
		if self.running_participations.contains(req.candidate_hash()) {
			return Ok(())
		}
		// Available capacity - participate right away (if we already have a recent block):
		if let Some((_, h)) = self.recent_block {
			if self.running_participations.len() < MAX_PARALLEL_PARTICIPATIONS {
				self.fork_participation(ctx, req, h)?;
				return Ok(())
			}
		}
		// Out of capacity/no recent block yet - queue:
		self.queue.queue(ctx.sender(), priority, req).await
	}

	/// Message from a worker task was received - get the outcome.
	///
	/// Call this function to keep participations going and to receive `ParticipationStatement`s.
	///
	/// This message has to be called for each received worker message, in order to make sure
	/// enough participation processes are running at any given time.
	///
	/// Returns: The received `ParticipationStatement` or a fatal error, in case
	/// something went wrong when dequeuing more requests (tasks could not be spawned).
	pub async fn get_participation_result<Context>(
		&mut self,
		ctx: &mut Context,
		msg: WorkerMessage,
	) -> FatalResult<ParticipationStatement> {
		let WorkerMessage(statement) = msg;
		self.running_participations.remove(&statement.candidate_hash);
		let recent_block = self.recent_block.expect("We never ever reset recent_block to `None` and we already received a result, so it must have been set before. qed.");
		self.dequeue_until_capacity(ctx, recent_block.1).await?;
		Ok(statement)
	}

	/// Process active leaves update.
	///
	/// Make sure we to dequeue participations if that became possible and update most recent
	/// block.
	pub async fn process_active_leaves_update<Context>(
		&mut self,
		ctx: &mut Context,
		update: &ActiveLeavesUpdate,
	) -> FatalResult<()> {
		if let Some(activated) = &update.activated {
			match self.recent_block {
				None => {
					self.recent_block = Some((activated.number, activated.hash));
					// Work got potentially unblocked:
					self.dequeue_until_capacity(ctx, activated.hash).await?;
				},
				Some((number, _)) if activated.number > number => {
					self.recent_block = Some((activated.number, activated.hash));
				},
				Some(_) => {},
			}
		}
		Ok(())
	}

	/// Dequeue until `MAX_PARALLEL_PARTICIPATIONS` is reached.
	async fn dequeue_until_capacity<Context>(
		&mut self,
		ctx: &mut Context,
		recent_head: Hash,
	) -> FatalResult<()> {
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
	fn fork_participation<Context>(
		&mut self,
		ctx: &mut Context,
		req: ParticipationRequest,
		recent_head: Hash,
	) -> FatalResult<()> {
		if self.running_participations.insert(*req.candidate_hash()) {
			let sender = ctx.sender().clone();
			ctx.spawn(
				"participation-worker",
				participate(self.worker_sender.clone(), sender, recent_head, req).boxed(),
			)
			.map_err(FatalError::SpawnFailed)?;
		}
		Ok(())
	}
}

async fn participate(
	mut result_sender: WorkerMessageSender,
	mut sender: impl overseer::DisputeCoordinatorSenderTrait,
	block_hash: Hash,
	req: ParticipationRequest,
) {
	#[cfg(test)]
	// Hack for tests, so we get recovery messages not too early.
	Delay::new(Duration::from_millis(100)).await;
	// in order to validate a candidate we need to start by recovering the
	// available data
	let (recover_available_data_tx, recover_available_data_rx) = oneshot::channel();
	sender
		.send_message(AvailabilityRecoveryMessage::RecoverAvailableData(
			req.candidate_receipt().clone(),
			req.session(),
			None,
			recover_available_data_tx,
		))
		.await;

	let available_data = match recover_available_data_rx.await {
		Err(oneshot::Canceled) => {
			gum::warn!(
				target: LOG_TARGET,
				"`Oneshot` got cancelled when recovering available data {:?}",
				req.candidate_hash(),
			);
			send_result(&mut result_sender, req, ParticipationOutcome::Error).await;
			return
		},
		Ok(Ok(data)) => data,
		Ok(Err(RecoveryError::Invalid)) => {
			// the available data was recovered but it is invalid, therefore we'll
			// vote negatively for the candidate dispute
			send_result(&mut result_sender, req, ParticipationOutcome::Invalid).await;
			return
		},
		Ok(Err(RecoveryError::Unavailable)) => {
			send_result(&mut result_sender, req, ParticipationOutcome::Unavailable).await;
			return
		},
	};

	// we also need to fetch the validation code which we can reference by its
	// hash as taken from the candidate descriptor
	let validation_code = match get_validation_code_by_hash(
		&mut sender,
		block_hash,
		req.candidate_receipt().descriptor.validation_code_hash,
	)
	.await
	{
		Ok(Some(code)) => code,
		Ok(None) => {
			gum::warn!(
				target: LOG_TARGET,
				"Validation code unavailable for code hash {:?} in the state of block {:?}",
				req.candidate_receipt().descriptor.validation_code_hash,
				block_hash,
			);

			send_result(&mut result_sender, req, ParticipationOutcome::Error).await;
			return
		},
		Err(err) => {
			gum::warn!(target: LOG_TARGET, ?err, "Error when fetching validation code.");
			send_result(&mut result_sender, req, ParticipationOutcome::Error).await;
			return
		},
	};

	// Issue a request to validate the candidate with the provided exhaustive
	// parameters
	//
	// We use the approval execution timeout because this is intended to
	// be run outside of backing and therefore should be subject to the
	// same level of leeway.
	let (validation_tx, validation_rx) = oneshot::channel();
	sender
		.send_message(CandidateValidationMessage::ValidateFromExhaustive(
			available_data.validation_data,
			validation_code,
			req.candidate_receipt().clone(),
			available_data.pov,
			APPROVAL_EXECUTION_TIMEOUT,
			validation_tx,
		))
		.await;

	// we cast votes (either positive or negative) depending on the outcome of
	// the validation and if valid, whether the commitments hash matches
	match validation_rx.await {
		Err(oneshot::Canceled) => {
			gum::warn!(
				target: LOG_TARGET,
				"`Oneshot` got cancelled when validating candidate {:?}",
				req.candidate_hash(),
			);
			send_result(&mut result_sender, req, ParticipationOutcome::Error).await;
			return
		},
		Ok(Err(err)) => {
			gum::warn!(
				target: LOG_TARGET,
				"Candidate {:?} validation failed with: {:?}",
				req.candidate_hash(),
				err,
			);

			send_result(&mut result_sender, req, ParticipationOutcome::Invalid).await;
		},

		Ok(Ok(ValidationResult::Invalid(invalid))) => {
			gum::warn!(
				target: LOG_TARGET,
				"Candidate {:?} considered invalid: {:?}",
				req.candidate_hash(),
				invalid,
			);

			send_result(&mut result_sender, req, ParticipationOutcome::Invalid).await;
		},
		Ok(Ok(ValidationResult::Valid(_, _))) => {
			send_result(&mut result_sender, req, ParticipationOutcome::Valid).await;
		},
	}
}

/// Helper function for sending the result back and report any error.
async fn send_result(
	sender: &mut WorkerMessageSender,
	req: ParticipationRequest,
	outcome: ParticipationOutcome,
) {
	if let Err(err) = sender.feed(WorkerMessage::from_request(req, outcome)).await {
		gum::error!(
			target: LOG_TARGET,
			?err,
			"Sending back participation result failed. Dispute coordinator not working properly!"
		);
	}
}
