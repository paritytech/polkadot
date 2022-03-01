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

use clap::{ArgEnum, Parser};
use color_eyre::eyre;
use polkadot_cli::{Cli, RunCmd};

pub(crate) mod interceptor;
pub(crate) mod shared;

mod variants;

use variants::*;

/// Define the different variants of behavior.
#[derive(Debug, Parser)]
#[clap(about = "Malus - the nemesis of polkadot.", version)]
#[clap(rename_all = "kebab-case")]
enum NemesisVariant {
	/// Suggest a candidate with an invalid proof of validity.
	SuggestGarbageCandidate(RunCmd),
	/// Back a candidate with a specifically crafted proof of validity.
	BackGarbageCandidate(RunCmd),
	/// Delayed disputing of ancestors that are perfectly fine.
	DisputeAncestor(RunCmd),

	#[allow(missing_docs)]
	#[clap(name = "prepare-worker", hide = true)]
	PvfPrepareWorker(polkadot_cli::ValidationWorkerCommand),

	#[allow(missing_docs)]
	#[clap(name = "execute-worker", hide = true)]
	PvfExecuteWorker(polkadot_cli::ValidationWorkerCommand),
}

#[derive(ArgEnum, Clone, Debug, PartialEq)]
#[clap(
	about = "Fake candidate validation - forces all `ValidateCandidate*` calls to return a configured result.",
	version
)]
#[clap(rename_all = "kebab-case")]
pub enum FakeCandidateValidation {
	Invalid,
	// TODO: impl Valid.
}

#[derive(Debug, Parser)]
#[allow(missing_docs)]
struct MalusCli {
	#[clap(subcommand)]
	pub variant: NemesisVariant,

	#[clap(long, arg_enum, ignore_case = true)]
	pub fake_backing_validation: Option<FakeCandidateValidation>,

	#[clap(long, arg_enum, ignore_case = true)]
	pub fake_approval_validation: Option<FakeCandidateValidation>,
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
			NemesisVariant::DisputeAncestor(cmd) => polkadot_cli::run_node(
				run_cmd(cmd),
				DisputeValidCandidates::new(
					self.fake_backing_validation,
					self.fake_approval_validation,
				),
			)?,
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
	let cli = MalusCli::parse();
	cli.launch()?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn subcommand_works() {
		let cli = MalusCli::try_parse_from(IntoIterator::into_iter([
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
