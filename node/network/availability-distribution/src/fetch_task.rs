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
use std::rc::Rc;

use futures::channel::oneshot;
use v1::AvailabilityFetchingResponse;

use super::{session_cache::SessionInfo, LOG_TARGET};
use polkadot_node_network_protocol::request_response::v1;
use polkadot_primitives::v1::{
	BlakeTwo256, CandidateDescriptor, CandidateHash, CoreState, ErasureChunk, Hash, HashT,
	OccupiedCore, SessionIndex, ValidatorId, ValidatorIndex, PARACHAIN_KEY_TYPE_ID,
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
	/// Chunk is currently being fetched.
	///
	/// Once the contained `Sender` is dropped, any still running task will be canceled.
	Fetching(oneshot::Sender<()>),
	/// Chunk has already been fetched successfully.
	Fetched,
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
	request: v1::AvailabilityFetchingRequest,

	/// Root hash, for verifying the chunks validity.
	erasure_root: Hash,

	/// Relay parent of the candidate to fetch.
	relay_parent: Hash,

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
			request: v1::AvailabilityFetchingRequest {
				candidate_hash: core.candidate_hash,
				index: session_info.our_index,
			},
			erasure_root: core.candidate_descriptor.erasure_root,
			relay_parent: core.candidate_descriptor.relay_parent,
			sender,
			receiver,
		};
		ctx.spawn("chunk-fetcher", Pin::new(Box::new(running.run())))
			.await?;
		FetchTask {
			live_in: HashSet::from(leaf),
			state: FetchedState::Fetching(handle),
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
	/// That is, it is either canceled or succeeded fetching the chunk.
	pub fn is_finished(&self) -> bool {
		match self.state {
			FetchedState::Fetched | FetchedState::Canceled => true,
			FetchedState::Fetching => false,
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
	async fn run(self) {
		let bad_validators = Vec::new();
		// Try validators in order:
		for index in self.group {
			// Send request:
			let resp = match do_request(index).await {
				Ok(resp) => resp,
				Err(TaskError::ShuttingDown) => {
					tracking::info("Node seems to be shutting down, canceling fetch task");
					return;
				}
				Err(TaskError::PeerError) => {
					bad_validators.push(index);
					continue;
				}
			};

			// Data valid?
			if !self.validate_response(&resp) {
				bad_validators.push(index);
				continue;
			}

			// Ok, let's store it and be happy.
			store_response(resp);
			break;
		}
		conclude(bad_validators);
	}

	/// Do request and return response, if successful.
	///
	/// Will also report peer if not successful.
	async fn do_request(
		&self,
		validator: ValidatorIndex,
	) -> std::result::Result<v1::AvailabilityFetchingResponse, TaskError> {
		let peer = self.get_peer_id(index)?;
		let (full_request, response_recv) =
			Requests::AvailabilityFetching(OutgoingRequest::new(peer, self.request));

		self.sender
			.send(FromFetchTask::Message(
				AllMessages::NetworkBridgeMessage::SendRequests(Vec::from(full_request)),
			))
			.await
			.map_err(|| TaskError::ShuttingDown)?;

		match response_recv.await {
			Ok(resp) => Some(resp),
			Err(RequestError::InvalidResponse(err)) => {}
			Err(RequestError::NetworkError(err)) => {}
			Err(RequestError::Canceled(err)) => {}
		}
		Err(PeerError)
	}

	fn get_peer_id(index: ValidatorIndex) -> Result<PeerId> {
		panic!("TO BE IMPLEMENTED");
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
				LOG_TARGET,
				err: ?err,
				"Sending concluded message for task failed"
			);
		}
	}
}
