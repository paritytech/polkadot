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

//! Metered variant of unbounded mpsc channels to be able to extract metrics.

use futures::{
	channel::mpsc,
	stream::Stream,
	task::{Context, Poll},
};

use std::{pin::Pin, result};

use super::Meter;

/// Create a wrapped `mpsc::channel` pair of `MeteredSender` and `MeteredReceiver`.
pub fn unbounded<T>() -> (UnboundedMeteredSender<T>, UnboundedMeteredReceiver<T>) {
	let (tx, rx) = mpsc::unbounded();
	let shared_meter = Meter::default();
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
				self.meter.note_received();
				Poll::Ready(x)
			},
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
				self.meter.note_received();
				Ok(Some(x))
			},
			None => Ok(None),
		}
	}
}

impl<T> futures::stream::FusedStream for UnboundedMeteredReceiver<T> {
	fn is_terminated(&self) -> bool {
		self.inner.is_terminated()
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

	/// Attempt to send message or fail immediately.
	pub fn unbounded_send(&self, msg: T) -> result::Result<(), mpsc::TrySendError<T>> {
		self.meter.note_sent();
		self.inner.unbounded_send(msg).map_err(|e| {
			self.meter.retract_sent();
			e
		})
	}
}
