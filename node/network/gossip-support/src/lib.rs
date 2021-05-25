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

//! This subsystem is responsible for keeping track of session changes
//! and issuing a connection request to the validators relevant to
//! the gossiping subsystems on every new session.

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};
use futures::{channel::oneshot, FutureExt as _};
use polkadot_node_subsystem::{
	messages::{
		AllMessages, GossipSupportMessage, NetworkBridgeMessage,
	},
	ActiveLeavesUpdate, FromOverseer, OverseerSignal,
	Subsystem, SpawnedSubsystem, SubsystemContext,
};
use polkadot_node_subsystem_util as util;
use polkadot_primitives::v1::{
	Hash, SessionIndex, AuthorityDiscoveryId,
};
use polkadot_node_network_protocol::peer_set::PeerSet;
use sp_keystore::{CryptoStore, SyncCryptoStorePtr};
use sp_application_crypto::{Public, AppKey};

const LOG_TARGET: &str = "parachain::gossip-support";
// How much time should we wait since the last
// authority discovery resolution failure.
const BACKOFF_DURATION: Duration = Duration::from_secs(5);

/// The Gossip Support subsystem.
pub struct GossipSupport {
	keystore: SyncCryptoStorePtr,
}

#[derive(Default)]
struct State {
	last_session_index: Option<SessionIndex>,
	// Some(timestamp) if we failed to resolve
	// at least a third of authorities the last time.
	// `None` otherwise.
	last_failure: Option<Instant>,
}

impl GossipSupport {
	/// Create a new instance of the [`GossipSupport`] subsystem.
	pub fn new(keystore: SyncCryptoStorePtr) -> Self {
		Self {
			keystore,
		}
	}

	async fn run<Context>(self, ctx: Context)
	where
		Context: SubsystemContext<Message = GossipSupportMessage>,
	{
		let mut state = State::default();
		self.run_inner(ctx, &mut state).await;
	}

	async fn run_inner<Context>(self, mut ctx: Context, state: &mut State)
	where
		Context: SubsystemContext<Message = GossipSupportMessage>,
	{
		let Self { keystore } = self;
		loop {
			let message = match ctx.recv().await {
				Ok(message) => message,
				Err(e) => {
					tracing::debug!(
						target: LOG_TARGET,
						err = ?e,
						"Failed to receive a message from Overseer, exiting"
					);
					return;
				},
			};
			match message {
				FromOverseer::Communication { .. } => {},
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated,
					..
				})) => {
					tracing::trace!(target: LOG_TARGET, "active leaves signal");

					let leaves = activated.into_iter().map(|a| a.hash);
					if let Err(e) = state.handle_active_leaves(&mut ctx, &keystore, leaves).await {
						tracing::debug!(target: LOG_TARGET, error = ?e);
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, _number)) => {},
				FromOverseer::Signal(OverseerSignal::Conclude) => {
					return;
				}
			}
		}
	}
}

async fn determine_relevant_authorities(
	ctx: &mut impl SubsystemContext,
	relay_parent: Hash,
) -> Result<Vec<AuthorityDiscoveryId>, util::Error> {
	let authorities = util::request_authorities(relay_parent, ctx.sender()).await.await??;
	tracing::debug!(
		target: LOG_TARGET,
		authority_count = ?authorities.len(),
		"Determined relevant authorities"
	);
	Ok(authorities)
}

/// Return an error if we're not a validator in the given set (do not have keys).
async fn ensure_i_am_an_authority(
	keystore: &SyncCryptoStorePtr,
	authorities: &[AuthorityDiscoveryId],
) -> Result<(), util::Error> {
	for v in authorities {
		if CryptoStore::has_keys(&**keystore, &[(v.to_raw_vec(), AuthorityDiscoveryId::ID)])
			.await
		{
			return Ok(());
		}
	}
	Err(util::Error::NotAValidator)
}

/// A helper function for making a `ConnectToValidators` request.
pub async fn connect_to_authorities(
	ctx: &mut impl SubsystemContext,
	validator_ids: Vec<AuthorityDiscoveryId>,
	peer_set: PeerSet,
) -> oneshot::Receiver<usize> {
	let (failed, failed_rx) = oneshot::channel();
	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ConnectToValidators {
			validator_ids,
			peer_set,
			failed,
		}
	)).await;
	failed_rx
}

impl State {
	/// 1. Determine if the current session index has changed.
	/// 2. If it has, determine relevant validators
	///    and issue a connection request.
	async fn handle_active_leaves(
		&mut self,
		ctx: &mut impl SubsystemContext,
		keystore: &SyncCryptoStorePtr,
		leaves: impl Iterator<Item = Hash>,
	) -> Result<(), util::Error> {
		for leaf in leaves {
			let current_index = util::request_session_index_for_child(leaf, ctx.sender()).await.await??;
			let since_failure = self.last_failure.map(|i| i.elapsed()).unwrap_or_default();
			let force_request = since_failure >= BACKOFF_DURATION;
			let maybe_new_session = match self.last_session_index {
				Some(i) if current_index <= i && !force_request => None,
				_ => Some((current_index, leaf)),
			};

			if let Some((new_session, relay_parent)) = maybe_new_session {
				tracing::debug!(
					target: LOG_TARGET,
					%new_session,
					%force_request,
					"New session detected",
				);
				let authorities = determine_relevant_authorities(ctx, relay_parent).await?;
				ensure_i_am_an_authority(keystore, &authorities).await?;
				let num = authorities.len();
				tracing::debug!(target: LOG_TARGET, %num, "Issuing a connection request");

				let failures = connect_to_authorities(
					ctx,
					authorities,
					PeerSet::Validation,
				).await;

				// we await for the request to be processed
				// this is fine, it should take much less time than one session
				let failures = failures.await.unwrap_or(num);

				self.last_session_index = Some(new_session);
				// issue another request for the same session
				// if at least a third of the authorities were not resolved
				self.last_failure = if failures >= num / 3 {
					Some(Instant::now())
				} else {
					None
				}
			}
		}

		Ok(())
	}
}

impl<Context> Subsystem<Context> for GossipSupport
where
	Context: SubsystemContext<Message = GossipSupportMessage> + Sync + Send,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = self.run(ctx)
			.map(|_| Ok(()))
			.boxed();

		SpawnedSubsystem {
			name: "gossip-support-subsystem",
			future,
		}
	}
}
