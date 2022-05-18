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
		assert_matches!(tx.meter().read(), Readout { sent: 4, received: 3, blocked: 0, tof } => {
			// every second in test, consumed before
			assert_eq!(dbg!(tof).len(), 1);
		});
		rx.try_next().unwrap();
		assert_matches!(rx.meter().read(), Readout { sent: 4, received: 4, blocked: 0, tof } => {
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

#[test]
fn blocked_send_is_metered() {
	let (mut bounded_sender, mut bounded_receiver) = channel::<Msg>(1);

	block_on(async move {
		assert!(bounded_sender.send(Msg::default()).await.is_ok());
		assert!(bounded_sender.send(Msg::default()).await.is_ok());
		assert!(bounded_sender.try_send(Msg::default()).is_err());

		assert_matches!(
			bounded_sender.meter().read(),
			Readout { sent: 2, received: 0, blocked: 1, .. }
		);
		bounded_receiver.try_next().unwrap();
		assert_matches!(
			bounded_receiver.meter().read(),
			Readout { sent: 2, received: 1, blocked: 1, .. }
		);
	});
}
