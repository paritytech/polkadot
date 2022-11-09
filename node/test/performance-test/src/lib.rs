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

//! A Polkadot performance tests utilities.

use polkadot_erasure_coding::{obtain_chunks, reconstruct};
use polkadot_node_core_pvf::{sc_executor_common, sp_maybe_compressed_blob};
use std::time::{Duration, Instant};

mod constants;

pub use constants::*;
pub use polkadot_node_primitives::VALIDATION_CODE_BOMB_LIMIT;

/// Value used for reference benchmark of erasure-coding.
pub const ERASURE_CODING_N_VALIDATORS: usize = 1024;

pub use kusama_runtime::WASM_BINARY;

#[allow(missing_docs)]
#[derive(thiserror::Error, Debug)]
pub enum PerfCheckError {
	#[error("This subcommand is only available in release mode")]
	WrongBuildType,

	#[error("This subcommand is only available when compiled with `{feature}`")]
	FeatureNotEnabled { feature: &'static str },

	#[error("No wasm code found for running the performance test")]
	WasmBinaryMissing,

	#[error("Failed to decompress wasm code")]
	CodeDecompressionFailed,

	#[error(transparent)]
	Wasm(#[from] sc_executor_common::error::WasmError),

	#[error(transparent)]
	ErasureCoding(#[from] polkadot_erasure_coding::Error),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(
		"Performance check not passed: exceeded the {limit:?} time limit, elapsed: {elapsed:?}"
	)]
	TimeOut { elapsed: Duration, limit: Duration },
}

/// Measures the time it takes to compile arbitrary wasm code.
pub fn measure_pvf_prepare(wasm_code: &[u8]) -> Result<Duration, PerfCheckError> {
	let start = Instant::now();

	let code = sp_maybe_compressed_blob::decompress(wasm_code, VALIDATION_CODE_BOMB_LIMIT)
		.or(Err(PerfCheckError::CodeDecompressionFailed))?;

	// Recreate the pipeline from the pvf prepare worker.
	let blob = polkadot_node_core_pvf::prevalidate(code.as_ref()).map_err(PerfCheckError::from)?;
	polkadot_node_core_pvf::prepare(blob).map_err(PerfCheckError::from)?;

	Ok(start.elapsed())
}

/// Measure the time it takes to break arbitrary data into chunks and reconstruct it back.
pub fn measure_erasure_coding(
	n_validators: usize,
	data: &[u8],
) -> Result<Duration, PerfCheckError> {
	let start = Instant::now();

	let chunks = obtain_chunks(n_validators, &data)?;
	let indexed_chunks = chunks.iter().enumerate().map(|(i, chunk)| (chunk.as_slice(), i));

	let _: Vec<u8> = reconstruct(n_validators, indexed_chunks)?;

	Ok(start.elapsed())
}
