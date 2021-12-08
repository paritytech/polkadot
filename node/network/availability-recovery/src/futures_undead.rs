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

//! FuturesUndead: A `FuturesUnordered` with support for semi canceled futures. Those undead
//! futures will still get polled, but will not count towards length. So length will only count
//! futures, which are still considered live.
//!
//! Use case: If futures take longer than we would like them too, we may be able to request the data
//! from somewhere else as well. We don't really want to cancel the old future, because maybe it
//! was almost done, thus we would have wasted time with our impatience. By simply making them
//! not count towards length, we can make sure to have enough "live" requests ongoing, while at the
//! same time taking advantage of some maybe "late" response from the undead.
//!

use std::{
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};

use futures::{future::BoxFuture, stream::FuturesUnordered, Future, Stream, StreamExt};
use polkadot_node_subsystem_util::TimeoutExt;

/// FuturesUndead - `FuturesUnordered` with semi canceled (undead) futures.
///
/// Limitations: Keeps track of undead futures by means of a counter, which is limited to 64
/// bits, so after `1.8*10^19` pushed futures, this implementation will panic.
pub struct FuturesUndead<Output> {
	/// Actual `FuturesUnordered`.
	inner: FuturesUnordered<Undead<Output>>,
	/// Next sequence number to assign to the next future that gets pushed.
	next_sequence: SequenceNumber,
	/// Sequence number of first future considered live.
	first_live: Option<SequenceNumber>,
	/// How many undead are there right now.
	undead: usize,
}

/// All futures get a number, to determine which are live.
#[derive(Eq, PartialEq, Copy, Clone, Debug, PartialOrd)]
struct SequenceNumber(usize);

struct Undead<Output> {
	inner: BoxFuture<'static, Output>,
	our_sequence: SequenceNumber,
}

impl<Output> FuturesUndead<Output> {
	pub fn new() -> Self {
		Self {
			inner: FuturesUnordered::new(),
			next_sequence: SequenceNumber(0),
			first_live: None,
			undead: 0,
		}
	}

	pub fn push(&mut self, f: BoxFuture<'static, Output>) {
		self.inner.push(Undead { inner: f, our_sequence: self.next_sequence });
		self.next_sequence.inc();
	}

	/// Make all contained futures undead.
	///
	/// They will no longer be counted on a call to `len`.
	pub fn soft_cancel(&mut self) {
		self.undead = self.inner.len();
		self.first_live = Some(self.next_sequence);
	}

	/// Number of contained futures minus undead.
	pub fn len(&self) -> usize {
		self.inner.len() - self.undead
	}

	/// Total number of futures, including undead.
	pub fn total_len(&self) -> usize {
		self.inner.len()
	}

	/// Wait for next future to return with timeout.
	///
	/// When timeout passes, return `None` and make all currently contained futures undead.
	pub async fn next_with_timeout(&mut self, timeout: Duration) -> Option<Output> {
		match self.next().timeout(timeout).await {
			// Timeout:
			None => {
				self.soft_cancel();
				None
			},
			Some(inner) => inner,
		}
	}
}

impl<Output> Stream for FuturesUndead<Output> {
	type Item = Output;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		match self.inner.poll_next_unpin(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(None) => Poll::Ready(None),
			Poll::Ready(Some((sequence, v))) => {
				// Cleanup in case we became completely empty:
				if self.inner.len() == 0 {
					*self = Self::new();
					return Poll::Ready(Some(v))
				}

				let first_live = match self.first_live {
					None => return Poll::Ready(Some(v)),
					Some(first_live) => first_live,
				};
				// An undead came back:
				if sequence < first_live {
					self.undead = self.undead.saturating_sub(1);
				}
				Poll::Ready(Some(v))
			},
		}
	}
}

impl SequenceNumber {
	pub fn inc(&mut self) {
		self.0 = self.0.checked_add(1).expect(
			"We don't expect an `UndeadFuture` to live long enough for 2^64 entries ever getting inserted."
		);
	}
}

impl<T> Future for Undead<T> {
	type Output = (SequenceNumber, T);
	fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		match self.inner.as_mut().poll(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(v) => Poll::Ready((self.our_sequence, v)),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::{executor, pending, FutureExt};

	#[test]
	fn cancel_sets_len_to_zero() {
		let mut undead = FuturesUndead::new();
		undead.push((async { () }).boxed());
		assert_eq!(undead.len(), 1);
		undead.soft_cancel();
		assert_eq!(undead.len(), 0);
	}

	#[test]
	fn finished_undead_does_not_change_len() {
		executor::block_on(async {
			let mut undead = FuturesUndead::new();
			undead.push(async { 1_i32 }.boxed());
			undead.push(async { 2_i32 }.boxed());
			assert_eq!(undead.len(), 2);
			undead.soft_cancel();
			assert_eq!(undead.len(), 0);
			undead.push(
				async {
					pending!();
					0_i32
				}
				.boxed(),
			);
			undead.next().await;
			assert_eq!(undead.len(), 1);
			undead.push(async { 9_i32 }.boxed());
			undead.soft_cancel();
			assert_eq!(undead.len(), 0);
		});
	}

	#[test]
	fn len_stays_correct_when_live_future_ends() {
		executor::block_on(async {
			let mut undead = FuturesUndead::new();
			undead.push(
				async {
					pending!();
					1_i32
				}
				.boxed(),
			);
			undead.push(
				async {
					pending!();
					2_i32
				}
				.boxed(),
			);
			assert_eq!(undead.len(), 2);
			undead.soft_cancel();
			assert_eq!(undead.len(), 0);
			undead.push(async { 0_i32 }.boxed());
			undead.push(async { 1_i32 }.boxed());
			undead.next().await;
			assert_eq!(undead.len(), 1);
			undead.next().await;
			assert_eq!(undead.len(), 0);
			undead.push(async { 9_i32 }.boxed());
			assert_eq!(undead.len(), 1);
		});
	}

	#[test]
	fn cleanup_works() {
		executor::block_on(async {
			let mut undead = FuturesUndead::new();
			undead.push(async { 1_i32 }.boxed());
			undead.soft_cancel();
			undead.push(async { 2_i32 }.boxed());
			undead.next().await;
			undead.next().await;
			assert_eq!(undead.first_live, None);
		});
	}
}
