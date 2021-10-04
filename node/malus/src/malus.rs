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

//! A malus or nemesis node launch code.

use color_eyre::eyre;
use polkadot_cli::{Cli, RunCmd};
use structopt::StructOpt;

pub(crate) mod interceptor;
pub(crate) mod shared;

mod variants;

use variants::*;

/// Define the different variants of behavior.
#[derive(Debug, StructOpt)]
#[structopt(about = "Malus - the nemesis of polkadot.")]
#[structopt(rename_all = "kebab-case")]
enum NemesisVariant {
	/// Suggest a candidate with an invalid proof of validity.
	SuggestGarbageCandidate(RunCmd),
	/// Back a candidate with a specifically crafted proof of validity.
	BackGarbageCandidate(RunCmd),
	/// Delayed disputing of ancestors that are perfectly fine.
	DisputeAncestor(RunCmd),

	#[allow(missing_docs)]
	#[structopt(name = "prepare-worker", setting = structopt::clap::AppSettings::Hidden)]
	PvfPrepareWorker(polkadot_cli::ValidationWorkerCommand),

	#[allow(missing_docs)]
	#[structopt(name = "execute-worker", setting = structopt::clap::AppSettings::Hidden)]
	PvfExecuteWorker(polkadot_cli::ValidationWorkerCommand),
}

#[derive(Debug, StructOpt)]
#[allow(missing_docs)]
struct MalusCli {
	#[structopt(subcommand)]
	pub variant: NemesisVariant,
}

fn run_cmd(run: RunCmd) -> Cli {
	Cli { subcommand: None, run }
}

impl MalusCli {
	/// Launch a malus node.
	fn launch(self) -> eyre::Result<()> {
		match self.variant {
			NemesisVariant::BackGarbageCandidate(cmd) =>
				polkadot_cli::run_node(run_cmd(cmd), BackGarbageCandidate)?,
			NemesisVariant::SuggestGarbageCandidate(cmd) =>
				polkadot_cli::run_node(run_cmd(cmd), SuggestGarbageCandidate)?,
			NemesisVariant::DisputeAncestor(cmd) =>
				polkadot_cli::run_node(run_cmd(cmd), DisputeValidCandidates)?,
			NemesisVariant::PvfPrepareWorker(cmd) => {
				#[cfg(target_os = "android")]
				{
					return Err("PVF preparation workers are not supported under this platform")
						.into()
				}

				#[cfg(not(target_os = "android"))]
				{
					polkadot_node_core_pvf::prepare_worker_entrypoint(&cmd.socket_path);
				}
			},
			NemesisVariant::PvfExecuteWorker(cmd) => {
				#[cfg(target_os = "android")]
				{
					return Err("PVF execution workers are not supported under this platform").into()
				}

				#[cfg(not(target_os = "android"))]
				{
					polkadot_node_core_pvf::execute_worker_entrypoint(&cmd.socket_path);
				}
			},
		}
		Ok(())
	}
}

fn main() -> eyre::Result<()> {
	color_eyre::install()?;
	let cli = MalusCli::from_args();
	cli.launch()?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn subcommand_works() {
		let cli = MalusCli::from_iter_safe(IntoIterator::into_iter([
			"malus",
			"dispute-ancestor",
			"--bob",
		]))
		.unwrap();
		assert_matches::assert_matches!(cli, MalusCli {
			variant: NemesisVariant::DisputeAncestor(run),
			..
		} => {
			assert!(run.base.bob);
		});
	}
}
