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

//! Time utilities for approval voting.

use futures::{
	future::BoxFuture,
	prelude::*,
	stream::{FusedStream, FuturesUnordered},
	Stream, StreamExt,
};

use polkadot_node_primitives::approval::v1::DelayTranche;
use sp_consensus_slots::Slot;
use std::{
	collections::HashSet,
	pin::Pin,
	task::Poll,
	time::{Duration, SystemTime},
};

use polkadot_primitives::{Hash, ValidatorIndex};
const TICK_DURATION_MILLIS: u64 = 500;

/// A base unit of time, starting from the Unix epoch, split into half-second intervals.
pub(crate) type Tick = u64;

/// A clock which allows querying of the current tick as well as
/// waiting for a tick to be reached.
pub(crate) trait Clock {
	/// Yields the current tick.
	fn tick_now(&self) -> Tick;

	/// Yields a future which concludes when the given tick is reached.
	fn wait(&self, tick: Tick) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
}

/// Extension methods for clocks.
pub(crate) trait ClockExt {
	fn tranche_now(&self, slot_duration_millis: u64, base_slot: Slot) -> DelayTranche;
}

impl<C: Clock + ?Sized> ClockExt for C {
	fn tranche_now(&self, slot_duration_millis: u64, base_slot: Slot) -> DelayTranche {
		self.tick_now()
			.saturating_sub(slot_number_to_tick(slot_duration_millis, base_slot)) as u32
	}
}

/// A clock which uses the actual underlying system clock.
pub(crate) struct SystemClock;

impl Clock for SystemClock {
	/// Yields the current tick.
	fn tick_now(&self) -> Tick {
		match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
			Err(_) => 0,
			Ok(d) => d.as_millis() as u64 / TICK_DURATION_MILLIS,
		}
	}

	/// Yields a future which concludes when the given tick is reached.
	fn wait(&self, tick: Tick) -> Pin<Box<dyn Future<Output = ()> + Send>> {
		let fut = async move {
			let now = SystemTime::now();
			let tick_onset = tick_to_time(tick);
			if now < tick_onset {
				if let Some(until) = tick_onset.duration_since(now).ok() {
					futures_timer::Delay::new(until).await;
				}
			}
		};

		Box::pin(fut)
	}
}

fn tick_to_time(tick: Tick) -> SystemTime {
	SystemTime::UNIX_EPOCH + Duration::from_millis(TICK_DURATION_MILLIS * tick)
}

/// assumes `slot_duration_millis` evenly divided by tick duration.
pub(crate) fn slot_number_to_tick(slot_duration_millis: u64, slot: Slot) -> Tick {
	let ticks_per_slot = slot_duration_millis / TICK_DURATION_MILLIS;
	u64::from(slot) * ticks_per_slot
}

/// A list of delayed futures that gets triggered when the waiting time has expired and it is
/// time to sign the candidate.
/// We have a timer per relay-chain block.
#[derive(Default)]
pub struct DelayedApprovalTimer {
	timers: FuturesUnordered<BoxFuture<'static, (Hash, ValidatorIndex)>>,
	blocks: HashSet<Hash>,
}

impl DelayedApprovalTimer {
	/// Starts a single timer per block hash
	///
	/// Guarantees that if a timer already exits for the give block hash,
	/// no additional timer is started.
	pub(crate) fn maybe_arm_timer(
		&mut self,
		wait_untill: Tick,
		clock: &dyn Clock,
		block_hash: Hash,
		validator_index: ValidatorIndex,
	) {
		if self.blocks.insert(block_hash) {
			let clock_wait = clock.wait(wait_untill);
			self.timers.push(Box::pin(async move {
				clock_wait.await;
				(block_hash, validator_index)
			}));
		}
	}
}

impl Stream for DelayedApprovalTimer {
	type Item = (Hash, ValidatorIndex);

	fn poll_next(
		mut self: std::pin::Pin<&mut Self>,
		cx: &mut std::task::Context<'_>,
	) -> std::task::Poll<Option<Self::Item>> {
		let poll_result = self.timers.poll_next_unpin(cx);
		match poll_result {
			Poll::Ready(Some(result)) => {
				self.blocks.remove(&result.0);
				Poll::Ready(Some(result))
			},
			_ => poll_result,
		}
	}
}

impl FusedStream for DelayedApprovalTimer {
	fn is_terminated(&self) -> bool {
		self.timers.is_terminated()
	}
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use futures::{executor::block_on, FutureExt, StreamExt};
	use futures_timer::Delay;
	use polkadot_primitives::{Hash, ValidatorIndex};

	use crate::time::{Clock, SystemClock};

	use super::DelayedApprovalTimer;

	#[test]
	fn test_select_empty_timer() {
		block_on(async move {
			let mut timer = DelayedApprovalTimer::default();

			for _ in 1..10 {
				let result = futures::select!(
					_ = timer.select_next_some() => {
						0
					}
					// Only this arm should fire
					_ = Delay::new(Duration::from_millis(100)).fuse() => {
						1
					}
				);

				assert_eq!(result, 1);
			}
		});
	}

	#[test]
	fn test_timer_functionality() {
		block_on(async move {
			let mut timer = DelayedApprovalTimer::default();
			let test_hashes =
				vec![Hash::repeat_byte(0x01), Hash::repeat_byte(0x02), Hash::repeat_byte(0x03)];
			for (index, hash) in test_hashes.iter().enumerate() {
				timer.maybe_arm_timer(
					SystemClock.tick_now() + index as u64,
					&SystemClock,
					*hash,
					ValidatorIndex::from(2),
				);
				timer.maybe_arm_timer(
					SystemClock.tick_now() + index as u64,
					&SystemClock,
					*hash,
					ValidatorIndex::from(2),
				);
			}
			let timeout_hash = Hash::repeat_byte(0x02);
			for i in 0..test_hashes.len() * 2 {
				let result = futures::select!(
					(hash, _) = timer.select_next_some() => {
						hash
					}
					// Timers should fire only once, so for the rest of the iterations we should timeout through here.
					_ = Delay::new(Duration::from_secs(2)).fuse() => {
						timeout_hash
					}
				);
				assert_eq!(test_hashes.get(i).cloned().unwrap_or(timeout_hash), result);
			}

			// Now check timer can be restarted if already fired
			for (index, hash) in test_hashes.iter().enumerate() {
				timer.maybe_arm_timer(
					SystemClock.tick_now() + index as u64,
					&SystemClock,
					*hash,
					ValidatorIndex::from(2),
				);
				timer.maybe_arm_timer(
					SystemClock.tick_now() + index as u64,
					&SystemClock,
					*hash,
					ValidatorIndex::from(2),
				);
			}

			for i in 0..test_hashes.len() * 2 {
				let result = futures::select!(
					(hash, _) = timer.select_next_some() => {
						hash
					}
					// Timers should fire only once, so for the rest of the iterations we should timeout through here.
					_ = Delay::new(Duration::from_secs(2)).fuse() => {
						timeout_hash
					}
				);
				assert_eq!(test_hashes.get(i).cloned().unwrap_or(timeout_hash), result);
			}
		});
	}
}
