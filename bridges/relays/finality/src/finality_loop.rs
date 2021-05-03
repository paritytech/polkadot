// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! The loop basically reads all missing headers and their finality proofs from the source client.
//! The proof for the best possible header is then submitted to the target node. The only exception
//! is the mandatory headers, which we always submit to the target node. For such headers, we
//! assume that the persistent proof either exists, or will eventually become available.

use crate::{FinalityProof, FinalitySyncPipeline, SourceHeader};

use async_trait::async_trait;
use backoff::backoff::Backoff;
use futures::{select, Future, FutureExt, Stream, StreamExt};
use headers_relay::sync_loop_metrics::SyncLoopMetrics;
use num_traits::{One, Saturating};
use relay_utils::{
	metrics::{GlobalMetrics, MetricsParams},
	relay_loop::Client as RelayClient,
	retry_backoff, FailedClient, MaybeConnectionError,
};
use std::{
	pin::Pin,
	time::{Duration, Instant},
};

/// Finality proof synchronization loop parameters.
#[derive(Debug, Clone)]
pub struct FinalitySyncParams {
	/// Interval at which we check updates on both clients. Normally should be larger than
	/// `min(source_block_time, target_block_time)`.
	///
	/// This parameter may be used to limit transactions rate. Increase the value && you'll get
	/// infrequent updates => sparse headers => potential slow down of bridge applications, but pallet storage
	/// won't be super large. Decrease the value to near `source_block_time` and you'll get
	/// transaction for (almost) every block of the source chain => all source headers will be known
	/// to the target chain => bridge applications will run faster, but pallet storage may explode
	/// (but if pruning is there, then it's fine).
	pub tick: Duration,
	/// Number of finality proofs to keep in internal buffer between loop wakeups.
	///
	/// While in "major syncing" state, we still read finality proofs from the stream. They're stored
	/// in the internal buffer between loop wakeups. When we're close to the tip of the chain, we may
	/// meet finality delays if headers are not finalized frequently. So instead of waiting for next
	/// finality proof to appear in the stream, we may use existing proof from that buffer.
	pub recent_finality_proofs_limit: usize,
	/// Timeout before we treat our transactions as lost and restart the whole sync process.
	pub stall_timeout: Duration,
}

/// Source client used in finality synchronization loop.
#[async_trait]
pub trait SourceClient<P: FinalitySyncPipeline>: RelayClient {
	/// Stream of new finality proofs. The stream is allowed to miss proofs for some
	/// headers, even if those headers are mandatory.
	type FinalityProofsStream: Stream<Item = P::FinalityProof>;

	/// Get best finalized block number.
	async fn best_finalized_block_number(&self) -> Result<P::Number, Self::Error>;

	/// Get canonical header and its finality proof by number.
	async fn header_and_finality_proof(
		&self,
		number: P::Number,
	) -> Result<(P::Header, Option<P::FinalityProof>), Self::Error>;

	/// Subscribe to new finality proofs.
	async fn finality_proofs(&self) -> Result<Self::FinalityProofsStream, Self::Error>;
}

/// Target client used in finality synchronization loop.
#[async_trait]
pub trait TargetClient<P: FinalitySyncPipeline>: RelayClient {
	/// Get best finalized source block number.
	async fn best_finalized_source_block_number(&self) -> Result<P::Number, Self::Error>;

	/// Submit header finality proof.
	async fn submit_finality_proof(&self, header: P::Header, proof: P::FinalityProof) -> Result<(), Self::Error>;
}

/// Return prefix that will be used by default to expose Prometheus metrics of the finality proofs sync loop.
pub fn metrics_prefix<P: FinalitySyncPipeline>() -> String {
	format!("{}_to_{}_Sync", P::SOURCE_NAME, P::TARGET_NAME)
}

/// Run finality proofs synchronization loop.
pub async fn run<P: FinalitySyncPipeline>(
	source_client: impl SourceClient<P>,
	target_client: impl TargetClient<P>,
	sync_params: FinalitySyncParams,
	metrics_params: MetricsParams,
	exit_signal: impl Future<Output = ()>,
) -> Result<(), String> {
	let exit_signal = exit_signal.shared();
	relay_utils::relay_loop(source_client, target_client)
		.with_metrics(Some(metrics_prefix::<P>()), metrics_params)
		.loop_metric(|registry, prefix| SyncLoopMetrics::new(registry, prefix))?
		.standalone_metric(|registry, prefix| GlobalMetrics::new(registry, prefix))?
		.expose()
		.await?
		.run(|source_client, target_client, metrics| {
			run_until_connection_lost(
				source_client,
				target_client,
				sync_params.clone(),
				metrics,
				exit_signal.clone(),
			)
		})
		.await
}

/// Unjustified headers container. Ordered by header number.
pub(crate) type UnjustifiedHeaders<H> = Vec<H>;
/// Finality proofs container. Ordered by target header number.
pub(crate) type FinalityProofs<P> = Vec<(
	<P as FinalitySyncPipeline>::Number,
	<P as FinalitySyncPipeline>::FinalityProof,
)>;
/// Reference to finality proofs container.
pub(crate) type FinalityProofsRef<'a, P> = &'a [(
	<P as FinalitySyncPipeline>::Number,
	<P as FinalitySyncPipeline>::FinalityProof,
)];

/// Error that may happen inside finality synchronization loop.
#[derive(Debug)]
pub(crate) enum Error<P: FinalitySyncPipeline, SourceError, TargetError> {
	/// Source client request has failed with given error.
	Source(SourceError),
	/// Target client request has failed with given error.
	Target(TargetError),
	/// Finality proof for mandatory header is missing from the source node.
	MissingMandatoryFinalityProof(P::Number),
	/// The synchronization has stalled.
	Stalled,
}

impl<P, SourceError, TargetError> Error<P, SourceError, TargetError>
where
	P: FinalitySyncPipeline,
	SourceError: MaybeConnectionError,
	TargetError: MaybeConnectionError,
{
	fn fail_if_connection_error(&self) -> Result<(), FailedClient> {
		match *self {
			Error::Source(ref error) if error.is_connection_error() => Err(FailedClient::Source),
			Error::Target(ref error) if error.is_connection_error() => Err(FailedClient::Target),
			Error::Stalled => Err(FailedClient::Both),
			_ => Ok(()),
		}
	}
}

/// Information about transaction that we have submitted.
#[derive(Debug, Clone)]
struct Transaction<Number> {
	/// Time when we have submitted this transaction.
	pub time: Instant,
	/// The number of the header we have submitted.
	pub submitted_header_number: Number,
}

/// Finality proofs stream that may be restarted.
pub(crate) struct RestartableFinalityProofsStream<S> {
	/// Flag that the stream needs to be restarted.
	pub(crate) needs_restart: bool,
	/// The stream itself.
	stream: Pin<Box<S>>,
}

#[cfg(test)]
impl<S> From<S> for RestartableFinalityProofsStream<S> {
	fn from(stream: S) -> Self {
		RestartableFinalityProofsStream {
			needs_restart: false,
			stream: Box::pin(stream),
		}
	}
}

/// Finality synchronization loop state.
struct FinalityLoopState<'a, P: FinalitySyncPipeline, FinalityProofsStream> {
	/// Synchronization loop progress.
	progress: &'a mut (Instant, Option<P::Number>),
	/// Finality proofs stream.
	finality_proofs_stream: &'a mut RestartableFinalityProofsStream<FinalityProofsStream>,
	/// Recent finality proofs that we have read from the stream.
	recent_finality_proofs: &'a mut FinalityProofs<P>,
	/// Last transaction that we have submitted to the target node.
	last_transaction: Option<Transaction<P::Number>>,
}

async fn run_until_connection_lost<P: FinalitySyncPipeline>(
	source_client: impl SourceClient<P>,
	target_client: impl TargetClient<P>,
	sync_params: FinalitySyncParams,
	metrics_sync: Option<SyncLoopMetrics>,
	exit_signal: impl Future<Output = ()>,
) -> Result<(), FailedClient> {
	let restart_finality_proofs_stream = || async {
		source_client.finality_proofs().await.map_err(|error| {
			log::error!(
				target: "bridge",
				"Failed to subscribe to {} justifications: {:?}. Going to reconnect",
				P::SOURCE_NAME,
				error,
			);

			FailedClient::Source
		})
	};

	let exit_signal = exit_signal.fuse();
	futures::pin_mut!(exit_signal);

	let mut finality_proofs_stream = RestartableFinalityProofsStream {
		needs_restart: false,
		stream: Box::pin(restart_finality_proofs_stream().await?),
	};
	let mut recent_finality_proofs = Vec::new();

	let mut progress = (Instant::now(), None);
	let mut retry_backoff = retry_backoff();
	let mut last_transaction = None;

	loop {
		// run loop iteration
		let iteration_result = run_loop_iteration(
			&source_client,
			&target_client,
			FinalityLoopState {
				progress: &mut progress,
				finality_proofs_stream: &mut finality_proofs_stream,
				recent_finality_proofs: &mut recent_finality_proofs,
				last_transaction: last_transaction.clone(),
			},
			&sync_params,
			&metrics_sync,
		)
		.await;

		// deal with errors
		let next_tick = match iteration_result {
			Ok(updated_last_transaction) => {
				last_transaction = updated_last_transaction;
				retry_backoff.reset();
				sync_params.tick
			}
			Err(error) => {
				log::error!(target: "bridge", "Finality sync loop iteration has failed with error: {:?}", error);
				error.fail_if_connection_error()?;
				retry_backoff
					.next_backoff()
					.unwrap_or(relay_utils::relay_loop::RECONNECT_DELAY)
			}
		};
		if finality_proofs_stream.needs_restart {
			log::warn!(target: "bridge", "{} finality proofs stream is being restarted", P::SOURCE_NAME);

			finality_proofs_stream.needs_restart = false;
			finality_proofs_stream.stream = Box::pin(restart_finality_proofs_stream().await?);
		}

		// wait till exit signal, or new source block
		select! {
			_ = async_std::task::sleep(next_tick).fuse() => {},
			_ = exit_signal => return Ok(()),
		}
	}
}

async fn run_loop_iteration<P, SC, TC>(
	source_client: &SC,
	target_client: &TC,
	state: FinalityLoopState<'_, P, SC::FinalityProofsStream>,
	sync_params: &FinalitySyncParams,
	metrics_sync: &Option<SyncLoopMetrics>,
) -> Result<Option<Transaction<P::Number>>, Error<P, SC::Error, TC::Error>>
where
	P: FinalitySyncPipeline,
	SC: SourceClient<P>,
	TC: TargetClient<P>,
{
	// read best source headers ids from source and target nodes
	let best_number_at_source = source_client
		.best_finalized_block_number()
		.await
		.map_err(Error::Source)?;
	let best_number_at_target = target_client
		.best_finalized_source_block_number()
		.await
		.map_err(Error::Target)?;
	if let Some(ref metrics_sync) = *metrics_sync {
		metrics_sync.update_best_block_at_source(best_number_at_source);
		metrics_sync.update_best_block_at_target(best_number_at_target);
	}
	*state.progress = print_sync_progress::<P>(*state.progress, best_number_at_source, best_number_at_target);

	// if we have already submitted header, then we just need to wait for it
	// if we're waiting too much, then we believe our transaction has been lost and restart sync
	if let Some(last_transaction) = state.last_transaction {
		if best_number_at_target >= last_transaction.submitted_header_number {
			// transaction has been mined && we can continue
		} else if last_transaction.time.elapsed() > sync_params.stall_timeout {
			log::error!(
				target: "bridge",
				"Finality synchronization from {} to {} has stalled. Going to restart",
				P::SOURCE_NAME,
				P::TARGET_NAME,
			);

			return Err(Error::Stalled);
		} else {
			return Ok(Some(last_transaction));
		}
	}

	// submit new header if we have something new
	match select_header_to_submit(
		source_client,
		target_client,
		state.finality_proofs_stream,
		state.recent_finality_proofs,
		best_number_at_source,
		best_number_at_target,
		sync_params,
	)
	.await?
	{
		Some((header, justification)) => {
			let new_transaction = Transaction {
				time: Instant::now(),
				submitted_header_number: header.number(),
			};

			log::debug!(
				target: "bridge",
				"Going to submit finality proof of {} header #{:?} to {}",
				P::SOURCE_NAME,
				new_transaction.submitted_header_number,
				P::TARGET_NAME,
			);

			target_client
				.submit_finality_proof(header, justification)
				.await
				.map_err(Error::Target)?;
			Ok(Some(new_transaction))
		}
		None => Ok(None),
	}
}

async fn select_header_to_submit<P, SC, TC>(
	source_client: &SC,
	target_client: &TC,
	finality_proofs_stream: &mut RestartableFinalityProofsStream<SC::FinalityProofsStream>,
	recent_finality_proofs: &mut FinalityProofs<P>,
	best_number_at_source: P::Number,
	best_number_at_target: P::Number,
	sync_params: &FinalitySyncParams,
) -> Result<Option<(P::Header, P::FinalityProof)>, Error<P, SC::Error, TC::Error>>
where
	P: FinalitySyncPipeline,
	SC: SourceClient<P>,
	TC: TargetClient<P>,
{
	// to see that the loop is progressing
	log::trace!(
		target: "bridge",
		"Considering range of headers ({:?}; {:?}]",
		best_number_at_target,
		best_number_at_source,
	);

	// read missing headers. if we see that the header schedules GRANDPA change, we need to
	// submit this header
	let selected_finality_proof = read_missing_headers::<P, SC, TC>(
		source_client,
		target_client,
		best_number_at_source,
		best_number_at_target,
	)
	.await?;
	let (mut unjustified_headers, mut selected_finality_proof) = match selected_finality_proof {
		SelectedFinalityProof::Mandatory(header, finality_proof) => return Ok(Some((header, finality_proof))),
		SelectedFinalityProof::Regular(unjustified_headers, header, finality_proof) => {
			(unjustified_headers, Some((header, finality_proof)))
		}
		SelectedFinalityProof::None(unjustified_headers) => (unjustified_headers, None),
	};

	// all headers that are missing from the target client are non-mandatory
	// => even if we have already selected some header and its persistent finality proof,
	// we may try to select better header by reading non-persistent proofs from the stream
	read_finality_proofs_from_stream::<P, _>(finality_proofs_stream, recent_finality_proofs);
	selected_finality_proof = select_better_recent_finality_proof::<P>(
		recent_finality_proofs,
		&mut unjustified_headers,
		selected_finality_proof,
	);

	// remove obsolete 'recent' finality proofs + keep its size under certain limit
	let oldest_finality_proof_to_keep = selected_finality_proof
		.as_ref()
		.map(|(header, _)| header.number())
		.unwrap_or(best_number_at_target);
	prune_recent_finality_proofs::<P>(
		oldest_finality_proof_to_keep,
		recent_finality_proofs,
		sync_params.recent_finality_proofs_limit,
	);

	Ok(selected_finality_proof)
}

/// Finality proof that has been selected by the `read_missing_headers` function.
pub(crate) enum SelectedFinalityProof<Header, FinalityProof> {
	/// Mandatory header and its proof has been selected. We shall submit proof for this header.
	Mandatory(Header, FinalityProof),
	/// Regular header and its proof has been selected. We may submit this proof, or proof for
	/// some better header.
	Regular(UnjustifiedHeaders<Header>, Header, FinalityProof),
	/// We haven't found any missing header with persistent proof at the target client.
	None(UnjustifiedHeaders<Header>),
}

/// Read missing headers and their persistent finality proofs from the target client.
///
/// If we have found some header with known proof, it is returned.
/// Otherwise, `SelectedFinalityProof::None` is returned.
///
/// Unless we have found mandatory header, all missing headers are collected and returned.
pub(crate) async fn read_missing_headers<P: FinalitySyncPipeline, SC: SourceClient<P>, TC: TargetClient<P>>(
	source_client: &SC,
	_target_client: &TC,
	best_number_at_source: P::Number,
	best_number_at_target: P::Number,
) -> Result<SelectedFinalityProof<P::Header, P::FinalityProof>, Error<P, SC::Error, TC::Error>> {
	let mut unjustified_headers = Vec::new();
	let mut selected_finality_proof = None;
	let mut header_number = best_number_at_target + One::one();
	while header_number <= best_number_at_source {
		let (header, finality_proof) = source_client
			.header_and_finality_proof(header_number)
			.await
			.map_err(Error::Source)?;
		let is_mandatory = header.is_mandatory();

		match (is_mandatory, finality_proof) {
			(true, Some(finality_proof)) => {
				log::trace!(target: "bridge", "Header {:?} is mandatory", header_number);
				return Ok(SelectedFinalityProof::Mandatory(header, finality_proof));
			}
			(true, None) => return Err(Error::MissingMandatoryFinalityProof(header.number())),
			(false, Some(finality_proof)) => {
				log::trace!(target: "bridge", "Header {:?} has persistent finality proof", header_number);
				unjustified_headers.clear();
				selected_finality_proof = Some((header, finality_proof));
			}
			(false, None) => {
				unjustified_headers.push(header);
			}
		}

		header_number = header_number + One::one();
	}

	Ok(match selected_finality_proof {
		Some((header, proof)) => SelectedFinalityProof::Regular(unjustified_headers, header, proof),
		None => SelectedFinalityProof::None(unjustified_headers),
	})
}

/// Read finality proofs from the stream.
pub(crate) fn read_finality_proofs_from_stream<P: FinalitySyncPipeline, FPS: Stream<Item = P::FinalityProof>>(
	finality_proofs_stream: &mut RestartableFinalityProofsStream<FPS>,
	recent_finality_proofs: &mut FinalityProofs<P>,
) {
	loop {
		let next_proof = finality_proofs_stream.stream.next();
		let finality_proof = match next_proof.now_or_never() {
			Some(Some(finality_proof)) => finality_proof,
			Some(None) => {
				finality_proofs_stream.needs_restart = true;
				break;
			}
			None => break,
		};

		recent_finality_proofs.push((finality_proof.target_header_number(), finality_proof));
	}
}

/// Try to select better header and its proof, given finality proofs that we
/// have recently read from the stream.
pub(crate) fn select_better_recent_finality_proof<P: FinalitySyncPipeline>(
	recent_finality_proofs: FinalityProofsRef<P>,
	unjustified_headers: &mut UnjustifiedHeaders<P::Header>,
	selected_finality_proof: Option<(P::Header, P::FinalityProof)>,
) -> Option<(P::Header, P::FinalityProof)> {
	if unjustified_headers.is_empty() || recent_finality_proofs.is_empty() {
		return selected_finality_proof;
	}

	const NOT_EMPTY_PROOF: &str = "we have checked that the vec is not empty; qed";

	// we need proofs for headers in range unjustified_range_begin..=unjustified_range_end
	let unjustified_range_begin = unjustified_headers.first().expect(NOT_EMPTY_PROOF).number();
	let unjustified_range_end = unjustified_headers.last().expect(NOT_EMPTY_PROOF).number();

	// we have proofs for headers in range buffered_range_begin..=buffered_range_end
	let buffered_range_begin = recent_finality_proofs.first().expect(NOT_EMPTY_PROOF).0;
	let buffered_range_end = recent_finality_proofs.last().expect(NOT_EMPTY_PROOF).0;

	// we have two ranges => find intersection
	let intersection_begin = std::cmp::max(unjustified_range_begin, buffered_range_begin);
	let intersection_end = std::cmp::min(unjustified_range_end, buffered_range_end);
	let intersection = intersection_begin..=intersection_end;

	// find last proof from intersection
	let selected_finality_proof_index = recent_finality_proofs
		.binary_search_by_key(intersection.end(), |(number, _)| *number)
		.unwrap_or_else(|index| index.saturating_sub(1));
	let (selected_header_number, finality_proof) = &recent_finality_proofs[selected_finality_proof_index];
	if !intersection.contains(selected_header_number) {
		return selected_finality_proof;
	}

	// now remove all obsolete headers and extract selected header
	let selected_header_position = unjustified_headers
		.binary_search_by_key(selected_header_number, |header| header.number())
		.expect("unjustified_headers contain all headers from intersection; qed");
	let selected_header = unjustified_headers.swap_remove(selected_header_position);
	Some((selected_header, finality_proof.clone()))
}

pub(crate) fn prune_recent_finality_proofs<P: FinalitySyncPipeline>(
	justified_header_number: P::Number,
	recent_finality_proofs: &mut FinalityProofs<P>,
	recent_finality_proofs_limit: usize,
) {
	let position =
		recent_finality_proofs.binary_search_by_key(&justified_header_number, |(header_number, _)| *header_number);

	// remove all obsolete elements
	*recent_finality_proofs = recent_finality_proofs.split_off(
		position
			.map(|position| position + 1)
			.unwrap_or_else(|position| position),
	);

	// now - limit vec by size
	let split_index = recent_finality_proofs
		.len()
		.saturating_sub(recent_finality_proofs_limit);
	*recent_finality_proofs = recent_finality_proofs.split_off(split_index);
}

fn print_sync_progress<P: FinalitySyncPipeline>(
	progress_context: (Instant, Option<P::Number>),
	best_number_at_source: P::Number,
	best_number_at_target: P::Number,
) -> (Instant, Option<P::Number>) {
	let (prev_time, prev_best_number_at_target) = progress_context;
	let now = Instant::now();

	let need_update = now - prev_time > Duration::from_secs(10)
		|| prev_best_number_at_target
			.map(|prev_best_number_at_target| {
				best_number_at_target.saturating_sub(prev_best_number_at_target) > 10.into()
			})
			.unwrap_or(true);

	if !need_update {
		return (prev_time, prev_best_number_at_target);
	}

	log::info!(
		target: "bridge",
		"Synced {:?} of {:?} headers",
		best_number_at_target,
		best_number_at_source,
	);
	(now, Some(best_number_at_target))
}
