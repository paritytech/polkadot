// Copyright (C) Parity Technologies (UK) Ltd.
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

#![cfg(test)]

use super::*;
use executor::block_on;
use futures::{channel::mpsc, executor, FutureExt, SinkExt, StreamExt};
use polkadot_primitives_test_helpers::AlwaysZeroRng;
use std::{
	sync::{
		atomic::{AtomicUsize, Ordering},
		Arc,
	},
	time::Duration,
};

#[test]
fn tick_tack_metronome() {
	let n = Arc::new(AtomicUsize::default());

	let (tick, mut block) = mpsc::unbounded();

	let metronome = {
		let n = n.clone();
		let stream = Metronome::new(Duration::from_millis(137_u64));
		stream
			.for_each(move |_res| {
				let _ = n.fetch_add(1, Ordering::Relaxed);
				let mut tick = tick.clone();
				async move {
					tick.send(()).await.expect("Test helper channel works. qed");
				}
			})
			.fuse()
	};

	let f2 = async move {
		block.next().await;
		assert_eq!(n.load(Ordering::Relaxed), 1_usize);
		block.next().await;
		assert_eq!(n.load(Ordering::Relaxed), 2_usize);
		block.next().await;
		assert_eq!(n.load(Ordering::Relaxed), 3_usize);
		block.next().await;
		assert_eq!(n.load(Ordering::Relaxed), 4_usize);
	}
	.fuse();

	futures::pin_mut!(f2);
	futures::pin_mut!(metronome);

	block_on(async move {
		// futures::join!(metronome, f2)
		futures::select!(
			_ = metronome => unreachable!("Metronome never stops. qed"),
			_ = f2 => (),
		)
	});
}

#[test]
fn subset_generation_check() {
	let mut values = (0_u8..=25).collect::<Vec<_>>();
	// 12 even numbers exist
	choose_random_subset::<u8, _>(|v| v & 0x01 == 0, &mut values, 12);
	values.sort();
	for (idx, v) in dbg!(values).into_iter().enumerate() {
		assert_eq!(v as usize, idx * 2);
	}
}

#[test]
fn subset_predefined_generation_check() {
	let mut values = (0_u8..=25).collect::<Vec<_>>();
	choose_random_subset_with_rng::<u8, _, _>(|_| false, &mut values, &mut AlwaysZeroRng, 12);
	assert_eq!(values.len(), 12);
	for (idx, v) in dbg!(values).into_iter().enumerate() {
		// Since shuffle actually shuffles the indexes from 1..len, then
		// our PRG that returns zeroes will shuffle 0 and 1, 1 and 2, ... len-2 and len-1
		assert_eq!(v as usize, idx + 1);
	}
}

#[test]
fn frequent_at_third_time() {
	let mut timestamps: Vec<u64> = Vec::with_capacity(MAX_FREQUENCY_TIMESTAMPS_SIZE);
	let rate = 1.0;

	assert!(!is_frequent(&mut timestamps, rate));
	assert!(!is_frequent(&mut timestamps, rate));

	assert!(is_frequent(&mut timestamps, rate));
}

#[test]
fn not_frequent_at_third_time_if_slow() {
	let mut timestamps: Vec<u64> = Vec::with_capacity(MAX_FREQUENCY_TIMESTAMPS_SIZE);
	let rate = 1000.0;

	assert!(!is_frequent(&mut timestamps, rate));
	assert!(!is_frequent(&mut timestamps, rate));

	std::thread::sleep(Duration::from_millis(10));
	assert!(!is_frequent(&mut timestamps, rate));
}

#[test]
fn keeps_only_last_records() {
	let mut timestamps: Vec<u64> = Vec::with_capacity(MAX_FREQUENCY_TIMESTAMPS_SIZE);
	let rate = 1.0;

	for _ in 0..(2 * MAX_FREQUENCY_TIMESTAMPS_SIZE) {
		let _ = is_frequent(&mut timestamps, rate);
	}

	assert!(timestamps.len() == MAX_FREQUENCY_TIMESTAMPS_SIZE);
}
