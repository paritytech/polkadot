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

use std::collections::{hash_map::Entry, HashMap, VecDeque};

use futures::future::pending;
use futures_timer::Delay;
use polkadot_node_network_protocol::request_response::{v1::DisputeRequest, IncomingRequest};
use polkadot_primitives::AuthorityDiscoveryId;

use crate::RECEIVE_RATE_LIMIT;

/// How many messages we are willing to queue per peer (validator).
///
/// The larger this value is, the larger bursts are allowed to be without us dropping messages. On
/// the flip side this gets allocated per validator, so for a size of 10 this will result
/// in `10_000 * size_of(IncomingRequest)` in the worst case.
///
/// `PEER_QUEUE_CAPACITY` must not be 0 for obvious reasons.
#[cfg(not(test))]
pub const PEER_QUEUE_CAPACITY: usize = 10;
#[cfg(test)]
pub const PEER_QUEUE_CAPACITY: usize = 2;

/// Queues for messages from authority peers for rate limiting.
///
/// Invariants ensured:
///
/// 1. No queue will ever have more than `PEER_QUEUE_CAPACITY` elements.
/// 2. There are no empty queues. Whenever a queue gets empty, it is removed. This way checking
///    whether there are any messages queued is cheap.
/// 3. As long as not empty, `pop_reqs` will, if called in sequence, not return `Ready` more often
///    than once for every `RECEIVE_RATE_LIMIT`, but it will always return Ready eventually.
/// 4. If empty `pop_reqs` will never return `Ready`, but will always be `Pending`.
pub struct PeerQueues {
	/// Actual queues.
	queues: HashMap<AuthorityDiscoveryId, VecDeque<IncomingRequest<DisputeRequest>>>,

	/// Delay timer for establishing the rate limit.
	rate_limit_timer: Option<Delay>,
}

impl PeerQueues {
	/// New empty `PeerQueues`.
	pub fn new() -> Self {
		Self { queues: HashMap::new(), rate_limit_timer: None }
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
			Entry::Vacant(vacant) => vacant.insert(VecDeque::new()),
			Entry::Occupied(occupied) => {
				if occupied.get().len() >= PEER_QUEUE_CAPACITY {
					return Err((occupied.key().clone(), req))
				}
				occupied.into_mut()
			},
		};
		queue.push_back(req);

		// We have at least one element to process - rate limit `timer` needs to exist now:
		self.ensure_timer();
		Ok(())
	}

	/// Pop all heads and return them for processing.
	///
	/// This gets one message from each peer that has sent at least one.
	///
	/// This function is rate limited, if called in sequence it will not return more often than
	/// every `RECEIVE_RATE_LIMIT`.
	///
	/// NOTE: If empty this function will not return `Ready` at all, but will always be `Pending`.
	pub async fn pop_reqs(&mut self) -> Vec<IncomingRequest<DisputeRequest>> {
		self.wait_for_timer().await;

		let mut heads = Vec::with_capacity(self.queues.len());
		let old_queues = std::mem::replace(&mut self.queues, HashMap::new());
		for (k, mut queue) in old_queues.into_iter() {
			let front = queue.pop_front();
			debug_assert!(front.is_some(), "Invariant that queues are never empty is broken.");

			if let Some(front) = front {
				heads.push(front);
			}
			if !queue.is_empty() {
				self.queues.insert(k, queue);
			}
		}

		if !self.is_empty() {
			// Still not empty - we should get woken at some point.
			self.ensure_timer();
		}

		heads
	}

	/// Whether or not all queues are empty.
	pub fn is_empty(&self) -> bool {
		self.queues.is_empty()
	}

	/// Ensure there is an active `timer`.
	///
	/// Checks whether one exists and if not creates one.
	fn ensure_timer(&mut self) -> &mut Delay {
		self.rate_limit_timer.get_or_insert(Delay::new(RECEIVE_RATE_LIMIT))
	}

	/// Wait for `timer` if it exists, or be `Pending` forever.
	///
	/// Afterwards it gets set back to `None`.
	async fn wait_for_timer(&mut self) {
		match self.rate_limit_timer.as_mut() {
			None => pending().await,
			Some(timer) => timer.await,
		}
		self.rate_limit_timer = None;
	}
}
