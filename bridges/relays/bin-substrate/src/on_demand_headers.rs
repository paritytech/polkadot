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

use crate::finality_pipeline::{SubstrateFinalitySyncPipeline, SubstrateFinalityToSubstrate};
use crate::finality_target::SubstrateFinalityTarget;

use bp_header_chain::justification::GrandpaJustification;
use finality_relay::{
	FinalitySyncPipeline, SourceClient as FinalitySourceClient, TargetClient as FinalityTargetClient,
};
use futures::{
	channel::{mpsc, oneshot},
	select, FutureExt, StreamExt,
};
use num_traits::{CheckedSub, Zero};
use relay_substrate_client::{
	finality_source::FinalitySource as SubstrateFinalitySource, BlockNumberOf, Chain, Client, HashOf, HeaderIdOf,
	SyncHeader,
};
use relay_utils::{
	metrics::MetricsParams, relay_loop::Client as RelayClient, BlockNumberBase, FailedClient, HeaderId,
	MaybeConnectionError,
};
use std::fmt::Debug;

/// On-demand Substrate <-> Substrate headers relay.
///
/// This relay may be started by messages whenever some other relay (e.g. messages relay) needs more
/// headers to be relayed to continue its regular work. When enough headers are relayed, on-demand
/// relay may be deactivated.
#[derive(Clone)]
pub struct OnDemandHeadersRelay<SourceChain: Chain> {
	/// Background task name.
	background_task_name: String,
	/// Required headers to background sender.
	required_header_tx: mpsc::Sender<HeaderId<SourceChain::Hash, SourceChain::BlockNumber>>,
}

impl<SourceChain: Chain> OnDemandHeadersRelay<SourceChain> {
	/// Create new on-demand headers relay.
	pub fn new<TargetChain: Chain, TargetSign>(
		source_client: Client<SourceChain>,
		target_client: Client<TargetChain>,
		pipeline: SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>,
		maximal_headers_difference: SourceChain::BlockNumber,
	) -> Self
	where
		SourceChain: Chain + Debug,
		SourceChain::BlockNumber: BlockNumberBase,
		TargetChain: Chain + Debug,
		TargetChain::BlockNumber: BlockNumberBase,
		TargetSign: Clone + Send + Sync + 'static,
		SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>: SubstrateFinalitySyncPipeline<
			Hash = HashOf<SourceChain>,
			Number = BlockNumberOf<SourceChain>,
			Header = SyncHeader<SourceChain::Header>,
			FinalityProof = GrandpaJustification<SourceChain::Header>,
			TargetChain = TargetChain,
		>,
		SubstrateFinalityTarget<TargetChain, SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>>:
			FinalityTargetClient<SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>>,
	{
		let (required_header_tx, required_header_rx) = mpsc::channel(1);
		async_std::task::spawn(async move {
			background_task(
				source_client,
				target_client,
				pipeline,
				maximal_headers_difference,
				required_header_rx,
			)
			.await;
		});

		let background_task_name = format!(
			"{}-background",
			on_demand_headers_relay_name::<SourceChain, TargetChain>()
		);
		OnDemandHeadersRelay {
			background_task_name,
			required_header_tx,
		}
	}

	/// Someone is asking us to relay given finalized header.
	pub fn require_finalized_header(&self, header_id: HeaderIdOf<SourceChain>) {
		if let Err(error) = self.required_header_tx.clone().try_send(header_id) {
			log::error!(
				target: "bridge",
				"Failed to send require header id {:?} to {:?}: {:?}",
				header_id,
				self.background_task_name,
				error,
			);
		}
	}
}

/// Background task that is responsible for starting and stopping headers relay when required.
async fn background_task<SourceChain, TargetChain, TargetSign>(
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	pipeline: SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>,
	maximal_headers_difference: SourceChain::BlockNumber,
	mut required_header_rx: mpsc::Receiver<HeaderIdOf<SourceChain>>,
) where
	SourceChain: Chain + Debug,
	SourceChain::BlockNumber: BlockNumberBase,
	TargetChain: Chain + Debug,
	TargetChain::BlockNumber: BlockNumberBase,
	TargetSign: Clone + Send + Sync + 'static,
	SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>: SubstrateFinalitySyncPipeline<
		Hash = HashOf<SourceChain>,
		Number = BlockNumberOf<SourceChain>,
		Header = SyncHeader<SourceChain::Header>,
		FinalityProof = GrandpaJustification<SourceChain::Header>,
		TargetChain = TargetChain,
	>,
	SubstrateFinalityTarget<TargetChain, SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>>:
		FinalityTargetClient<SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>>,
{
	let relay_task_name = on_demand_headers_relay_name::<SourceChain, TargetChain>();
	let mut finality_source = SubstrateFinalitySource::<
		_,
		SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>,
	>::new(source_client.clone());
	let mut finality_target = SubstrateFinalityTarget::new(target_client.clone(), pipeline.clone());

	let mut active_headers_relay = None;
	let mut required_header_number = Zero::zero();
	let mut relay_exited_rx = futures::future::pending().left_future();

	loop {
		// wait for next target block or for new required header
		select! {
			_ = async_std::task::sleep(TargetChain::AVERAGE_BLOCK_INTERVAL).fuse() => {},
			required_header_id = required_header_rx.next() => {
				match required_header_id {
					Some(required_header_id) => {
						if required_header_id.0 > required_header_number {
							required_header_number = required_header_id.0;
						}
					},
					None => {
						// that's the only way to exit background task - to drop `required_header_tx`
						break
					},
				}
			},
			_ = relay_exited_rx => {
				// there could be a situation when we're receiving exit signals after we
				// have already stopped relay or when we have already started new relay.
				// but it isn't critical, because even if we'll accidentally stop new relay
				// we'll restart it almost immediately
				stop_on_demand_headers_relay(active_headers_relay.take()).await;
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
			continue;
		}

		// read best finalized source header number from target
		let best_finalized_source_header_at_target =
			best_finalized_source_header_at_target::<SourceChain, _, _>(&finality_target, &relay_task_name).await;
		if matches!(best_finalized_source_header_at_target, Err(ref e) if e.is_connection_error()) {
			relay_utils::relay_loop::reconnect_failed_client(
				FailedClient::Target,
				relay_utils::relay_loop::RECONNECT_DELAY,
				&mut finality_source,
				&mut finality_target,
			)
			.await;
			continue;
		}

		// start or stop headers relay if required
		let action = select_on_demand_relay_action::<SourceChain>(
			best_finalized_source_header_at_source.ok(),
			best_finalized_source_header_at_target.ok(),
			required_header_number,
			maximal_headers_difference,
			&relay_task_name,
			active_headers_relay.is_some(),
		);
		match action {
			OnDemandRelayAction::Start => {
				let (relay_exited_tx, new_relay_exited_rx) = oneshot::channel();
				active_headers_relay = start_on_demand_headers_relay(
					relay_task_name.clone(),
					relay_exited_tx,
					source_client.clone(),
					target_client.clone(),
					pipeline.clone(),
				);
				if active_headers_relay.is_some() {
					relay_exited_rx = new_relay_exited_rx.right_future();
				}
			}
			OnDemandRelayAction::Stop => {
				stop_on_demand_headers_relay(active_headers_relay.take()).await;
			}
			OnDemandRelayAction::None => (),
		}
	}
}

/// Read best finalized source block number from source client.
///
/// Returns `None` if we have failed to read the number.
async fn best_finalized_source_header_at_source<SourceChain: Chain, P>(
	finality_source: &SubstrateFinalitySource<SourceChain, P>,
	relay_task_name: &str,
) -> Result<SourceChain::BlockNumber, <SubstrateFinalitySource<SourceChain, P> as RelayClient>::Error>
where
	SubstrateFinalitySource<SourceChain, P>: FinalitySourceClient<P>,
	P: FinalitySyncPipeline<Number = SourceChain::BlockNumber>,
{
	finality_source.best_finalized_block_number().await.map_err(|error| {
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
async fn best_finalized_source_header_at_target<SourceChain: Chain, TargetChain: Chain, P>(
	finality_target: &SubstrateFinalityTarget<TargetChain, P>,
	relay_task_name: &str,
) -> Result<SourceChain::BlockNumber, <SubstrateFinalityTarget<TargetChain, P> as RelayClient>::Error>
where
	SubstrateFinalityTarget<TargetChain, P>: FinalityTargetClient<P>,
	P: FinalitySyncPipeline<Number = SourceChain::BlockNumber>,
{
	finality_target
		.best_finalized_source_block_number()
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
}

/// What to do with the on-demand relay task?
#[derive(Debug, PartialEq)]
enum OnDemandRelayAction {
	Start,
	Stop,
	None,
}

fn select_on_demand_relay_action<C: Chain>(
	best_finalized_source_header_at_source: Option<C::BlockNumber>,
	best_finalized_source_header_at_target: Option<C::BlockNumber>,
	mut required_source_header_at_target: C::BlockNumber,
	maximal_headers_difference: C::BlockNumber,
	relay_task_name: &str,
	is_active: bool,
) -> OnDemandRelayAction {
	// if we have been unable to read header number from the target, then let's assume
	// that it is the same as required header number. Otherwise we risk submitting
	// unneeded transactions
	let best_finalized_source_header_at_target =
		best_finalized_source_header_at_target.unwrap_or(required_source_header_at_target);

	// if we have been unable to read header number from the source, then let's assume
	// that it is the same as at the target
	let best_finalized_source_header_at_source =
		best_finalized_source_header_at_source.unwrap_or(best_finalized_source_header_at_target);

	// if there are too many source headers missing from the target node, require some
	// new headers at target
	//
	// why do we need that? When complex headers+messages relay is used, it'll normally only relay
	// headers when there are undelivered messages/confirmations. But security model of the
	// `pallet-bridge-grandpa` module relies on the fact that headers are synced in real-time and
	// that it'll see authorities-change header before unbonding period will end for previous
	// authorities set.
	let current_headers_difference = best_finalized_source_header_at_source
		.checked_sub(&best_finalized_source_header_at_target)
		.unwrap_or_else(Zero::zero);
	if current_headers_difference > maximal_headers_difference {
		required_source_header_at_target = best_finalized_source_header_at_source;

		// don't log if relay is already running
		if !is_active {
			log::trace!(
				target: "bridge",
				"Too many {} headers missing at target in {} relay ({} vs {}). Going to sync up to the {}",
				C::NAME,
				relay_task_name,
				best_finalized_source_header_at_source,
				best_finalized_source_header_at_target,
				best_finalized_source_header_at_source,
			);
		}
	}

	// now let's select what to do with relay
	let needs_to_be_active = required_source_header_at_target > best_finalized_source_header_at_target;
	match (needs_to_be_active, is_active) {
		(true, false) => OnDemandRelayAction::Start,
		(false, true) => OnDemandRelayAction::Stop,
		_ => OnDemandRelayAction::None,
	}
}

/// On-demand headers relay task name.
fn on_demand_headers_relay_name<SourceChain: Chain, TargetChain: Chain>() -> String {
	format!("on-demand-{}-to-{}", SourceChain::NAME, TargetChain::NAME)
}

/// Start on-demand headers relay task.
fn start_on_demand_headers_relay<SourceChain: Chain, TargetChain: Chain, TargetSign>(
	task_name: String,
	relay_exited_tx: oneshot::Sender<()>,
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	pipeline: SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>,
) -> Option<async_std::task::JoinHandle<()>>
where
	SourceChain::BlockNumber: BlockNumberBase,
	SubstrateFinalityToSubstrate<SourceChain, TargetChain, TargetSign>: SubstrateFinalitySyncPipeline<
		Hash = HashOf<SourceChain>,
		Number = BlockNumberOf<SourceChain>,
		Header = SyncHeader<SourceChain::Header>,
		FinalityProof = GrandpaJustification<SourceChain::Header>,
		TargetChain = TargetChain,
	>,
	TargetSign: 'static,
{
	let headers_relay_future =
		crate::finality_pipeline::run(pipeline, source_client, target_client, true, MetricsParams::disabled());
	let closure_task_name = task_name.clone();
	async_std::task::Builder::new()
		.name(task_name.clone())
		.spawn(async move {
			log::info!(target: "bridge", "Starting {} headers relay", closure_task_name);
			let result = headers_relay_future.await;
			log::trace!(target: "bridge", "{} headers relay has exited. Result: {:?}", closure_task_name, result);
			let _ = relay_exited_tx.send(());
		})
		.map_err(|error| {
			log::error!(
				target: "bridge",
				"Failed to start {} relay: {:?}",
				task_name,
				error,
			);
		})
		.ok()
}

/// Stop on-demand headers relay task.
async fn stop_on_demand_headers_relay(task: Option<async_std::task::JoinHandle<()>>) {
	if let Some(task) = task {
		let task_name = task
			.task()
			.name()
			.expect("on-demand tasks are always started with name; qed")
			.to_string();
		log::trace!(target: "bridge", "Cancelling {} headers relay", task_name);
		task.cancel().await;
		log::info!(target: "bridge", "Cancelled {} headers relay", task_name);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	type TestChain = relay_millau_client::Millau;

	const AT_SOURCE: Option<bp_millau::BlockNumber> = Some(10);
	const AT_TARGET: Option<bp_millau::BlockNumber> = Some(1);

	#[test]
	fn starts_relay_when_headers_are_required() {
		assert_eq!(
			select_on_demand_relay_action::<TestChain>(AT_SOURCE, AT_TARGET, 5, 100, "test", false),
			OnDemandRelayAction::Start,
		);

		assert_eq!(
			select_on_demand_relay_action::<TestChain>(AT_SOURCE, AT_TARGET, 5, 100, "test", true),
			OnDemandRelayAction::None,
		);
	}

	#[test]
	fn starts_relay_when_too_many_headers_missing() {
		assert_eq!(
			select_on_demand_relay_action::<TestChain>(AT_SOURCE, AT_TARGET, 0, 5, "test", false),
			OnDemandRelayAction::Start,
		);

		assert_eq!(
			select_on_demand_relay_action::<TestChain>(AT_SOURCE, AT_TARGET, 0, 5, "test", true),
			OnDemandRelayAction::None,
		);
	}

	#[test]
	fn stops_relay_if_required_header_is_synced() {
		assert_eq!(
			select_on_demand_relay_action::<TestChain>(AT_SOURCE, AT_TARGET, AT_TARGET.unwrap(), 100, "test", true),
			OnDemandRelayAction::Stop,
		);

		assert_eq!(
			select_on_demand_relay_action::<TestChain>(AT_SOURCE, AT_TARGET, AT_TARGET.unwrap(), 100, "test", false),
			OnDemandRelayAction::None,
		);
	}
}
