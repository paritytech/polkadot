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
#[derive(Debug, StructOpt, Clone)]
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
#[derive(Debug, StructOpt, Clone)]
pub struct ValidationWorkerCommand {
	#[allow(missing_docs)]
	pub mem_id: String,
}

#[allow(missing_docs)]
#[derive(Debug, StructOpt, Clone)]
pub struct RunCmd {
	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub base: sc_cli::RunCmd,

	/// Force using Kusama native runtime.
	#[structopt(long = "force-kusama")]
	pub force_kusama: bool,
}

#[allow(missing_docs)]
#[derive(Debug, StructOpt, Clone)]
#[structopt(settings = &[
	structopt::clap::AppSettings::GlobalVersion,
	structopt::clap::AppSettings::ArgsNegateSubcommands,
	structopt::clap::AppSettings::SubcommandsNegateReqs,
])]
pub struct Cli {
	#[allow(missing_docs)]
	#[structopt(subcommand)]
	pub subcommand: Option<Subcommand>,

	#[allow(missing_docs)]
	#[structopt(flatten)]
	pub run: RunCmd,

	#[allow(missing_docs)]
	#[structopt(long = "enable-authority-discovery")]
	pub authority_discovery_enabled: bool,

	/// Setup a GRANDPA scheduled voting pause.
	///
	/// This parameter takes two values, namely a block number and a delay (in
	/// blocks). After the given block number is finalized the GRANDPA voter
	/// will temporarily stop voting for new blocks until the given delay has
	/// elapsed (i.e. until a block at height `pause_block + delay` is imported).
	#[structopt(long = "grandpa-pause", number_of_values(2))]
	pub grandpa_pause: Vec<u32>,
}
