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
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use merlin::Transcript;
use schnorrkel::{
	vrf::{VRFOutput, VRFProof},
	Keypair,
};
use std::time::Duration;

use futures::{channel::oneshot, Stream, StreamExt, TryStreamExt};

fn vrf_sign(keypair: &Keypair, transcript: Transcript, extra: Transcript) -> (VRFOutput, VRFProof) {
	let (vrf_in_out, vrf_proof, _) = keypair.vrf_sign_extra(transcript, extra);

	(vrf_in_out.to_output(), vrf_proof)
}

fn time_1000_vrf_signatures(c: &mut Criterion) {
	let mut group = c.benchmark_group("check");
	let mut transcript = Transcript::new(b"dummy");
	transcript.append_message(b"label", b"message");

	let mut extra = Transcript::new(b"extra");
	extra.append_message(b"label", b"message");

	let mut prng = rand_core::OsRng;
	let keypair = schnorrkel::Keypair::generate_with(&mut prng);

	let (output, proof) = vrf_sign(&keypair, transcript.clone(), extra.clone());

	group.throughput(Throughput::Elements(1000));

	group.bench_with_input(BenchmarkId::from_parameter("no-pool"), &keypair, |b, _n| {
		let public = keypair.public;
		let mut responses = Vec::new();
		b.iter(|| {
			for _ in 0..1000 {
				let mut transcript = Transcript::new(b"dummy");
				transcript.append_message(b"label", b"message");

				let mut extra = Transcript::new(b"extra");
				extra.append_message(b"label", b"message");
				let output = output.clone();
				let proof = proof.clone();

				let (vrf_in_out, _) =
					public.vrf_verify_extra(transcript, &output, &proof, extra).unwrap();
				responses.push(vrf_in_out);
			}
		});
	});

	group.finish();
}

fn time_1000_vrf_signatures_pool(c: &mut Criterion) {
	let mut group = c.benchmark_group("check");
	let mut transcript = Transcript::new(b"dummy");
	transcript.append_message(b"label", b"message");

	let mut extra = Transcript::new(b"extra");
	extra.append_message(b"label", b"message");

	let mut prng = rand_core::OsRng;
	let keypair = schnorrkel::Keypair::generate_with(&mut prng);

	let (output, proof) = vrf_sign(&keypair, transcript.clone(), extra.clone());

	for i in 0..5 {
		group.throughput(Throughput::Elements(1000));

		group.bench_with_input(
			BenchmarkId::from_parameter(format!("pool_size_{}", 2u32.pow(i))),
			&keypair,
			|b, _n| {
				let public = keypair.public;
				b.iter(|| {
					let mut futures = futures::stream::FuturesUnordered::new();
					let proof = proof.clone();

					let main_runtime = tokio::runtime::Runtime::new().unwrap();
					let cpu_pool = tokio::runtime::Builder::new_multi_thread()
						.max_blocking_threads(2usize.pow(i))
						.build()
						.unwrap();
					let cpu_pool = cpu_pool.handle().clone();

					main_runtime.block_on(async move {
						for _ in 0..1000 {
							let mut transcript = Transcript::new(b"dummy");
							transcript.append_message(b"label", b"message");

							let mut extra = Transcript::new(b"extra");
							extra.append_message(b"label", b"message");
							let (tx, rx) = oneshot::channel();
							let output = output.clone();
							let proof = proof.clone();

							cpu_pool.spawn_blocking(move || {
								let (vrf_in_out, _) = public
									.vrf_verify_extra(transcript, &output, &proof, extra)
									.unwrap();
								let _ = tx.send(vrf_in_out);
							});
							futures.push(rx);
						}

						while !futures.is_empty() {
							futures::select! {
								m = futures.select_next_some() => {
									match m {
										Ok(vrf_in_out) => {

										},
										Err(err) => panic!("Err {:?}", err)
									}
								}
							}
						}
					});
				});
			},
		);
	}

	group.finish();
}

fn criterion_config() -> Criterion {
	Criterion::default()
		.sample_size(10)
		.warm_up_time(Duration::from_millis(3000))
		.measurement_time(Duration::from_secs(30))
}

criterion_group!(
	name = check_1000_vrf_signatures;
	config = criterion_config();
	targets = time_1000_vrf_signatures, time_1000_vrf_signatures_pool
);
criterion_main!(check_1000_vrf_signatures);
