// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! The Collator Protocol allows collators and validators talk to each other.
//! This subsystem implements both sides of the collator protocol.

#![deny(missing_docs, unused_crate_dependencies)]
#![recursion_limit="256"]

use std::time::Duration;
use futures::{channel::oneshot, FutureExt, TryFutureExt};
use thiserror::Error;

use polkadot_subsystem::{
	Subsystem, SubsystemContext, SubsystemError, SpawnedSubsystem,
	errors::RuntimeApiError,
	messages::{
		AllMessages, CollatorProtocolMessage, NetworkBridgeMessage,
	},
};
use polkadot_node_network_protocol::{
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_primitives::v1::CollatorId;
use polkadot_node_subsystem_util::{
	self as util,
	metrics::prometheus,
};

mod collator_side;
mod validator_side;

const LOG_TARGET: &'static str = "collator_protocol";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
enum Error {
	#[error(transparent)]
	Subsystem(#[from] SubsystemError),
	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),
	#[error(transparent)]
	UtilError(#[from] util::Error),
	#[error(transparent)]
	Prometheus(#[from] prometheus::PrometheusError),
}

type Result<T> = std::result::Result<T, Error>;

/// What side of the collator protocol is being engaged
pub enum ProtocolSide {
	/// Validators operate on the relay chain.
	Validator(validator_side::Metrics),
	/// Collators operate on a parachain.
	Collator(CollatorId, collator_side::Metrics),
}

/// The collator protocol subsystem.
pub struct CollatorProtocolSubsystem {
	protocol_side: ProtocolSide,
}

impl CollatorProtocolSubsystem {
	/// Start the collator protocol.
	/// If `id` is `Some` this is a collator side of the protocol.
	/// If `id` is `None` this is a validator side of the protocol.
	/// Caller must provide a registry for prometheus metrics.
	pub fn new(protocol_side: ProtocolSide) -> Self {
		Self {
			protocol_side,
		}
	}

	#[tracing::instrument(skip(self, ctx), fields(subsystem = LOG_TARGET))]
	async fn run<Context>(self, ctx: Context) -> Result<()>
	where
		Context: SubsystemContext<Message = CollatorProtocolMessage>,
	{
		match self.protocol_side {
			ProtocolSide::Validator(metrics) => validator_side::run(
				ctx,
				REQUEST_TIMEOUT,
				metrics,
			).await,
			ProtocolSide::Collator(id, metrics) => collator_side::run(
				ctx,
				id,
				metrics,
			).await,
		}.map_err(|e| {
			SubsystemError::with_origin("collator-protocol", e).into()
		})
	}
}

impl<Context> Subsystem<Context> for CollatorProtocolSubsystem
where
	Context: SubsystemContext<Message = CollatorProtocolMessage> + Sync + Send,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self
			.run(ctx)
			.map_err(|e| SubsystemError::with_origin("collator-protocol", e))
			.boxed();

		SpawnedSubsystem {
			name: "collator-protocol-subsystem",
			future,
		}
	}
}

/// Modify the reputation of a peer based on its behavior.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn modify_reputation<Context>(ctx: &mut Context, peer: PeerId, rep: Rep)
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>,
{
	tracing::trace!(
		target: LOG_TARGET,
		rep = ?rep,
		peer_id = %peer,
		"reputation change for peer",
	);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep),
	)).await;
}
