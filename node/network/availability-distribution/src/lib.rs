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

use futures::{future::Either, FutureExt, StreamExt, TryFutureExt};

use sp_keystore::SyncCryptoStorePtr;

use polkadot_node_network_protocol::request_response::{v1, IncomingRequestReceiver};
use polkadot_node_subsystem::{
	messages::AvailabilityDistributionMessage, overseer, FromOrchestra, OverseerSignal,
	SpawnedSubsystem, SubsystemError,
};

/// Error and [`Result`] type for this subsystem.
mod error;
use error::{log_error, FatalError, Result};

use polkadot_node_subsystem_util::runtime::RuntimeInfo;

/// `Requester` taking care of requesting chunks for candidates pending availability.
mod requester;
use requester::Requester;

/// Handing requests for PoVs during backing.
mod pov_requester;

/// Responding to erasure chunk requests:
mod responder;
use responder::{run_chunk_receiver, run_pov_receiver};

mod metrics;
/// Prometheus `Metrics` for availability distribution.
pub use metrics::Metrics;

#[cfg(test)]
mod tests;

const LOG_TARGET: &'static str = "parachain::availability-distribution";

/// The availability distribution subsystem.
pub struct AvailabilityDistributionSubsystem {
	/// Easy and efficient runtime access for this subsystem.
	runtime: RuntimeInfo,
	/// Receivers to receive messages from.
	recvs: IncomingRequestReceivers,
	/// Prometheus metrics.
	metrics: Metrics,
}

/// Receivers to be passed into availability distribution.
pub struct IncomingRequestReceivers {
	/// Receiver for incoming PoV requests.
	pub pov_req_receiver: IncomingRequestReceiver<v1::PoVFetchingRequest>,
	/// Receiver for incoming availability chunk requests.
	pub chunk_req_receiver: IncomingRequestReceiver<v1::ChunkFetchingRequest>,
}

#[overseer::subsystem(AvailabilityDistribution, error=SubsystemError, prefix=self::overseer)]
impl<Context> AvailabilityDistributionSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self
			.run(ctx)
			.map_err(|e| SubsystemError::with_origin("availability-distribution", e))
			.boxed();

		SpawnedSubsystem { name: "availability-distribution-subsystem", future }
	}
}

#[overseer::contextbounds(AvailabilityDistribution, prefix = self::overseer)]
impl AvailabilityDistributionSubsystem {
	/// Create a new instance of the availability distribution.
	pub fn new(
		keystore: SyncCryptoStorePtr,
		recvs: IncomingRequestReceivers,
		metrics: Metrics,
	) -> Self {
		let runtime = RuntimeInfo::new(Some(keystore));
		Self { runtime, recvs, metrics }
	}

	/// Start processing work as passed on from the Overseer.
	async fn run<Context>(self, mut ctx: Context) -> std::result::Result<(), FatalError> {
		let Self { mut runtime, recvs, metrics } = self;

		let IncomingRequestReceivers { pov_req_receiver, chunk_req_receiver } = recvs;
		let mut requester = Requester::new(metrics.clone()).fuse();

		{
			let sender = ctx.sender().clone();
			ctx.spawn(
				"pov-receiver",
				run_pov_receiver(sender.clone(), pov_req_receiver, metrics.clone()).boxed(),
			)
			.map_err(FatalError::SpawnTask)?;

			ctx.spawn(
				"chunk-receiver",
				run_chunk_receiver(sender, chunk_req_receiver, metrics.clone()).boxed(),
			)
			.map_err(FatalError::SpawnTask)?;
		}

		loop {
			let action = {
				let mut subsystem_next = ctx.recv().fuse();
				futures::select! {
					subsystem_msg = subsystem_next => Either::Left(subsystem_msg),
					from_task = requester.next() => Either::Right(from_task),
				}
			};

			// Handle task messages sending:
			let message = match action {
				Either::Left(subsystem_msg) =>
					subsystem_msg.map_err(|e| FatalError::IncomingMessageChannel(e))?,
				Either::Right(from_task) => {
					let from_task = from_task.ok_or(FatalError::RequesterExhausted)?;
					ctx.send_message(from_task).await;
					continue
				},
			};
			match message {
				FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => {
					log_error(
						requester
							.get_mut()
							.update_fetching_heads(&mut ctx, &mut runtime, update)
							.await,
						"Error in Requester::update_fetching_heads",
					)?;
				},
				FromOrchestra::Signal(OverseerSignal::BlockFinalized(..)) => {},
				FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
				FromOrchestra::Communication {
					msg:
						AvailabilityDistributionMessage::FetchPoV {
							relay_parent,
							from_validator,
							para_id,
							candidate_hash,
							pov_hash,
							tx,
						},
				} => {
					log_error(
						pov_requester::fetch_pov(
							&mut ctx,
							&mut runtime,
							relay_parent,
							from_validator,
							para_id,
							candidate_hash,
							pov_hash,
							tx,
							metrics.clone(),
						)
						.await,
						"pov_requester::fetch_pov",
					)?;
				},
			}
		}
	}
}
