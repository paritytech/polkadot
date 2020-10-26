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

#![deny(missing_docs)]

use std::time::Duration;
use futures::{channel::oneshot, FutureExt};
use log::trace;

use polkadot_subsystem::{
	Subsystem, SubsystemContext, SubsystemError, SpawnedSubsystem,
	errors::RuntimeApiError,
	messages::{
		AllMessages, CollatorProtocolMessage, NetworkBridgeMessage,
	},
};
use polkadot_node_network_protocol::{
	PeerId, ReputationChange as Rep,
};
use polkadot_primitives::v1::CollatorId;
use polkadot_node_subsystem_util::{
	self as util,
	metrics::{self, prometheus},
};

mod collator_side;
mod validator_side;

const TARGET: &'static str = "colp";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	Subsystem(SubsystemError),
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	RuntimeApi(RuntimeApiError),
	#[from]
	UtilError(util::Error),
	#[from]
	Prometheus(prometheus::PrometheusError),
}

impl From<util::validator_discovery::Error> for Error {
	fn from(me: util::validator_discovery::Error) -> Self {
		match me {
			util::validator_discovery::Error::Subsystem(s) => Error::Subsystem(s),
			util::validator_discovery::Error::RuntimeApi(ra) => Error::RuntimeApi(ra),
			util::validator_discovery::Error::Oneshot(c) => Error::Oneshot(c),
		}
	}
}

type Result<T> = std::result::Result<T, Error>;

enum ProtocolSide {
	Validator(validator_side::Metrics),
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
	pub fn new(id: Option<CollatorId>, registry: Option<&prometheus::Registry>) -> Self {
		use metrics::Metrics;
		let protocol_side = match id {
			Some(id) => ProtocolSide::Collator(id, collator_side::Metrics::register(registry)),
			None => ProtocolSide::Validator(validator_side::Metrics::register(registry)),
		};

		Self {
			protocol_side,
		}
	}

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
		}
	}
}

impl<Context> Subsystem<Context> for CollatorProtocolSubsystem
where
	Context: SubsystemContext<Message = CollatorProtocolMessage> + Sync + Send,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem {
			name: "collator-protocol-subsystem",
			future: Box::pin(async move { self.run(ctx) }.map(|_| ())),
		}
	}
}

/// Modify the reputation of a peer based on its behavior.
async fn modify_reputation<Context>(ctx: &mut Context, peer: PeerId, rep: Rep) -> Result<()>
where
	Context: SubsystemContext<Message = CollatorProtocolMessage>,
{
	trace!(
		target: TARGET,
		"Reputation change of {:?} for peer {:?}", rep, peer,
	);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep),
	)).await?;

	Ok(())
}
