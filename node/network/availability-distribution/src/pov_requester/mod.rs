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

//! PoV requester takes care of requesting PoVs from validators of a backing group.

use futures::channel::mpsc;
use lru::LruCache;

use polkadot_node_network_protocol::{PeerId, peer_set::PeerSet};
use polkadot_primitives::v1::{AuthorityDiscoveryId, Hash, SessionIndex};
use polkadot_subsystem::{ActiveLeavesUpdate, SubsystemContext, messages::{AllMessages, NetworkBridgeMessage}};

use crate::runtime::Runtime;

/// Number of sessions we want to keep in the LRU.
const NUM_SESSIONS: usize = 2;

pub struct PoVRequester {

	/// We only ever care about being connected to validators of at most two sessions.
	///
	/// So we keep an LRU for managing connection requests of size 2.
	connected_validators: LruCache<SessionIndex, mpsc::Receiver<(AuthorityDiscoveryId, PeerId)>>,
}

impl PoVRequester {
	/// Create a new requester for PoVs.
	pub fn new() -> Self {
		Self {
			connected_validators: LruCache::new(NUM_SESSIONS),
		}
	}

	/// Make sure we are connected to the right set of validators.
	///
	/// On every `ActiveLeavesUpdate`, we check whether we are connected properly to our current
	/// validator group.
	pub async fn update_connected_validators<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut Runtime,
		update: &ActiveLeavesUpdate,
	) -> super::Result<()>
	where
		Context: SubsystemContext,
	{
		let activated = update.activated.iter().map(|(h, _)| h);
		let activated_sessions =
			get_activated_sessions(ctx, runtime, activated).await?;

		for (parent, session_index) in activated_sessions {
			if self.connected_validators.contains(&session_index) {
				continue
			}
			self.connected_validators.put(
				session_index,
				connect_to_relevant_validators(ctx, runtime, parent, session_index).await?
			);
		}
		Ok(())
	}

}

async fn get_activated_sessions<Context>(ctx: &mut Context, runtime: &mut Runtime, new_heads: impl Iterator<Item = &Hash>) 
	-> super::Result<impl Iterator<Item = (Hash, SessionIndex)>>
where
	Context: SubsystemContext,
{
	let mut sessions = Vec::new();
	for parent in new_heads {
		sessions.push((*parent, runtime.get_session_index(ctx, *parent).await?));
	}
	Ok(sessions.into_iter())
}

async fn connect_to_relevant_validators<Context>(
	ctx: &mut Context,
	runtime: &mut Runtime,
	parent: Hash,
	session: SessionIndex
) 
	-> super::Result<mpsc::Receiver<(AuthorityDiscoveryId, PeerId)>>
where
	Context: SubsystemContext,
{
	let validator_ids = determine_relevant_validators(ctx, runtime, parent, session).await?;
	// We don't actually care about `PeerId`s, just keeping receiver so we stay connected:
	let (tx, rx) = mpsc::channel(0);
	ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToValidators {
		validator_ids, peer_set: PeerSet::Validation, connected: tx
	})).await;
	Ok(rx)
}

async fn determine_relevant_validators<Context>(
	ctx: &mut Context,
	runtime: &mut Runtime,
	parent: Hash,
	session: SessionIndex,
) 
	-> super::Result<Vec<AuthorityDiscoveryId>>
where
	Context: SubsystemContext,
{
	let info = runtime.get_session_info_by_index(ctx, parent, session).await?;
	if let Some(validator_info) = &info.validator_info {
		let indeces = info.session_info.validator_groups.get(validator_info.our_group.0 as usize)
			.expect("Our group got retrieved from that session info, it must exist. qed.")
			.clone();
		Ok(indeces.into_iter().map(|i| info.session_info.discovery_keys[i.0 as usize].clone()).collect())
	} else {
		Ok(Vec::new())
	}
}
