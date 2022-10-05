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
#![deny(unused_crate_dependencies)]
#![recursion_limit = "256"]

use std::time::Duration;

use futures::{FutureExt, TryFutureExt};

use sp_keystore::SyncCryptoStorePtr;

use polkadot_node_network_protocol::{
	request_response::{v1 as request_v1, vstaging as protocol_vstaging, IncomingRequestReceiver},
	PeerId, UnifiedReputationChange as Rep,
};
use polkadot_primitives::v2::{CollatorPair, Hash};

use polkadot_node_subsystem::{
	errors::SubsystemError,
	messages::{NetworkBridgeTxMessage, RuntimeApiMessage, RuntimeApiRequest},
	overseer, SpawnedSubsystem,
};

mod error;

mod collator_side;
mod validator_side;

const LOG_TARGET: &'static str = "parachain::collator-protocol";

/// The maximum depth a candidate can occupy for any relay parent.
/// 'depth' is defined as the amount of blocks between the para
/// head in a relay-chain block's state and a candidate with a
/// particular relay-parent.
///
/// This value is only used for limiting the number of candidates
/// we accept and distribute per relay parent.
const MAX_CANDIDATE_DEPTH: usize = 4;

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
		keystore: SyncCryptoStorePtr,
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

	async fn run<Context>(self, ctx: Context) -> std::result::Result<(), error::FatalError> {
		match self.protocol_side {
			ProtocolSide::Validator { keystore, eviction_policy, metrics } =>
				validator_side::run(ctx, keystore, eviction_policy, metrics).await,
			ProtocolSide::Collator {
				peer_id,
				collator_pair,
				request_receiver_v1,
				request_receiver_vstaging,
				metrics,
			} =>
				collator_side::run(
					ctx,
					peer_id,
					collator_pair,
					request_receiver_v1,
					request_receiver_vstaging,
					metrics,
				)
				.await,
		}
	}
}

#[overseer::subsystem(CollatorProtocol, error=SubsystemError, prefix=self::overseer)]
impl<Context> CollatorProtocolSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self
			.run(ctx)
			.map_err(|e| SubsystemError::with_origin("collator-protocol", e))
			.boxed();

		SpawnedSubsystem { name: "collator-protocol-subsystem", future }
	}
}

/// Modify the reputation of a peer based on its behavior.
async fn modify_reputation(
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

	sender.send_message(NetworkBridgeTxMessage::ReportPeer(peer, rep)).await;
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ProspectiveParachainsMode {
	// v2 runtime API: no prospective parachains.
	Disabled,
	// vstaging runtime API: prospective parachains.
	Enabled,
}

impl ProspectiveParachainsMode {
	fn is_enabled(&self) -> bool {
		matches!(self, Self::Enabled)
	}
}

async fn prospective_parachains_mode<Sender>(
	sender: &mut Sender,
	leaf_hash: Hash,
) -> Result<ProspectiveParachainsMode, error::Error>
where
	Sender: polkadot_node_subsystem::CollatorProtocolSenderTrait,
{
	let (tx, rx) = futures::channel::oneshot::channel();
	sender
		.send_message(RuntimeApiMessage::Request(leaf_hash, RuntimeApiRequest::Version(tx)))
		.await;

	let version = rx
		.await
		.map_err(error::Error::CancelledRuntimeApiVersion)?
		.map_err(error::Error::RuntimeApi)?;

	if version >= RuntimeApiRequest::VALIDITY_CONSTRAINTS {
		Ok(ProspectiveParachainsMode::Enabled)
	} else {
		if version < 2 {
			gum::warn!(
				target: LOG_TARGET,
				"Runtime API version is {}, it is expected to be at least 2. Prospective parachains are disabled",
				version
			);
		}
		Ok(ProspectiveParachainsMode::Disabled)
	}
}
