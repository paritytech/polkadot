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

use futures::FutureExt;
use log::trace;

use polkadot_subsystem::{
	Subsystem, SubsystemContext, SubsystemError, SpawnedSubsystem,
	metrics::{self, prometheus},
	messages::{
		AllMessages, CollatorProtocolMessage, NetworkBridgeMessage,
	},
};
use polkadot_node_network_protocol::{
	PeerId, ReputationChange as Rep,
};

mod collator_side;
mod validator_side;

const TARGET: &'static str = "colp";

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	Subsystem(SubsystemError),
}

type Result<T> = std::result::Result<T, Error>;

enum ProtocolSide {
	Validator,
	Collator,
}

pub struct CollatorProtocolSubsystem {
	protocol_side: ProtocolSide, 
}

impl CollatorProtocolSubsystem {
	/// Instantiate the collator protocol side of this subsystem
	pub fn new_collator_side() -> Self {
		Self {
			protocol_side: ProtocolSide::Collator,
		}
	}

	/// Instantiate the validator protocol side of this subsystem.
	pub fn new_validator_side() -> Self {
		Self {
			protocol_side: ProtocolSide::Validator,
		}
	}

	async fn run_collator_side<Context>(self, ctx: Context) -> Result<()>
	where
		Context: SubsystemContext<Message = CollatorProtocolMessage>,
	{
		collator_side::run(ctx).await
	}

	async fn run_validator_side<Context>(self, ctx: Context) -> Result<()>
	where
		Context: SubsystemContext<Message = CollatorProtocolMessage>,
	{
		validator_side::run(ctx).await
	}
}

#[derive(Clone)]
struct ValidatorMetrics;

#[derive(Clone)]
struct CollatorMetrics;

#[derive(Clone)]
enum MetricsInner {
	ValidatorMetrics(ValidatorMetrics),
	CollatorMetrics(CollatorMetrics),
}

/// Collator protocol metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) 
		-> std::result::Result<Self, prometheus::PrometheusError> {
		Ok(Metrics(None))
	}
}

impl<Context> Subsystem<Context> for CollatorProtocolSubsystem
where
	Context: SubsystemContext<Message = CollatorProtocolMessage> + Sync + Send,
{
	type Metrics = Metrics;

	fn start(self, ctx: Context) -> SpawnedSubsystem {
		match self.protocol_side {
			ProtocolSide::Collator => SpawnedSubsystem {
				name: "collator-protocol-subsystem",
				future: Box::pin(async move { self.run_collator_side(ctx) }.map(|_| ())),
			},
			ProtocolSide::Validator => SpawnedSubsystem {
				name: "collator-protocol-subsystem",
				future: Box::pin(async move { self.run_validator_side(ctx) }.map(|_| ())),
			}
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
