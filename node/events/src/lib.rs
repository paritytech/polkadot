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

//! Implements the Node side events Subsystem

#![deny(unused_crate_dependencies, unused_results)]
#![warn(missing_docs)]

use futures::prelude::*;

use polkadot_node_subsystem::{
	messages::NodeEventsMessage, overseer, FromOrchestra, OverseerSignal, SpawnedSubsystem,
	SubsystemError, SubsystemResult,
};

use sc_utils::notification::{NotificationSender, NotificationStream, TracingKeyStr};
use serde::{Deserialize, Serialize};

pub mod rpc;

/// Supported node events.
#[derive(Clone, Serialize, Deserialize)]
pub enum NodeEvents {
	/// A dummy event for testing.
	Dummy(String),
}

/// Provides tracing key for GRANDPA justifications stream.
#[derive(Clone)]
pub struct NodeEventsTracingKey;
impl TracingKeyStr for NodeEventsTracingKey {
	const TRACING_KEY: &'static str = "mpsc_node_events_notification_stream";
}

/// Used to send notifications about node events.
pub type NodeEventsSender = NotificationSender<NodeEvents>;

/// The receiving half of the node events channel.
///
/// Used to receive notifications about events generated during parachain consensus.
pub type NodeEventsStream = NotificationStream<NodeEvents, NodeEventsTracingKey>;

const LOG_TARGET: &str = "parachain::node-events";

/// The node events Subsystem implementation.
pub struct NodeEventsSubsystem {
	event_sender: NodeEventsSender,
}

impl NodeEventsSubsystem {
	/// Create a new Chain API subsystem with the given client.
	pub fn new(event_sender: NodeEventsSender) -> Self {
		NodeEventsSubsystem { event_sender }
	}
}

#[overseer::subsystem(NodeEvents, error = SubsystemError, prefix = self::overseer)]
impl<Context> NodeEventsSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = run::<Context>(ctx, self)
			.map_err(|e| SubsystemError::with_origin("node-events", e))
			.boxed();
		SpawnedSubsystem { future, name: "node-events-subsystem" }
	}
}

#[overseer::contextbounds(NodeEvents, prefix = self::overseer)]
async fn run<Context>(mut ctx: Context, subsystem: NodeEventsSubsystem) -> SubsystemResult<()> {
	loop {
		match ctx.recv().await? {
			FromOrchestra::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOrchestra::Signal(OverseerSignal::ActiveLeaves(_)) => {},
			FromOrchestra::Signal(OverseerSignal::BlockFinalized(..)) => {},
			FromOrchestra::Communication { msg } => match msg {
				NodeEventsMessage::Dummy(text) => {
					gum::info!(target: LOG_TARGET, ?text, "it works");
					subsystem
						.event_sender
						.notify(|| Ok::<NodeEvents, ()>(NodeEvents::Dummy(text)))
						.unwrap();
				},
			},
		}
	}
}
