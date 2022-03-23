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

//! On-demand Substrate -> Substrate headers relay.

use async_std::sync::{Arc, Mutex};
use futures::{select, FutureExt};
use num_traits::{One, Zero};

use finality_relay::{FinalitySyncParams, SourceHeader, TargetClient as FinalityTargetClient};
use relay_substrate_client::{
	AccountIdOf, AccountKeyPairOf, BlockNumberOf, Chain, Client, HeaderIdOf, HeaderOf, SyncHeader,
	TransactionSignScheme,
};
use relay_utils::{
	metrics::MetricsParams, relay_loop::Client as RelayClient, FailedClient, MaybeConnectionError,
};

use crate::{
	finality_pipeline::{SubstrateFinalitySyncPipeline, RECENT_FINALITY_PROOFS_LIMIT},
	finality_source::{RequiredHeaderNumberRef, SubstrateFinalitySource},
	finality_target::SubstrateFinalityTarget,
	TransactionParams, STALL_TIMEOUT,
};

/// On-demand Substrate <-> Substrate headers relay.
///
/// This relay may be requested to sync more headers, whenever some other relay (e.g. messages
/// relay) needs it to continue its regular work. When enough headers are relayed, on-demand stops
/// syncing headers.
#[derive(Clone)]
pub struct OnDemandHeadersRelay<SourceChain: Chain> {
	/// Relay task name.
	relay_task_name: String,
	/// Shared reference to maximal required finalized header number.
	required_header_number: RequiredHeaderNumberRef<SourceChain>,
}

impl<SourceChain: Chain> OnDemandHeadersRelay<SourceChain> {
	/// Create new on-demand headers relay.
	pub fn new<P: SubstrateFinalitySyncPipeline<SourceChain = SourceChain>>(
		source_client: Client<P::SourceChain>,
		target_client: Client<P::TargetChain>,
		target_transaction_params: TransactionParams<AccountKeyPairOf<P::TransactionSignScheme>>,
		only_mandatory_headers: bool,
	) -> Self
	where
		AccountIdOf<P::TargetChain>:
			From<<AccountKeyPairOf<P::TransactionSignScheme> as sp_core::Pair>::Public>,
		P::TransactionSignScheme: TransactionSignScheme<Chain = P::TargetChain>,
	{
		let required_header_number = Arc::new(Mutex::new(Zero::zero()));
		let this = OnDemandHeadersRelay {
			relay_task_name: on_demand_headers_relay_name::<P::SourceChain, P::TargetChain>(),
			required_header_number: required_header_number.clone(),
		};
		async_std::task::spawn(async move {
			background_task::<P>(
				source_client,
				target_client,
				target_transaction_params,
				only_mandatory_headers,
				required_header_number,
			)
			.await;
		});

		this
	}

	/// Someone is asking us to relay given finalized header.
	pub async fn require_finalized_header(&self, header_id: HeaderIdOf<SourceChain>) {
		let mut required_header_number = self.required_header_number.lock().await;
		if header_id.0 > *required_header_number {
			log::trace!(
				target: "bridge",
				"More {} headers required in {} relay. Going to sync up to the {}",
				SourceChain::NAME,
				self.relay_task_name,
				header_id.0,
			);

			*required_header_number = header_id.0;
		}
	}
}

/// Background task that is responsible for starting headers relay.
async fn background_task<P: SubstrateFinalitySyncPipeline>(
	source_client: Client<P::SourceChain>,
	target_client: Client<P::TargetChain>,
	target_transaction_params: TransactionParams<AccountKeyPairOf<P::TransactionSignScheme>>,
	only_mandatory_headers: bool,
	required_header_number: RequiredHeaderNumberRef<P::SourceChain>,
) where
	AccountIdOf<P::TargetChain>:
		From<<AccountKeyPairOf<P::TransactionSignScheme> as sp_core::Pair>::Public>,
	P::TransactionSignScheme: TransactionSignScheme<Chain = P::TargetChain>,
{
	let relay_task_name = on_demand_headers_relay_name::<P::SourceChain, P::TargetChain>();
	let target_transactions_mortality = target_transaction_params.mortality;
	let mut finality_source = SubstrateFinalitySource::<P>::new(
		source_client.clone(),
		Some(required_header_number.clone()),
	);
	let mut finality_target =
		SubstrateFinalityTarget::new(target_client.clone(), target_transaction_params);
	let mut latest_non_mandatory_at_source = Zero::zero();

	let mut restart_relay = true;
	let finality_relay_task = futures::future::Fuse::terminated();
	futures::pin_mut!(finality_relay_task);

	loop {
		select! {
			_ = async_std::task::sleep(P::TargetChain::AVERAGE_BLOCK_INTERVAL).fuse() => {},
			_ = finality_relay_task => {
				// this should never happen in practice given the current code
				restart_relay = true;
			},
		}

		// read best finalized source header number from source
		let best_finalized_source_header_at_source =
			best_finalized_source_header_at_source(&finality_source, &relay_task_name).await;
		if matches!(best_finalized_source_header_at_source, Err(ref e) if e.is_connection_error()) {
			relay_utils::relay_loop::reconnect_failed_client(
				FailedClient::Source,
				relay_utils::relay_loop::RECONNECT_DELAY,
				&mut finality_source,
				&mut finality_target,
			)
			.await;
			continue
		}

		// read best finalized source header number from target
		let best_finalized_source_header_at_target =
			best_finalized_source_header_at_target::<P>(&finality_target, &relay_task_name).await;
		if matches!(best_finalized_source_header_at_target, Err(ref e) if e.is_connection_error()) {
			relay_utils::relay_loop::reconnect_failed_client(
				FailedClient::Target,
				relay_utils::relay_loop::RECONNECT_DELAY,
				&mut finality_source,
				&mut finality_target,
			)
			.await;
			continue
		}

		// submit mandatory header if some headers are missing
		let best_finalized_source_header_at_source_fmt =
			format!("{:?}", best_finalized_source_header_at_source);
		let best_finalized_source_header_at_target_fmt =
			format!("{:?}", best_finalized_source_header_at_target);
		let required_header_number_value = *required_header_number.lock().await;
		let mandatory_scan_range = mandatory_headers_scan_range::<P::SourceChain>(
			best_finalized_source_header_at_source.ok(),
			best_finalized_source_header_at_target.ok(),
			required_header_number_value,
		)
		.await;

		log::trace!(
			target: "bridge",
			"Mandatory headers scan range in {}: ({:?}, {:?}, {:?}) -> {:?}",
			relay_task_name,
			required_header_number_value,
			best_finalized_source_header_at_source_fmt,
			best_finalized_source_header_at_target_fmt,
			mandatory_scan_range,
		);

		if let Some(mandatory_scan_range) = mandatory_scan_range {
			let relay_mandatory_header_result = relay_mandatory_header_from_range(
				&finality_source,
				&required_header_number,
				best_finalized_source_header_at_target_fmt,
				(
					std::cmp::max(mandatory_scan_range.0, latest_non_mandatory_at_source),
					mandatory_scan_range.1,
				),
				&relay_task_name,
			)
			.await;
			match relay_mandatory_header_result {
				Ok(true) => (),
				Ok(false) => {
					// there are no (or we don't need to relay them) mandatory headers in the range
					// => to avoid scanning the same headers over and over again, remember that
					latest_non_mandatory_at_source = mandatory_scan_range.1;

					log::trace!(
						target: "bridge",
						"No mandatory {} headers in the range {:?} of {} relay",
						P::SourceChain::NAME,
						mandatory_scan_range,
						relay_task_name,
					);
				},
				Err(e) => {
					log::warn!(
						target: "bridge",
						"Failed to scan mandatory {} headers range in {} relay (range: {:?}): {:?}",
						P::SourceChain::NAME,
						relay_task_name,
						mandatory_scan_range,
						e,
					);

					if e.is_connection_error() {
						relay_utils::relay_loop::reconnect_failed_client(
							FailedClient::Source,
							relay_utils::relay_loop::RECONNECT_DELAY,
							&mut finality_source,
							&mut finality_target,
						)
						.await;
						continue
					}
				},
			}
		}

		// start/restart relay
		if restart_relay {
			let stall_timeout = relay_substrate_client::transaction_stall_timeout(
				target_transactions_mortality,
				P::TargetChain::AVERAGE_BLOCK_INTERVAL,
				STALL_TIMEOUT,
			);

			log::info!(
				target: "bridge",
				"Starting {} relay\n\t\
					Only mandatory headers: {}\n\t\
					Tx mortality: {:?} (~{}m)\n\t\
					Stall timeout: {:?}",
				relay_task_name,
				only_mandatory_headers,
				target_transactions_mortality,
				stall_timeout.as_secs_f64() / 60.0f64,
				stall_timeout,
			);

			finality_relay_task.set(
				finality_relay::run(
					finality_source.clone(),
					finality_target.clone(),
					FinalitySyncParams {
						tick: std::cmp::max(
							P::SourceChain::AVERAGE_BLOCK_INTERVAL,
							P::TargetChain::AVERAGE_BLOCK_INTERVAL,
						),
						recent_finality_proofs_limit: RECENT_FINALITY_PROOFS_LIMIT,
						stall_timeout,
						only_mandatory_headers,
					},
					MetricsParams::disabled(),
					futures::future::pending(),
				)
				.fuse(),
			);

			restart_relay = false;
		}
	}
}

/// Returns `Some()` with inclusive range of headers which must be scanned for mandatory headers
/// and the first of such headers must be submitted to the target node.
async fn mandatory_headers_scan_range<C: Chain>(
	best_finalized_source_header_at_source: Option<C::BlockNumber>,
	best_finalized_source_header_at_target: Option<C::BlockNumber>,
	required_header_number: BlockNumberOf<C>,
) -> Option<(C::BlockNumber, C::BlockNumber)> {
	// if we have been unable to read header number from the target, then let's assume
	// that it is the same as required header number. Otherwise we risk submitting
	// unneeded transactions
	let best_finalized_source_header_at_target =
		best_finalized_source_header_at_target.unwrap_or(required_header_number);

	// if we have been unable to read header number from the source, then let's assume
	// that it is the same as at the target
	let best_finalized_source_header_at_source =
		best_finalized_source_header_at_source.unwrap_or(best_finalized_source_header_at_target);

	// if relay is already asked to sync more headers than we have at source, don't do anything yet
	if required_header_number >= best_finalized_source_header_at_source {
		return None
	}

	Some((
		best_finalized_source_header_at_target + One::one(),
		best_finalized_source_header_at_source,
	))
}

/// Try to find mandatory header in the inclusive headers range and, if one is found, ask to relay
/// it.
///
/// Returns `true` if header was found and (asked to be) relayed and `false` otherwise.
async fn relay_mandatory_header_from_range<P: SubstrateFinalitySyncPipeline>(
	finality_source: &SubstrateFinalitySource<P>,
	required_header_number: &RequiredHeaderNumberRef<P::SourceChain>,
	best_finalized_source_header_at_target: String,
	range: (BlockNumberOf<P::SourceChain>, BlockNumberOf<P::SourceChain>),
	relay_task_name: &str,
) -> Result<bool, relay_substrate_client::Error> {
	// search for mandatory header first
	let mandatory_source_header_number =
		find_mandatory_header_in_range(finality_source, range).await?;

	// if there are no mandatory headers - we have nothing to do
	let mandatory_source_header_number = match mandatory_source_header_number {
		Some(mandatory_source_header_number) => mandatory_source_header_number,
		None => return Ok(false),
	};

	// `find_mandatory_header` call may take a while => check if `required_header_number` is still
	// less than our `mandatory_source_header_number` before logging anything
	let mut required_header_number = required_header_number.lock().await;
	if *required_header_number >= mandatory_source_header_number {
		return Ok(false)
	}

	log::trace!(
		target: "bridge",
		"Too many {} headers missing at target in {} relay ({} vs {}). Going to sync up to the mandatory {}",
		P::SourceChain::NAME,
		relay_task_name,
		best_finalized_source_header_at_target,
		range.1,
		mandatory_source_header_number,
	);

	*required_header_number = mandatory_source_header_number;
	Ok(true)
}

/// Read best finalized source block number from source client.
///
/// Returns `None` if we have failed to read the number.
async fn best_finalized_source_header_at_source<P: SubstrateFinalitySyncPipeline>(
	finality_source: &SubstrateFinalitySource<P>,
	relay_task_name: &str,
) -> Result<BlockNumberOf<P::SourceChain>, relay_substrate_client::Error> {
	finality_source.on_chain_best_finalized_block_number().await.map_err(|error| {
		log::error!(
			target: "bridge",
			"Failed to read best finalized source header from source in {} relay: {:?}",
			relay_task_name,
			error,
		);

		error
	})
}

/// Read best finalized source block number from target client.
///
/// Returns `None` if we have failed to read the number.
async fn best_finalized_source_header_at_target<P: SubstrateFinalitySyncPipeline>(
	finality_target: &SubstrateFinalityTarget<P>,
	relay_task_name: &str,
) -> Result<BlockNumberOf<P::SourceChain>, <SubstrateFinalityTarget<P> as RelayClient>::Error>
where
	AccountIdOf<P::TargetChain>:
		From<<AccountKeyPairOf<P::TransactionSignScheme> as sp_core::Pair>::Public>,
	P::TransactionSignScheme: TransactionSignScheme<Chain = P::TargetChain>,
{
	finality_target
		.best_finalized_source_block_id()
		.await
		.map_err(|error| {
			log::error!(
				target: "bridge",
				"Failed to read best finalized source header from target in {} relay: {:?}",
				relay_task_name,
				error,
			);

			error
		})
		.map(|id| id.0)
}

/// Read first mandatory header in given inclusive range.
///
/// Returns `Ok(None)` if there were no mandatory headers in the range.
async fn find_mandatory_header_in_range<P: SubstrateFinalitySyncPipeline>(
	finality_source: &SubstrateFinalitySource<P>,
	range: (BlockNumberOf<P::SourceChain>, BlockNumberOf<P::SourceChain>),
) -> Result<Option<BlockNumberOf<P::SourceChain>>, relay_substrate_client::Error> {
	let mut current = range.0;
	while current <= range.1 {
		let header: SyncHeader<HeaderOf<P::SourceChain>> =
			finality_source.client().header_by_number(current).await?.into();
		if header.is_mandatory() {
			return Ok(Some(current))
		}

		current += One::one();
	}

	Ok(None)
}

/// On-demand headers relay task name.
fn on_demand_headers_relay_name<SourceChain: Chain, TargetChain: Chain>() -> String {
	format!("on-demand-{}-to-{}", SourceChain::NAME, TargetChain::NAME)
}

#[cfg(test)]
mod tests {
	use super::*;

	type TestChain = relay_rococo_client::Rococo;

	const AT_SOURCE: Option<bp_rococo::BlockNumber> = Some(10);
	const AT_TARGET: Option<bp_rococo::BlockNumber> = Some(1);

	#[async_std::test]
	async fn mandatory_headers_scan_range_selects_range_if_some_headers_are_missing() {
		assert_eq!(
			mandatory_headers_scan_range::<TestChain>(AT_SOURCE, AT_TARGET, 0,).await,
			Some((AT_TARGET.unwrap() + 1, AT_SOURCE.unwrap())),
		);
	}

	#[async_std::test]
	async fn mandatory_headers_scan_range_selects_nothing_if_already_queued() {
		assert_eq!(
			mandatory_headers_scan_range::<TestChain>(AT_SOURCE, AT_TARGET, AT_SOURCE.unwrap(),)
				.await,
			None,
		);
	}
}
