// Copyright 2022 Parity Technologies (UK) Ltd.
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

use std::collections::{hash_map::Entry, HashMap, VecDeque};

use polkadot_node_network_protocol::request_response::{v1::DisputeRequest, IncomingRequest};
use polkadot_primitives::v2::AuthorityDiscoveryId;

/// How many messages we are willing to queue per peer (validator).
///
/// The larger this value is, the larger bursts are allowed to be without us dropping messages. On
/// the flipside we should this gets allocated per validator, so for a size of 10 this will result
/// in 10_000 * size_of(IncomingRequest).
///
/// `PEER_QUEUE_CAPACITY` must not be 0 for obvious reasons.
pub const PEER_QUEUE_CAPACITY: usize = 10;

/// Queues for messages from authority peers.
///
/// Two invariants are ensured:
///
/// 1. No queue will ever have more than `PEER_QUEUE_CAPACITY` elements.
/// 2. There are no empty queues. Whenever a queue gets empty, it is removed. This way checking
///    whether there are any messages queued is cheap.
pub struct PeerQueues {
	queues: HashMap<AuthorityDiscoveryId, VecDeque<IncomingRequest<DisputeRequest>>>,
}

impl PeerQueues {
	/// New empty `PeerQueues`
	pub fn new() -> Self {
		Self { queues: HashMap::new() }
	}

	/// Push an incoming request for a given authority.
	///
	/// Returns: `Ok(())` if succeeded, `Err((args))` if capacity is reached.
	pub fn push_req(
		&mut self,
		peer: AuthorityDiscoveryId,
		req: IncomingRequest<DisputeRequest>,
	) -> Result<(), (AuthorityDiscoveryId, IncomingRequest<DisputeRequest>)> {
		let queue = match self.queues.entry(peer) {
			Entry::Vacant(vacant) => {
				vacant.insert(VecDeque::new())
			},
			Entry::Occupied(occupied) => {
				if occupied.get().len() >= PEER_QUEUE_CAPACITY {
					return Err((occupied.key().clone(), req))
				}
				occupied.get_mut()
			},
		};
		queue.push_back(req);
		Ok(())
	}

	/// Pop all heads and return them for processing.
	pub fn pop_reqs(&mut self) -> Vec<IncomingRequest<DisputeRequest>> {
		let mut heads = Vec::with_capacity(self.queues.len());
		let mut new_queues = HashMap::new();
		for (k, queue) in self.queues.into_iter() {
			let front = queue.pop_front();
			debug_assert!(front.is_some(), "Invariant that queues are never empty is broken.");

			if let Some(front) = front {
				heads.push(front);
			}
			if !queue.is_empty() {
				new_queues.insert(k, queue);
			}
		}
		self.queues = new_queues;
		heads
	}

	/// Whether or not all queues are empty.
	pub fn is_empty(&self) -> bool {
		self.queues.is_empty()
	}
}
