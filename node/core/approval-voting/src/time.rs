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

//! Time utilities for approval voting.

use futures::prelude::*;
use polkadot_node_primitives::approval::DelayTranche;
use sp_consensus_slots::Slot;
use std::{
	pin::Pin,
	time::{Duration, SystemTime},
};

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
