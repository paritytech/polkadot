// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Entrypoint for running headers synchronization loop.

use crate::sync::{HeadersSync, HeadersSyncParams};
use crate::sync_loop_metrics::SyncLoopMetrics;
use crate::sync_types::{HeaderIdOf, HeaderStatus, HeadersSyncPipeline, QueuedHeader, SubmittedHeaders};

use async_trait::async_trait;
use futures::{future::FutureExt, stream::StreamExt};
use num_traits::{Saturating, Zero};
use relay_utils::{
	format_ids, interval,
	metrics::{start as metrics_start, GlobalMetrics, MetricsParams},
	process_future_result,
	relay_loop::Client as RelayClient,
	retry_backoff, FailedClient, MaybeConnectionError, StringifiedMaybeConnectionError,
};
use std::{
	collections::HashSet,
	future::Future,
	time::{Duration, Instant},
};

/// When we submit headers to target node, but see no updates of best
/// source block known to target node during STALL_SYNC_TIMEOUT seconds,
/// we consider that our headers are rejected because there has been reorg in target chain.
/// This reorg could invalidate our knowledge about sync process (i.e. we have asked if
/// HeaderA is known to target, but then reorg happened and the answer is different
/// now) => we need to reset sync.
/// The other option is to receive **EVERY** best target header and check if it is
/// direct child of previous best header. But: (1) subscription doesn't guarantee that
/// the subscriber will receive every best header (2) reorg won't always lead to sync
/// stall and restart is a heavy operation (we forget all in-memory headers).
const STALL_SYNC_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Delay after we have seen update of best source header at target node,
/// for us to treat sync stalled. ONLY when relay operates in backup mode.
const BACKUP_STALL_SYNC_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Interval between calling sync maintain procedure.
const MAINTAIN_INTERVAL: Duration = Duration::from_secs(30);

/// Source client trait.
#[async_trait]
pub trait SourceClient<P: HeadersSyncPipeline>: RelayClient {
	/// Get best block number.
	async fn best_block_number(&self) -> Result<P::Number, Self::Error>;

	/// Get header by hash.
	async fn header_by_hash(&self, hash: P::Hash) -> Result<P::Header, Self::Error>;

	/// Get canonical header by number.
	async fn header_by_number(&self, number: P::Number) -> Result<P::Header, Self::Error>;

	/// Get completion data by header hash.
	async fn header_completion(&self, id: HeaderIdOf<P>)
		-> Result<(HeaderIdOf<P>, Option<P::Completion>), Self::Error>;

	/// Get extra data by header hash.
	async fn header_extra(
		&self,
		id: HeaderIdOf<P>,
		header: QueuedHeader<P>,
	) -> Result<(HeaderIdOf<P>, P::Extra), Self::Error>;
}

/// Target client trait.
#[async_trait]
pub trait TargetClient<P: HeadersSyncPipeline>: RelayClient {
	/// Returns ID of best header known to the target node.
	async fn best_header_id(&self) -> Result<HeaderIdOf<P>, Self::Error>;

	/// Returns true if header is known to the target node.
	async fn is_known_header(&self, id: HeaderIdOf<P>) -> Result<(HeaderIdOf<P>, bool), Self::Error>;

	/// Submit headers.
	async fn submit_headers(&self, headers: Vec<QueuedHeader<P>>) -> SubmittedHeaders<HeaderIdOf<P>, Self::Error>;

	/// Returns ID of headers that require to be 'completed' before children can be submitted.
	async fn incomplete_headers_ids(&self) -> Result<HashSet<HeaderIdOf<P>>, Self::Error>;

	/// Submit completion data for header.
	async fn complete_header(&self, id: HeaderIdOf<P>, completion: P::Completion)
		-> Result<HeaderIdOf<P>, Self::Error>;

	/// Returns true if header requires extra data to be submitted.
	async fn requires_extra(&self, header: QueuedHeader<P>) -> Result<(HeaderIdOf<P>, bool), Self::Error>;
}

/// Synchronization maintain procedure.
#[async_trait]
pub trait SyncMaintain<P: HeadersSyncPipeline>: Clone + Send + Sync {
	/// Run custom maintain procedures. This is guaranteed to be called when both source and target
	/// clients are unoccupied.
	async fn maintain(&self, _sync: &mut HeadersSync<P>) {}
}

impl<P: HeadersSyncPipeline> SyncMaintain<P> for () {}

/// Run headers synchronization.
#[allow(clippy::too_many_arguments)]
pub fn run<P: HeadersSyncPipeline, TC: TargetClient<P>>(
	source_client: impl SourceClient<P>,
	source_tick: Duration,
	target_client: TC,
	target_tick: Duration,
	sync_maintain: impl SyncMaintain<P>,
	sync_params: HeadersSyncParams,
	metrics_params: Option<MetricsParams>,
	exit_signal: impl Future<Output = ()>,
) {
	let exit_signal = exit_signal.shared();

	let metrics_global = GlobalMetrics::default();
	let metrics_sync = SyncLoopMetrics::default();
	let metrics_enabled = metrics_params.is_some();
	metrics_start(
		format!("{}_to_{}_Sync", P::SOURCE_NAME, P::TARGET_NAME),
		metrics_params,
		&metrics_global,
		&metrics_sync,
	);

	relay_utils::relay_loop::run(
		relay_utils::relay_loop::RECONNECT_DELAY,
		source_client,
		target_client,
		|source_client, target_client| {
			run_until_connection_lost(
				source_client,
				source_tick,
				target_client,
				target_tick,
				sync_maintain.clone(),
				sync_params.clone(),
				if metrics_enabled {
					Some(metrics_global.clone())
				} else {
					None
				},
				if metrics_enabled {
					Some(metrics_sync.clone())
				} else {
					None
				},
				exit_signal.clone(),
			)
		},
	);
}

/// Run headers synchronization.
#[allow(clippy::too_many_arguments)]
async fn run_until_connection_lost<P: HeadersSyncPipeline, TC: TargetClient<P>>(
	source_client: impl SourceClient<P>,
	source_tick: Duration,
	target_client: TC,
	target_tick: Duration,
	sync_maintain: impl SyncMaintain<P>,
	sync_params: HeadersSyncParams,
	metrics_global: Option<GlobalMetrics>,
	metrics_sync: Option<SyncLoopMetrics>,
	exit_signal: impl Future<Output = ()>,
) -> Result<(), FailedClient> {
	let mut progress_context = (Instant::now(), None, None);

	let mut sync = HeadersSync::<P>::new(sync_params);
	let mut stall_countdown = None;
	let mut last_update_time = Instant::now();

	let mut source_retry_backoff = retry_backoff();
	let mut source_client_is_online = false;
	let mut source_best_block_number_required = false;
	let source_best_block_number_future = source_client.best_block_number().fuse();
	let source_new_header_future = futures::future::Fuse::terminated();
	let source_orphan_header_future = futures::future::Fuse::terminated();
	let source_extra_future = futures::future::Fuse::terminated();
	let source_completion_future = futures::future::Fuse::terminated();
	let source_go_offline_future = futures::future::Fuse::terminated();
	let source_tick_stream = interval(source_tick).fuse();

	let mut target_retry_backoff = retry_backoff();
	let mut target_client_is_online = false;
	let mut target_best_block_required = false;
	let mut target_incomplete_headers_required = true;
	let target_best_block_future = target_client.best_header_id().fuse();
	let target_incomplete_headers_future = futures::future::Fuse::terminated();
	let target_extra_check_future = futures::future::Fuse::terminated();
	let target_existence_status_future = futures::future::Fuse::terminated();
	let target_submit_header_future = futures::future::Fuse::terminated();
	let target_complete_header_future = futures::future::Fuse::terminated();
	let target_go_offline_future = futures::future::Fuse::terminated();
	let target_tick_stream = interval(target_tick).fuse();

	let mut maintain_required = false;
	let maintain_stream = interval(MAINTAIN_INTERVAL).fuse();

	let exit_signal = exit_signal.fuse();

	futures::pin_mut!(
		source_best_block_number_future,
		source_new_header_future,
		source_orphan_header_future,
		source_extra_future,
		source_completion_future,
		source_go_offline_future,
		source_tick_stream,
		target_best_block_future,
		target_incomplete_headers_future,
		target_extra_check_future,
		target_existence_status_future,
		target_submit_header_future,
		target_complete_header_future,
		target_go_offline_future,
		target_tick_stream,
		maintain_stream,
		exit_signal
	);

	loop {
		futures::select! {
			source_best_block_number = source_best_block_number_future => {
				source_best_block_number_required = false;

				source_client_is_online = process_future_result(
					source_best_block_number,
					&mut source_retry_backoff,
					|source_best_block_number| sync.source_best_header_number_response(source_best_block_number),
					&mut source_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving best header number from {}", P::SOURCE_NAME),
				).fail_if_connection_error(FailedClient::Source)?;
			},
			source_new_header = source_new_header_future => {
				source_client_is_online = process_future_result(
					source_new_header,
					&mut source_retry_backoff,
					|source_new_header| sync.headers_mut().header_response(source_new_header),
					&mut source_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving header from {} node", P::SOURCE_NAME),
				).fail_if_connection_error(FailedClient::Source)?;
			},
			source_orphan_header = source_orphan_header_future => {
				source_client_is_online = process_future_result(
					source_orphan_header,
					&mut source_retry_backoff,
					|source_orphan_header| sync.headers_mut().header_response(source_orphan_header),
					&mut source_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving orphan header from {} node", P::SOURCE_NAME),
				).fail_if_connection_error(FailedClient::Source)?;
			},
			source_extra = source_extra_future => {
				source_client_is_online = process_future_result(
					source_extra,
					&mut source_retry_backoff,
					|(header, extra)| sync.headers_mut().extra_response(&header, extra),
					&mut source_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving extra data from {} node", P::SOURCE_NAME),
				).fail_if_connection_error(FailedClient::Source)?;
			},
			source_completion = source_completion_future => {
				source_client_is_online = process_future_result(
					source_completion,
					&mut source_retry_backoff,
					|(header, completion)| sync.headers_mut().completion_response(&header, completion),
					&mut source_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving completion data from {} node", P::SOURCE_NAME),
				).fail_if_connection_error(FailedClient::Source)?;
			},
			_ = source_go_offline_future => {
				source_client_is_online = true;
			},
			_ = source_tick_stream.next() => {
				if sync.is_almost_synced() {
					source_best_block_number_required = true;
				}
			},
			target_best_block = target_best_block_future => {
				target_best_block_required = false;

				target_client_is_online = process_future_result(
					target_best_block,
					&mut target_retry_backoff,
					|target_best_block| {
						let head_updated = sync.target_best_header_response(target_best_block);
						if head_updated {
							last_update_time = Instant::now();
						}
						match head_updated {
							// IF head is updated AND there are still our transactions:
							// => restart stall countdown timer
							true if sync.headers().headers_in_status(HeaderStatus::Submitted) != 0 =>
								stall_countdown = Some(Instant::now()),
							// IF head is updated AND there are no our transactions:
							// => stop stall countdown timer
							true => stall_countdown = None,
							// IF head is not updated AND stall countdown is not yet completed
							// => do nothing
							false if stall_countdown
								.map(|stall_countdown| stall_countdown.elapsed() < STALL_SYNC_TIMEOUT)
								.unwrap_or(true)
								=> (),
							// IF head is not updated AND stall countdown has completed
							// => restart sync
							false => {
								log::info!(
									target: "bridge",
									"Sync has stalled. Restarting {} headers synchronization.",
									P::SOURCE_NAME,
								);
								stall_countdown = None;
								sync.restart();
							},
						}
					},
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving best known {} header from {} node", P::SOURCE_NAME, P::TARGET_NAME),
				).fail_if_connection_error(FailedClient::Target)?;
			},
			incomplete_headers_ids = target_incomplete_headers_future => {
				target_incomplete_headers_required = false;

				target_client_is_online = process_future_result(
					incomplete_headers_ids,
					&mut target_retry_backoff,
					|incomplete_headers_ids| sync.headers_mut().incomplete_headers_response(incomplete_headers_ids),
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving incomplete headers from {} node", P::TARGET_NAME),
				).fail_if_connection_error(FailedClient::Target)?;
			},
			target_existence_status = target_existence_status_future => {
				target_client_is_online = process_future_result(
					target_existence_status,
					&mut target_retry_backoff,
					|(target_header, target_existence_status)| sync
						.headers_mut()
						.maybe_orphan_response(&target_header, target_existence_status),
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving existence status from {} node", P::TARGET_NAME),
				).fail_if_connection_error(FailedClient::Target)?;
			},
			submitted_headers = target_submit_header_future => {
				// following line helps Rust understand the type of `submitted_headers` :/
				let submitted_headers: SubmittedHeaders<HeaderIdOf<P>, TC::Error> = submitted_headers;
				let submitted_headers_str = format!("{}", submitted_headers);
				let all_headers_rejected = submitted_headers.submitted.is_empty()
					&& submitted_headers.incomplete.is_empty();
				let has_submitted_headers = sync.headers().headers_in_status(HeaderStatus::Submitted) != 0;

				let maybe_fatal_error = match submitted_headers.fatal_error {
					Some(fatal_error) => Err(StringifiedMaybeConnectionError::new(
						fatal_error.is_connection_error(),
						format!("{:?}", fatal_error),
					)),
					None if all_headers_rejected && !has_submitted_headers =>
						Err(StringifiedMaybeConnectionError::new(false, "All headers were rejected".into())),
					None => Ok(()),
				};

				let no_fatal_error = maybe_fatal_error.is_ok();
				target_client_is_online = process_future_result(
					maybe_fatal_error,
					&mut target_retry_backoff,
					|_| {},
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error submitting headers to {} node", P::TARGET_NAME),
				).fail_if_connection_error(FailedClient::Target)?;

				log::debug!(target: "bridge", "Header submit result: {}", submitted_headers_str);

				sync.headers_mut().headers_submitted(submitted_headers.submitted);
				sync.headers_mut().add_incomplete_headers(false, submitted_headers.incomplete);

				// when there's no fatal error, but node has rejected all our headers we may
				// want to pause until our submitted headers will be accepted
				if no_fatal_error && all_headers_rejected && has_submitted_headers {
					sync.pause_submit();
				}
			},
			target_complete_header_result = target_complete_header_future => {
				target_client_is_online = process_future_result(
					target_complete_header_result,
					&mut target_retry_backoff,
					|completed_header| sync.headers_mut().header_completed(&completed_header),
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error completing headers at {}", P::TARGET_NAME),
				).fail_if_connection_error(FailedClient::Target)?;
			},
			target_extra_check_result = target_extra_check_future => {
				target_client_is_online = process_future_result(
					target_extra_check_result,
					&mut target_retry_backoff,
					|(header, extra_check_result)| sync
						.headers_mut()
						.maybe_extra_response(&header, extra_check_result),
					&mut target_go_offline_future,
					async_std::task::sleep,
					|| format!("Error retrieving receipts requirement from {} node", P::TARGET_NAME),
				).fail_if_connection_error(FailedClient::Target)?;
			},
			_ = target_go_offline_future => {
				target_client_is_online = true;
			},
			_ = target_tick_stream.next() => {
				target_best_block_required = true;
				target_incomplete_headers_required = true;
			},

			_ = maintain_stream.next() => {
				maintain_required = true;
			},
			_ = exit_signal => {
				return Ok(());
			}
		}

		// update metrics
		if let Some(ref metrics_global) = metrics_global {
			metrics_global.update().await;
		}
		if let Some(ref metrics_sync) = metrics_sync {
			metrics_sync.update(&sync);
		}

		// print progress
		progress_context = print_sync_progress(progress_context, &sync);

		// run maintain procedures
		if maintain_required && source_client_is_online && target_client_is_online {
			log::debug!(target: "bridge", "Maintaining headers sync loop");
			maintain_required = false;
			sync_maintain.maintain(&mut sync).await;
		}

		// If the target client is accepting requests we update the requests that
		// we want it to run
		if !maintain_required && target_client_is_online {
			// NOTE: Is is important to reset this so that we only have one
			// request being processed by the client at a time. This prevents
			// race conditions like receiving two transactions with the same
			// nonce from the client.
			target_client_is_online = false;

			// The following is how we prioritize requests:
			//
			// 1. Get best block
			//     - Stops us from downloading or submitting new blocks
			//     - Only called rarely
			//
			// 2. Get incomplete headers
			//     - Stops us from submitting new blocks
			//     - Only called rarely
			//
			// 3. Get complete headers
			//     - Stops us from submitting new blocks
			//
			// 4. Check if we need extra data from source
			//     - Stops us from downloading or submitting new blocks
			//
			// 5. Check existence of header
			//     - Stops us from submitting new blocks
			//
			// 6. Submit header

			if target_best_block_required {
				log::debug!(target: "bridge", "Asking {} about best block", P::TARGET_NAME);
				target_best_block_future.set(target_client.best_header_id().fuse());
			} else if target_incomplete_headers_required {
				log::debug!(target: "bridge", "Asking {} about incomplete headers", P::TARGET_NAME);
				target_incomplete_headers_future.set(target_client.incomplete_headers_ids().fuse());
			} else if let Some((id, completion)) = sync.headers_mut().header_to_complete() {
				log::debug!(
					target: "bridge",
					"Going to complete header: {:?}",
					id,
				);

				target_complete_header_future.set(target_client.complete_header(id, completion.clone()).fuse());
			} else if let Some(header) = sync.headers().header(HeaderStatus::MaybeExtra) {
				log::debug!(
					target: "bridge",
					"Checking if header submission requires extra: {:?}",
					header.id(),
				);

				target_extra_check_future.set(target_client.requires_extra(header.clone()).fuse());
			} else if let Some(header) = sync.headers().header(HeaderStatus::MaybeOrphan) {
				// for MaybeOrphan we actually ask for parent' header existence
				let parent_id = header.parent_id();

				log::debug!(
					target: "bridge",
					"Asking {} node for existence of: {:?}",
					P::TARGET_NAME,
					parent_id,
				);

				target_existence_status_future.set(target_client.is_known_header(parent_id).fuse());
			} else if let Some(headers) =
				sync.select_headers_to_submit(last_update_time.elapsed() > BACKUP_STALL_SYNC_TIMEOUT)
			{
				log::debug!(
					target: "bridge",
					"Submitting {} header(s) to {} node: {:?}",
					headers.len(),
					P::TARGET_NAME,
					format_ids(headers.iter().map(|header| header.id())),
				);

				let headers = headers.into_iter().cloned().collect();
				target_submit_header_future.set(target_client.submit_headers(headers).fuse());

				// remember that we have submitted some headers
				if stall_countdown.is_none() {
					stall_countdown = Some(Instant::now());
				}
			} else {
				target_client_is_online = true;
			}
		}

		// If the source client is accepting requests we update the requests that
		// we want it to run
		if !maintain_required && source_client_is_online {
			// NOTE: Is is important to reset this so that we only have one
			// request being processed by the client at a time. This prevents
			// race conditions like receiving two transactions with the same
			// nonce from the client.
			source_client_is_online = false;

			// The following is how we prioritize requests:
			//
			// 1. Get best block
			//     - Stops us from downloading or submitting new blocks
			//     - Only called rarely
			//
			// 2. Download completion data
			//     - Stops us from submitting new blocks
			//
			// 3. Download extra data
			//     - Stops us from submitting new blocks
			//
			// 4. Download missing headers
			//     - Stops us from downloading or submitting new blocks
			//
			// 5. Downloading new headers

			if source_best_block_number_required {
				log::debug!(target: "bridge", "Asking {} node about best block number", P::SOURCE_NAME);
				source_best_block_number_future.set(source_client.best_block_number().fuse());
			} else if let Some(id) = sync.headers_mut().incomplete_header() {
				log::debug!(
					target: "bridge",
					"Retrieving completion data for header: {:?}",
					id,
				);
				source_completion_future.set(source_client.header_completion(id).fuse());
			} else if let Some(header) = sync.headers().header(HeaderStatus::Extra) {
				let id = header.id();
				log::debug!(
					target: "bridge",
					"Retrieving extra data for header: {:?}",
					id,
				);
				source_extra_future.set(source_client.header_extra(id, header.clone()).fuse());
			} else if let Some(header) = sync.select_orphan_header_to_download() {
				// for Orphan we actually ask for parent' header
				let parent_id = header.parent_id();

				// if we have end up with orphan header#0, then we are misconfigured
				if parent_id.0.is_zero() {
					log::error!(
						target: "bridge",
						"Misconfiguration. Genesis {} header is considered orphan by {} node",
						P::SOURCE_NAME,
						P::TARGET_NAME,
					);
					return Ok(());
				}

				log::debug!(
					target: "bridge",
					"Going to download orphan header from {} node: {:?}",
					P::SOURCE_NAME,
					parent_id,
				);

				source_orphan_header_future.set(source_client.header_by_hash(parent_id.1).fuse());
			} else if let Some(id) = sync.select_new_header_to_download() {
				log::debug!(
					target: "bridge",
					"Going to download new header from {} node: {:?}",
					P::SOURCE_NAME,
					id,
				);

				source_new_header_future.set(source_client.header_by_number(id).fuse());
			} else {
				source_client_is_online = true;
			}
		}
	}
}

/// Print synchronization progress.
fn print_sync_progress<P: HeadersSyncPipeline>(
	progress_context: (Instant, Option<P::Number>, Option<P::Number>),
	eth_sync: &HeadersSync<P>,
) -> (Instant, Option<P::Number>, Option<P::Number>) {
	let (prev_time, prev_best_header, prev_target_header) = progress_context;
	let now_time = Instant::now();
	let (now_best_header, now_target_header) = eth_sync.status();

	let need_update = now_time - prev_time > Duration::from_secs(10)
		|| match (prev_best_header, now_best_header) {
			(Some(prev_best_header), Some(now_best_header)) => {
				now_best_header.0.saturating_sub(prev_best_header) > 10.into()
			}
			_ => false,
		};
	if !need_update {
		return (prev_time, prev_best_header, prev_target_header);
	}

	log::info!(
		target: "bridge",
		"Synced {:?} of {:?} headers",
		now_best_header.map(|id| id.0),
		now_target_header,
	);
	(now_time, now_best_header.clone().map(|id| id.0), *now_target_header)
}
