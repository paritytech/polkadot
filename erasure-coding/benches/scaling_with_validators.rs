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

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use polkadot_primitives::Hash;
use std::time::Duration;

fn chunks(n_validators: usize, pov: &Vec<u8>) -> Vec<Vec<u8>> {
	polkadot_erasure_coding::obtain_chunks(n_validators, pov).unwrap()
}

fn erasure_root(n_validators: usize, pov: &Vec<u8>) -> Hash {
	let chunks = chunks(n_validators, pov);
	polkadot_erasure_coding::branches(&chunks).root()
}

fn construct_and_reconstruct_5mb_pov(c: &mut Criterion) {
	const N_VALIDATORS: [usize; 6] = [200, 500, 1000, 2000, 10_000, 50_000];

	const KB: usize = 1024;
	const MB: usize = 1024 * KB;

	let pov = vec![0xfe; 5 * MB];

	let mut group = c.benchmark_group("construct");
	for n_validators in N_VALIDATORS {
		let expected_root = erasure_root(n_validators, &pov);

		group.throughput(Throughput::Bytes(pov.len() as u64));
		group.bench_with_input(
			BenchmarkId::from_parameter(n_validators),
			&n_validators,
			|b, &n| {
				b.iter(|| {
					let root = erasure_root(n, &pov);
					assert_eq!(root, expected_root);
				});
			},
		);
	}
	group.finish();

	let mut group = c.benchmark_group("reconstruct");
	for n_validators in N_VALIDATORS {
		let all_chunks = chunks(n_validators, &pov);

		let mut c: Vec<_> = all_chunks.iter().enumerate().map(|(i, c)| (&c[..], i)).collect();
		let last_chunks = c.split_off((c.len() - 1) * 2 / 3);

		group.throughput(Throughput::Bytes(pov.len() as u64));
		group.bench_with_input(
			BenchmarkId::from_parameter(n_validators),
			&n_validators,
			|b, &n| {
				b.iter(|| {
					let _pov: Vec<u8> =
						polkadot_erasure_coding::reconstruct(n, last_chunks.clone()).unwrap();
				});
			},
		);
	}
	group.finish();
}

fn criterion_config() -> Criterion {
	Criterion::default()
		.sample_size(15)
		.warm_up_time(Duration::from_millis(200))
		.measurement_time(Duration::from_secs(3))
}

criterion_group!(
	name = re_construct;
	config = criterion_config();
	targets = construct_and_reconstruct_5mb_pov,
);
criterion_main!(re_construct);
