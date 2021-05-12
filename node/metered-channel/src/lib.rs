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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use derive_more::{Add, Display};

mod bounded;
mod unbounded;

pub use self::bounded::*;
pub use self::unbounded::*;

/// A peek into the inner state of a meter.
#[derive(Debug, Clone, Default)]
pub struct Meter {
	// Number of sends on this channel.
	sent: Arc<AtomicUsize>,
	// Number of receives on this channel.
	received: Arc<AtomicUsize>,
}

/// A readout of sizes from the meter. Note that it is possible, due to asynchrony, for received
/// to be slightly higher than sent.
#[derive(Debug, Add, Display, Clone, Default, PartialEq)]
#[display(fmt = "(sent={} received={})", sent, received)]
pub struct Readout {
	/// The amount of messages sent on the channel, in aggregate.
	pub sent: usize,
	/// The amount of messages received on the channel, in aggregate.
	pub received: usize,
}

impl Meter {
	/// Count the number of items queued up inside the channel.
	pub fn read(&self) -> Readout {
		// when obtaining we don't care much about off by one
		// accuracy
		Readout {
			sent: self.sent.load(Ordering::Relaxed),
			received: self.received.load(Ordering::Relaxed),
		}
	}

	fn note_sent(&self) {
		self.sent.fetch_add(1, Ordering::Relaxed);
	}

	fn retract_sent(&self) {
		self.sent.fetch_sub(1, Ordering::Relaxed);
	}

	fn note_received(&self) {
		self.received.fetch_add(1, Ordering::Relaxed);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor::block_on;
	use futures::StreamExt;

	#[derive(Clone, Copy, Debug, Default)]
	struct Msg {
		val: u8,
	}

	#[test]
	fn try_send_try_next() {
		block_on(async move {
			let (mut tx, mut rx) = channel::<Msg>(5);
			let msg = Msg::default();
			assert_eq!(rx.meter().read(), Readout { sent: 0, received: 0 });
			tx.try_send(msg).unwrap();
			assert_eq!(tx.meter().read(), Readout { sent: 1, received: 0 });
			tx.try_send(msg).unwrap();
			tx.try_send(msg).unwrap();
			tx.try_send(msg).unwrap();
			assert_eq!(tx.meter().read(), Readout { sent: 4, received: 0 });
			rx.try_next().unwrap();
			assert_eq!(rx.meter().read(), Readout { sent: 4, received: 1 });
			rx.try_next().unwrap();
			rx.try_next().unwrap();
			assert_eq!(tx.meter().read(), Readout { sent: 4, received: 3 });
			rx.try_next().unwrap();
			assert_eq!(rx.meter().read(), Readout { sent: 4, received: 4 });
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
					assert_eq!(tx.meter().read(), Readout { sent: 0, received: 0 });
					tx.try_send(msg).unwrap();
					assert_eq!(tx.meter().read(), Readout { sent: 1, received: 0 });
					tx.try_send(msg).unwrap();
					tx.try_send(msg).unwrap();
					tx.try_send(msg).unwrap();
					ready.send(()).expect("Helper oneshot channel must work. qed");
				},
				async move {
					go.await.expect("Helper oneshot channel must work. qed");
					assert_eq!(rx.meter().read(), Readout { sent: 4, received: 0 });
					rx.try_next().unwrap();
					assert_eq!(rx.meter().read(), Readout { sent: 4, received: 1 });
					rx.try_next().unwrap();
					rx.try_next().unwrap();
					assert_eq!(rx.meter().read(), Readout { sent: 4, received: 3 });
					rx.try_next().unwrap();
					assert_eq!(dbg!(rx.meter().read()), Readout { sent: 4, received: 4 });
				}
			)
		});
	}

	use std::time::Duration;
	use futures_timer::Delay;

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
		let (mut unbounded, _) = unbounded::<Msg>();

		block_on(async move {
			assert!(bounded.send(Msg::default()).await.is_err());
			assert!(bounded.try_send(Msg::default()).is_err());
			assert_eq!(bounded.meter().read(), Readout { sent: 0, received: 0 });

			assert!(unbounded.send(Msg::default()).await.is_err());
			assert!(unbounded.unbounded_send(Msg::default()).is_err());
			assert_eq!(unbounded.meter().read(), Readout { sent: 0, received: 0 });
		});
	}
}
