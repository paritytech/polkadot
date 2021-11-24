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

use polkadot_node_core_pvf::sc_executor_common;
use std::time::Duration;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error(transparent)]
	PolkadotService(#[from] service::Error),

	#[error(transparent)]
	SubstrateCli(#[from] sc_cli::Error),

	#[error(transparent)]
	SubstrateService(#[from] sc_service::Error),

	#[error(transparent)]
	PerfCheck(#[from] PerfCheckError),

	#[error("Other: {0}")]
	Other(String),
}

#[allow(missing_docs)]
#[derive(thiserror::Error, Debug)]
pub enum PerfCheckError {
	#[error("This subcommand is only available in release mode")]
	WrongBuildType,

	#[error("No wasm code found for running the performance test")]
	WasmBinaryMissing,

	#[error("Failed to decompress wasm code")]
	CodeDecompressionFailed,

	#[error(transparent)]
	Wasm(#[from] sc_executor_common::error::WasmError),

	#[error(
		"Performance check not passed: exceeded the {limit:?} time limit, elapsed: {elapsed:?}"
	)]
	TimeOut { elapsed: Duration, limit: Duration },
}
