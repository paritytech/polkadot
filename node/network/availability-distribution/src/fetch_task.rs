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
use std::pin::Pin;
use std::rc::Rc;

use futures::channel::mpsc;
use futures::channel::oneshot;

use sc_network::PeerId;

use polkadot_erasure_coding::branch_hash;
use polkadot_node_network_protocol::request_response::{
	request::{OutgoingRequest, RequestError, Requests},
	v1::{AvailabilityFetchingRequest, AvailabilityFetchingResponse},
};
use polkadot_primitives::v1::{
	BlakeTwo256, CandidateDescriptor, CandidateHash, CoreState, ErasureChunk, GroupIndex, Hash,
	HashT, OccupiedCore, SessionIndex, ValidatorId, ValidatorIndex, PARACHAIN_KEY_TYPE_ID,
};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityDistributionMessage, AvailabilityStoreMessage, ChainApiMessage,
	NetworkBridgeEvent, NetworkBridgeMessage, RuntimeApiMessage, RuntimeApiRequest,
};
use polkadot_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	jaeger, ActiveLeavesUpdate, FromOverseer, OverseerSignal, PerLeafSpan, SpawnedSubsystem,
	Subsystem, SubsystemContext, SubsystemError,
};

use super::{session_cache::SessionInfo, LOG_TARGET};

pub struct FetchTask {
	/// For what relay parents this task is relevant.
	///
	/// In other words, for which relay chain parents this candidate is considered live.
	/// This is updated on every `ActiveLeavesUpdate` and enables us to know when we can safely
	/// stop keeping track of that candidate/chunk.
	live_in: HashSet<Hash>,

	/// We keep the task around in state `Fetched` until `live_in` becomes empty, to make
	/// sure we won't re-fetch an already fetched candidate.
	state: FetchedState,

	/// Session information.
	session: Rc<SessionInfo>,
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
	/// In case of `None` everything was fine, in case of `Some` some validators in the group
	/// did not serve us our chunk as expected.
	Concluded(Option<BadValidators>),
}

/// Report of bad validators.
pub struct BadValidators {
	/// The session index that was used.
	pub session_index: SessionIndex,
	/// The group the not properly responding validators are.
	pub group_index: GroupIndex,
	/// The indeces of the bad validators.
	pub bad_validators: Vec<ValidatorIndex>,
}

/// Information a running task needs.
struct RunningTask {
	/// For what session we have been spawned.
	session_index: SessionIndex,

	/// Index of validator group.
	group_index: GroupIndex,

	/// Validators to request the chunk from.
	group: Vec<ValidatorIndex>,

	/// The request to send.
	request: AvailabilityFetchingRequest,

	/// Root hash, for verifying the chunks validity.
	erasure_root: Hash,

	/// Relay parent of the candidate to fetch.
	relay_parent: Hash,

	/// Hash of the candidate we are fetching our chunk for.
	candidate_hash: CandidateHash,

	/// Sender for communicating with other subsystems and reporting results.
	sender: mpsc::Sender<FromFetchTask>,

	/// Receive `Canceled` errors here.
	receiver: oneshot::Receiver<()>,
}

impl FetchTask {
	/// Start fetching a chunk.
	pub async fn start<Context>(
		ctx: &mut Context,
		leaf: Hash,
		core: OccupiedCore,
		session_info: Rc<SessionInfo>,
		sender: mpsc::Sender<FromFetchTask>,
	) -> Self
	where
		Context: SubsystemContext,
	{
		let (handle, receiver) = oneshot::channel();
		let running =  RunningTask {
			session_index: session_info.session_index,
			group_index: core.group_responsible,
			group: session_info.validator_groups.get(core.group_responsible).expect("The responsible group of a candidate should be available in the corresponding session. qed.").clone(),
			request: AvailabilityFetchingRequest {
				candidate_hash: core.candidate_hash,
				index: session_info.our_index,
			},
			erasure_root: core.candidate_descriptor.erasure_root,
			relay_parent: core.candidate_descriptor.relay_parent,
			candidate_hash: core.candidate_hash,
			sender,
			receiver,
		};
		ctx.spawn("chunk-fetcher", Pin::new(Box::new(running.run())))
			.await?;
		FetchTask {
			live_in: HashSet::from(leaf),
			state: FetchedState::Started(handle),
			session: session_info,
		}
	}

	/// Add the given leaf to the relay parents which are making this task relevant.
	pub fn add_leaf(&mut self, leaf: Hash) {
		self.live_in.insert(leaf);
	}

	/// Remove leaves and cancel the task, if it was the last one and the task has still been
	/// fetching.
	pub fn remove_leaves(&mut self, leaves: HashSet<Hash>) {
		self.live_in.difference(leaves);
		if self.live_in.is_empty() {
			// TODO: Make sure, to actually cancel the task.
			self.state = FetchedState::Canceled
		}
	}

	/// Whether or not this task can be considered finished.
	///
	/// That is, it is either canceled, succeeded or failed.
	pub fn is_finished(&self) -> bool {
		match self.state {
			FetchedState::Canceled => true,
			FetchedState::Started(sender) => sender.is_canceled(),
		}
	}

	/// Retrieve the relay parent providing the context for this candidate.
	pub fn get_relay_parent(&self) -> Hash {
		self.descriptor.relay_parent
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

type Result<T> = std::result::Result<T, TaskError>;

impl RunningTask {
	/// Fetch and store chunk.
	///
	/// Try validators in backing group in order.
	async fn run(self) {
		let bad_validators = Vec::new();
		// Try validators in order:
		for index in self.group {
			// Send request:
			let peer_id = self.get_peer_id(index)?;
			let resp = match self.do_request(peer_id).await {
				Ok(resp) => resp,
				Err(TaskError::ShuttingDown) => {
					tracing::info!(
						target: LOG_TARGET,
						"Node seems to be shutting down, canceling fetch task"
					);
					return;
				}
				Err(TaskError::PeerError) => {
					bad_validators.push(index);
					continue;
				}
			};
			let chunk = match resp {
				AvailabilityFetchingResponse::Chunk(chunk) => chunk,
			};

			// Data genuine?
			if !self.validate_chunk(peer_id, &chunk) {
				bad_validators.push(index);
				continue;
			}

			// Ok, let's store it and be happy:
			self.store_chunk(chunk).await;
			break;
		}
		self.conclude(bad_validators);
	}

	/// Do request and return response, if successful.
	async fn do_request(
		&self,
		peer: PeerId,
	) -> std::result::Result<AvailabilityFetchingResponse, TaskError> {
		let (full_request, response_recv) = OutgoingRequest::new(peer, self.request);
		let requests = Requests::AvailabilityFetching(Vec::from(full_request));

		self.sender
			.send(FromFetchTask::Message(
				AllMessages::NetworkBridgeMessage::SendRequests(Vec::from(full_request)),
			))
			.await
			.map_err(|| TaskError::ShuttingDown)?;

		match response_recv.await {
			Ok(resp) => Ok(resp),
			Err(RequestError::InvalidResponse(err)) => {
				tracing::warn!(
					target: LOG_TARGET,
					"Peer sent us invalid erasure chunk data"
				);
				Err(TaskError::PeerError)
			}
			Err(RequestError::NetworkError(err)) => {
				tracing::warn!(
					target: LOG_TARGET,
					"Some network error occurred when fetching erasure chunk"
				);
				Err(TaskError::PeerError)
			}
			Err(RequestError::Canceled(err)) => {
				tracing::warn!(target: LOG_TARGET, "Erasure chunk request got canceled");
				Err(TaskError::PeerError)
			}
		}
	}

	fn validate_chunk(&self, peer_id: &PeerId, chunk: &ErasureChunk) -> bool {
		let anticipated_hash =
			match branch_hash(&self.erasure_root, &chunk.proof, chunk.index as usize) {
				Ok(hash) => hash,
				Err(e) => {
					tracing::trace!(
					target: LOG_TARGET,
					candidate_hash = ?self.candidate_hash,
					origin = ?peer_id,
					error = ?e,
					"Failed to calculate chunk merkle proof",
					);
					return false;
				}
			};
		let erasure_chunk_hash = BlakeTwo256::hash(&chunk.chunk);
		if anticipated_hash != erasure_chunk_hash {
			tracing::warn!(target: LOG_TARGET, origin = ?peer_id,  "Received chunk does not match merkle tree");
			return false;
		}
		true
	}

	fn get_peer_id(index: ValidatorIndex) -> Result<PeerId> {
		panic!("TO BE IMPLEMENTED");
	}

	/// Store given chunk and log any error.
	async fn store_chunk(&self, chunk: ErasureChunk) {
		let (tx, rx) = oneshot::channel();
		self.sender
			.send(FromFetchTask::Message(AllMessages::AvailabilityStore(
				AvailabilityStoreMessage::StoreChunk {
					candidate_hash: self.candidate_hash,
					relay_parent: self.relay_parent,
					chunk,
					tx,
				},
			)))
			.await;

		if let Err(oneshot::Canceled) = rx.await {
			tracing::error!(target: LOG_TARGET, "Storing erasure chunk failed");
		}
	}

	/// Tell subsystem we are done.
	async fn conclude(&self, bad_validators: Vec<ValidatorIndex>) {
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
