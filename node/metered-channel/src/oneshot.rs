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
	convert::TryFrom,
	pin::Pin,
	sync::{
		atomic::{AtomicU64, AtomicU8, Ordering},
		Arc,
	},
	task::{Context, Poll},
	time::{Duration, Instant},
};

use futures::{
	channel::oneshot::{self, Canceled, Cancellation},
	future::{Fuse, FusedFuture},
	prelude::*,
};
use futures_timer::Delay;
use num_enum::{IntoPrimitive, TryFromPrimitive};

/// Provides the reason for termination, or [`Self::Unfinished`] if none exists.
#[derive(Debug, Clone, Copy, IntoPrimitive, TryFromPrimitive, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
	Unfinished = 0,
	Compeletion = 1,
	Cancellation = 2,
	HardTimeout = 3,
}

/// A peek into delay and state of a `oneshot` channel.
#[derive(Debug, Clone, Default)]
pub struct Meter {
	/// Delay between first poll and completion or cancellation.
	first_poll_till_end_in_millis: Arc<AtomicU64>,
	/// Determine the reason of completion.
	reason: Arc<AtomicU8>,
}

impl Meter {
	/// Obtain the duration of a finished or canceled
	/// `oneshot` channel.
	/// Returns `None` if the channel still exists.
	pub fn duration(&self) -> Option<Duration> {
		if self.reason() == Reason::Unfinished {
			None
		} else {
			let millis = self.first_poll_till_end_in_millis.load(Ordering::Acquire);
			Some(Duration::from_millis(millis))
		}
	}

	/// Obtain the reason to the channel termination.
	pub fn reason(&self) -> Reason {
		let reason = self.reason.load(Ordering::Acquire);
		Reason::try_from(reason).expect(
			"Input is originally a `Termination` before cast, hence recovery always works. qed",
		)
	}
}

/// Create a new pair of
pub fn channel<T>(
	name: &'static str,
	soft_timeout: Duration,
	hard_timeout: Duration,
) -> (OneshotMeteredSender<T>, OneshotMeteredReceiver<T>) {
	let (tx, rx) = oneshot::channel();
	let shared_meter = Meter::default();

	(
		OneshotMeteredSender { name, inner: tx, shared_meter: shared_meter.clone() },
		OneshotMeteredReceiver {
			name,
			inner: rx,
			soft_timeout,
			hard_timeout,
			soft_timeout_fut: None,
			hard_timeout_fut: None,
			first_poll_timestamp: None,
			shared_meter,
		},
	)
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error(transparent)]
	Canceled(#[from] Canceled),
	#[error("No response within {}", Duration::as_secs_f64(.0))]
	HardTimeout(Duration),
}

/// Oneshot sender, created by [`channel`].
#[derive(Debug)]
pub struct OneshotMeteredSender<T> {
	name: &'static str,
	inner: oneshot::Sender<(Instant, T)>,
	shared_meter: Meter,
}

impl<T> OneshotMeteredSender<T> {
	/// Send a value.
	pub fn send(self, t: T) -> Result<(), T> {
		let Self { inner, name: _, shared_meter: _ } = self;
		inner.send((Instant::now(), t)).map_err(|(_, t)| t)
	}

	/// Poll if the thing is already cancelled.
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
	pub fn is_connected_to(&self, receiver: &OneshotMeteredReceiver<T>) -> bool {
		self.inner.is_connected_to(&receiver.inner)
	}
}

/// Oneshot receiver, created by [`channel`].
#[derive(Debug)]
pub struct OneshotMeteredReceiver<T> {
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
	shared_meter: Meter,
}

impl<T> OneshotMeteredReceiver<T> {
	pub fn close(&mut self) {
		self.inner.close()
	}

	pub fn try_recv(&mut self) -> Result<Option<T>, Error> {
		match self.inner.try_recv() {
			Ok(Some((_when, val))) => Ok(Some(val)),
			Err(e) => Err(Error::from(e)),
			Ok(None) => Ok(None),
		}
	}
}

impl<T> OneshotMeteredReceiver<T> {
	pub fn meter(&self) -> Meter {
		self.shared_meter.clone()
	}

	fn update_meter(&mut self, sent_at_timestamp: Instant, reason: Reason) {
		let delta: Duration = sent_at_timestamp.elapsed();
		let val = u64::try_from(delta.as_millis()).unwrap_or(u64::MAX);
		self.shared_meter.first_poll_till_end_in_millis.store(val, Ordering::Release);

		self.shared_meter.reason.store(reason as u8, Ordering::Release);
	}
}

impl<T> FusedFuture for OneshotMeteredReceiver<T> {
	fn is_terminated(&self) -> bool {
		self.inner.is_terminated()
	}
}

impl<T> Future for OneshotMeteredReceiver<T> {
	type Output = Result<T, Error>;

	fn poll(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Result<T, Error>> {
		let mut me = self.as_mut();

		if me.first_poll_timestamp.is_none() {
			me.first_poll_timestamp = Some(Instant::now());
			me.soft_timeout_fut = Some(Delay::new(me.soft_timeout.clone()).fuse());
			me.hard_timeout_fut = Some(Delay::new(me.hard_timeout.clone()));
		}

		let soft_timeout = unsafe {
			self.as_mut().map_unchecked_mut(|this| this.soft_timeout_fut.as_mut().unwrap())
		};
		if soft_timeout.poll(ctx).is_ready() {
			tracing::warn!("Oneshot `{name}` exceeded the soft threshold", name = &self.name);
		}

		let hard_timeout = unsafe {
			self.as_mut().map_unchecked_mut(|this| this.hard_timeout_fut.as_mut().unwrap())
		};
		if hard_timeout.poll(ctx).is_ready() {
			let first_poll_timestamp = self.first_poll_timestamp.unwrap();
			self.update_meter(first_poll_timestamp, Reason::HardTimeout);
			return Poll::Ready(Err(Error::HardTimeout(self.hard_timeout.clone())))
		}

		match Pin::new(&mut self.inner).poll(ctx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(Err(e)) => {
				let first_poll_timestamp = self.first_poll_timestamp.unwrap();
				self.update_meter(first_poll_timestamp, Reason::Cancellation);
				Poll::Ready(Err(Error::from(e)))
			},
			Poll::Ready(Ok((sent_at_timestamp, val))) => {
				self.update_meter(sent_at_timestamp, Reason::Compeletion);
				Poll::Ready(Ok(val))
			},
		}
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
		S: Fn(OneshotMeteredSender<DummyItem>) -> FS,
		R: Fn(OneshotMeteredReceiver<DummyItem>) -> FR,
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
				let meter = dbg!(rx.meter());
				let x = rx.await.unwrap();
				assert_eq!(x, DummyItem::default());
				dbg!(meter);
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
				let meter = dbg!(rx.meter());
				assert_matches!(rx.await, Err(Error::Canceled(_)));
				dbg!(meter);
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
				let meter = dbg!(rx.meter());
				assert_matches!(rx.await, e @ Err(Error::HardTimeout(_)) => {
					println!("{:?}", e);
				});
				dbg!(meter);
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
				let meter = dbg!(rx.meter());
				assert_matches!(rx.await, Ok(_));
				dbg!(meter);
			},
		);
	}
}
