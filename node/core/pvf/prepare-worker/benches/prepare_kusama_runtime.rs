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

use criterion::{criterion_group, criterion_main, Criterion, SamplingMode};
use polkadot_node_core_pvf_common::{prepare::PrepareJobKind, pvf::PvfPrepData};
use polkadot_node_core_pvf_prepare_worker::{prepare, prevalidate};
use polkadot_primitives::ExecutorParams;
use std::time::Duration;

fn do_prepare_kusama_runtime(pvf: PvfPrepData) {
	let blob = match prevalidate(&pvf.code()) {
		Err(err) => panic!("{:?}", err),
		Ok(b) => b,
	};

	match prepare(blob, &pvf.executor_params()) {
		Ok(_) => (),
		Err(err) => panic!("{:?}", err),
	}
}

fn prepare_kusama_runtime(c: &mut Criterion) {
	let blob = kusama_runtime::WASM_BINARY.unwrap();
	let pvf = match sp_maybe_compressed_blob::decompress(&blob, 64 * 1024 * 1024) {
		Ok(code) => PvfPrepData::from_code(
			code.into_owned(),
			ExecutorParams::default(),
			Duration::from_secs(360),
			PrepareJobKind::Compilation,
		),
		Err(e) => {
			panic!("Cannot decompress blob: {:?}", e);
		},
	};

	let mut group = c.benchmark_group("kusama");
	group.sampling_mode(SamplingMode::Flat);
	group.sample_size(20);
	group.measurement_time(Duration::from_secs(240));
	group.bench_function("prepare Kusama runtime", |b| {
		b.iter(|| do_prepare_kusama_runtime(pvf.clone()))
	});
	group.finish();
}

criterion_group!(preparation, prepare_kusama_runtime);
criterion_main!(preparation);
