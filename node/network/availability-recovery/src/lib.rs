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
use std::pin::Pin;

use futures::{channel::{oneshot, mpsc}, prelude::*, stream::FuturesUnordered};
use futures_timer::Delay;
use lru::LruCache;
use rand::{seq::SliceRandom, thread_rng};
use streamunordered::{StreamUnordered, StreamYield};

use polkadot_primitives::v1::{
	AuthorityDiscoveryId, AvailableData, CandidateReceipt, CandidateHash,
	Hash, ErasureChunk, ValidatorId, ValidatorIndex,
	SessionInfo, SessionIndex, BlakeTwo256, HashT,
};
use polkadot_subsystem::{
	SubsystemContext, SubsystemResult, SubsystemError, Subsystem, SpawnedSubsystem, FromOverseer,
	OverseerSignal, ActiveLeavesUpdate,
	errors::RecoveryError,
	messages::{
		AvailabilityStoreMessage, AvailabilityRecoveryMessage, AllMessages, NetworkBridgeMessage,
		NetworkBridgeEvent,
	},
};
use polkadot_node_network_protocol::{
	peer_set::PeerSet, v1 as protocol_v1, PeerId, ReputationChange as Rep, RequestId,
};
use polkadot_node_subsystem_util::{
	Timeout, TimeoutExt,
	request_session_info_ctx,
};
use polkadot_erasure_coding::{branches, branch_hash, recovery_threshold, obtain_chunks_v1};
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

// A period to poll and clean AwaitedChunks.
const AWAITED_CHUNKS_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);

/// The Availability Recovery Subsystem.
pub struct AvailabilityRecoverySubsystem;

type ChunkResponse = Result<(PeerId, ErasureChunk), RecoveryError>;

/// Data we keep around for every chunk that we are awaiting.
struct AwaitedChunk {
	/// Index of the validator we have requested this chunk from.
	validator_index: ValidatorIndex,

	/// The hash of the candidate the chunks belongs to.
	candidate_hash: CandidateHash,

	/// Token to cancel the connection request to the validator.
	token: usize,

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
		AuthorityDiscoveryId,
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

	/// Discovery ids of `validators`.
	validator_authority_keys: Vec<AuthorityDiscoveryId>,

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

const fn is_unavailable(
	received_chunks: usize,
	requesting_chunks: usize,
	n_validators: usize,
	threshold: usize,
) -> bool {
	received_chunks + requesting_chunks + n_validators < threshold
}

impl Interaction {
	async fn launch_parallel_requests(&mut self) -> error::Result<()> {
		while self.requesting_chunks.len() < N_PARALLEL {
			if let Some(validator_index) = self.shuffling.pop() {
				let (tx, rx) = oneshot::channel();

				self.to_state.send(FromInteraction::MakeRequest(
					self.validator_authority_keys[validator_index as usize].clone(),
					self.candidate_hash.clone(),
					validator_index,
					tx,
				)).await.map_err(error::Error::ClosedToState)?;

				self.requesting_chunks.push(rx.timeout(CHUNK_REQUEST_TIMEOUT));
			} else {
				break;
			}
		}

		Ok(())
	}

	async fn wait_for_chunks(&mut self) -> error::Result<()> {
		// Check if the requesting chunks is not empty not to poll to completion.
		if self.requesting_chunks.is_empty() {
			return Ok(());
		}

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
							self.to_state.send(FromInteraction::ReportPeer(
									peer_id.clone(),
									COST_MERKLE_PROOF_INVALID,
							)).await.map_err(error::Error::ClosedToState)?;
						}
					} else {
						self.to_state.send(FromInteraction::ReportPeer(
								peer_id.clone(),
								COST_MERKLE_PROOF_INVALID,
						)).await.map_err(error::Error::ClosedToState)?;
					}

					self.received_chunks.insert(peer_id, chunk);
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
					);
					// we break here to launch another request.
					break;
				}
			}
		}

		Ok(())
	}

	async fn run(mut self) -> error::Result<()> {
		loop {
			if is_unavailable(
				self.received_chunks.len(),
				self.requesting_chunks.len(),
				self.shuffling.len(),
				self.threshold,
			) {
				self.to_state.send(FromInteraction::Concluded(
					self.candidate_hash,
					Err(RecoveryError::Unavailable),
				)).await.map_err(error::Error::ClosedToState)?;

				return Ok(());
			}

			self.launch_parallel_requests().await?;

			self.wait_for_chunks().await?;

			// If received_chunks has more than threshold entries, attempt to recover the data.
			// If that fails, or a re-encoding of it doesn't match the expected erasure root,
			// break and issue a FromInteraction::Concluded(RecoveryError::Invalid).
			// Otherwise, issue a FromInteraction::Concluded(Ok(())).
			if self.received_chunks.len() >= self.threshold {
				let concluded = match polkadot_erasure_coding::reconstruct_v1(
					self.validators.len(),
					self.received_chunks.values().map(|c| (&c.chunk[..], c.index as usize)),
				) {
					Ok(data) => {
						if reconstructed_data_matches_root(self.validators.len(), &self.erasure_root, &data) {
							FromInteraction::Concluded(self.candidate_hash.clone(), Ok(data))
						} else {
							FromInteraction::Concluded(
								self.candidate_hash.clone(),
								Err(RecoveryError::Invalid),
							)
						}
					}
					Err(_) => FromInteraction::Concluded(
						self.candidate_hash.clone(),
						Err(RecoveryError::Invalid),
					),
				};

				self.to_state.send(concluded).await.map_err(error::Error::ClosedToState)?;
				return Ok(());
			}
		}
	}
}

fn reconstructed_data_matches_root(
	n_validators: usize,
	expected_root: &Hash,
	data: &AvailableData,
) -> bool {
	let chunks = match obtain_chunks_v1(n_validators, data) {
		Ok(chunks) => chunks,
		Err(e) => {
			tracing::debug!(
				target: LOG_TARGET,
				err = ?e,
				"Failed to obtain chunks",
			);
			return false;
		}
	};

	let branches = branches(&chunks);

	branches.root() == *expected_root
}

struct State {
	/// Each interaction is implemented as its own async task,
	/// and these handles are for communicating with them.
	interactions: HashMap<CandidateHash, InteractionHandle>,

	/// A recent block hash for which state should be available.
	live_block_hash: Hash,

	/// We are waiting for these validators to connect and as soon as they
	/// do to request the needed chunks we are awaitinf for.
	discovering_validators: HashMap<AuthorityDiscoveryId, Vec<AwaitedChunk>>,

	/// Requests that we have issued to the already connected validators
	/// about the chunks we are interested in.
	live_chunk_requests: HashMap<RequestId, (PeerId, AwaitedChunk)>,

	/// Derive request ids from this.
	next_request_id: RequestId,

	connecting_validators: StreamUnordered<mpsc::Receiver<(AuthorityDiscoveryId, PeerId)>>,

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
			connecting_validators: StreamUnordered::new(),
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
				state.live_block_hash = hash.0;
			}

			Ok(false)
		}
	    OverseerSignal::BlockFinalized(_, _) => Ok(false)
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
	let threshold = recovery_threshold(session_info.validators.len())?;
	let to_state = state.from_interaction_tx.clone();
	let candidate_hash = receipt.hash();
	let erasure_root = receipt.descriptor.erasure_root;
	let validators = session_info.validators.clone();
	let validator_authority_keys = session_info.discovery_keys.clone();
	let mut shuffling: Vec<_> = (0..validators.len() as ValidatorIndex).collect();

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
		validator_authority_keys,
		validators,
		shuffling,
		threshold,
		candidate_hash,
		erasure_root,
		received_chunks: HashMap::new(),
		requesting_chunks: FuturesUnordered::new(),
	};

	let future = async move {
		if let Err(e) = interaction.run().await {
			tracing::debug!(
				target: LOG_TARGET,
				err = ?e,
				"Interaction finished with an error",
			);
		}
	}.boxed();

	if let Err(e) = ctx.spawn("recovery interaction", future).await {
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
	session_index: SessionIndex,
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
			response_sender
				.send(Err(RecoveryError::Unavailable))
				.map_err(|_| error::Error::CanceledResponseSender)?;
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
			let (tx, rx) = mpsc::channel(2);

			let message = NetworkBridgeMessage::ConnectToValidators {
				validator_ids: vec![id.clone()],
				peer_set: PeerSet::Validation,
				connected: tx,
			};

			ctx.send_message(AllMessages::NetworkBridge(message)).await;

			let token = state.connecting_validators.push(rx);

			state.discovering_validators.entry(id).or_default().push(AwaitedChunk {
				validator_index,
				candidate_hash,
				token,
				response,
			});
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
							report_peer(ctx, peer, COST_UNEXPECTED_CHUNK).await;
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
	authority_id: AuthorityDiscoveryId,
	peer_id: PeerId,
) -> error::Result<()> {
	if let Some(discovering) = state.discovering_validators.remove(&authority_id) {
		for chunk in discovering {
			issue_chunk_request(state, ctx, peer_id.clone(), chunk).await?;
		}
	}

	Ok(())
}

/// Awaited chunks info that `State` holds has to be cleaned up
/// periodically since there is no way `Interaction` can communicate
/// a timedout request.
fn cleanup_awaited_chunks(state: &mut State) {
	let mut removed_tokens = Vec::new();

	for (_, v) in state.discovering_validators.iter_mut() {
		v.retain(|e| if !e.response.is_canceled() {
			removed_tokens.push(e.token);
			false
		} else {
			true
		});
	}

	for token in removed_tokens {
		Pin::new(&mut state.connecting_validators).remove(token);
	}

	state.discovering_validators.retain(|_, v| !v.is_empty());
	state.live_chunk_requests.retain(|_, v| !v.1.response.is_canceled());
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

		let awaited_chunk_cleanup_interval = futures::stream::repeat(()).then(|_| async move {
			Delay::new(AWAITED_CHUNKS_CLEANUP_INTERVAL).await;
		});

		futures::pin_mut!(awaited_chunk_cleanup_interval);

		loop {
			futures::select_biased! {
				_v = awaited_chunk_cleanup_interval.next() => {
					cleanup_awaited_chunks(&mut state);
				}
				v = state.connecting_validators.next() => {
					if let Some((v, token)) = v {
						match v {
							StreamYield::Item(v) => {
								if let Err(e) = handle_validator_connected(
									&mut state,
									&mut ctx,
									v.0,
									v.1,
								).await {
									tracing::warn!(
										target: LOG_TARGET,
										err = ?e,
										"Failed to handle a newly connected validator",
									);
								}
							}
							StreamYield::Finished(_) => {
								Pin::new(&mut state.connecting_validators).remove(token);
							}
						}
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
									session_index,
									response_sender,
								) => {
									if let Err(e) = handle_recover(
										&mut state,
										&mut ctx,
										receipt,
										session_index,
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
