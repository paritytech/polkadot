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

use std::{cmp::Ordering, collections::BinaryHeap, time::Instant};

use futures::future::pending;
use futures_timer::Delay;

/// Wait asynchronously for given `Instant`s one after the other.
///
/// `PendingWake`s can be inserted and `WaitingQueue` makes sure to wake calls to `pop` so those
/// structs can be processed when they are deemed ready.
pub struct WaitingQueue<Payload> {
	/// All pending wakes we are supposed to wait on in order.
	pending_wakes: BinaryHeap<PendingWake<Payload>>,
	/// Wait for next `PendingWake`.
	waker: Option<Delay>,
}

/// Represents some event waiting to be processed at `ready_at`.
///
/// This is an event in `WaitingQueue`. It provides an `Ord` instance, that sorts descending with
/// regard to `Instant` (so we get a `min-heap` with the earliest `Instant` at the top.
#[derive(Eq, PartialEq)]
pub struct PendingWake<Payload> {
	pub payload: Payload,
	pub ready_at: Instant,
}

impl<Payload: Eq + Ord> WaitingQueue<Payload> {
	/// Get a new empty `WaitingQueue`.
	///
	/// If you call `pop` on this queue immediately it will always return `Poll::Pending`.
	pub fn new() -> Self {
		Self { pending_wakes: BinaryHeap::new(), waker: None }
	}

	/// Push a `PendingWake`.
	///
	/// The next call to `pop` will make sure to wake soon enough to process that newly event in a
	/// timely manner.
	pub fn push(&mut self, wake: PendingWake<Payload>) {
		self.pending_wakes.push(wake);
		// Reset waker as it is potentially obsolete now:
		self.waker = None;
	}

	/// Pop the next ready item.
	///
	/// In contrast to `pop` this function does not wait, if nothing is ready right now as
	/// determined by the passed `now` timestamp, this function simply returns `None`.
	pub fn pop_ready(&mut self, now: Instant) -> Option<PendingWake<Payload>> {
		let is_ready = self.pending_wakes.peek().map_or(false, |p| p.ready_at <= now);
		if is_ready {
			Some(self.pending_wakes.pop().expect("We just peeked. qed."))
		} else {
			None
		}
	}

	/// Don't pop, just wait until something is ready.
	///
	/// Once this function returns `Poll::Ready(())` `pop_ready()` will return `Some`.
	///
	/// Whether ready or not is determined based on the passed timestamp `now` which should be the
	/// current time as returned by `Instant::now()`
	///
	/// This function waits asynchronously for an item to become ready. If there is no more item,
	/// this call will wait forever (return Poll::Pending without scheduling a wake).
	pub async fn wait_ready(&mut self, now: Instant) {
		if let Some(waker) = &mut self.waker {
			// Previous timer was not done yet.
			waker.await
		}
		loop {
			let next_waiting = self.pending_wakes.peek();
			let is_ready = next_waiting.map_or(false, |p| p.ready_at <= now);
			if !is_ready {
				self.waker = next_waiting.map(|p| Delay::new(p.ready_at.duration_since(now)));
				match &mut self.waker {
					None => return pending().await,
					Some(waker) => waker.await,
				}
			} else {
				return
			}
		}
	}
}

impl<Payload: Eq + Ord> PartialOrd<PendingWake<Payload>> for PendingWake<Payload> {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl<Payload: Ord> Ord for PendingWake<Payload> {
	fn cmp(&self, other: &Self) -> Ordering {
		// Reverse order for min-heap:
		match other.ready_at.cmp(&self.ready_at) {
			Ordering::Equal => other.payload.cmp(&self.payload),
			o => o,
		}
	}
}
