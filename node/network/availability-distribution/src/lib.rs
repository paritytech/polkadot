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


use sp_keystore::SyncCryptoStorePtr;

use polkadot_subsystem::{
	jaeger, errors::{ChainApiError, RuntimeApiError}, PerLeafSpan,
	ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemError, messages::AvailabilityDistributionMessage
};

/// Error and [`Result`] type for this subsystem.
mod error;
pub use error::Error;
use error::Result;

/// The actual implementation of running availability distribution.
mod state;
/// State of a running availability-distribution subsystem.
use state::ProtocolState;

/// A task fetching a particular chunk.
mod fetch_task;

/// Cache for session information.
mod session_cache;

const LOG_TARGET: &'static str = "availability_distribution";



/// Availability Distribution metrics.
/// TODO: Dummy for now.
type Metrics = ();

/// The bitfield distribution subsystem.
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
			.run(ctx, ProtocolState::new())
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
	async fn run<Context>(self, mut ctx: Context, state: &mut ProtocolState) -> Result<()>
	where
		Context: SubsystemContext<Message = AvailabilityDistributionMessage> + Sync + Send,
	{
		loop {
			let message = ctx.recv().await?;
			match message {
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
					// Update the relay chain heads we are fetching our pieces for:
					state.update_fetching_heads(&mut ctx, update)?;
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {}
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					return Ok(());
				}
				FromOverseer::Communication {
					msg: AvailabilityDistributionMessage::AvailabilityFetchingRequest(_),
				} => { 
					// TODO: Implement issue 2306:
					tracing::warn!(
						target: LOG_TARGET,
						"To be implemented, see: https://github.com/paritytech/polkadot/issues/2306!",
						);
				}
			}
		}
	}
}

