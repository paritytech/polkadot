// Copyright (C) Parity Technologies (UK) Ltd.
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

use std::{
	collections::{HashMap, VecDeque},
	iter::Iterator,
	num::NonZeroUsize,
	pin::Pin,
	time::Duration,
};

use futures::{
	channel::oneshot::{self, channel},
	future::{Future, FutureExt, RemoteHandle},
	pin_mut,
	prelude::*,
	sink::SinkExt,
	stream::{FuturesUnordered, StreamExt},
	task::{Context, Poll},
};
use lru::LruCache;
use rand::seq::SliceRandom;

use fatality::Nested;
use polkadot_erasure_coding::{
	branch_hash, branches, obtain_chunks_v1, recovery_threshold, Error as ErasureEncodingError,
};
#[cfg(not(test))]
use polkadot_node_network_protocol::request_response::CHUNK_REQUEST_TIMEOUT;
use polkadot_node_network_protocol::{
	request_response::{
		self as req_res, outgoing::RequestError, v1 as request_v1, IncomingRequestReceiver,
		OutgoingRequest, Recipient, Requests,
	},
	IfDisconnected, UnifiedReputationChange as Rep,
};
use polkadot_node_primitives::{AvailableData, ErasureChunk};
use polkadot_node_subsystem::{
	errors::RecoveryError,
	jaeger,
	messages::{AvailabilityRecoveryMessage, AvailabilityStoreMessage, NetworkBridgeTxMessage},
	overseer, ActiveLeavesUpdate, FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
	SubsystemResult,
};
use polkadot_node_subsystem_util::request_session_info;
use polkadot_primitives::{
	AuthorityDiscoveryId, BlakeTwo256, BlockNumber, CandidateHash, CandidateReceipt, GroupIndex,
	Hash, HashT, IndexedVec, SessionIndex, SessionInfo, ValidatorId, ValidatorIndex,
};

mod error;
mod futures_undead;
mod metrics;
use metrics::Metrics;

use futures_undead::FuturesUndead;
use sc_network::{OutboundFailure, RequestFailure};

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "parachain::availability-recovery";

// How many parallel recovery tasks should be running at once.
const N_PARALLEL: usize = 50;

// Size of the LRU cache where we keep recovered data.
const LRU_SIZE: NonZeroUsize = match NonZeroUsize::new(16) {
	Some(cap) => cap,
	None => panic!("Availability-recovery cache size must be non-zero."),
};

const COST_INVALID_REQUEST: Rep = Rep::CostMajor("Peer sent unparsable request");

/// Time after which we consider a request to have failed
///
/// and we should try more peers. Note in theory the request times out at the network level,
/// measurements have shown, that in practice requests might actually take longer to fail in
/// certain occasions. (The very least, authority discovery is not part of the timeout.)
///
/// For the time being this value is the same as the timeout on the networking layer, but as this
/// timeout is more soft than the networking one, it might make sense to pick different values as
/// well.
#[cfg(not(test))]
const TIMEOUT_START_NEW_REQUESTS: Duration = CHUNK_REQUEST_TIMEOUT;
#[cfg(test)]
const TIMEOUT_START_NEW_REQUESTS: Duration = Duration::from_millis(100);

/// PoV size limit in bytes for which prefer fetching from backers.
const SMALL_POV_LIMIT: usize = 128 * 1024;

#[derive(Clone, PartialEq)]
/// The strategy we use to recover the PoV.
pub enum RecoveryStrategy {
	/// We always try the backing group first, then fallback to validator chunks.
	BackersFirstAlways,
	/// We try the backing group first if PoV size is lower than specified, then fallback to
	/// validator chunks.
	BackersFirstIfSizeLower(usize),
	/// We always recover using validator chunks.
	ChunksAlways,
	/// Do not request data from the availability store.
	/// This is the useful for nodes where the
	/// availability-store subsystem is not expected to run,
	/// such as collators.
	BypassAvailabilityStore,
}

impl RecoveryStrategy {
	/// Returns true if the strategy needs backing group index.
	pub fn needs_backing_group(&self) -> bool {
		match self {
			RecoveryStrategy::BackersFirstAlways | RecoveryStrategy::BackersFirstIfSizeLower(_) =>
				true,
			_ => false,
		}
	}

	/// Returns the PoV size limit in bytes for `BackersFirstIfSizeLower` strategy, otherwise
	/// `None`.
	pub fn pov_size_limit(&self) -> Option<usize> {
		match *self {
			RecoveryStrategy::BackersFirstIfSizeLower(limit) => Some(limit),
			_ => None,
		}
	}
}
/// The Availability Recovery Subsystem.
pub struct AvailabilityRecoverySubsystem {
	/// PoV recovery strategy to use.
	recovery_strategy: RecoveryStrategy,
	/// Receiver for available data requests.
	req_receiver: IncomingRequestReceiver<request_v1::AvailableDataFetchingRequest>,
	/// Metrics for this subsystem.
	metrics: Metrics,
}

struct RequestFromBackers {
	// a random shuffling of the validators from the backing group which indicates the order
	// in which we connect to them and request the chunk.
	shuffled_backers: Vec<ValidatorIndex>,
	// channel to the erasure task handler.
	erasure_task_tx: futures::channel::mpsc::Sender<ErasureTask>,
}

struct RequestChunksFromValidators {
	/// How many request have been unsuccessful so far.
	error_count: usize,
	/// Total number of responses that have been received.
	///
	/// including failed ones.
	total_received_responses: usize,
	/// a random shuffling of the validators which indicates the order in which we connect to the
	/// validators and request the chunk from them.
	shuffling: VecDeque<ValidatorIndex>,
	/// Chunks received so far.
	received_chunks: HashMap<ValidatorIndex, ErasureChunk>,
	/// Pending chunk requests with soft timeout.
	requesting_chunks: FuturesUndead<Result<Option<ErasureChunk>, (ValidatorIndex, RequestError)>>,
	// channel to the erasure task handler.
	erasure_task_tx: futures::channel::mpsc::Sender<ErasureTask>,
}

struct RecoveryParams {
	/// Discovery ids of `validators`.
	validator_authority_keys: Vec<AuthorityDiscoveryId>,

	/// Validators relevant to this `RecoveryTask`.
	validators: IndexedVec<ValidatorIndex, ValidatorId>,

	/// The number of pieces needed.
	threshold: usize,

	/// A hash of the relevant candidate.
	candidate_hash: CandidateHash,

	/// The root of the erasure encoding of the para block.
	erasure_root: Hash,

	/// Metrics to report
	metrics: Metrics,

	/// Do not request data from availability-store
	bypass_availability_store: bool,
}

/// Source the availability data either by means
/// of direct request response protocol to
/// backers (a.k.a. fast-path), or recover from chunks.
enum Source {
	RequestFromBackers(RequestFromBackers),
	RequestChunks(RequestChunksFromValidators),
}

/// Expensive erasure coding computations that we want to run on a blocking thread.
enum ErasureTask {
	/// Reconstructs `AvailableData` from chunks given `n_validators`.
	Reconstruct(
		usize,
		HashMap<ValidatorIndex, ErasureChunk>,
		oneshot::Sender<Result<AvailableData, ErasureEncodingError>>,
	),
	/// Re-encode `AvailableData` into erasure chunks in order to verify the provided root hash of
	/// the Merkle tree.
	Reencode(usize, Hash, AvailableData, oneshot::Sender<Option<AvailableData>>),
}

/// A stateful reconstruction of availability data in reference to
/// a candidate hash.
struct RecoveryTask<Sender> {
	sender: Sender,

	/// The parameters of the recovery process.
	params: RecoveryParams,

	/// The source to obtain the availability data from.
	source: Source,

	// channel to the erasure task handler.
	erasure_task_tx: futures::channel::mpsc::Sender<ErasureTask>,
}

impl RequestFromBackers {
	fn new(
		mut backers: Vec<ValidatorIndex>,
		erasure_task_tx: futures::channel::mpsc::Sender<ErasureTask>,
	) -> Self {
		backers.shuffle(&mut rand::thread_rng());

		RequestFromBackers { shuffled_backers: backers, erasure_task_tx }
	}

	// Run this phase to completion.
	async fn run(
		&mut self,
		params: &RecoveryParams,
		sender: &mut impl overseer::AvailabilityRecoverySenderTrait,
	) -> Result<AvailableData, RecoveryError> {
		gum::trace!(
			target: LOG_TARGET,
			candidate_hash = ?params.candidate_hash,
			erasure_root = ?params.erasure_root,
			"Requesting from backers",
		);
		loop {
			// Pop the next backer, and proceed to next phase if we're out.
			let validator_index =
				self.shuffled_backers.pop().ok_or_else(|| RecoveryError::Unavailable)?;

			// Request data.
			let (req, response) = OutgoingRequest::new(
				Recipient::Authority(
					params.validator_authority_keys[validator_index.0 as usize].clone(),
				),
				req_res::v1::AvailableDataFetchingRequest { candidate_hash: params.candidate_hash },
			);

			sender
				.send_message(NetworkBridgeTxMessage::SendRequests(
					vec![Requests::AvailableDataFetchingV1(req)],
					IfDisconnected::ImmediateError,
				))
				.await;

			match response.await {
				Ok(req_res::v1::AvailableDataFetchingResponse::AvailableData(data)) => {
					let (reencode_tx, reencode_rx) = channel();
					self.erasure_task_tx
						.send(ErasureTask::Reencode(
							params.validators.len(),
							params.erasure_root,
							data,
							reencode_tx,
						))
						.await
						.map_err(|_| RecoveryError::ChannelClosed)?;

					let reencode_response =
						reencode_rx.await.map_err(|_| RecoveryError::ChannelClosed)?;

					if let Some(data) = reencode_response {
						gum::trace!(
							target: LOG_TARGET,
							candidate_hash = ?params.candidate_hash,
							"Received full data",
						);

						return Ok(data)
					} else {
						gum::debug!(
							target: LOG_TARGET,
							candidate_hash = ?params.candidate_hash,
							?validator_index,
							"Invalid data response",
						);

						// it doesn't help to report the peer with req/res.
					}
				},
				Ok(req_res::v1::AvailableDataFetchingResponse::NoSuchData) => {},
				Err(e) => gum::debug!(
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

impl RequestChunksFromValidators {
	fn new(
		n_validators: u32,
		erasure_task_tx: futures::channel::mpsc::Sender<ErasureTask>,
	) -> Self {
		let mut shuffling: Vec<_> = (0..n_validators).map(ValidatorIndex).collect();
		shuffling.shuffle(&mut rand::thread_rng());

		RequestChunksFromValidators {
			error_count: 0,
			total_received_responses: 0,
			shuffling: shuffling.into(),
			received_chunks: HashMap::new(),
			requesting_chunks: FuturesUndead::new(),
			erasure_task_tx,
		}
	}

	fn is_unavailable(&self, params: &RecoveryParams) -> bool {
		is_unavailable(
			self.chunk_count(),
			self.requesting_chunks.total_len(),
			self.shuffling.len(),
			params.threshold,
		)
	}

	fn chunk_count(&self) -> usize {
		self.received_chunks.len()
	}

	fn insert_chunk(&mut self, validator_index: ValidatorIndex, chunk: ErasureChunk) {
		self.received_chunks.insert(validator_index, chunk);
	}

	fn can_conclude(&self, params: &RecoveryParams) -> bool {
		self.chunk_count() >= params.threshold || self.is_unavailable(params)
	}

	/// Desired number of parallel requests.
	///
	/// For the given threshold (total required number of chunks) get the desired number of
	/// requests we want to have running in parallel at this time.
	fn get_desired_request_count(&self, threshold: usize) -> usize {
		// Upper bound for parallel requests.
		// We want to limit this, so requests can be processed within the timeout and we limit the
		// following feedback loop:
		// 1. Requests fail due to timeout
		// 2. We request more chunks to make up for it
		// 3. Bandwidth is spread out even more, so we get even more timeouts
		// 4. We request more chunks to make up for it ...
		let max_requests_boundary = std::cmp::min(N_PARALLEL, threshold);
		// How many chunks are still needed?
		let remaining_chunks = threshold.saturating_sub(self.chunk_count());
		// What is the current error rate, so we can make up for it?
		let inv_error_rate =
			self.total_received_responses.checked_div(self.error_count).unwrap_or(0);
		// Actual number of requests we want to have in flight in parallel:
		std::cmp::min(
			max_requests_boundary,
			remaining_chunks + remaining_chunks.checked_div(inv_error_rate).unwrap_or(0),
		)
	}

	async fn launch_parallel_requests<Sender>(
		&mut self,
		params: &RecoveryParams,
		sender: &mut Sender,
	) where
		Sender: overseer::AvailabilityRecoverySenderTrait,
	{
		let num_requests = self.get_desired_request_count(params.threshold);
		let candidate_hash = &params.candidate_hash;
		let already_requesting_count = self.requesting_chunks.len();

		gum::debug!(
			target: LOG_TARGET,
			?candidate_hash,
			?num_requests,
			error_count= ?self.error_count,
			total_received = ?self.total_received_responses,
			threshold = ?params.threshold,
			?already_requesting_count,
			"Requesting availability chunks for a candidate",
		);
		let mut requests = Vec::with_capacity(num_requests - already_requesting_count);

		while self.requesting_chunks.len() < num_requests {
			if let Some(validator_index) = self.shuffling.pop_back() {
				let validator = params.validator_authority_keys[validator_index.0 as usize].clone();
				gum::trace!(
					target: LOG_TARGET,
					?validator,
					?validator_index,
					?candidate_hash,
					"Requesting chunk",
				);

				// Request data.
				let raw_request = req_res::v1::ChunkFetchingRequest {
					candidate_hash: params.candidate_hash,
					index: validator_index,
				};

				let (req, res) = OutgoingRequest::new(Recipient::Authority(validator), raw_request);
				requests.push(Requests::ChunkFetchingV1(req));

				params.metrics.on_chunk_request_issued();
				let timer = params.metrics.time_chunk_request();

				self.requesting_chunks.push(Box::pin(async move {
					let _timer = timer;
					match res.await {
						Ok(req_res::v1::ChunkFetchingResponse::Chunk(chunk)) =>
							Ok(Some(chunk.recombine_into_chunk(&raw_request))),
						Ok(req_res::v1::ChunkFetchingResponse::NoSuchChunk) => Ok(None),
						Err(e) => Err((validator_index, e)),
					}
				}));
			} else {
				break
			}
		}

		sender
			.send_message(NetworkBridgeTxMessage::SendRequests(
				requests,
				IfDisconnected::TryConnect,
			))
			.await;
	}

	/// Wait for a sufficient amount of chunks to reconstruct according to the provided `params`.
	async fn wait_for_chunks(&mut self, params: &RecoveryParams) {
		let metrics = &params.metrics;

		// Wait for all current requests to conclude or time-out, or until we reach enough chunks.
		// We also declare requests undead, once `TIMEOUT_START_NEW_REQUESTS` is reached and will
		// return in that case for `launch_parallel_requests` to fill up slots again.
		while let Some(request_result) =
			self.requesting_chunks.next_with_timeout(TIMEOUT_START_NEW_REQUESTS).await
		{
			self.total_received_responses += 1;

			match request_result {
				Ok(Some(chunk)) =>
					if is_chunk_valid(params, &chunk) {
						metrics.on_chunk_request_succeeded();
						gum::trace!(
							target: LOG_TARGET,
							candidate_hash = ?params.candidate_hash,
							validator_index = ?chunk.index,
							"Received valid chunk",
						);
						self.insert_chunk(chunk.index, chunk);
					} else {
						metrics.on_chunk_request_invalid();
						self.error_count += 1;
					},
				Ok(None) => {
					metrics.on_chunk_request_no_such_chunk();
					self.error_count += 1;
				},
				Err((validator_index, e)) => {
					self.error_count += 1;

					gum::trace!(
						target: LOG_TARGET,
						candidate_hash= ?params.candidate_hash,
						err = ?e,
						?validator_index,
						"Failure requesting chunk",
					);

					match e {
						RequestError::InvalidResponse(_) => {
							metrics.on_chunk_request_invalid();

							gum::debug!(
								target: LOG_TARGET,
								candidate_hash = ?params.candidate_hash,
								err = ?e,
								?validator_index,
								"Chunk fetching response was invalid",
							);
						},
						RequestError::NetworkError(err) => {
							// No debug logs on general network errors - that became very spammy
							// occasionally.
							if let RequestFailure::Network(OutboundFailure::Timeout) = err {
								metrics.on_chunk_request_timeout();
							} else {
								metrics.on_chunk_request_error();
							}

							self.shuffling.push_front(validator_index);
						},
						RequestError::Canceled(_) => {
							metrics.on_chunk_request_error();

							self.shuffling.push_front(validator_index);
						},
					}
				},
			}

			// Stop waiting for requests when we either can already recover the data
			// or have gotten firm 'No' responses from enough validators.
			if self.can_conclude(params) {
				gum::debug!(
					target: LOG_TARGET,
					candidate_hash = ?params.candidate_hash,
					received_chunks_count = ?self.chunk_count(),
					requested_chunks_count = ?self.requesting_chunks.len(),
					threshold = ?params.threshold,
					"Can conclude availability for a candidate",
				);
				break
			}
		}
	}

	async fn run<Sender>(
		&mut self,
		params: &RecoveryParams,
		sender: &mut Sender,
	) -> Result<AvailableData, RecoveryError>
	where
		Sender: overseer::AvailabilityRecoverySenderTrait,
	{
		let metrics = &params.metrics;

		// First query the store for any chunks we've got.
		if !params.bypass_availability_store {
			let (tx, rx) = oneshot::channel();
			sender
				.send_message(AvailabilityStoreMessage::QueryAllChunks(params.candidate_hash, tx))
				.await;

			match rx.await {
				Ok(chunks) => {
					// This should either be length 1 or 0. If we had the whole data,
					// we wouldn't have reached this stage.
					let chunk_indices: Vec<_> = chunks.iter().map(|c| c.index).collect();
					self.shuffling.retain(|i| !chunk_indices.contains(i));

					for chunk in chunks {
						if is_chunk_valid(params, &chunk) {
							gum::trace!(
								target: LOG_TARGET,
								candidate_hash = ?params.candidate_hash,
								validator_index = ?chunk.index,
								"Found valid chunk on disk"
							);
							self.insert_chunk(chunk.index, chunk);
						} else {
							gum::error!(
								target: LOG_TARGET,
								"Loaded invalid chunk from disk! Disk/Db corruption _very_ likely - please fix ASAP!"
							);
						};
					}
				},
				Err(oneshot::Canceled) => {
					gum::warn!(
						target: LOG_TARGET,
						candidate_hash = ?params.candidate_hash,
						"Failed to reach the availability store"
					);
				},
			}
		}

		let _recovery_timer = metrics.time_full_recovery();

		loop {
			if self.is_unavailable(&params) {
				gum::debug!(
					target: LOG_TARGET,
					candidate_hash = ?params.candidate_hash,
					erasure_root = ?params.erasure_root,
					received = %self.chunk_count(),
					requesting = %self.requesting_chunks.len(),
					total_requesting = %self.requesting_chunks.total_len(),
					n_validators = %params.validators.len(),
					"Data recovery is not possible",
				);

				metrics.on_recovery_failed();

				return Err(RecoveryError::Unavailable)
			}

			self.launch_parallel_requests(params, sender).await;
			self.wait_for_chunks(params).await;

			// If received_chunks has more than threshold entries, attempt to recover the data.
			// If that fails, or a re-encoding of it doesn't match the expected erasure root,
			// return Err(RecoveryError::Invalid)
			if self.chunk_count() >= params.threshold {
				let recovery_duration = metrics.time_erasure_recovery();

				// Send request to reconstruct available data from chunks.
				let (avilable_data_tx, available_data_rx) = channel();
				self.erasure_task_tx
					.send(ErasureTask::Reconstruct(
						params.validators.len(),
						std::mem::take(&mut self.received_chunks),
						avilable_data_tx,
					))
					.await
					.map_err(|_| RecoveryError::ChannelClosed)?;

				let available_data_response =
					available_data_rx.await.map_err(|_| RecoveryError::ChannelClosed)?;

				return match available_data_response {
					Ok(data) => {
						// Send request to re-encode the chunks and check merkle root.
						let (reencode_tx, reencode_rx) = channel();
						self.erasure_task_tx
							.send(ErasureTask::Reencode(
								params.validators.len(),
								params.erasure_root,
								data,
								reencode_tx,
							))
							.await
							.map_err(|_| RecoveryError::ChannelClosed)?;

						let reencode_response =
							reencode_rx.await.map_err(|_| RecoveryError::ChannelClosed)?;

						if let Some(data) = reencode_response {
							gum::trace!(
								target: LOG_TARGET,
								candidate_hash = ?params.candidate_hash,
								erasure_root = ?params.erasure_root,
								"Data recovery complete",
							);
							metrics.on_recovery_succeeded();

							Ok(data)
						} else {
							recovery_duration.map(|rd| rd.stop_and_discard());
							gum::trace!(
								target: LOG_TARGET,
								candidate_hash = ?params.candidate_hash,
								erasure_root = ?params.erasure_root,
								"Data recovery - root mismatch",
							);
							metrics.on_recovery_invalid();

							Err(RecoveryError::Invalid)
						}
					},
					Err(err) => {
						recovery_duration.map(|rd| rd.stop_and_discard());
						gum::trace!(
							target: LOG_TARGET,
							candidate_hash = ?params.candidate_hash,
							erasure_root = ?params.erasure_root,
							?err,
							"Data recovery error ",
						);
						metrics.on_recovery_invalid();

						Err(RecoveryError::Invalid)
					},
				}
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

/// Check validity of a chunk.
fn is_chunk_valid(params: &RecoveryParams, chunk: &ErasureChunk) -> bool {
	let anticipated_hash =
		match branch_hash(&params.erasure_root, chunk.proof(), chunk.index.0 as usize) {
			Ok(hash) => hash,
			Err(e) => {
				gum::debug!(
					target: LOG_TARGET,
					candidate_hash = ?params.candidate_hash,
					validator_index = ?chunk.index,
					error = ?e,
					"Invalid Merkle proof",
				);
				return false
			},
		};
	let erasure_chunk_hash = BlakeTwo256::hash(&chunk.chunk);
	if anticipated_hash != erasure_chunk_hash {
		gum::debug!(
			target: LOG_TARGET,
			candidate_hash = ?params.candidate_hash,
			validator_index = ?chunk.index,
			"Merkle proof mismatch"
		);
		return false
	}
	true
}

/// Re-encode the data into erasure chunks in order to verify
/// the root hash of the provided Merkle tree, which is built
/// on-top of the encoded chunks.
///
/// This (expensive) check is necessary, as otherwise we can't be sure that some chunks won't have
/// been tampered with by the backers, which would result in some validators considering the data
/// valid and some invalid as having fetched different set of chunks. The checking of the Merkle
/// proof for individual chunks only gives us guarantees, that we have fetched a chunk belonging to
/// a set the backers have committed to.
///
/// NOTE: It is fine to do this check with already decoded data, because if the decoding failed for
/// some validators, we can be sure that chunks have been tampered with (by the backers) or the
/// data was invalid to begin with. In the former case, validators fetching valid chunks will see
/// invalid data as well, because the root won't match. In the latter case the situation is the
/// same for anyone anyways.
fn reconstructed_data_matches_root(
	n_validators: usize,
	expected_root: &Hash,
	data: &AvailableData,
	metrics: &Metrics,
) -> bool {
	let _timer = metrics.time_reencode_chunks();

	let chunks = match obtain_chunks_v1(n_validators, data) {
		Ok(chunks) => chunks,
		Err(e) => {
			gum::debug!(
				target: LOG_TARGET,
				err = ?e,
				"Failed to obtain chunks",
			);
			return false
		},
	};

	let branches = branches(&chunks);

	branches.root() == *expected_root
}

impl<Sender> RecoveryTask<Sender>
where
	Sender: overseer::AvailabilityRecoverySenderTrait,
{
	async fn run(mut self) -> Result<AvailableData, RecoveryError> {
		// First just see if we have the data available locally.
		if !self.params.bypass_availability_store {
			let (tx, rx) = oneshot::channel();
			self.sender
				.send_message(AvailabilityStoreMessage::QueryAvailableData(
					self.params.candidate_hash,
					tx,
				))
				.await;

			match rx.await {
				Ok(Some(data)) => return Ok(data),
				Ok(None) => {},
				Err(oneshot::Canceled) => {
					gum::warn!(
						target: LOG_TARGET,
						candidate_hash = ?self.params.candidate_hash,
						"Failed to reach the availability store",
					)
				},
			}
		}

		self.params.metrics.on_recovery_started();

		loop {
			// These only fail if we cannot reach the underlying subsystem, which case there is
			// nothing meaningful we can do.
			match self.source {
				Source::RequestFromBackers(ref mut from_backers) => {
					match from_backers.run(&self.params, &mut self.sender).await {
						Ok(data) => break Ok(data),
						Err(RecoveryError::Invalid) => break Err(RecoveryError::Invalid),
						Err(RecoveryError::ChannelClosed) =>
							break Err(RecoveryError::ChannelClosed),
						Err(RecoveryError::Unavailable) =>
							self.source = Source::RequestChunks(RequestChunksFromValidators::new(
								self.params.validators.len() as _,
								self.erasure_task_tx.clone(),
							)),
					}
				},
				Source::RequestChunks(ref mut from_all) =>
					break from_all.run(&self.params, &mut self.sender).await,
			}
		}
	}
}

/// Accumulate all awaiting sides for some particular `AvailableData`.
struct RecoveryHandle {
	candidate_hash: CandidateHash,
	remote: RemoteHandle<Result<AvailableData, RecoveryError>>,
	awaiting: Vec<oneshot::Sender<Result<AvailableData, RecoveryError>>>,
}

impl Future for RecoveryHandle {
	type Output = Option<(CandidateHash, Result<AvailableData, RecoveryError>)>;

	fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		let mut indices_to_remove = Vec::new();
		for (i, awaiting) in self.awaiting.iter_mut().enumerate().rev() {
			if let Poll::Ready(()) = awaiting.poll_canceled(cx) {
				indices_to_remove.push(i);
			}
		}

		// these are reverse order, so remove is fine.
		for index in indices_to_remove {
			gum::debug!(
				target: LOG_TARGET,
				candidate_hash = ?self.candidate_hash,
				"Receiver for available data dropped.",
			);

			self.awaiting.swap_remove(index);
		}

		if self.awaiting.is_empty() {
			gum::debug!(
				target: LOG_TARGET,
				candidate_hash = ?self.candidate_hash,
				"All receivers for available data dropped.",
			);

			return Poll::Ready(None)
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

/// Cached result of an availability recovery operation.
#[derive(Debug, Clone)]
enum CachedRecovery {
	/// Availability was successfully retrieved before.
	Valid(AvailableData),
	/// Availability was successfully retrieved before, but was found to be invalid.
	Invalid,
}

impl CachedRecovery {
	/// Convert back to	`Result` to deliver responses.
	fn into_result(self) -> Result<AvailableData, RecoveryError> {
		match self {
			Self::Valid(d) => Ok(d),
			Self::Invalid => Err(RecoveryError::Invalid),
		}
	}
}

impl TryFrom<Result<AvailableData, RecoveryError>> for CachedRecovery {
	type Error = ();
	fn try_from(o: Result<AvailableData, RecoveryError>) -> Result<CachedRecovery, Self::Error> {
		match o {
			Ok(d) => Ok(Self::Valid(d)),
			Err(RecoveryError::Invalid) => Ok(Self::Invalid),
			// We don't want to cache unavailable state, as that state might change, so if
			// requested again we want to try again!
			Err(RecoveryError::Unavailable) => Err(()),
			Err(RecoveryError::ChannelClosed) => Err(()),
		}
	}
}

struct State {
	/// Each recovery task is implemented as its own async task,
	/// and these handles are for communicating with them.
	ongoing_recoveries: FuturesUnordered<RecoveryHandle>,

	/// A recent block hash for which state should be available.
	live_block: (BlockNumber, Hash),

	/// An LRU cache of recently recovered data.
	availability_lru: LruCache<CandidateHash, CachedRecovery>,
}

impl Default for State {
	fn default() -> Self {
		Self {
			ongoing_recoveries: FuturesUnordered::new(),
			live_block: (0, Hash::default()),
			availability_lru: LruCache::new(LRU_SIZE),
		}
	}
}

#[overseer::subsystem(AvailabilityRecovery, error=SubsystemError, prefix=self::overseer)]
impl<Context> AvailabilityRecoverySubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self
			.run(ctx)
			.map_err(|e| SubsystemError::with_origin("availability-recovery", e))
			.boxed();
		SpawnedSubsystem { name: "availability-recovery-subsystem", future }
	}
}

/// Handles a signal from the overseer.
async fn handle_signal(state: &mut State, signal: OverseerSignal) -> SubsystemResult<bool> {
	match signal {
		OverseerSignal::Conclude => Ok(true),
		OverseerSignal::ActiveLeaves(ActiveLeavesUpdate { activated, .. }) => {
			// if activated is non-empty, set state.live_block to the highest block in `activated`
			if let Some(activated) = activated {
				if activated.number > state.live_block.0 {
					state.live_block = (activated.number, activated.hash)
				}
			}

			Ok(false)
		},
		OverseerSignal::BlockFinalized(_, _) => Ok(false),
	}
}

/// Machinery around launching recovery tasks into the background.
#[overseer::contextbounds(AvailabilityRecovery, prefix = self::overseer)]
async fn launch_recovery_task<Context>(
	state: &mut State,
	ctx: &mut Context,
	session_info: SessionInfo,
	receipt: CandidateReceipt,
	mut backing_group: Option<GroupIndex>,
	response_sender: oneshot::Sender<Result<AvailableData, RecoveryError>>,
	metrics: &Metrics,
	recovery_strategy: &RecoveryStrategy,
	erasure_task_tx: futures::channel::mpsc::Sender<ErasureTask>,
) -> error::Result<()> {
	let candidate_hash = receipt.hash();
	let params = RecoveryParams {
		validator_authority_keys: session_info.discovery_keys.clone(),
		validators: session_info.validators.clone(),
		threshold: recovery_threshold(session_info.validators.len())?,
		candidate_hash,
		erasure_root: receipt.descriptor.erasure_root,
		metrics: metrics.clone(),
		bypass_availability_store: recovery_strategy == &RecoveryStrategy::BypassAvailabilityStore,
	};

	if let Some(small_pov_limit) = recovery_strategy.pov_size_limit() {
		// Get our own chunk size to get an estimate of the PoV size.
		let chunk_size: Result<Option<usize>, error::Error> =
			query_chunk_size(ctx, candidate_hash).await;
		if let Ok(Some(chunk_size)) = chunk_size {
			let pov_size_estimate = chunk_size.saturating_mul(session_info.validators.len()) / 3;
			let prefer_backing_group = pov_size_estimate < small_pov_limit;

			gum::trace!(
				target: LOG_TARGET,
				?candidate_hash,
				pov_size_estimate,
				small_pov_limit,
				enabled = prefer_backing_group,
				"Prefer fetch from backing group",
			);

			backing_group = backing_group.filter(|_| {
				// We keep the backing group only if `1/3` of chunks sum up to less than
				// `small_pov_limit`.
				prefer_backing_group
			});
		}
	}

	let phase = backing_group
		.and_then(|g| session_info.validator_groups.get(g))
		.map(|group| {
			Source::RequestFromBackers(RequestFromBackers::new(
				group.clone(),
				erasure_task_tx.clone(),
			))
		})
		.unwrap_or_else(|| {
			Source::RequestChunks(RequestChunksFromValidators::new(
				params.validators.len() as _,
				erasure_task_tx.clone(),
			))
		});

	let recovery_task =
		RecoveryTask { sender: ctx.sender().clone(), params, source: phase, erasure_task_tx };

	let (remote, remote_handle) = recovery_task.run().remote_handle();

	state.ongoing_recoveries.push(RecoveryHandle {
		candidate_hash,
		remote: remote_handle,
		awaiting: vec![response_sender],
	});

	if let Err(e) = ctx.spawn("recovery-task", Box::pin(remote)) {
		gum::warn!(
			target: LOG_TARGET,
			err = ?e,
			"Failed to spawn a recovery task",
		);
	}

	Ok(())
}

/// Handles an availability recovery request.
#[overseer::contextbounds(AvailabilityRecovery, prefix = self::overseer)]
async fn handle_recover<Context>(
	state: &mut State,
	ctx: &mut Context,
	receipt: CandidateReceipt,
	session_index: SessionIndex,
	backing_group: Option<GroupIndex>,
	response_sender: oneshot::Sender<Result<AvailableData, RecoveryError>>,
	metrics: &Metrics,
	recovery_strategy: &RecoveryStrategy,
	erasure_task_tx: futures::channel::mpsc::Sender<ErasureTask>,
) -> error::Result<()> {
	let candidate_hash = receipt.hash();

	let span = jaeger::Span::new(candidate_hash, "availbility-recovery")
		.with_stage(jaeger::Stage::AvailabilityRecovery);

	if let Some(result) =
		state.availability_lru.get(&candidate_hash).cloned().map(|v| v.into_result())
	{
		if let Err(e) = response_sender.send(result) {
			gum::warn!(
				target: LOG_TARGET,
				err = ?e,
				"Error responding with an availability recovery result",
			);
		}
		return Ok(())
	}

	if let Some(i) =
		state.ongoing_recoveries.iter_mut().find(|i| i.candidate_hash == candidate_hash)
	{
		i.awaiting.push(response_sender);
		return Ok(())
	}

	let _span = span.child("not-cached");
	let session_info = request_session_info(state.live_block.1, session_index, ctx.sender())
		.await
		.await
		.map_err(error::Error::CanceledSessionInfo)??;

	let _span = span.child("session-info-ctx-received");
	match session_info {
		Some(session_info) =>
			launch_recovery_task(
				state,
				ctx,
				session_info,
				receipt,
				backing_group,
				response_sender,
				metrics,
				recovery_strategy,
				erasure_task_tx,
			)
			.await,
		None => {
			gum::warn!(target: LOG_TARGET, "SessionInfo is `None` at {:?}", state.live_block);
			response_sender
				.send(Err(RecoveryError::Unavailable))
				.map_err(|_| error::Error::CanceledResponseSender)?;
			Ok(())
		},
	}
}

/// Queries a chunk from av-store.
#[overseer::contextbounds(AvailabilityRecovery, prefix = self::overseer)]
async fn query_full_data<Context>(
	ctx: &mut Context,
	candidate_hash: CandidateHash,
) -> error::Result<Option<AvailableData>> {
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AvailabilityStoreMessage::QueryAvailableData(candidate_hash, tx))
		.await;

	rx.await.map_err(error::Error::CanceledQueryFullData)
}

/// Queries a chunk from av-store.
#[overseer::contextbounds(AvailabilityRecovery, prefix = self::overseer)]
async fn query_chunk_size<Context>(
	ctx: &mut Context,
	candidate_hash: CandidateHash,
) -> error::Result<Option<usize>> {
	let (tx, rx) = oneshot::channel();
	ctx.send_message(AvailabilityStoreMessage::QueryChunkSize(candidate_hash, tx))
		.await;

	rx.await.map_err(error::Error::CanceledQueryFullData)
}

#[overseer::contextbounds(AvailabilityRecovery, prefix = self::overseer)]
impl AvailabilityRecoverySubsystem {
	/// Create a new instance of `AvailabilityRecoverySubsystem` which never requests the  
	/// `AvailabilityStoreSubsystem` subsystem.
	pub fn with_availability_store_skip(
		req_receiver: IncomingRequestReceiver<request_v1::AvailableDataFetchingRequest>,
		metrics: Metrics,
	) -> Self {
		Self { recovery_strategy: RecoveryStrategy::BypassAvailabilityStore, req_receiver, metrics }
	}

	/// Create a new instance of `AvailabilityRecoverySubsystem` which starts with a fast path to
	/// request data from backers.
	pub fn with_fast_path(
		req_receiver: IncomingRequestReceiver<request_v1::AvailableDataFetchingRequest>,
		metrics: Metrics,
	) -> Self {
		Self { recovery_strategy: RecoveryStrategy::BackersFirstAlways, req_receiver, metrics }
	}

	/// Create a new instance of `AvailabilityRecoverySubsystem` which requests only chunks
	pub fn with_chunks_only(
		req_receiver: IncomingRequestReceiver<request_v1::AvailableDataFetchingRequest>,
		metrics: Metrics,
	) -> Self {
		Self { recovery_strategy: RecoveryStrategy::ChunksAlways, req_receiver, metrics }
	}

	/// Create a new instance of `AvailabilityRecoverySubsystem` which requests chunks if PoV is
	/// above a threshold.
	pub fn with_chunks_if_pov_large(
		req_receiver: IncomingRequestReceiver<request_v1::AvailableDataFetchingRequest>,
		metrics: Metrics,
	) -> Self {
		Self {
			recovery_strategy: RecoveryStrategy::BackersFirstIfSizeLower(SMALL_POV_LIMIT),
			req_receiver,
			metrics,
		}
	}

	async fn run<Context>(self, mut ctx: Context) -> SubsystemResult<()> {
		let mut state = State::default();
		let Self { recovery_strategy, mut req_receiver, metrics } = self;

		let (erasure_task_tx, erasure_task_rx) = futures::channel::mpsc::channel(16);
		let mut erasure_task_rx = erasure_task_rx.fuse();

		// `ThreadPoolBuilder` spawns the tasks using `spawn_blocking`. For each worker there will
		// be a `mpsc` channel created. Each of these workers take the `Receiver` and poll it in an
		// infinite loop. All of the sender ends of the channel are sent as a vec which we then use
		// to create a `Cycle` iterator. We use this iterator to assign work in a round-robin
		// fashion to the workers in the pool.
		//
		// How work is dispatched to the pool from the recovery tasks:
		// - Once a recovery task finishes retrieving the availability data, it needs to reconstruct
		//   from chunks and/or
		// re-encode the data which are heavy CPU computations.
		// To do so it sends an `ErasureTask` to the main loop via the `erasure_task` channel, and
		// waits for the results over a `oneshot` channel.
		// - In the subsystem main loop we poll the `erasure_task_rx` receiver.
		// - We forward the received `ErasureTask` to the `next()` sender yielded by the `Cycle`
		//   iterator.
		// - Some worker thread handles it and sends the response over the `oneshot` channel.

		// Create a thread pool with 2 workers.
		let mut to_pool = ThreadPoolBuilder::build(
			// Pool is guaranteed to have at least 1 worker thread.
			NonZeroUsize::new(2).expect("There are 2 threads; qed"),
			metrics.clone(),
			&mut ctx,
		)
		.into_iter()
		.cycle();

		loop {
			let recv_req = req_receiver.recv(|| vec![COST_INVALID_REQUEST]).fuse();
			pin_mut!(recv_req);
			futures::select! {
				erasure_task = erasure_task_rx.next() => {
					match erasure_task {
						Some(task) => {
							let send_result = to_pool
								.next()
								.expect("Pool size is `NonZeroUsize`; qed")
								.send(task)
								.await
								.map_err(|_| RecoveryError::ChannelClosed);

							if let Err(err) = send_result {
								gum::warn!(
									target: LOG_TARGET,
									?err,
									"Failed to send erasure coding task",
								);
							}
						},
						None => {
							gum::debug!(
								target: LOG_TARGET,
								"Erasure task channel closed",
							);

							return Err(SubsystemError::with_origin("availability-recovery", RecoveryError::ChannelClosed))
						}
					}
				}
				v = ctx.recv().fuse() => {
					match v? {
						FromOrchestra::Signal(signal) => if handle_signal(
							&mut state,
							signal,
						).await? {
							return Ok(());
						}
						FromOrchestra::Communication { msg } => {
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
										maybe_backing_group.filter(|_| recovery_strategy.needs_backing_group()),
										response_sender,
										&metrics,
										&recovery_strategy,
										erasure_task_tx.clone(),
									).await {
										gum::warn!(
											target: LOG_TARGET,
											err = ?e,
											"Error handling a recovery request",
										);
									}
								}
							}
						}
					}
				}
				in_req = recv_req => {
					match in_req.into_nested().map_err(|fatal| SubsystemError::with_origin("availability-recovery", fatal))? {
						Ok(req) => {
							if recovery_strategy == RecoveryStrategy::BypassAvailabilityStore {
								gum::debug!(
									target: LOG_TARGET,
									"Skipping request to availability-store.",
								);
								let _ = req.send_response(None.into());
								continue
							}
							match query_full_data(&mut ctx, req.payload.candidate_hash).await {
								Ok(res) => {
									let _ = req.send_response(res.into());
								}
								Err(e) => {
									gum::debug!(
										target: LOG_TARGET,
										err = ?e,
										"Failed to query available data.",
									);

									let _ = req.send_response(None.into());
								}
							}
						}
						Err(jfyi) => {
							gum::debug!(
								target: LOG_TARGET,
								error = ?jfyi,
								"Decoding incoming request failed"
							);
							continue
						}
					}
				}
				output = state.ongoing_recoveries.select_next_some() => {
					if let Some((candidate_hash, result)) = output {
						if let Ok(recovery) = CachedRecovery::try_from(result) {
							state.availability_lru.put(candidate_hash, recovery);
						}
					}
				}
			}
		}
	}
}

// A simple thread pool implementation using `spawn_blocking` threads.
struct ThreadPoolBuilder;

const MAX_THREADS: NonZeroUsize = match NonZeroUsize::new(4) {
	Some(max_threads) => max_threads,
	None => panic!("MAX_THREADS must be non-zero"),
};

impl ThreadPoolBuilder {
	// Creates a pool of `size` workers, where 1 <= `size` <= `MAX_THREADS`.
	//
	// Each worker is created by `spawn_blocking` and takes the receiver side of a channel
	// while all of the senders are returned to the caller. Each worker runs `erasure_task_thread`
	// that polls the `Receiver` for an `ErasureTask` which is expected to be CPU intensive. The
	// larger the input (more or larger chunks/availability data), the more CPU cycles will be
	// spent.
	//
	// For example, for 32KB PoVs, we'd expect re-encode to eat as much as 90ms and 500ms for
	// 2.5MiB.
	//
	// After executing such a task, the worker sends the response via a provided `oneshot` sender.
	//
	// The caller is responsible for routing work to the workers.
	#[overseer::contextbounds(AvailabilityRecovery, prefix = self::overseer)]
	pub fn build<Context>(
		size: NonZeroUsize,
		metrics: Metrics,
		ctx: &mut Context,
	) -> Vec<futures::channel::mpsc::Sender<ErasureTask>> {
		// At least 1 task, at most `MAX_THREADS.
		let size = std::cmp::min(size, MAX_THREADS);
		let mut senders = Vec::new();

		for index in 0..size.into() {
			let (tx, rx) = futures::channel::mpsc::channel(8);
			senders.push(tx);

			if let Err(e) = ctx
				.spawn_blocking("erasure-task", Box::pin(erasure_task_thread(metrics.clone(), rx)))
			{
				gum::warn!(
					target: LOG_TARGET,
					err = ?e,
					index,
					"Failed to spawn a erasure task",
				);
			}
		}
		senders
	}
}

// Handles CPU intensive operation on a dedicated blocking thread.
async fn erasure_task_thread(
	metrics: Metrics,
	mut ingress: futures::channel::mpsc::Receiver<ErasureTask>,
) {
	loop {
		match ingress.next().await {
			Some(ErasureTask::Reconstruct(n_validators, chunks, sender)) => {
				let _ = sender.send(polkadot_erasure_coding::reconstruct_v1(
					n_validators,
					chunks.values().map(|c| (&c.chunk[..], c.index.0 as usize)),
				));
			},
			Some(ErasureTask::Reencode(n_validators, root, available_data, sender)) => {
				let metrics = metrics.clone();

				let maybe_data = if reconstructed_data_matches_root(
					n_validators,
					&root,
					&available_data,
					&metrics,
				) {
					Some(available_data)
				} else {
					None
				};

				let _ = sender.send(maybe_data);
			},
			None => {
				gum::debug!(
					target: LOG_TARGET,
					"Erasure task channel closed. Node shutting down ?",
				);
			},
		}
	}
}
