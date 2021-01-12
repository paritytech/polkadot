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

/// Create a wrapped `mpsc::channel` pair of `MeteredSender` and `MeteredReceiver`.
pub fn channel<T>(capacity: usize, name: &'static str) -> (MeteredSender<T>, MeteredReceiver<T>) {
	let (tx, rx) = mpsc::channel(capacity);
	let mut shared_meter = Meter::default();
	shared_meter.name = name;
	let tx = MeteredSender { meter: shared_meter.clone(), inner: tx };
	let rx = MeteredReceiver { meter: shared_meter, inner: rx };
	(tx, rx)
}

/// A receiver tracking the messages consumed by itself.
#[derive(Debug)]
pub struct MeteredReceiver<T> {
	// count currently contained messages
	meter: Meter,
	inner: mpsc::Receiver<T>,
}

impl<T> std::ops::Deref for MeteredReceiver<T> {
	type Target = mpsc::Receiver<T>;
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl<T> std::ops::DerefMut for MeteredReceiver<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.inner
	}
}

impl<T> Stream for MeteredReceiver<T> {
	type Item = T;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		match mpsc::Receiver::poll_next(Pin::new(&mut self.inner), cx) {
			Poll::Ready(x) => {
				// always use Ordering::SeqCst to avoid underflows
				self.meter.fill.fetch_sub(1, Ordering::SeqCst);
				Poll::Ready(x)
			}
			other => other,
		}
	}

	/// Don't rely on the unreliable size hint.
	fn size_hint(&self) -> (usize, Option<usize>) {
		self.inner.size_hint()
	}
}

impl<T> MeteredReceiver<T> {
	/// Get an updated accessor object for all metrics collected.
	pub fn meter(&self) -> &Meter {
		&self.meter
	}

	/// Attempt to receive the next item.
	pub fn try_next(&mut self) -> Result<Option<T>, mpsc::TryRecvError> {
		match self.inner.try_next()? {
			Some(x) => {
				self.meter.fill.fetch_sub(1, Ordering::SeqCst);
				Ok(Some(x))
			}
			None => Ok(None),
		}
	}
}

/// The sender component, tracking the number of items
/// sent across it.
#[derive(Debug)]
pub struct MeteredSender<T> {
	meter: Meter,
	inner: mpsc::Sender<T>,
}

impl<T> Clone for MeteredSender<T> {
	fn clone(&self) -> Self {
		Self { meter: self.meter.clone(), inner: self.inner.clone() }
	}
}

impl<T> std::ops::Deref for MeteredSender<T> {
	type Target = mpsc::Sender<T>;
	fn deref(&self) -> &Self::Target {
		&self.inner
	}
}

impl<T> std::ops::DerefMut for MeteredSender<T> {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.inner
	}
}

impl<T> MeteredSender<T> {
	/// Get an updated accessor object for all metrics collected.
	pub fn meter(&self) -> &Meter {
		&self.meter
	}

	/// Send message, wait until capacity is available.
	pub async fn send(&mut self, item: T) -> result::Result<(), mpsc::SendError>
	where
		Self: Unpin,
	{
		self.meter.fill.fetch_add(1, Ordering::SeqCst);
		let fut = self.inner.send(item);
		futures::pin_mut!(fut);
		fut.await
	}

	/// Attempt to send message or fail immediately.
	pub fn try_send(&mut self, msg: T) -> result::Result<(), mpsc::TrySendError<T>> {
		self.inner.try_send(msg)?;
		self.meter.fill.fetch_add(1, Ordering::SeqCst);
		Ok(())
	}
}

pub use unbounded::*;

mod unbounded {
	use super::*;

	/// Create a wrapped `mpsc::channel` pair of `MeteredSender` and `MeteredReceiver`.
	pub fn unbounded<T>(name: &'static str) -> (UnboundedMeteredSender<T>, UnboundedMeteredReceiver<T>) {
		let (tx, rx) = mpsc::unbounded();
		let mut shared_meter = Meter::default();
		shared_meter.name = name;
		let tx = UnboundedMeteredSender { meter: shared_meter.clone(), inner: tx };
		let rx = UnboundedMeteredReceiver { meter: shared_meter, inner: rx };
		(tx, rx)
	}

	/// A receiver tracking the messages consumed by itself.
	#[derive(Debug)]
	pub struct UnboundedMeteredReceiver<T> {
		// count currently contained messages
		meter: Meter,
		inner: mpsc::UnboundedReceiver<T>,
	}

	impl<T> std::ops::Deref for UnboundedMeteredReceiver<T> {
		type Target = mpsc::UnboundedReceiver<T>;
		fn deref(&self) -> &Self::Target {
			&self.inner
		}
	}

	impl<T> std::ops::DerefMut for UnboundedMeteredReceiver<T> {
		fn deref_mut(&mut self) -> &mut Self::Target {
			&mut self.inner
		}
	}

	impl<T> Stream for UnboundedMeteredReceiver<T> {
		type Item = T;
		fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
			match mpsc::UnboundedReceiver::poll_next(Pin::new(&mut self.inner), cx) {
				Poll::Ready(x) => {
					// always use Ordering::SeqCst to avoid underflows
					self.meter.fill.fetch_sub(1, Ordering::SeqCst);
					Poll::Ready(x)
				}
				other => other,
			}
		}

		/// Don't rely on the unreliable size hint.
		fn size_hint(&self) -> (usize, Option<usize>) {
			self.inner.size_hint()
		}
	}

	impl<T> UnboundedMeteredReceiver<T> {
		/// Get an updated accessor object for all metrics collected.
		pub fn meter(&self) -> &Meter {
			&self.meter
		}

		/// Attempt to receive the next item.
		pub fn try_next(&mut self) -> Result<Option<T>, mpsc::TryRecvError> {
			match self.inner.try_next()? {
				Some(x) => {
					self.meter.fill.fetch_sub(1, Ordering::SeqCst);
					Ok(Some(x))
				}
				None => Ok(None),
			}
		}
	}

	/// The sender component, tracking the number of items
	/// sent across it.
	#[derive(Debug)]
	pub struct UnboundedMeteredSender<T> {
		meter: Meter,
		inner: mpsc::UnboundedSender<T>,
	}

	impl<T> Clone for UnboundedMeteredSender<T> {
		fn clone(&self) -> Self {
			Self { meter: self.meter.clone(), inner: self.inner.clone() }
		}
	}

	impl<T> std::ops::Deref for UnboundedMeteredSender<T> {
		type Target = mpsc::UnboundedSender<T>;
		fn deref(&self) -> &Self::Target {
			&self.inner
		}
	}

	impl<T> std::ops::DerefMut for UnboundedMeteredSender<T> {
		fn deref_mut(&mut self) -> &mut Self::Target {
			&mut self.inner
		}
	}

	impl<T> UnboundedMeteredSender<T> {
		/// Get an updated accessor object for all metrics collected.
		pub fn meter(&self) -> &Meter {
			&self.meter
		}

		/// Send message, wait until capacity is available.
		pub async fn send(&mut self, item: T) -> result::Result<(), mpsc::SendError>
		where
			Self: Unpin,
		{
			self.meter.fill.fetch_add(1, Ordering::SeqCst);
			let fut = self.inner.send(item);
			futures::pin_mut!(fut);
			fut.await
		}
	}
}

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
				},
				async move {
					Delay::new(Duration::from_millis(100)).await;
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
