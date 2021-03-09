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

use polkadot_subsystem::{
	messages::AvailabilityDistributionMessage, FromOverseer, OverseerSignal, SpawnedSubsystem,
	Subsystem, SubsystemContext, SubsystemError,
};

/// Error and [`Result`] type for this subsystem.
mod error;
pub use error::Error;
use error::Result;

/// `Requester` taking care of requesting chunks for candidates pending availability.
mod requester;
use requester::Requester;

/// Responding to erasure chunk requests:
mod responder;
use responder::answer_request_log;

/// Cache for session information.
mod session_cache;

mod metrics;
/// Prometheus `Metrics` for availability distribution.
pub use metrics::Metrics;

#[cfg(test)]
mod tests;

const LOG_TARGET: &'static str = "availability_distribution";

/// The availability distribution subsystem.
pub struct AvailabilityDistributionSubsystem {
	/// Pointer to a keystore, which is required for determining this nodes validator index.
	keystore: SyncCryptoStorePtr,
	/// Prometheus metrics.
	metrics: Metrics,
}

impl<Context> Subsystem<Context> for AvailabilityDistributionSubsystem
where
	Context: SubsystemContext<Message = AvailabilityDistributionMessage> + Sync + Send,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self
			.run(ctx)
			.map_err(|e| SubsystemError::with_origin("availability-distribution", e))
			.boxed();

		SpawnedSubsystem {
			name: "availability-distribution-subsystem",
			future,
		}
	}
}

impl AvailabilityDistributionSubsystem {
	/// Create a new instance of the availability distribution.
	pub fn new(keystore: SyncCryptoStorePtr, metrics: Metrics) -> Self {
		Self { keystore, metrics }
	}

	/// Start processing work as passed on from the Overseer.
	async fn run<Context>(self, mut ctx: Context) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage> + Sync + Send,
	{
		let mut requester = Requester::new(self.keystore.clone(), self.metrics.clone()).fuse();
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
				Either::Left(subsystem_msg) => {
					subsystem_msg.map_err(|e| Error::IncomingMessageChannel(e))?
				}
				Either::Right(from_task) => {
					let from_task = from_task.ok_or(Error::RequesterExhausted)?;
					ctx.send_message(from_task).await;
					continue;
				}
			};
			match message {
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
					// Update the relay chain heads we are fetching our pieces for:
					requester
						.get_mut()
						.update_fetching_heads(&mut ctx, update)
						.await?;
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {}
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					return Ok(());
				}
				FromOverseer::Communication {
					msg: AvailabilityDistributionMessage::AvailabilityFetchingRequest(req),
				} => {
					answer_request_log(&mut ctx, req, &self.metrics).await
				}
			}
		}
	}
}
