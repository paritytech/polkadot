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

//! Metered variant of mpsc channels to be able to extract metrics.

use std::sync::{
	atomic::{AtomicUsize, Ordering},
	Arc,
};

use derive_more::Display;

mod bounded;
pub mod oneshot;
mod unbounded;

pub use self::{bounded::*, unbounded::*};

pub use std::time::Duration;

/// A peek into the inner state of a meter.
#[derive(Debug, Clone)]
pub struct Meter {
	// Number of sends on this channel.
	sent: Arc<AtomicUsize>,
	// Number of receives on this channel.
	received: Arc<AtomicUsize>,
	// Atomic ringbuffer of tha last 50 time of flight values
	tof: Arc<crossbeam_queue::ArrayQueue<Duration>>,
}

impl std::default::Default for Meter {
	fn default() -> Self {
		Self {
			sent: Arc::new(AtomicUsize::new(0)),
			received: Arc::new(AtomicUsize::new(0)),
			tof: Arc::new(crossbeam_queue::ArrayQueue::new(100)),
		}
	}
}

/// A readout of sizes from the meter. Note that it is possible, due to asynchrony, for received
/// to be slightly higher than sent.
#[derive(Debug, Display, Clone, Default, PartialEq)]
#[display(fmt = "(sent={} received={})", sent, received)]
pub struct Readout {
	/// The amount of messages sent on the channel, in aggregate.
	pub sent: usize,
	/// The amount of messages received on the channel, in aggregate.
	pub received: usize,
	/// Time of flight in micro seconds (us)
	pub tof: Vec<Duration>,
}

impl Meter {
	/// Count the number of items queued up inside the channel.
	pub fn read(&self) -> Readout {
		// when obtaining we don't care much about off by one
		// accuracy
		Readout {
			sent: self.sent.load(Ordering::Relaxed),
			received: self.received.load(Ordering::Relaxed),
			tof: {
				let mut acc = Vec::with_capacity(self.tof.len());
				while let Some(value) = self.tof.pop() {
					acc.push(value)
				}
				acc
			},
		}
	}

	fn note_sent(&self) -> usize {
		self.sent.fetch_add(1, Ordering::Relaxed)
	}

	fn retract_sent(&self) {
		self.sent.fetch_sub(1, Ordering::Relaxed);
	}

	fn note_received(&self) {
		self.received.fetch_add(1, Ordering::Relaxed);
	}

	fn note_time_of_flight(&self, tof: Duration) {
		let _ = self.tof.force_push(tof);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use assert_matches::assert_matches;
	use futures::{executor::block_on, StreamExt};

	#[derive(Clone, Copy, Debug, Default)]
	struct Msg {
		val: u8,
	}

	#[test]
	fn try_send_try_next() {
		block_on(async move {
			let (mut tx, mut rx) = channel::<Msg>(5);
			let msg = Msg::default();
			assert_matches!(rx.meter().read(), Readout { sent: 0, received: 0, .. });
			tx.try_send(msg).unwrap();
			assert_matches!(tx.meter().read(), Readout { sent: 1, received: 0, .. });
			tx.try_send(msg).unwrap();
			tx.try_send(msg).unwrap();
			tx.try_send(msg).unwrap();
			assert_matches!(tx.meter().read(), Readout { sent: 4, received: 0, .. });
			rx.try_next().unwrap();
			assert_matches!(rx.meter().read(), Readout { sent: 4, received: 1, .. });
			rx.try_next().unwrap();
			rx.try_next().unwrap();
			assert_matches!(tx.meter().read(), Readout { sent: 4, received: 3, tof } => {
				// every second in test, consumed before
				assert_eq!(dbg!(tof).len(), 1);
			});
			rx.try_next().unwrap();
			assert_matches!(rx.meter().read(), Readout { sent: 4, received: 4, tof } => {
				// every second in test, consumed before
				assert_eq!(dbg!(tof).len(), 0);
			});
			assert!(rx.try_next().is_err());
		});
	}

	#[test]
	fn with_tasks() {
		let (ready, go) = futures::channel::oneshot::channel();

		let (mut tx, mut rx) = channel::<Msg>(5);
		block_on(async move {
			futures::join!(
				async move {
					let msg = Msg::default();
					assert_matches!(tx.meter().read(), Readout { sent: 0, received: 0, .. });
					tx.try_send(msg).unwrap();
					assert_matches!(tx.meter().read(), Readout { sent: 1, received: 0, .. });
					tx.try_send(msg).unwrap();
					tx.try_send(msg).unwrap();
					tx.try_send(msg).unwrap();
					ready.send(()).expect("Helper oneshot channel must work. qed");
				},
				async move {
					go.await.expect("Helper oneshot channel must work. qed");
					assert_matches!(rx.meter().read(), Readout { sent: 4, received: 0, .. });
					rx.try_next().unwrap();
					assert_matches!(rx.meter().read(), Readout { sent: 4, received: 1, .. });
					rx.try_next().unwrap();
					rx.try_next().unwrap();
					assert_matches!(rx.meter().read(), Readout { sent: 4, received: 3, .. });
					rx.try_next().unwrap();
					assert_matches!(dbg!(rx.meter().read()), Readout { sent: 4, received: 4, .. });
				}
			)
		});
	}

	use futures_timer::Delay;
	use std::time::Duration;

	#[test]
	fn stream_and_sink() {
		let (mut tx, mut rx) = channel::<Msg>(5);

		block_on(async move {
			futures::join!(
				async move {
					for i in 0..15 {
						println!("Sent #{} with a backlog of {} items", i + 1, tx.meter().read());
						let msg = Msg { val: i as u8 + 1u8 };
						tx.send(msg).await.unwrap();
						assert!(tx.meter().read().sent > 0usize);
						Delay::new(Duration::from_millis(20)).await;
					}
					()
				},
				async move {
					while let Some(msg) = rx.next().await {
						println!("rx'd one {} with {} backlogged", msg.val, rx.meter().read());
						Delay::new(Duration::from_millis(29)).await;
					}
				}
			)
		});
	}

	#[test]
	fn failed_send_does_not_inc_sent() {
		let (mut bounded, _) = channel::<Msg>(5);
		let (unbounded, _) = unbounded::<Msg>();

		block_on(async move {
			assert!(bounded.send(Msg::default()).await.is_err());
			assert!(bounded.try_send(Msg::default()).is_err());
			assert_matches!(bounded.meter().read(), Readout { sent: 0, received: 0, .. });

			assert!(unbounded.unbounded_send(Msg::default()).is_err());
			assert_matches!(unbounded.meter().read(), Readout { sent: 0, received: 0, .. });
		});
	}
}
