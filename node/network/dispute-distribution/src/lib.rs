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
	messages::DisputeDistributionMessage, FromOverseer, OverseerSignal, SpawnedSubsystem,
	Subsystem, SubsystemContext, SubsystemError,
};

mod sender;

/// Error and [`Result`] type for this subsystem.
mod error;
use error::Fatal;
use error::{Result, log_error};

use polkadot_node_subsystem_util::runtime::RuntimeInfo;

mod metrics;
/// Prometheus `Metrics` for dispute distribution.
pub use metrics::Metrics;

#[cfg(test)]
mod tests;

const LOG_TARGET: &'static str = "parachain::dispute-distribution";

/// The dispute distribution subsystem.
pub struct DisputeDistributionSubsystem {
	/// Easy and efficient runtime access for this subsystem.
	runtime: RuntimeInfo,
	/// All ongoing dispute sendings this subsystem is aware of.
	disputes_sending: HashMap<CandidateHash, DisputeInfo>,

	/// Sender to be cloned for `SendTask`s.
	tx: mpsc::Sender<FromSendingTask>,

	/// Receive messages from `SendTask`.
	rx: mpsc::Receiver<FromSendingTask>,

	/// Prometheus metrics.
	metrics: Metrics,
}

/// Dispute state for a particular disputed candidate.
struct DisputeInfo {
	/// The request we are supposed to get out to all parachain validators of the dispute's session
	/// and to all current authorities.
	request: DisputeRequest,
	/// The set of authorities we need to send our messages to. This set will change at session
	/// boundaries. It will always be at least the parachain validators of the session where the
	/// dispute happened and the authorities of the current sessions as determined by active heads.
	deliveries: HashMap<AuthorityDiscoveryId, DeliveryStatus>,
}

/// Status of a particular vote/statement delivery to a particular validator.
enum DeliveryStatus {
	/// Request is still in flight.
	Pending(RemoteHandle<()>),
	/// Request failed - waiting for retry.
	Failed,
	/// Succeeded - no need to send request to this peer anymore.
	Succeeded,
}

impl<Context> Subsystem<Context> for DisputeDistributionSubsystem
where
	Context: SubsystemContext<Message = DisputeDistributionMessage> + Sync + Send,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self
			.run(ctx)
			.map_err(|e| SubsystemError::with_origin("dispute-distribution", e))
			.boxed();

		SpawnedSubsystem {
			name: "dispute-distribution-subsystem",
			future,
		}
	}
}

impl DisputeDistributionSubsystem {

	/// Create a new instance of the availability distribution.
	pub fn new(keystore: SyncCryptoStorePtr, metrics: Metrics) -> Self {
		let runtime = RuntimeInfo::new(Some(keystore));
		Self { runtime,  metrics }
	}

	/// Start processing work as passed on from the Overseer.
	async fn run<Context>(mut self, mut ctx: Context) -> std::result::Result<(), Fatal>
	where
		Context: SubsystemContext<Message = DisputeDistributionMessage> + Sync + Send,
	{
		for msg in ctx.recv().await {
			match msg {
				FromOverSeer::Communication(msg) =>
					self.handle_subsystem_message(&mut self, &mut ctx, msg).await,
			}
		}

	}

	/// Handle `DisputeDistributionMessage`s.
	async fn handle_subsystem_message<Context: SubsystemContext> (
		mut self,
		ctx: &mut Context,
		msg: DisputeDistributionMessage
	) -> std::result::Result<(), Fatal>
	{
		match msg {
			DistputeDistributionMessage::SendDispute(dispute_msg) =>
				self.start_send_dispute(&mut ctx, dispute_msg).await,
		}
	}


	/// Initiates sending a dispute message to peers.
	async fn start_send_dispute<Context>(&mut self, ctx: &mut Context) {
	}
}
