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

//! Metered variant of oneshot channels to be able to extract delays caused by delayed responses.

use std::{
	ops::Deref,
	pin::Pin,
	task::{Context, Poll},
	time::{Duration, Instant},
};

use futures::{
	channel::oneshot::{self, Canceled, Cancellation},
	future::{Fuse, FusedFuture},
	prelude::*,
};
use futures_timer::Delay;

/// Provides the reason for termination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
	Completion = 1,
	Cancellation = 2,
	HardTimeout = 3,
}

/// Obtained measurements by the `Receiver` side of the `MeteredOneshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measurements {
	/// Duration between first poll and polling termination.
	first_poll_till_end: Duration,
	/// Duration starting with creation until polling termination.
	creation_till_end: Duration,
	/// Reason for resolving the future.
	reason: Reason,
}

impl Measurements {
	/// Obtain the duration of a finished or canceled
	/// `oneshot` channel.
	pub fn duration_since_first_poll(&self) -> &Duration {
		&self.first_poll_till_end
	}

	/// Obtain the duration of a finished or canceled
	/// `oneshot` channel.
	pub fn duration_since_creation(&self) -> &Duration {
		&self.creation_till_end
	}

	/// Obtain the reason to the channel termination.
	pub fn reason(&self) -> &Reason {
		&self.reason
	}
}

/// Create a new pair of `OneshotMetered{Sender,Receiver}`.
pub fn channel<T>(
	name: &'static str,
	soft_timeout: Duration,
	hard_timeout: Duration,
) -> (MeteredSender<T>, MeteredReceiver<T>) {
	let (tx, rx) = oneshot::channel();

	(
		MeteredSender { name, inner: tx },
		MeteredReceiver {
			name,
			inner: rx,
			soft_timeout,
			hard_timeout,
			soft_timeout_fut: None,
			hard_timeout_fut: None,
			first_poll_timestamp: None,
			creation_timestamp: Instant::now(),
		},
	)
}

#[allow(missing_docs)]
#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("Oneshot was canceled.")]
	Canceled(#[source] Canceled, Measurements),
	#[error("Oneshot did not receive a response within {}", Duration::as_secs_f64(.0))]
	HardTimeout(Duration, Measurements),
}

impl Measurable for Error {
	fn measurements(&self) -> Measurements {
		match self {
			Self::Canceled(_, measurements) => measurements.clone(),
			Self::HardTimeout(_, measurements) => measurements.clone(),
		}
	}
}

/// Oneshot sender, created by [`channel`].
#[derive(Debug)]
pub struct MeteredSender<T> {
	name: &'static str,
	inner: oneshot::Sender<(Instant, T)>,
}

impl<T> MeteredSender<T> {
	/// Send a value.
	pub fn send(self, t: T) -> Result<(), T> {
		let Self { inner, name: _ } = self;
		inner.send((Instant::now(), t)).map_err(|(_, t)| t)
	}

	/// Poll if the thing is already canceled.
	pub fn poll_canceled(&mut self, ctx: &mut Context<'_>) -> Poll<()> {
		self.inner.poll_canceled(ctx)
	}

	/// Access the cancellation object.
	pub fn cancellation(&mut self) -> Cancellation<'_, (Instant, T)> {
		self.inner.cancellation()
	}

	/// Check the cancellation state.
	pub fn is_canceled(&self) -> bool {
		self.inner.is_canceled()
	}

	/// Verify if the `receiver` is connected to the `sender` [`Self`].
	pub fn is_connected_to(&self, receiver: &MeteredReceiver<T>) -> bool {
		self.inner.is_connected_to(&receiver.inner)
	}
}

/// Oneshot receiver, created by [`channel`].
#[derive(Debug)]
pub struct MeteredReceiver<T> {
	name: &'static str,
	inner: oneshot::Receiver<(Instant, T)>,
	/// Soft timeout, on expire a warning is printed.
	soft_timeout_fut: Option<Fuse<Delay>>,
	soft_timeout: Duration,
	/// Hard timeout, terminating the sender.
	hard_timeout_fut: Option<Delay>,
	hard_timeout: Duration,
	/// The first time the receiver was polled.
	first_poll_timestamp: Option<Instant>,
	creation_timestamp: Instant,
}

impl<T> MeteredReceiver<T> {
	pub fn close(&mut self) {
		self.inner.close()
	}

	/// Attempts to receive a message outside of the context of a task.
	///
	/// A return value of `None` must be considered immediately stale (out of
	/// date) unless [`close`](MeteredReceiver::close) has been called first.
	///
	/// Returns an error if the sender was dropped.
	pub fn try_recv(&mut self) -> Result<Option<OutputWithMeasurements<T>>, Error> {
		match self.inner.try_recv() {
			Ok(Some((when, value))) => {
				let measurements = self.create_measurement(when, Reason::Completion);
				Ok(Some(OutputWithMeasurements { value, measurements }))
			},
			Err(e) => {
				let measurements = self.create_measurement(
					self.first_poll_timestamp.unwrap_or_else(|| Instant::now()),
					Reason::Cancellation,
				);
				Err(Error::Canceled(e, measurements))
			},
			Ok(None) => Ok(None),
		}
	}

	/// Helper to create a measurement.
	///
	/// `start` determines the first possible time where poll can resolve with `Ready`.
	fn create_measurement(&self, start: Instant, reason: Reason) -> Measurements {
		let end = Instant::now();
		Measurements {
			// negative values are ok, if `send` was called before we poll for the first time.
			first_poll_till_end: end - start,
			creation_till_end: end - self.creation_timestamp,
			reason,
		}
	}
}

impl<T> FusedFuture for MeteredReceiver<T> {
	fn is_terminated(&self) -> bool {
		self.inner.is_terminated()
	}
}

impl<T> Future for MeteredReceiver<T> {
	type Output = Result<OutputWithMeasurements<T>, Error>;

	fn poll(
		mut self: Pin<&mut Self>,
		ctx: &mut Context<'_>,
	) -> Poll<Result<OutputWithMeasurements<T>, Error>> {
		let first_poll_timestamp =
			self.first_poll_timestamp.get_or_insert_with(|| Instant::now()).clone();

		let soft_timeout = self.soft_timeout.clone();
		let soft_timeout = self
			.soft_timeout_fut
			.get_or_insert_with(move || Delay::new(soft_timeout).fuse());

		if Pin::new(soft_timeout).poll(ctx).is_ready() {
			tracing::warn!("Oneshot `{name}` exceeded the soft threshold", name = &self.name);
		}

		let hard_timeout = self.hard_timeout.clone();
		let hard_timeout =
			self.hard_timeout_fut.get_or_insert_with(move || Delay::new(hard_timeout));

		if Pin::new(hard_timeout).poll(ctx).is_ready() {
			let measurements = self.create_measurement(first_poll_timestamp, Reason::HardTimeout);
			return Poll::Ready(Err(Error::HardTimeout(self.hard_timeout.clone(), measurements)))
		}

		match Pin::new(&mut self.inner).poll(ctx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(Err(e)) => {
				let measurements =
					self.create_measurement(first_poll_timestamp, Reason::Cancellation);
				Poll::Ready(Err(Error::Canceled(e, measurements)))
			},
			Poll::Ready(Ok((ref sent_at_timestamp, value))) => {
				let measurements =
					self.create_measurement(sent_at_timestamp.clone(), Reason::Completion);
				Poll::Ready(Ok(OutputWithMeasurements::<T> { value, measurements }))
			},
		}
	}
}

/// A dummy trait that allows implementing `measurements` for `Result<_,_>`.
pub trait Measurable {
	/// Obtain a set of measurements represented by the `Measurements` type.
	fn measurements(&self) -> Measurements;
}

impl<T> Measurable for Result<OutputWithMeasurements<T>, Error> {
	fn measurements(&self) -> Measurements {
		match self {
			Err(err) => err.measurements(),
			Ok(val) => val.measurements(),
		}
	}
}

/// A wrapping type for the actual type `T` that is sent with the
/// oneshot yet allow to attach `Measurements` to it.
///
/// Implements `AsRef` besides others for easier access to the inner,
/// wrapped type.
#[derive(Clone, Debug)]
pub struct OutputWithMeasurements<T> {
	value: T,
	measurements: Measurements,
}

impl<T> Measurable for OutputWithMeasurements<T> {
	fn measurements(&self) -> Measurements {
		self.measurements.clone()
	}
}

impl<T> OutputWithMeasurements<T> {
	/// Converts the wrapper type into it's inner value.
	///
	/// `trait Into` cannot be implemented due to conflicts.
	pub fn into(self) -> T {
		self.value
	}
}

impl<T> AsRef<T> for OutputWithMeasurements<T> {
	fn as_ref(&self) -> &T {
		&self.value
	}
}

impl<T> Deref for OutputWithMeasurements<T> {
	type Target = T;

	fn deref(&self) -> &Self::Target {
		&self.value
	}
}

#[cfg(test)]
mod tests {
	use assert_matches::assert_matches;
	use futures::{executor::ThreadPool, task::SpawnExt};

	use super::*;

	#[derive(Clone, PartialEq, Eq, Debug)]
	struct DummyItem {
		vals: [u8; 256],
	}

	impl Default for DummyItem {
		fn default() -> Self {
			Self { vals: [0u8; 256] }
		}
	}

	fn test_launch<S, R, FS, FR>(name: &'static str, gen_sender_test: S, gen_receiver_test: R)
	where
		S: Fn(MeteredSender<DummyItem>) -> FS,
		R: Fn(MeteredReceiver<DummyItem>) -> FR,
		FS: Future<Output = ()> + Send + 'static,
		FR: Future<Output = ()> + Send + 'static,
	{
		let _ = env_logger::builder().is_test(true).filter_level(LevelFilter::Trace).try_init();

		let pool = ThreadPool::new().unwrap();
		let (tx, rx) = channel(name, Duration::from_secs(1), Duration::from_secs(3));
		futures::executor::block_on(async move {
			let handle_receiver = pool.spawn_with_handle(gen_receiver_test(rx)).unwrap();
			let handle_sender = pool.spawn_with_handle(gen_sender_test(tx)).unwrap();
			futures::future::select(
				futures::future::join(handle_sender, handle_receiver),
				Delay::new(Duration::from_secs(5)),
			)
			.await;
		});
	}

	use log::LevelFilter;

	#[test]
	fn easy() {
		test_launch(
			"easy",
			|tx| async move {
				tx.send(DummyItem::default()).unwrap();
			},
			|rx| async move {
				let x = rx.await.unwrap();
				let measurements = x.measurements();
				assert_eq!(x.as_ref(), &DummyItem::default());
				dbg!(measurements);
			},
		);
	}

	#[test]
	fn cancel_by_drop() {
		test_launch(
			"cancel_by_drop",
			|tx| async move {
				Delay::new(Duration::from_secs(2)).await;
				drop(tx);
			},
			|rx| async move {
				let result = rx.await;
				assert_matches!(result, Err(Error::Canceled(_, _)));
				dbg!(result.measurements());
			},
		);
	}

	#[test]
	fn starve_till_hard_timeout() {
		test_launch(
			"starve_till_timeout",
			|tx| async move {
				Delay::new(Duration::from_secs(4)).await;
				let _ = tx.send(DummyItem::default());
			},
			|rx| async move {
				let result = rx.await;
				assert_matches!(&result, e @ &Err(Error::HardTimeout(_, _)) => {
					println!("{:?}", e);
				});
				dbg!(result.measurements());
			},
		);
	}

	#[test]
	fn starve_till_soft_timeout_then_food() {
		test_launch(
			"starve_till_soft_timeout_then_food",
			|tx| async move {
				Delay::new(Duration::from_secs(2)).await;
				let _ = tx.send(DummyItem::default());
			},
			|rx| async move {
				let result = rx.await;
				assert_matches!(result, Ok(_));
				dbg!(result.measurements());
			},
		);
	}
}
