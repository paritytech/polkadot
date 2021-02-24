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
	/// Build a chain specification.
	BuildSpec(sc_cli::BuildSpecCmd),

	/// Validate blocks.
	CheckBlock(sc_cli::CheckBlockCmd),

	/// Export blocks.
	ExportBlocks(sc_cli::ExportBlocksCmd),

	/// Export the state of a given block into a chain spec.
	ExportState(sc_cli::ExportStateCmd),

	/// Import blocks.
	ImportBlocks(sc_cli::ImportBlocksCmd),

	/// Remove the whole chain.
	PurgeChain(sc_cli::PurgeChainCmd),

	/// Revert the chain to a previous state.
	Revert(sc_cli::RevertCmd),

	#[allow(missing_docs)]
	#[structopt(name = "validation-worker", setting = structopt::clap::AppSettings::Hidden)]
	ValidationWorker(ValidationWorkerCommand),

	/// The custom benchmark subcommand benchmarking runtime pallets.
	#[structopt(
		name = "benchmark",
		about = "Benchmark runtime pallets."
	)]
	Benchmark(frame_benchmarking_cli::BenchmarkCmd),

	/// Testing subcommand for runtime testing and trying.
	#[cfg(feature = "try-runtime")]
	TryRuntime(try_runtime_cli::TryRuntimeCmd),

	/// Key management cli utilities
	Key(sc_cli::KeySubcommand),
}

#[allow(missing_docs)]
#[derive(Debug, StructOpt)]
pub struct ValidationWorkerCommand {
	/// The path that the executor can use for its caching purposes.
	pub cache_base_path: std::path::PathBuf,

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

	/// Force using Rococo native runtime.
	#[structopt(long = "force-rococo")]
	pub force_rococo: bool,

	/// Setup a GRANDPA scheduled voting pause.
	///
	/// This parameter takes two values, namely a block number and a delay (in
	/// blocks). After the given block number is finalized the GRANDPA voter
	/// will temporarily stop voting for new blocks until the given delay has
	/// elapsed (i.e. until a block at height `pause_block + delay` is imported).
	#[structopt(long = "grandpa-pause", number_of_values(2))]
	pub grandpa_pause: Vec<u32>,

	/// Add the destination address to the jaeger agent.
	///
	/// Must be valid socket address, of format `IP:Port`
	/// commonly `127.0.0.1:6831`.
	#[structopt(long)]
	pub jaeger_agent: Option<std::net::SocketAddr>,
}

#[allow(missing_docs)]
#[derive(Debug, StructOpt)]
pub struct Cli {
	#[structopt(subcommand)]
	pub subcommand: Option<Subcommand>,
	#[structopt(flatten)]
	pub run: RunCmd,
}
