// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot CLI library.

use structopt::StructOpt;

#[allow(missing_docs)]
#[derive(Debug, StructOpt)]
pub enum Subcommand {
	#[allow(missing_docs)]
	#[structopt(flatten)]
	Base(sc_cli::Subcommand),

	#[allow(missing_docs)]
	#[structopt(name = "validation-worker", setting = structopt::clap::AppSettings::Hidden)]
	ValidationWorker(ValidationWorkerCommand),

	/// The custom benchmark subcommmand benchmarking runtime pallets.
	#[structopt(
		name = "benchmark",
		about = "Benchmark runtime pallets."
	)]
	Benchmark(frame_benchmarking_cli::BenchmarkCmd),
}

#[allow(missing_docs)]
#[derive(Debug, StructOpt)]
pub struct ValidationWorkerCommand {
	#[allow(missing_docs)]
	pub mem_id: String,
}

#[allow(missing_docs)]
#[derive(Debug, StructOpt)]
pub struct RunCmd {
	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub base: sc_cli::RunCmd,

	/// Force using Kusama native runtime.
	#[structopt(long = "force-kusama")]
	pub force_kusama: bool,

	/// Force using Westend native runtime.
	#[structopt(long = "force-westend")]
	pub force_westend: bool,

	/// Disable the authority discovery module on validator or sentry nodes.
	///
	/// Enabled by default on validator and sentry nodes. Always disabled on
	/// non validator or sentry nodes.
	///
	/// When enabled:
	///
	/// (1) As a validator node: Make oneself discoverable by publishing either
	///     ones own network addresses, or the ones of ones sentry nodes
	///     (configured via the `sentry-nodes` flag).
	///
	/// (2) As a validator or sentry node: Discover addresses of validators or
	///     addresses of their sentry nodes and maintain a permanent connection
	///     to a subset.
	#[structopt(long = "disable-authority-discovery")]
	pub authority_discovery_disabled: bool,

	/// Setup a GRANDPA scheduled voting pause.
	///
	/// This parameter takes two values, namely a block number and a delay (in
	/// blocks). After the given block number is finalized the GRANDPA voter
	/// will temporarily stop voting for new blocks until the given delay has
	/// elapsed (i.e. until a block at height `pause_block + delay` is imported).
	#[structopt(long = "grandpa-pause", number_of_values(2))]
	pub grandpa_pause: Vec<u32>,
}

#[allow(missing_docs)]
#[derive(Debug, StructOpt)]
pub struct Cli {
	#[structopt(subcommand)]
	pub subcommand: Option<Subcommand>,

	#[structopt(flatten)]
	pub run: RunCmd,
}
