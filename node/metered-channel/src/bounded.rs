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

//! Metered variant of bounded mpsc channels to be able to extract metrics.

use futures::{
	channel::mpsc,
	sink::SinkExt,
	stream::Stream,
	task::{Context, Poll},
};

use std::{pin::Pin, result};

use super::{measure_tof_check, CoarseInstant, MaybeTimeOfFlight, Meter};

/// Create a wrapped `mpsc::channel` pair of `MeteredSender` and `MeteredReceiver`.
pub fn channel<T>(capacity: usize) -> (MeteredSender<T>, MeteredReceiver<T>) {
	let (tx, rx) = mpsc::channel::<MaybeTimeOfFlight<T>>(capacity);
	let shared_meter = Meter::default();
	let tx = MeteredSender { meter: shared_meter.clone(), inner: tx };
	let rx = MeteredReceiver { meter: shared_meter, inner: rx };
	(tx, rx)
}

/// A receiver tracking the messages consumed by itself.
#[derive(Debug)]
pub struct MeteredReceiver<T> {
	// count currently contained messages
	meter: Meter,
	inner: mpsc::Receiver<MaybeTimeOfFlight<T>>,
}

impl<T> std::ops::Deref for MeteredReceiver<T> {
	type Target = mpsc::Receiver<MaybeTimeOfFlight<T>>;
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
			Poll::Ready(maybe_value) => Poll::Ready(self.maybe_meter_tof(maybe_value)),
			Poll::Pending => Poll::Pending,
		}
	}

	/// Don't rely on the unreliable size hint.
	fn size_hint(&self) -> (usize, Option<usize>) {
		self.inner.size_hint()
	}
}

impl<T> MeteredReceiver<T> {
	fn maybe_meter_tof(&mut self, maybe_value: Option<MaybeTimeOfFlight<T>>) -> Option<T> {
		self.meter.note_received();
		maybe_value.map(|value| {
			match value {
				MaybeTimeOfFlight::<T>::WithTimeOfFlight(value, tof_start) => {
					// do not use `.elapsed()` of `std::time`, it may panic
					// `coarsetime` does a saturating sub for all `CoarseInstant` substractions
					let duration = tof_start.elapsed();
					self.meter.note_time_of_flight(duration);
					value
				},
				MaybeTimeOfFlight::<T>::Bare(value) => value,
			}
			.into()
		})
	}

	/// Get an updated accessor object for all metrics collected.
	pub fn meter(&self) -> &Meter {
		&self.meter
	}

	/// Attempt to receive the next item.
	pub fn try_next(&mut self) -> Result<Option<T>, mpsc::TryRecvError> {
		match self.inner.try_next()? {
			Some(value) => Ok(self.maybe_meter_tof(Some(value))),
			None => Ok(None),
		}
	}
}

impl<T> futures::stream::FusedStream for MeteredReceiver<T> {
	fn is_terminated(&self) -> bool {
		self.inner.is_terminated()
	}
}

/// The sender component, tracking the number of items
/// sent across it.
#[derive(Debug)]
pub struct MeteredSender<T> {
	meter: Meter,
	inner: mpsc::Sender<MaybeTimeOfFlight<T>>,
}

impl<T> Clone for MeteredSender<T> {
	fn clone(&self) -> Self {
		Self { meter: self.meter.clone(), inner: self.inner.clone() }
	}
}

impl<T> std::ops::Deref for MeteredSender<T> {
	type Target = mpsc::Sender<MaybeTimeOfFlight<T>>;
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
	fn prepare_with_tof(&self, item: T) -> MaybeTimeOfFlight<T> {
		let previous = self.meter.note_sent();
		let item = if measure_tof_check(previous) {
			MaybeTimeOfFlight::WithTimeOfFlight(item, CoarseInstant::now())
		} else {
			MaybeTimeOfFlight::Bare(item)
		};
		item
	}

	/// Get an updated accessor object for all metrics collected.
	pub fn meter(&self) -> &Meter {
		&self.meter
	}

	/// Send message, wait until capacity is available.
	pub async fn send(&mut self, msg: T) -> result::Result<(), mpsc::SendError>
	where
		Self: Unpin,
	{
		match self.try_send(msg) {
			Err(send_err) => {
				if !send_err.is_full() {
					return Err(send_err.into_send_error())
				}

				let msg = send_err.into_inner();
				self.meter.note_sent();
				let fut = self.inner.send(msg);
				futures::pin_mut!(fut);
				fut.await.map_err(|e| {
					self.meter.retract_sent();
					e
				})
			},
			_ => Ok(()),
		}
	}

	/// Attempt to send message or fail immediately.
	pub fn try_send(
		&mut self,
		msg: T,
	) -> result::Result<(), mpsc::TrySendError<MaybeTimeOfFlight<T>>> {
		let msg = self.prepare_with_tof(msg);
		self.inner.try_send(msg).map_err(|e| {
			if e.is_full() {
				// Count bounded channel sends that block.
				self.meter.note_blocked();
			}
			self.meter.retract_sent();
			e
		})
	}
}
