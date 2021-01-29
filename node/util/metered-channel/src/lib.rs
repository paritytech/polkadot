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

use futures::{channel::mpsc, task::Poll, task::Context, sink::SinkExt, stream::Stream};

use std::result;
use std::sync::Arc;
use std::pin::Pin;

mod bounded;
mod unbounded;

pub use self::bounded::*;
pub use self::unbounded::*;

/// A peek into the inner state of a meter.
#[derive(Debug, Clone, Default)]
pub struct Meter {
	/// Name of the receiver and sender pair.
	name: &'static str,
	// fill state of the channel
	fill: Arc<AtomicUsize>,
}

impl Meter {
	/// Count the number of items queued up inside the channel.
	pub fn queue_count(&self) -> usize {
		// when obtaining we don't care much about off by one
		// accuracy
		self.fill.load(Ordering::Relaxed)
	}

	/// Obtain the name of the channel `Sender` and `Receiver` pair.
	pub fn name(&self) -> &'static str {
		self.name
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
			let (mut tx, mut rx) = channel::<Msg>(5, "goofy");
			let msg = Msg::default();
			assert_eq!(rx.meter().queue_count(), 0);
			tx.try_send(msg).unwrap();
			assert_eq!(tx.meter().queue_count(), 1);
			tx.try_send(msg).unwrap();
			tx.try_send(msg).unwrap();
			tx.try_send(msg).unwrap();
			assert_eq!(tx.meter().queue_count(), 4);
			rx.try_next().unwrap();
			assert_eq!(rx.meter().queue_count(), 3);
			rx.try_next().unwrap();
			rx.try_next().unwrap();
			assert_eq!(tx.meter().queue_count(), 1);
			rx.try_next().unwrap();
			assert_eq!(rx.meter().queue_count(), 0);
			assert!(rx.try_next().is_err());
		});
	}

	#[test]
	fn with_tasks() {
		let (ready, go) = futures::channel::oneshot::channel();

		let (mut tx, mut rx) = channel::<Msg>(5, "goofy");
		block_on(async move {
			futures::join!(
				async move {
					let msg = Msg::default();
					assert_eq!(tx.meter().queue_count(), 0);
					tx.try_send(msg).unwrap();
					assert_eq!(tx.meter().queue_count(), 1);
					tx.try_send(msg).unwrap();
					tx.try_send(msg).unwrap();
					tx.try_send(msg).unwrap();
					ready.send(()).expect("Helper oneshot channel must work. qed");
				},
				async move {
					go.await.expect("Helper oneshot channel must work. qed");
					assert_eq!(rx.meter().queue_count(), 4);
					rx.try_next().unwrap();
					assert_eq!(rx.meter().queue_count(), 3);
					rx.try_next().unwrap();
					rx.try_next().unwrap();
					assert_eq!(rx.meter().queue_count(), 1);
					rx.try_next().unwrap();
					assert_eq!(dbg!(rx.meter().queue_count()), 0);
				}
			)
		});
	}

	use std::time::Duration;
	use futures_timer::Delay;

	#[test]
	fn stream_and_sink() {
		let (mut tx, mut rx) = channel::<Msg>(5, "goofy");

		block_on(async move {
			futures::join!(
				async move {
					for i in 0..15 {
						println!("Sent #{} with a backlog of {} items", i + 1, tx.meter().queue_count());
						let msg = Msg { val: i as u8 + 1u8 };
						tx.send(msg).await.unwrap();
						assert!(tx.meter().queue_count() > 0usize);
						Delay::new(Duration::from_millis(20)).await;
					}
					()
				},
				async move {
					while let Some(msg) = rx.next().await {
						println!("rx'd one {} with {} backlogged", msg.val, rx.meter().queue_count());
						Delay::new(Duration::from_millis(29)).await;
					}
				}
			)
		});
	}
}
