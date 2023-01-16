// Copyright 2017-2021 Parity Technologies (UK) Ltd.
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

use futures::prelude::*;
use futures_timer::Delay;
use std::{
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};

#[derive(Copy, Clone)]
enum MetronomeState {
	Snooze,
	SetAlarm,
}

/// Create a stream of ticks with a defined cycle duration.
pub struct Metronome {
	delay: Delay,
	period: Duration,
	state: MetronomeState,
}

impl Metronome {
	/// Create a new metronome source with a defined cycle duration.
	pub fn new(cycle: Duration) -> Self {
		let period = cycle.into();
		Self { period, delay: Delay::new(period), state: MetronomeState::Snooze }
	}
}

impl futures::Stream for Metronome {
	type Item = ();
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		loop {
			match self.state {
				MetronomeState::SetAlarm => {
					let val = self.period;
					self.delay.reset(val);
					self.state = MetronomeState::Snooze;
				},
				MetronomeState::Snooze => {
					if !Pin::new(&mut self.delay).poll(cx).is_ready() {
						break
					}
					self.state = MetronomeState::SetAlarm;
					return Poll::Ready(Some(()))
				},
			}
		}
		Poll::Pending
	}
}
