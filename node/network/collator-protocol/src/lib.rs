// Copyright (C) Parity Technologies (UK) Ltd.
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
#![deny(unused_crate_dependencies)]
#![recursion_limit = "256"]

use std::time::{Duration, Instant};

use futures::{
	stream::{FusedStream, StreamExt},
	FutureExt, TryFutureExt,
};

use polkadot_node_subsystem_util::reputation::ReputationAggregator;
use sp_keystore::KeystorePtr;

use polkadot_node_network_protocol::{
	request_response::{v1 as request_v1, vstaging as protocol_vstaging, IncomingRequestReceiver},
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_primitives::CollatorPair;

use polkadot_node_subsystem::{errors::SubsystemError, overseer, DummySubsystem, SpawnedSubsystem};

mod error;

mod collator_side;
mod validator_side;

const LOG_TARGET: &'static str = "parachain::collator-protocol";

/// A collator eviction policy - how fast to evict collators which are inactive.
#[derive(Debug, Clone, Copy)]
pub struct CollatorEvictionPolicy {
	/// How fast to evict collators who are inactive.
	pub inactive_collator: Duration,
	/// How fast to evict peers which don't declare their para.
	pub undeclared: Duration,
}

impl Default for CollatorEvictionPolicy {
	fn default() -> Self {
		CollatorEvictionPolicy {
			inactive_collator: Duration::from_secs(24),
			undeclared: Duration::from_secs(1),
		}
	}
}

/// What side of the collator protocol is being engaged
pub enum ProtocolSide {
	/// Validators operate on the relay chain.
	Validator {
		/// The keystore holding validator keys.
		keystore: KeystorePtr,
		/// An eviction policy for inactive peers or validators.
		eviction_policy: CollatorEvictionPolicy,
		/// Prometheus metrics for validators.
		metrics: validator_side::Metrics,
	},
	/// Collators operate on a parachain.
	Collator {
		/// Local peer id.
		peer_id: PeerId,
		/// Parachain collator pair.
		collator_pair: CollatorPair,
		/// Receiver for v1 collation fetching requests.
		request_receiver_v1: IncomingRequestReceiver<request_v1::CollationFetchingRequest>,
		/// Receiver for vstaging collation fetching requests.
		request_receiver_vstaging:
			IncomingRequestReceiver<protocol_vstaging::CollationFetchingRequest>,
		/// Metrics.
		metrics: collator_side::Metrics,
	},
	/// No protocol side, just disable it.
	None,
}

/// The collator protocol subsystem.
pub struct CollatorProtocolSubsystem {
	protocol_side: ProtocolSide,
}

#[overseer::contextbounds(CollatorProtocol, prefix = self::overseer)]
impl CollatorProtocolSubsystem {
	/// Start the collator protocol.
	/// If `id` is `Some` this is a collator side of the protocol.
	/// If `id` is `None` this is a validator side of the protocol.
	/// Caller must provide a registry for prometheus metrics.
	pub fn new(protocol_side: ProtocolSide) -> Self {
		Self { protocol_side }
	}
}

#[overseer::subsystem(CollatorProtocol, error=SubsystemError, prefix=self::overseer)]
impl<Context> CollatorProtocolSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = match self.protocol_side {
			ProtocolSide::Validator { keystore, eviction_policy, metrics } =>
				validator_side::run(ctx, keystore, eviction_policy, metrics)
					.map_err(|e| SubsystemError::with_origin("collator-protocol", e))
					.boxed(),
			ProtocolSide::Collator {
				peer_id,
				collator_pair,
				request_receiver_v1,
				request_receiver_vstaging,
				metrics,
			} => collator_side::run(
				ctx,
				peer_id,
				collator_pair,
				request_receiver_v1,
				request_receiver_vstaging,
				metrics,
			)
			.map_err(|e| SubsystemError::with_origin("collator-protocol", e))
			.boxed(),
			ProtocolSide::None => return DummySubsystem.start(ctx),
		};

		SpawnedSubsystem { name: "collator-protocol-subsystem", future }
	}
}

/// Modify the reputation of a peer based on its behavior.
async fn modify_reputation(
	reputation: &mut ReputationAggregator,
	sender: &mut impl overseer::CollatorProtocolSenderTrait,
	peer: PeerId,
	rep: Rep,
) {
	gum::trace!(
		target: LOG_TARGET,
		rep = ?rep,
		peer_id = %peer,
		"reputation change for peer",
	);

	reputation.modify(sender, peer, rep).await;
}

/// Wait until tick and return the timestamp for the following one.
async fn wait_until_next_tick(last_poll: Instant, period: Duration) -> Instant {
	let now = Instant::now();
	let next_poll = last_poll + period;

	if next_poll > now {
		futures_timer::Delay::new(next_poll - now).await
	}

	Instant::now()
}

/// Returns an infinite stream that yields with an interval of `period`.
fn tick_stream(period: Duration) -> impl FusedStream<Item = ()> {
	futures::stream::unfold(Instant::now(), move |next_check| async move {
		Some(((), wait_until_next_tick(next_check, period).await))
	})
	.fuse()
}
