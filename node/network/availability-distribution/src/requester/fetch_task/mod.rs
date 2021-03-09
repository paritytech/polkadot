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

use futures::channel::mpsc;
use futures::channel::oneshot;
use futures::future::select;
use futures::{FutureExt, SinkExt};

use polkadot_erasure_coding::branch_hash;
use polkadot_node_network_protocol::request_response::{
	request::{OutgoingRequest, RequestError, Requests},
	v1::{AvailabilityFetchingRequest, AvailabilityFetchingResponse},
};
use polkadot_primitives::v1::{
	AuthorityDiscoveryId, BlakeTwo256, ErasureChunk, GroupIndex, Hash, HashT, OccupiedCore,
	SessionIndex,
};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityStoreMessage, NetworkBridgeMessage,
};
use polkadot_subsystem::{SubsystemContext, jaeger};

use crate::{
	error::{Error, Result},
	session_cache::{BadValidators, SessionInfo},
	LOG_TARGET,
	metrics::{Metrics, SUCCEEDED, FAILED},
};

#[cfg(test)]
mod tests;

/// Configuration for a `FetchTask`
///
/// This exists to separate preparation of a `FetchTask` from actual starting it, which is
/// beneficial as this allows as for taking session info by reference.
pub struct FetchTaskConfig {
	prepared_running: Option<RunningTask>,
	live_in: HashSet<Hash>,
}

/// Information about a task fetching an erasure chunk.
pub struct FetchTask {
	/// For what relay parents this task is relevant.
	///
	/// In other words, for which relay chain parents this candidate is considered live.
	/// This is updated on every `ActiveLeavesUpdate` and enables us to know when we can safely
	/// stop keeping track of that candidate/chunk.
	live_in: HashSet<Hash>,

	/// We keep the task around in until `live_in` becomes empty, to make
	/// sure we won't re-fetch an already fetched candidate.
	state: FetchedState,
}

/// State of a particular candidate chunk fetching process.
enum FetchedState {
	/// Chunk fetch has started.
	///
	/// Once the contained `Sender` is dropped, any still running task will be canceled.
	Started(oneshot::Sender<()>),
	/// All relevant live_in have been removed, before we were able to get our chunk.
	Canceled,
}

/// Messages sent from `FetchTask`s to be handled/forwarded.
pub enum FromFetchTask {
	/// Message to other subsystem.
	Message(AllMessages),

	/// Concluded with result.
	///
	/// In case of `None` everything was fine, in case of `Some`, some validators in the group
	/// did not serve us our chunk as expected.
	Concluded(Option<BadValidators>),
}

/// Information a running task needs.
struct RunningTask {
	/// For what session we have been spawned.
	session_index: SessionIndex,

	/// Index of validator group to fetch the chunk from.
	///
	/// Needef for reporting bad validators.
	group_index: GroupIndex,

	/// Validators to request the chunk from.
	///
	/// This vector gets drained during execution of the task (it will be empty afterwards).
	group: Vec<AuthorityDiscoveryId>,

	/// The request to send.
	request: AvailabilityFetchingRequest,

	/// Root hash, for verifying the chunks validity.
	erasure_root: Hash,

	/// Relay parent of the candidate to fetch.
	relay_parent: Hash,

	/// Sender for communicating with other subsystems and reporting results.
	sender: mpsc::Sender<FromFetchTask>,
	
	/// Prometheues metrics for reporting results.
	metrics: Metrics,

	/// Span tracking the fetching of this chunk.
	span: jaeger::Span,
}

impl FetchTaskConfig {
	/// Create a new configuration for a [`FetchTask`].
	///
	/// The result of this function can be passed into [`FetchTask::start`].
	pub fn new(
		leaf: Hash,
		core: &OccupiedCore,
		sender: mpsc::Sender<FromFetchTask>,
		metrics: Metrics,
		session_info: &SessionInfo,
	) -> Self {
		let live_in = vec![leaf].into_iter().collect();

		// Don't run tasks for our backing group:
		if session_info.our_group == core.group_responsible {
			return FetchTaskConfig {
				live_in,
				prepared_running: None,
			};
		}

		let mut span = jaeger::candidate_hash_span(&core.candidate_hash, "availability-distribution");
		span.add_stage(jaeger::Stage::AvailabilityDistribution);

		let prepared_running = RunningTask {
			session_index: session_info.session_index,
			group_index: core.group_responsible,
			group: session_info.validator_groups.get(core.group_responsible.0 as usize)
				.expect("The responsible group of a candidate should be available in the corresponding session. qed.")
				.clone(),
			request: AvailabilityFetchingRequest {
				candidate_hash: core.candidate_hash,
				index: session_info.our_index,
			},
			erasure_root: core.candidate_descriptor.erasure_root,
			relay_parent: core.candidate_descriptor.relay_parent,
			metrics,
			sender,
			span,
		};
		FetchTaskConfig {
			live_in,
			prepared_running: Some(prepared_running),
		}
	}
}

impl FetchTask {
	/// Start fetching a chunk.
	///
	/// A task handling the fetching of the configured chunk will be spawned.
	#[tracing::instrument(level = "trace", skip(config, ctx), fields(subsystem = LOG_TARGET))]
	pub async fn start<Context>(config: FetchTaskConfig, ctx: &mut Context) -> Result<Self>
	where
		Context: SubsystemContext,
	{
		let FetchTaskConfig {
			prepared_running,
			live_in,
		} = config;

		if let Some(running) = prepared_running {
			let (handle, kill) = oneshot::channel();

			ctx.spawn("chunk-fetcher", running.run(kill).boxed())
				.await
				.map_err(|e| Error::SpawnTask(e))?;

			Ok(FetchTask {
				live_in,
				state: FetchedState::Started(handle),
			})
		} else {
			Ok(FetchTask {
				live_in,
				state: FetchedState::Canceled,
			})
		}
	}

	/// Add the given leaf to the relay parents which are making this task relevant.
	///
	/// This is for book keeping, so we know we are already fetching a given chunk.
	pub fn add_leaf(&mut self, leaf: Hash) {
		self.live_in.insert(leaf);
	}

	/// Remove leaves and cancel the task, if it was the last one and the task has still been
	/// fetching.
	pub fn remove_leaves(&mut self, leaves: &HashSet<Hash>) {
		self.live_in.difference(leaves);
		if self.live_in.is_empty() && !self.is_finished() {
			self.state = FetchedState::Canceled
		}
	}

	/// Whether or not there are still relay parents around with this candidate pending
	/// availability.
	pub fn is_live(&self) -> bool {
		!self.live_in.is_empty()
	}

	/// Whether or not this task can be considered finished.
	///
	/// That is, it is either canceled, succeeded or failed.
	pub fn is_finished(&self) -> bool {
		match &self.state {
			FetchedState::Canceled => true,
			FetchedState::Started(sender) => sender.is_canceled(),
		}
	}
}

/// Things that can go wrong in task execution.
#[derive(Debug)]
enum TaskError {
	/// The peer failed to deliver a correct chunk for some reason (has been reported as
	/// appropriate).
	PeerError,
	/// This very node is seemingly shutting down (sending of message failed).
	ShuttingDown,
}

impl RunningTask {
	#[tracing::instrument(level = "trace", skip(self, kill), fields(subsystem = LOG_TARGET))]
	async fn run(self, kill: oneshot::Receiver<()>) {
		// Wait for completion/or cancel.
		let run_it = self.run_inner();
		futures::pin_mut!(run_it);
		let _ = select(run_it, kill).await;
	}

	/// Fetch and store chunk.
	///
	/// Try validators in backing group in order.
	async fn run_inner(mut self) {
		let mut bad_validators = Vec::new();
		let mut label = FAILED;
		let mut count: u32 = 0;
		let mut _span = self.span.child_builder("fetch-task")
			.with_chunk_index(self.request.index.0)
			.with_relay_parent(&self.relay_parent)
			.build();
		// Try validators in reverse order:
		while let Some(validator) = self.group.pop() {
			let _try_span = _span.child("try");
			// Report retries:
			if count > 0 {
				self.metrics.on_retry();
			}
			count +=1;

			// Send request:
			let resp = match self.do_request(&validator).await {
				Ok(resp) => resp,
				Err(TaskError::ShuttingDown) => {
					tracing::info!(
						target: LOG_TARGET,
						"Node seems to be shutting down, canceling fetch task"
					);
					self.metrics.on_fetch(FAILED);
					return
				}
				Err(TaskError::PeerError) => {
					bad_validators.push(validator);
					continue
				}
			};
			let chunk = match resp {
				AvailabilityFetchingResponse::Chunk(resp) => {
					resp.recombine_into_chunk(&self.request)
				}
				AvailabilityFetchingResponse::NoSuchChunk => {
					tracing::debug!(
						target: LOG_TARGET,
						validator = ?validator,
						"Validator did not have our chunk"
					);
					bad_validators.push(validator);
					continue
				}
			};

			// Data genuine?
			if !self.validate_chunk(&validator, &chunk) {
				bad_validators.push(validator);
				continue;
			}

			// Ok, let's store it and be happy:
			self.store_chunk(chunk).await;
			label = SUCCEEDED;
			_span.add_string_tag("success", "true");
			break;
		}
		_span.add_int_tag("tries", count as _);
		self.metrics.on_fetch(label);
		self.conclude(bad_validators).await;
	}

	/// Do request and return response, if successful.
	async fn do_request(
		&mut self,
		validator: &AuthorityDiscoveryId,
	) -> std::result::Result<AvailabilityFetchingResponse, TaskError> {
		let (full_request, response_recv) =
			OutgoingRequest::new(validator.clone(), self.request);
		let requests = Requests::AvailabilityFetching(full_request);

		self.sender
			.send(FromFetchTask::Message(AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendRequests(vec![requests]),
			)))
			.await
			.map_err(|_| TaskError::ShuttingDown)?;

		match response_recv.await {
			Ok(resp) => Ok(resp),
			Err(RequestError::InvalidResponse(err)) => {
				tracing::warn!(
					target: LOG_TARGET,
					origin= ?validator,
					err= ?err,
					"Peer sent us invalid erasure chunk data"
				);
				Err(TaskError::PeerError)
			}
			Err(RequestError::NetworkError(err)) => {
				tracing::warn!(
					target: LOG_TARGET,
					origin= ?validator,
					err= ?err,
					"Some network error occurred when fetching erasure chunk"
				);
				Err(TaskError::PeerError)
			}
			Err(RequestError::Canceled(oneshot::Canceled)) => {
				tracing::warn!(target: LOG_TARGET,
							   origin= ?validator,
							   "Erasure chunk request got canceled");
				Err(TaskError::PeerError)
			}
		}
	}

	fn validate_chunk(&self, validator: &AuthorityDiscoveryId, chunk: &ErasureChunk) -> bool {
		let anticipated_hash =
			match branch_hash(&self.erasure_root, &chunk.proof, chunk.index.0 as usize) {
				Ok(hash) => hash,
				Err(e) => {
					tracing::warn!(
						target: LOG_TARGET,
						candidate_hash = ?self.request.candidate_hash,
						origin = ?validator,
						error = ?e,
						"Failed to calculate chunk merkle proof",
					);
					return false;
				}
			};
		let erasure_chunk_hash = BlakeTwo256::hash(&chunk.chunk);
		if anticipated_hash != erasure_chunk_hash {
			tracing::warn!(target: LOG_TARGET, origin = ?validator,  "Received chunk does not match merkle tree");
			return false;
		}
		true
	}

	/// Store given chunk and log any error.
	async fn store_chunk(&mut self, chunk: ErasureChunk) {
		let (tx, rx) = oneshot::channel();
		let r = self
			.sender
			.send(FromFetchTask::Message(AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::StoreChunk {
					candidate_hash: self.request.candidate_hash,
					relay_parent: self.relay_parent,
					chunk,
					tx,
				},
			)))
			.await;
		if let Err(err) = r {
			tracing::error!(target: LOG_TARGET, err= ?err, "Storing erasure chunk failed, system shutting down?");
		}

		if let Err(oneshot::Canceled) = rx.await {
			tracing::error!(target: LOG_TARGET, "Storing erasure chunk failed");
		}
	}

	/// Tell subsystem we are done.
	async fn conclude(&mut self, bad_validators: Vec<AuthorityDiscoveryId>) {
		let payload = if bad_validators.is_empty() {
			None
		} else {
			Some(BadValidators {
				session_index: self.session_index,
				group_index: self.group_index,
				bad_validators,
			})
		};
		if let Err(err) = self.sender.send(FromFetchTask::Concluded(payload)).await {
			tracing::warn!(
				target: LOG_TARGET,
				err= ?err,
				"Sending concluded message for task failed"
			);
		}
	}
}
