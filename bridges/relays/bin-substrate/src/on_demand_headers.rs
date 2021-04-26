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
use finality_relay::TargetClient as FinalityTargetClient;
use futures::{
	channel::{mpsc, oneshot},
	select, FutureExt, StreamExt,
};
use num_traits::Zero;
use relay_substrate_client::{BlockNumberOf, Chain, Client, HashOf, HeaderIdOf, SyncHeader};
use relay_utils::{metrics::MetricsParams, BlockNumberBase, HeaderId};
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
			background_task(source_client, target_client, pipeline, required_header_rx).await;
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
	let finality_target = SubstrateFinalityTarget::new(target_client.clone(), pipeline.clone());

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

		// read best finalized source block from target
		let available_header_number = match finality_target.best_finalized_source_block_number().await {
			Ok(available_header_number) => available_header_number,
			Err(error) => {
				log::error!(
					target: "bridge",
					"Failed to read best finalized {} header from {} in {} relay: {:?}",
					SourceChain::NAME,
					TargetChain::NAME,
					relay_task_name,
					error,
				);

				// we don't know what's happening with target client, so better to stop on-demand relay than
				// submit unneeded transactions
				// => assume that required header is known to the target node
				required_header_number
			}
		};

		// start or stop headers relay if required
		let activate = required_header_number > available_header_number;
		match (activate, active_headers_relay.is_some()) {
			(true, false) => {
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
			(false, true) => {
				stop_on_demand_headers_relay(active_headers_relay.take()).await;
			}
			_ => (),
		}
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
		crate::finality_pipeline::run(pipeline, source_client, target_client, MetricsParams::disabled());
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
