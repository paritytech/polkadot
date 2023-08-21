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

use std::{cmp::Ordering, collections::BinaryHeap, time::Instant};

use futures::future::pending;
use futures_timer::Delay;

/// Wait asynchronously for given `Instant`s one after the other.
///
/// `PendingWake`s can be inserted and `WaitingQueue` makes `wait_ready()` to always wait for the
/// next `Instant` in the queue.
pub struct WaitingQueue<Payload> {
	/// All pending wakes we are supposed to wait on in order.
	pending_wakes: BinaryHeap<PendingWake<Payload>>,
	/// Wait for next `PendingWake`.
	timer: Option<Delay>,
}

/// Represents some event waiting to be processed at `ready_at`.
///
/// This is an event in `WaitingQueue`. It provides an `Ord` instance, that sorts descending with
/// regard to `Instant` (so we get a `min-heap` with the earliest `Instant` at the top).
#[derive(Eq, PartialEq)]
pub struct PendingWake<Payload> {
	pub payload: Payload,
	pub ready_at: Instant,
}

impl<Payload: Eq + Ord> WaitingQueue<Payload> {
	/// Get a new empty `WaitingQueue`.
	///
	/// If you call `pop` on this queue immediately, it will always return `Poll::Pending`.
	pub fn new() -> Self {
		Self { pending_wakes: BinaryHeap::new(), timer: None }
	}

	/// Push a `PendingWake`.
	///
	/// The next call to `wait_ready` will make sure to wake soon enough to process that new event
	/// in a timely manner.
	pub fn push(&mut self, wake: PendingWake<Payload>) {
		self.pending_wakes.push(wake);
		// Reset timer as it is potentially obsolete now:
		self.timer = None;
	}

	/// Pop the next ready item.
	///
	/// This function does not wait, if nothing is ready right now as determined by the passed
	/// `now` time stamp, this function simply returns `None`.
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
	/// Once this function returns `Poll::Ready(())` `pop_ready()` will return `Some`, if passed
	/// the same `Instant`.
	///
	/// Whether ready or not is determined based on the passed time stamp `now` which should be the
	/// current time as returned by `Instant::now()`
	///
	/// This function waits asynchronously for an item to become ready. If there is no more item,
	/// this call will wait forever (return Poll::Pending without scheduling a wake).
	pub async fn wait_ready(&mut self, now: Instant) {
		if let Some(timer) = &mut self.timer {
			// Previous timer was not done yet.
			timer.await
		}

		let next_waiting = self.pending_wakes.peek();
		let is_ready = next_waiting.map_or(false, |p| p.ready_at <= now);
		if is_ready {
			return
		}

		self.timer = next_waiting.map(|p| Delay::new(p.ready_at.duration_since(now)));
		match &mut self.timer {
			None => return pending().await,
			Some(timer) => timer.await,
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
#[cfg(test)]
mod tests {
	use std::{
		task::Poll,
		time::{Duration, Instant},
	};

	use assert_matches::assert_matches;
	use futures::{future::poll_fn, pin_mut, Future};

	use crate::LOG_TARGET;

	use super::{PendingWake, WaitingQueue};

	#[test]
	fn wait_ready_waits_for_earliest_event_always() {
		sp_tracing::try_init_simple();
		let mut queue = WaitingQueue::new();
		let now = Instant::now();
		let start = now;
		queue.push(PendingWake { payload: 1u32, ready_at: now + Duration::from_millis(3) });
		// Push another one in order:
		queue.push(PendingWake { payload: 2u32, ready_at: now + Duration::from_millis(5) });
		// Push one out of order:
		queue.push(PendingWake { payload: 0u32, ready_at: now + Duration::from_millis(1) });
		// Push another one at same timestamp (should become ready at the same time)
		queue.push(PendingWake { payload: 10u32, ready_at: now + Duration::from_millis(1) });

		futures::executor::block_on(async move {
			// No time passed yet - nothing should be ready.
			assert!(queue.pop_ready(now).is_none(), "No time has passed, nothing should be ready");

			// Receive them in order at expected times:
			queue.wait_ready(now).await;
			gum::trace!(target: LOG_TARGET, "After first wait.");

			let now = start + Duration::from_millis(1);
			assert!(Instant::now() - start >= Duration::from_millis(1));
			assert_eq!(queue.pop_ready(now).map(|p| p.payload), Some(0u32));
			// One more should be ready:
			assert_eq!(queue.pop_ready(now).map(|p| p.payload), Some(10u32));
			assert!(queue.pop_ready(now).is_none(), "No more entry expected to be ready.");

			queue.wait_ready(now).await;
			gum::trace!(target: LOG_TARGET, "After second wait.");
			let now = start + Duration::from_millis(3);
			assert!(Instant::now() - start >= Duration::from_millis(3));
			assert_eq!(queue.pop_ready(now).map(|p| p.payload), Some(1u32));
			assert!(queue.pop_ready(now).is_none(), "No more entry expected to be ready.");

			// Push in between wait:
			poll_fn(|cx| {
				let fut = queue.wait_ready(now);
				pin_mut!(fut);
				assert_matches!(fut.poll(cx), Poll::Pending);
				Poll::Ready(())
			})
			.await;
			queue.push(PendingWake { payload: 3u32, ready_at: start + Duration::from_millis(4) });

			queue.wait_ready(now).await;
			// Newly pushed element should have become ready:
			gum::trace!(target: LOG_TARGET, "After third wait.");
			let now = start + Duration::from_millis(4);
			assert!(Instant::now() - start >= Duration::from_millis(4));
			assert_eq!(queue.pop_ready(now).map(|p| p.payload), Some(3u32));
			assert!(queue.pop_ready(now).is_none(), "No more entry expected to be ready.");

			queue.wait_ready(now).await;
			gum::trace!(target: LOG_TARGET, "After fourth wait.");
			let now = start + Duration::from_millis(5);
			assert!(Instant::now() - start >= Duration::from_millis(5));
			assert_eq!(queue.pop_ready(now).map(|p| p.payload), Some(2u32));
			assert!(queue.pop_ready(now).is_none(), "No more entry expected to be ready.");

			// queue empty - should wait forever now:
			poll_fn(|cx| {
				let fut = queue.wait_ready(now);
				pin_mut!(fut);
				assert_matches!(fut.poll(cx), Poll::Pending);
				Poll::Ready(())
			})
			.await;
		});
	}
}
