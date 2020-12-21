// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Availability Recovery Subsystem of Polkadot.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::time::Duration;

use futures::prelude::*;
use futures::stream::FuturesUnordered;
use futures::channel::{oneshot, mpsc};
use lru::LruCache;

use polkadot_primitives::v1::{
	AvailableData, CandidateReceipt, CandidateHash,
	Hash, ErasureChunk, ValidatorId, ValidatorIndex,
	SessionInfo, SessionIndex, BlakeTwo256, HashT,
};
use polkadot_subsystem::{
	SubsystemContext, SubsystemResult, SubsystemError, Subsystem, SpawnedSubsystem, FromOverseer,
	OverseerSignal, ActiveLeavesUpdate,
	errors::RecoveryError,
	messages::{
		AvailabilityStoreMessage, AvailabilityRecoveryMessage, AllMessages, NetworkBridgeMessage,
	},
};
use polkadot_node_network_protocol::{
	v1 as protocol_v1, NetworkBridgeEvent, PeerId, ReputationChange as Rep, RequestId,
};
use polkadot_node_subsystem_util::{
	Timeout, TimeoutExt,
	validator_discovery,
	request_session_index_for_child_ctx,
	request_session_info_ctx,
};
use polkadot_erasure_coding::{branch_hash, recovery_threshold};
mod error;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "availability_recovery";

const COST_MERKLE_PROOF_INVALID: Rep = Rep::new(-100, "Merkle proof was invalid");
const COST_UNEXPECTED_CHUNK: Rep = Rep::new(-100, "Peer has sent an unexpected chunk");

// How many parallel requests interaction should have going at once.
const N_PARALLEL: usize = 50;

// Size of the LRU cache where we keep recovered data.
const LRU_SIZE: usize = 16;

// A timeout for a chunk request.
const CHUNK_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// The Availability Recovery Subsystem.
pub struct AvailabilityRecoverySubsystem;

type ChunkResponse = Result<(PeerId, ErasureChunk), RecoveryError>;

/// Data we keep around for every chunk that we are awaiting.
struct AwaitedChunk {
	/// Index of the validator we have requested this chunk from.
	validator_index: ValidatorIndex,

	/// The hash of the candidate the chunks belongs to.
	candidate_hash: CandidateHash,

	/// Result sender.
	response: oneshot::Sender<ChunkResponse>,
}

/// Accumulate all awaiting sides for some particular `AvailableData`.
struct InteractionHandle {
	awaiting: Vec<oneshot::Sender<Result<AvailableData, RecoveryError>>>,
}

/// A message received by main code from an async `Interaction` task.
#[derive(Debug)]
enum FromInteraction {
	/// An interaction concluded.
	Concluded(CandidateHash, Result<AvailableData, RecoveryError>),

	/// Make a request of a particular chunk from a particular validator.
	MakeRequest(
		ValidatorId,
		CandidateHash,
		ValidatorIndex,
		oneshot::Sender<ChunkResponse>,
	),

	/// Report a peer.
	ReportPeer(
		PeerId,
		Rep,
	),
}

/// A state of a single interaction reconstructing an available data.
struct Interaction {
	/// A communication channel with the `State`.
	to_state: mpsc::Sender<FromInteraction>,

	/// Validators relevant to this `Interaction`.
	validators: Vec<ValidatorId>,

	/// A random shuffling of the validators which indicates the order in which we connect
	/// to the validators and request the chunk from them.
	shuffling: Vec<ValidatorIndex>,

	/// The number of pieces needed.
	threshold: usize,

	/// A hash of the relevant candidate.
	candidate_hash: CandidateHash,

	/// The root of the erasure encoding of the para block.
	erasure_root: Hash,

	/// The chunks that we have received from peers.
	received_chunks: HashMap<PeerId, ErasureChunk>,

	/// The chunk requests that are waiting to complete.
	requesting_chunks: FuturesUnordered<Timeout<oneshot::Receiver<ChunkResponse>>>,
}

impl Interaction {
	async fn run(mut self) {
		loop {
			// If it's empty and requesting_chunks is empty,
			// break and issue a FromInteraction::Concluded(RecoveryError::Unavailable)
			if self.requesting_chunks.is_empty() && self.shuffling.is_empty() {
				if self.to_state.send(FromInteraction::Concluded(
					self.candidate_hash,
					Err(RecoveryError::Unavailable),
				)).await.is_err() {
					return;
				};
				break;
			}

			while self.requesting_chunks.len() < N_PARALLEL {
				if let Some(validator_index) = self.shuffling.pop() {
					let (tx, rx) = oneshot::channel();

					if self.to_state.send(FromInteraction::MakeRequest(
						self.validators[validator_index as usize].clone(),
						self.candidate_hash.clone(),
						validator_index,
						tx,
					)).await.is_err() {
						return;
					};

					self.requesting_chunks.push(rx.timeout(CHUNK_REQUEST_TIMEOUT));
				} else {
					break;
				}
			}

			// Check if the requesting chunks is not empty not to poll to completion.
			if !self.requesting_chunks.is_empty() {
				// Poll for new updates from requesting_chunks.
				while let Some(request_result) = self.requesting_chunks.next().await {
					match request_result {
						Some(Ok(Ok((peer_id, chunk)))) => {
							// Check merkle proofs of any received chunks, and any failures should
							// lead to issuance of a FromInteraction::ReportPeer message.
							if let Ok(anticipated_hash) = branch_hash(
								&self.erasure_root,
								&chunk.proof,
								chunk.index as usize,
							) {
								let erasure_chunk_hash = BlakeTwo256::hash(&chunk.chunk);

								if erasure_chunk_hash != anticipated_hash {
									if self.to_state.send(FromInteraction::ReportPeer(
											peer_id,
											COST_MERKLE_PROOF_INVALID,
									)).await.is_err() {
										return;
									}
								} else {
									self.received_chunks.insert(peer_id, chunk);
								}
							} else {
								if self.to_state.send(FromInteraction::ReportPeer(
										peer_id,
										COST_MERKLE_PROOF_INVALID,
								)).await.is_err() {
									return;
								}
							}
						}
						Some(Err(e)) => {
							tracing::debug!(
								target: LOG_TARGET,
								err = ?e,
								"A response channel was cacelled while waiting for a chunk",
							);
						}
						Some(Ok(Err(e))) => {
							tracing::debug!(
								target: LOG_TARGET,
								err = ?e,
								"A chunk request ended with an error",
							);
						}
						None => {
							tracing::debug!(
								target: LOG_TARGET,
								"A chunk request has timed out",
							)
						}
					}
				}
			}

			// If received_chunks has more than threshold entries, attempt to recover the data.
			// If that fails, or a re-encoding of it doesn't match the expected erasure root,
			// break and issue a FromInteraction::Concluded(RecoveryError::Invalid).
			// Otherwise, issue a FromInteraction::Concluded(Ok(())).
			if self.received_chunks.len() >= self.threshold {
				let concluded = match polkadot_erasure_coding::reconstruct_v1(
					self.validators.len(),
					self.received_chunks.values().map(|c| (&c.chunk[..], c.index as usize)),
				) {
					Ok(data) => FromInteraction::Concluded(self.candidate_hash.clone(), Ok(data)),
					Err(_) => FromInteraction::Concluded(self.candidate_hash.clone(), Err(RecoveryError::Invalid)),
				};

				if let Err(e) = self.to_state.send(concluded).await {
					tracing::warn!(
						target: LOG_TARGET,
						err = ?e,
						"Failed to send Concluded result from interaction",
					);
				}
				return;
			}
		}
	}
}

struct State {
	/// Each interaction is implemented as its own async task,
	/// and these handles are for communicating with them.
	interactions: HashMap<CandidateHash, InteractionHandle>,

	/// A recent block hash for which state should be available.
	live_block_hash: Hash,

	/// We are waiting for these validators to connect and as soon as they
	/// do to request the needed chunks we are awaitinf for.
	discovering_validators: HashMap<ValidatorId, Vec<AwaitedChunk>>,

	/// Requests that we have issued to the already connected validators
	/// about the chunks we are interested in.
	live_chunk_requests: HashMap<RequestId, (PeerId, AwaitedChunk)>,

	/// Derive request ids from this.
	next_request_id: RequestId,

	/// Connect to relevant groups of validators at different relay parents.
	connection_requests: validator_discovery::ConnectionRequests,

	/// interaction communication. This is cloned and given to interactions that are spun up.
	from_interaction_tx: mpsc::Sender<FromInteraction>,

	/// receiver for messages from interactions.
	from_interaction_rx: mpsc::Receiver<FromInteraction>,

	/// An LRU cache of recently recovered data.
	availability_lru: LruCache<CandidateHash, Result<AvailableData, RecoveryError>>,
}

impl Default for State {
	fn default() -> Self {
		let (from_interaction_tx, from_interaction_rx) = mpsc::channel(16);

		Self {
			from_interaction_tx,
			from_interaction_rx,
			interactions: HashMap::new(),
			live_block_hash: Hash::default(),
			discovering_validators: HashMap::new(),
			live_chunk_requests: HashMap::new(),
			next_request_id: 0,
			connection_requests: Default::default(),
			availability_lru: LruCache::new(LRU_SIZE),
		}
	}
}

impl<C> Subsystem<C> for AvailabilityRecoverySubsystem
	where C: SubsystemContext<Message = AvailabilityRecoveryMessage>
{
	fn start(self, ctx: C) -> SpawnedSubsystem {
		let future = self.run(ctx)
			.map_err(|e| SubsystemError::with_origin("availability-recovery", e))
			.boxed();
		SpawnedSubsystem {
			name: "availability-recovery-subsystem",
			future,
		}
	}
}

/// Handles a signal from the overseer.
async fn handle_signal(
	state: &mut State,
	signal: OverseerSignal,
) -> SubsystemResult<bool> {
	match signal {
		OverseerSignal::Conclude => Ok(true),
		OverseerSignal::ActiveLeaves(ActiveLeavesUpdate { activated, .. }) => {
			// if activated is non-empty, set state.live_block_hash to the first block in Activated.
			if let Some(hash) = activated.get(0) {
				state.live_block_hash = *hash;
			}

			Ok(false)
		}
		_ => Ok(false),
	}
}

/// Report a reputation change for a peer.
async fn report_peer(
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	peer: PeerId,
	rep: Rep,
) {
	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::ReportPeer(peer, rep))).await;
}

/// Machinery around launching interactions into the background.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn launch_interaction(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	session_index: SessionIndex,
	session_info: SessionInfo,
	receipt: CandidateReceipt,
	response_sender: oneshot::Sender<Result<AvailableData, RecoveryError>>,
) -> error::Result<()> {
	use rand::seq::SliceRandom;
	use rand::thread_rng;

	let threshold = recovery_threshold(session_info.validators.len())?;
	let to_state = state.from_interaction_tx.clone();
	let candidate_hash = receipt.hash();
	let erasure_root = receipt.descriptor.erasure_root;
	let validators = session_info.validators.clone();
	let mut shuffling: Vec<_> = (0..validators.len() as u32).collect();

	state.interactions.insert(
		candidate_hash.clone(),
		InteractionHandle {
			awaiting: vec![response_sender],
		}
	);

	{
		// make borrow checker happy.
		let mut rng = thread_rng();
		shuffling.shuffle(&mut rng);
	}

	let interaction = Interaction {
		to_state,
		validators,
		shuffling,
		threshold,
		candidate_hash,
		erasure_root,
		received_chunks: HashMap::new(),
		requesting_chunks: FuturesUnordered::new(),
	};

	if let Err(e) = ctx.spawn("recovery interaction", interaction.run().boxed()).await {
		tracing::warn!(
			target: LOG_TARGET,
			err = ?e,
			"Failed to spawn a recovery interaction task",
		);
	}

	Ok(())
}

/// Handles an availability recovery request.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_recover(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	receipt: CandidateReceipt,
	response_sender: oneshot::Sender<Result<AvailableData, RecoveryError>>,
) -> error::Result<()> {
	let candidate_hash = receipt.hash();

	if let Some(result) = state.availability_lru.get(&candidate_hash) {
		if let Err(e) = response_sender.send(result.clone()) {
			tracing::warn!(
				target: LOG_TARGET,
				err = ?e,
				"Error responding with an availability recovery result",
			);
		}
		return Ok(());
	}

	if let Some(interaction) = state.interactions.get_mut(&candidate_hash) {
		interaction.awaiting.push(response_sender);
		return Ok(());
	}

	let session_index = request_session_index_for_child_ctx(
		state.live_block_hash,
		ctx,
	).await?.await.map_err(error::Error::CanceledSessionIndex)??;

	let session_info = request_session_info_ctx(
		state.live_block_hash,
		session_index,
		ctx,
	).await?.await.map_err(error::Error::CanceledSessionInfo)??;

	match session_info {
		Some(session_info) => {
			launch_interaction(
				state,
				ctx,
				session_index,
				session_info,
				receipt,
				response_sender,
			).await
		}
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"SessionInfo is `None` at {}", state.live_block_hash,
			);
			Ok(())
		}
	}
}

/// Queries a chunk from av-store.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn query_chunk(
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	candidate_hash: CandidateHash,
	validator_index: ValidatorIndex,
) -> error::Result<Option<ErasureChunk>> {
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(
		AvailabilityStoreMessage::QueryChunk(candidate_hash, validator_index, tx),
	)).await;

	Ok(rx.await.map_err(error::Error::CanceledQueryChunk)?)
}

/// Handles message from interaction.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_from_interaction(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	from_interaction: FromInteraction,
) -> error::Result<()> {
	match from_interaction {
		FromInteraction::Concluded(candidate_hash, result) => {
			// Load the entry from the interactions map.
			// It should always exist, if not for logic errors.
			if let Some(interaction) = state.interactions.remove(&candidate_hash) {
				// Send the result to each member of awaiting.
				for awaiting in interaction.awaiting {
					if let Err(_) = awaiting.send(result.clone()) {
						tracing::debug!(
							target: LOG_TARGET,
							"An awaiting side of the interaction has been canceled",
						);
					}
				}
			} else {
				tracing::warn!(
					target: LOG_TARGET,
					"Interaction under candidate hash {} is missing",
					candidate_hash,
				);
			}

			state.availability_lru.put(candidate_hash, result);
		}
		FromInteraction::MakeRequest(id, candidate_hash, validator_index, response) => {
			let relay_parent = state.live_block_hash;

			// Take the validators we are already connecting to and merge them with the
			// newly requested one to create a new connection request.
			let new_discovering_validators_set = state.discovering_validators.keys()
				.cloned()
				.chain(std::iter::once(id.clone()))
				.collect();

			// Issue a NetworkBridgeMessage::ConnectToValidators.
			// Add the stream of connected validator events to state.connecting_validators.
			match validator_discovery::connect_to_validators(
				ctx,
				relay_parent,
				new_discovering_validators_set,
			).await {
				Ok(new_connection_request) => {
					state.connection_requests.put(relay_parent, new_connection_request);
					// Add an AwaitedChunk to the discovering_validators map under discovery_pub.
					let awaited_chunk = AwaitedChunk {
						validator_index,
						candidate_hash,
						response,
					};

					state.discovering_validators.entry(id).or_default().push(awaited_chunk);
				}
				Err(e) => {
					tracing::debug!(
						target: LOG_TARGET,
						err = ?e,
						"Failed to create a validator connection request",
					);
				}
			}
		}
		FromInteraction::ReportPeer(peer_id, rep) => {
			report_peer(ctx, peer_id, rep).await;
		}
	}

	Ok(())
}

/// Handles a network bridge update.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn handle_network_update(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	update: NetworkBridgeEvent<protocol_v1::AvailabilityRecoveryMessage>,
) -> error::Result<()> {
	match update {
		NetworkBridgeEvent::PeerMessage(peer, message) => {
			match message {
				protocol_v1::AvailabilityRecoveryMessage::RequestChunk(
					request_id,
					candidate_hash,
					validator_index,
				) => {
					// Issue a
					// AvailabilityStore::QueryChunk(candidate-hash, validator_index, response)
					// message.
					let chunk = query_chunk(ctx, candidate_hash, validator_index).await?;

					// Whatever the result, issue an
					// AvailabilityRecoveryV1Message::Chunk(r_id, response) message.
					let wire_message = protocol_v1::AvailabilityRecoveryMessage::Chunk(
						request_id,
						chunk,
					);

					ctx.send_message(AllMessages::NetworkBridge(
						NetworkBridgeMessage::SendValidationMessage(
							vec![peer],
							protocol_v1::ValidationProtocol::AvailabilityRecovery(wire_message),
						),
					)).await;
				}
				protocol_v1::AvailabilityRecoveryMessage::Chunk(request_id, chunk) => {
					match state.live_chunk_requests.remove(&request_id) {
						None => {
							// If there doesn't exist one, report the peer and return.
							report_peer(ctx, peer, COST_UNEXPECTED_CHUNK).await;
						}
						Some((peer_id, awaited_chunk)) if peer_id == peer => {
							// If there exists an entry under r_id, remove it.
							// Send the chunk response on the awaited_chunk for the interaction to handle.
							if let Some(chunk) = chunk {
								if awaited_chunk.response.send(Ok((peer_id, chunk))).is_err() {
									tracing::debug!(
										target: LOG_TARGET,
										"A sending side of the recovery request is closed",
									);
								}
							}
						}
						Some(a) => {
							// If the peer in the entry doesn't match the sending peer,
							// reinstate the entry, report the peer, and return
							state.live_chunk_requests.insert(request_id, a);
						}
					}
				}
			}
		}
		// We do not really need to track the peers' views in this subsystem
		// since the peers are _required_ to have the data we are interested in.
		NetworkBridgeEvent::PeerViewChange(_, _) => {}
		NetworkBridgeEvent::OurViewChange(_) => {}
		// All peer connections are handled via validator discovery API.
		NetworkBridgeEvent::PeerConnected(_, _) => {}
		NetworkBridgeEvent::PeerDisconnected(_) => {}
	}

	Ok(())
}

/// Issues a chunk request to the validator we've been waiting for to connect to us.
async fn issue_chunk_request(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	peer_id: PeerId,
	awaited_chunk: AwaitedChunk,
) -> error::Result<()> {
	let request_id = state.next_request_id;
	state.next_request_id += 1;

	let wire_message = protocol_v1::AvailabilityRecoveryMessage::RequestChunk(
		request_id,
		awaited_chunk.candidate_hash,
		awaited_chunk.validator_index,
	);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::SendValidationMessage(
			vec![peer_id.clone()],
			protocol_v1::ValidationProtocol::AvailabilityRecovery(wire_message),
		),
	)).await;

	state.live_chunk_requests.insert(request_id, (peer_id, awaited_chunk));

	Ok(())
}

/// Handles a newly connected validator in the context of some relay leaf.
async fn handle_validator_connected(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	validator_id: ValidatorId,
	peer_id: PeerId,
) -> error::Result<()> {
	if let Some(discovering) = state.discovering_validators.remove(&validator_id) {
		for chunk in discovering {
			issue_chunk_request(state, ctx, peer_id.clone(), chunk).await?;
		}
	}

	Ok(())
}

impl AvailabilityRecoverySubsystem {
	/// Create a new instance of `AvailabilityRecoverySubsystem`.
	pub fn new() -> Self {
		Self
	}

	async fn run(
		self,
		mut ctx: impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	) -> SubsystemResult<()> {
		let mut state = State::default();

		loop {
			futures::select_biased! {
				v = state.connection_requests.next().fuse() => {
					if let Err(e) = handle_validator_connected(
						&mut state,
						&mut ctx,
						v.validator_id,
						v.peer_id,
					).await {
						tracing::warn!(
							target: LOG_TARGET,
							err = ?e,
							"Failed to handle a newly connected validator",
						);
					}
				}
				v = ctx.recv().fuse() => {
					match v? {
						FromOverseer::Signal(signal) => if handle_signal(
							&mut state,
							signal,
						).await? {
							return Ok(());
						}
						FromOverseer::Communication { msg } => {
							match msg {
								AvailabilityRecoveryMessage::RecoverAvailableData(
									receipt,
									response_sender,
								) => {
									if let Err(e) = handle_recover(
										&mut state,
										&mut ctx,
										receipt,
										response_sender,
									).await {
										tracing::warn!(
											target: LOG_TARGET,
											err = ?e,
											"Error handling a recovery request",
										);
									}
								}
								AvailabilityRecoveryMessage::NetworkBridgeUpdateV1(event) => {
									if let Err(e) = handle_network_update(
										&mut state,
										&mut ctx,
										event,
									).await {
										tracing::warn!(
											target: LOG_TARGET,
											err = ?e,
											"Error handling a network bridge update",
										);
									}
								}
							}
						}
					}
				}
				from_interaction = state.from_interaction_rx.next() => {
					if let Some(from_interaction) = from_interaction {
						if let Err(e) = handle_from_interaction(
							&mut state,
							&mut ctx,
							from_interaction,
						).await {
							tracing::warn!(
								target: LOG_TARGET,
								err = ?e,
								"Error handling message from interaction",
							);
						}
					}
				}
			}
		}
	}
}
