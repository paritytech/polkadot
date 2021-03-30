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
use std::pin::Pin;

use futures::{channel::oneshot, prelude::*, stream::FuturesUnordered};
use futures::future::{BoxFuture, RemoteHandle, FutureExt};
use futures::task::{Context, Poll};
use lru::LruCache;
use rand::seq::SliceRandom;

use polkadot_primitives::v1::{
	AuthorityDiscoveryId, CandidateReceipt, CandidateHash,
	Hash, ValidatorId, ValidatorIndex,
	SessionInfo, SessionIndex, BlakeTwo256, HashT, GroupIndex, BlockNumber,
};
use polkadot_node_primitives::{ErasureChunk, AvailableData};
use polkadot_subsystem::{
	SubsystemContext, SubsystemResult, SubsystemError, Subsystem, SpawnedSubsystem, FromOverseer,
	OverseerSignal, ActiveLeavesUpdate, SubsystemSender,
	errors::RecoveryError,
	jaeger,
	messages::{
		AvailabilityStoreMessage, AvailabilityRecoveryMessage, AllMessages, NetworkBridgeMessage,
	},
};
use polkadot_node_network_protocol::{
	IfDisconnected,
	request_response::{
		self as req_res, OutgoingRequest, Recipient, Requests,
		request::RequestError,
	},
};
use polkadot_node_subsystem_util::request_session_info_ctx;
use polkadot_erasure_coding::{branches, branch_hash, recovery_threshold, obtain_chunks_v1};
mod error;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "parachain::availability-recovery";

// How many parallel requests interaction should have going at once.
const N_PARALLEL: usize = 50;

// Size of the LRU cache where we keep recovered data.
const LRU_SIZE: usize = 16;

/// The Availability Recovery Subsystem.
pub struct AvailabilityRecoverySubsystem {
	fast_path: bool,
}

struct RequestFromBackersPhase {
	// a random shuffling of the validators from the backing group which indicates the order
	// in which we connect to them and request the chunk.
	shuffled_backers: Vec<ValidatorIndex>,
}

struct RequestChunksPhase {
	// a random shuffling of the validators which indicates the order in which we connect to the validators and
	// request the chunk from them.
	shuffling: Vec<ValidatorIndex>,
	received_chunks: HashMap<ValidatorIndex, ErasureChunk>,
	requesting_chunks: FuturesUnordered<BoxFuture<
		'static,
		Result<Option<ErasureChunk>, (ValidatorIndex, RequestError)>>,
	>,
}

struct InteractionParams {
	/// Discovery ids of `validators`.
	validator_authority_keys: Vec<AuthorityDiscoveryId>,

	/// Validators relevant to this `Interaction`.
	validators: Vec<ValidatorId>,

	/// The number of pieces needed.
	threshold: usize,

	/// A hash of the relevant candidate.
	candidate_hash: CandidateHash,

	/// The root of the erasure encoding of the para block.
	erasure_root: Hash,
}

enum InteractionPhase {
	RequestFromBackers(RequestFromBackersPhase),
	RequestChunks(RequestChunksPhase),
}

/// A state of a single interaction reconstructing an available data.
struct Interaction<S> {
	sender: S,

	/// The parameters of the interaction.
	params: InteractionParams,

	/// The phase of the interaction.
	phase: InteractionPhase,
}

impl RequestFromBackersPhase {
	fn new(mut backers: Vec<ValidatorIndex>) -> Self {
		backers.shuffle(&mut rand::thread_rng());

		RequestFromBackersPhase {
			shuffled_backers: backers,
		}
	}

	// Run this phase to completion.
	async fn run(
		&mut self,
		params: &InteractionParams,
		sender: &mut impl SubsystemSender,
	) -> Result<AvailableData, RecoveryError> {
		tracing::trace!(
			target: LOG_TARGET,
			candidate_hash = ?params.candidate_hash,
			erasure_root = ?params.erasure_root,
			"Requesting from backers",
		);
		loop {
			// Pop the next backer, and proceed to next phase if we're out.
			let validator_index = match self.shuffled_backers.pop() {
				None => return Err(RecoveryError::Unavailable),
				Some(i) => i,
			};

			// Request data.
			let (req, res) = OutgoingRequest::new(
				Recipient::Authority(params.validator_authority_keys[validator_index.0 as usize].clone()),
				req_res::v1::AvailableDataFetchingRequest { candidate_hash: params.candidate_hash },
			);

			sender.send_message(NetworkBridgeMessage::SendRequests(
				vec![Requests::AvailableDataFetching(req)],
				IfDisconnected::TryConnect,
			).into()).await;

			match res.await {
				Ok(req_res::v1::AvailableDataFetchingResponse::AvailableData(data)) => {
					if reconstructed_data_matches_root(params.validators.len(), &params.erasure_root, &data) {
						tracing::trace!(
							target: LOG_TARGET,
							candidate_hash = ?params.candidate_hash,
							"Received full data",
						);

						return Ok(data);
					} else {
						tracing::debug!(
							target: LOG_TARGET,
							candidate_hash = ?params.candidate_hash,
							?validator_index,
							"Invalid data response",
						);

						// it doesn't help to report the peer with req/res.
					}
				}
				Ok(req_res::v1::AvailableDataFetchingResponse::NoSuchData) => {}
				Err(e) => tracing::debug!(
					target: LOG_TARGET,
					candidate_hash = ?params.candidate_hash,
					?validator_index,
					err = ?e,
					"Error fetching full available data."
				),
			}
		}
	}
}

impl RequestChunksPhase {
	fn new(n_validators: u32) -> Self {
		let mut shuffling: Vec<_> = (0..n_validators).map(ValidatorIndex).collect();
		shuffling.shuffle(&mut rand::thread_rng());

		RequestChunksPhase {
			shuffling,
			received_chunks: HashMap::new(),
			requesting_chunks: FuturesUnordered::new(),
		}
	}

	async fn launch_parallel_requests(
		&mut self,
		params: &InteractionParams,
		sender: &mut impl SubsystemSender,
	) {
		let max_requests = std::cmp::min(N_PARALLEL, params.threshold);
		while self.requesting_chunks.len() < max_requests {
			if let Some(validator_index) = self.shuffling.pop() {
				let validator = params.validator_authority_keys[validator_index.0 as usize].clone();
				tracing::trace!(
					target: LOG_TARGET,
					?validator,
					?validator_index,
					candidate_hash = ?params.candidate_hash,
					"Requesting chunk",
				);

				// Request data.
				let raw_request = req_res::v1::ChunkFetchingRequest {
					candidate_hash: params.candidate_hash,
					index: validator_index,
				};

				let (req, res) = OutgoingRequest::new(
					Recipient::Authority(validator),
					raw_request.clone(),
				);

				sender.send_message(NetworkBridgeMessage::SendRequests(
					vec![Requests::ChunkFetching(req)],
					IfDisconnected::TryConnect,
				).into()).await;

				self.requesting_chunks.push(Box::pin(async move {
					match res.await {
						Ok(req_res::v1::ChunkFetchingResponse::Chunk(chunk))
							=> Ok(Some(chunk.recombine_into_chunk(&raw_request))),
						Ok(req_res::v1::ChunkFetchingResponse::NoSuchChunk) => Ok(None),
						Err(e) => Err((validator_index, e)),
					}
				}));
			} else {
				break;
			}
		}
	}

	async fn wait_for_chunks(
		&mut self,
		params: &InteractionParams,
	) {
		// Poll for new updates from requesting_chunks.
		while let Poll::Ready(Some(request_result))
			= futures::poll!(self.requesting_chunks.next())
		{
			match request_result {
				Ok(Some(chunk)) => {
					// Check merkle proofs of any received chunks.

					let validator_index = chunk.index;

					if let Ok(anticipated_hash) = branch_hash(
						&params.erasure_root,
						&chunk.proof,
						chunk.index.0 as usize,
					) {
						let erasure_chunk_hash = BlakeTwo256::hash(&chunk.chunk);

						if erasure_chunk_hash != anticipated_hash {
							tracing::debug!(
								target: LOG_TARGET,
								?validator_index,
								"Merkle proof mismatch",
							);
						} else {
							tracing::trace!(
								target: LOG_TARGET,
								?validator_index,
								"Received valid chunk.",
							);
							self.received_chunks.insert(validator_index, chunk);
						}
					} else {
						tracing::debug!(
							target: LOG_TARGET,
							?validator_index,
							"Invalid Merkle proof",
						);
					}
				}
				Ok(None) => {}
				Err((validator_index, e)) => {
					tracing::debug!(
						target: LOG_TARGET,
						err = ?e,
						?validator_index,
						"Failure requesting chunk",
					);

					match e {
						RequestError::InvalidResponse(_) => {}
						RequestError::NetworkError(_) | RequestError::Canceled(_) => {
							self.shuffling.push(validator_index);
						}
					}
				}
			}
		}
	}

	async fn run(
		&mut self,
		params: &InteractionParams,
		sender: &mut impl SubsystemSender,
	) -> Result<AvailableData, RecoveryError> {
		loop {
			if is_unavailable(
				self.received_chunks.len(),
				self.requesting_chunks.len(),
				self.shuffling.len(),
				params.threshold,
			) {
				tracing::debug!(
					target: LOG_TARGET,
					candidate_hash = ?params.candidate_hash,
					erasure_root = ?params.erasure_root,
					received = %self.received_chunks.len(),
					requesting = %self.requesting_chunks.len(),
					n_validators = %params.validators.len(),
					"Data recovery is not possible",
				);

				return Err(RecoveryError::Unavailable);
			}

			self.launch_parallel_requests(params, sender).await;
			self.wait_for_chunks(params).await;

			// If received_chunks has more than threshold entries, attempt to recover the data.
			// If that fails, or a re-encoding of it doesn't match the expected erasure root,
			// return Err(RecoveryError::Invalid)
			if self.received_chunks.len() >= params.threshold {
				return match polkadot_erasure_coding::reconstruct_v1(
					params.validators.len(),
					self.received_chunks.values().map(|c| (&c.chunk[..], c.index.0 as usize)),
				) {
					Ok(data) => {
						if reconstructed_data_matches_root(params.validators.len(), &params.erasure_root, &data) {
							tracing::trace!(
								target: LOG_TARGET,
								candidate_hash = ?params.candidate_hash,
								erasure_root = ?params.erasure_root,
								"Data recovery complete",
							);

							Ok(data)
						} else {
							tracing::trace!(
								target: LOG_TARGET,
								candidate_hash = ?params.candidate_hash,
								erasure_root = ?params.erasure_root,
								"Data recovery - root mismatch",
							);

							Err(RecoveryError::Invalid)
						}
					}
					Err(err) => {
						tracing::trace!(
							target: LOG_TARGET,
							candidate_hash = ?params.candidate_hash,
							erasure_root = ?params.erasure_root,
							?err,
							"Data recovery error ",
						);

						Err(RecoveryError::Invalid)
					},
				};
			}
		}
	}
}

const fn is_unavailable(
	received_chunks: usize,
	requesting_chunks: usize,
	unrequested_validators: usize,
	threshold: usize,
) -> bool {
	received_chunks + requesting_chunks + unrequested_validators < threshold
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

impl<S: SubsystemSender> Interaction<S> {
	async fn run(mut self) -> Result<AvailableData, RecoveryError> {
		loop {
			// These only fail if we cannot reach the underlying subsystem, which case there is nothing
			// meaningful we can do.
			match self.phase {
				InteractionPhase::RequestFromBackers(ref mut from_backers) => {
					match from_backers.run(&self.params, &mut self.sender).await {
						Ok(data) => break Ok(data),
						Err(RecoveryError::Invalid) => break Err(RecoveryError::Invalid),
						Err(RecoveryError::Unavailable) => {
							self.phase = InteractionPhase::RequestChunks(
								RequestChunksPhase::new(self.params.validators.len() as _)
							)
						}
					}
				}
				InteractionPhase::RequestChunks(ref mut from_all) => {
					break from_all.run(&self.params, &mut self.sender).await;
				}
			}
		}
	}
}

/// Accumulate all awaiting sides for some particular `AvailableData`.
struct InteractionHandle {
	candidate_hash: CandidateHash,
	remote: RemoteHandle<Result<AvailableData, RecoveryError>>,
	awaiting: Vec<oneshot::Sender<Result<AvailableData, RecoveryError>>>,
}

impl Future for InteractionHandle {
	type Output = Option<(CandidateHash, Result<AvailableData, RecoveryError>)>;

	fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let mut indices_to_remove = Vec::new();
		for (i, awaiting) in self.awaiting.iter_mut().enumerate().rev() {
			if let Poll::Ready(()) =  awaiting.poll_canceled(cx) {
				indices_to_remove.push(i);
			}
		}

		// these are reverse order, so remove is fine.
		for index in indices_to_remove {
			tracing::debug!(
				target: LOG_TARGET,
				candidate_hash = ?self.candidate_hash,
				"Receiver for available data dropped.",
			);

			self.awaiting.swap_remove(index);
		}

		if self.awaiting.is_empty() {
			tracing::debug!(
				target: LOG_TARGET,
				candidate_hash = ?self.candidate_hash,
				"All receivers for available data dropped.",
			);

			return Poll::Ready(None);
		}

		let remote = &mut self.remote;
		futures::pin_mut!(remote);
		let result = futures::ready!(remote.poll(cx));

		for awaiting in self.awaiting.drain(..) {
			let _ = awaiting.send(result.clone());
		}

		Poll::Ready(Some((self.candidate_hash, result)))
	}
}

struct State {
	/// Each interaction is implemented as its own async task,
	/// and these handles are for communicating with them.
	interactions: FuturesUnordered<InteractionHandle>,

	/// A recent block hash for which state should be available.
	live_block: (BlockNumber, Hash),

	/// An LRU cache of recently recovered data.
	availability_lru: LruCache<CandidateHash, Result<AvailableData, RecoveryError>>,
}

impl Default for State {
	fn default() -> Self {
		Self {
			interactions: FuturesUnordered::new(),
			live_block: (0, Hash::default()),
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
			// if activated is non-empty, set state.live_block to the highest block in `activated`
			for activated in activated {
				if activated.number > state.live_block.0 {
					state.live_block = (activated.number, activated.hash)
				}
			}

			Ok(false)
		}
		OverseerSignal::BlockFinalized(_, _) => Ok(false)
	}
}

/// Machinery around launching interactions into the background.
#[tracing::instrument(level = "trace", skip(ctx, state), fields(subsystem = LOG_TARGET))]
async fn launch_interaction(
	state: &mut State,
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	session_index: SessionIndex,
	session_info: SessionInfo,
	receipt: CandidateReceipt,
	backing_group: Option<GroupIndex>,
	response_sender: oneshot::Sender<Result<AvailableData, RecoveryError>>,
) -> error::Result<()> {
	let candidate_hash = receipt.hash();

	let params = InteractionParams {
		validator_authority_keys: session_info.discovery_keys.clone(),
		validators: session_info.validators.clone(),
		threshold: recovery_threshold(session_info.validators.len())?,
		candidate_hash,
		erasure_root: receipt.descriptor.erasure_root,
	};

	let phase = backing_group
		.and_then(|g| session_info.validator_groups.get(g.0 as usize))
		.map(|group| InteractionPhase::RequestFromBackers(
			RequestFromBackersPhase::new(group.clone())
		))
		.unwrap_or_else(|| InteractionPhase::RequestChunks(
			RequestChunksPhase::new(params.validators.len() as _)
		));

	let interaction = Interaction {
		sender: ctx.sender().clone(),
		params,
		phase,
	};

	let (remote, remote_handle) = interaction.run().remote_handle();

	state.interactions.push(InteractionHandle {
		candidate_hash,
		remote: remote_handle,
		awaiting: vec![response_sender],
	});

	if let Err(e) = ctx.spawn("recovery interaction", Box::pin(remote)).await {
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
	backing_group: Option<GroupIndex>,
	response_sender: oneshot::Sender<Result<AvailableData, RecoveryError>>,
) -> error::Result<()> {
	let candidate_hash = receipt.hash();

	let span = jaeger::Span::new(candidate_hash, "availbility-recovery")
		.with_stage(jaeger::Stage::AvailabilityRecovery);

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

	if let Some(i) = state.interactions.iter_mut().find(|i| i.candidate_hash == candidate_hash) {
		i.awaiting.push(response_sender);
		return Ok(());
	}

	let _span = span.child("not-cached");
	let session_info = request_session_info_ctx(
		state.live_block.1,
		session_index,
		ctx,
	).await?.await.map_err(error::Error::CanceledSessionInfo)??;

	let _span = span.child("session-info-ctx-received");
	match session_info {
		Some(session_info) => {
			launch_interaction(
				state,
				ctx,
				session_index,
				session_info,
				receipt,
				backing_group,
				response_sender,
			).await
		}
		None => {
			tracing::warn!(
				target: LOG_TARGET,
				"SessionInfo is `None` at {:?}", state.live_block,
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
async fn query_full_data(
	ctx: &mut impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	candidate_hash: CandidateHash,
) -> error::Result<Option<AvailableData>> {
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AllMessages::AvailabilityStore(
		AvailabilityStoreMessage::QueryAvailableData(candidate_hash, tx),
	)).await;

	Ok(rx.await.map_err(error::Error::CanceledQueryFullData)?)
}

impl AvailabilityRecoverySubsystem {
	/// Create a new instance of `AvailabilityRecoverySubsystem` which starts with a fast path to request data from backers.
	pub fn with_fast_path() -> Self {
		Self { fast_path: true }
	}

	/// Create a new instance of `AvailabilityRecoverySubsystem` which requests only chunks
	pub fn with_chunks_only() -> Self {
		Self { fast_path: false }
	}

	async fn run(
		self,
		mut ctx: impl SubsystemContext<Message = AvailabilityRecoveryMessage>,
	) -> SubsystemResult<()> {
		let mut state = State::default();

		loop {
			futures::select! {
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
									maybe_backing_group,
									response_sender,
								) => {
									if let Err(e) = handle_recover(
										&mut state,
										&mut ctx,
										receipt,
										session_index,
										maybe_backing_group.filter(|_| self.fast_path),
										response_sender,
									).await {
										tracing::warn!(
											target: LOG_TARGET,
											err = ?e,
											"Error handling a recovery request",
										);
									}
								}
								AvailabilityRecoveryMessage::AvailableDataFetchingRequest(req) => {
									match query_full_data(&mut ctx, req.payload.candidate_hash).await {
										Ok(res) => {
											let _ = req.send_response(res.into());
										}
										Err(e) => {
											tracing::debug!(
												target: LOG_TARGET,
												err = ?e,
												"Failed to query available data.",
											);

											let _ = req.send_response(None.into());
										}
									}
								}
							}
						}
					}
				}
				output = state.interactions.next() => {
					if let Some((candidate_hash, result)) = output.flatten() {
						state.availability_lru.put(candidate_hash, result);
					}
				}
			}
		}
	}
}
