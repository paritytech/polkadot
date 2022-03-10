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
use polkadot_node_primitives::InvalidCandidate;

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
	DisputeAncestor(DisputeAncestorOptions),

	#[allow(missing_docs)]
	#[clap(name = "prepare-worker", hide = true)]
	PvfPrepareWorker(polkadot_cli::ValidationWorkerCommand),

	#[allow(missing_docs)]
	#[clap(name = "execute-worker", hide = true)]
	PvfExecuteWorker(polkadot_cli::ValidationWorkerCommand),
}

#[derive(ArgEnum, Clone, Debug, PartialEq)]
#[clap(rename_all = "kebab-case")]
pub enum FakeCandidateValidation {
	Disabled,
	BackingInvalid,
	ApprovalInvalid,
	BackingAndApprovalInvalid,
	// TODO: impl Valid.
}

/// Candidate invalidity details
#[derive(ArgEnum, Clone, Debug, PartialEq)]
#[clap(rename_all = "kebab-case")]
pub enum FakeCandidateValidationError {
	/// Validation outputs check doesn't pass.
	InvalidOutputs,
	/// Failed to execute.`validate_block`. This includes function panicking.
	ExecutionError,
	/// Execution timeout.
	Timeout,
	/// Validation input is over the limit.
	ParamsTooLarge,
	/// Code size is over the limit.
	CodeTooLarge,
	/// Code does not decompress correctly.
	CodeDecompressionFailure,
	/// PoV does not decompress correctly.
	PoVDecompressionFailure,
	/// Validation function returned invalid data.
	BadReturn,
	/// Invalid relay chain parent.
	BadParent,
	/// POV hash does not match.
	PoVHashMismatch,
	/// Bad collator signature.
	BadSignature,
	/// Para head hash does not match.
	ParaHeadHashMismatch,
	/// Validation code hash does not match.
	CodeHashMismatch,
}

impl Into<InvalidCandidate> for FakeCandidateValidationError {
	fn into(self) -> InvalidCandidate {
		match self {
			FakeCandidateValidationError::ExecutionError =>
				InvalidCandidate::ExecutionError("Malus".into()),
			FakeCandidateValidationError::InvalidOutputs => InvalidCandidate::InvalidOutputs,
			FakeCandidateValidationError::Timeout => InvalidCandidate::Timeout,
			FakeCandidateValidationError::ParamsTooLarge => InvalidCandidate::ParamsTooLarge(666),
			FakeCandidateValidationError::CodeTooLarge => InvalidCandidate::CodeTooLarge(666),
			FakeCandidateValidationError::CodeDecompressionFailure =>
				InvalidCandidate::CodeDecompressionFailure,
			FakeCandidateValidationError::PoVDecompressionFailure =>
				InvalidCandidate::PoVDecompressionFailure,
			FakeCandidateValidationError::BadReturn => InvalidCandidate::BadReturn,
			FakeCandidateValidationError::BadParent => InvalidCandidate::BadParent,
			FakeCandidateValidationError::PoVHashMismatch => InvalidCandidate::PoVHashMismatch,
			FakeCandidateValidationError::BadSignature => InvalidCandidate::BadSignature,
			FakeCandidateValidationError::ParaHeadHashMismatch =>
				InvalidCandidate::ParaHeadHashMismatch,
			FakeCandidateValidationError::CodeHashMismatch => InvalidCandidate::CodeHashMismatch,
		}
	}
}

#[derive(Debug, Parser)]
#[allow(missing_docs)]
struct MalusCli {
	#[clap(subcommand)]
	pub variant: NemesisVariant,
}
#[derive(Clone, Debug, Parser)]
#[clap(rename_all = "kebab-case")]
#[allow(missing_docs)]
struct DisputeAncestorOptions {
	#[clap(long, arg_enum, ignore_case = true, default_value_t = FakeCandidateValidation::Disabled)]
	pub fake_validation: FakeCandidateValidation,

	#[clap(long, arg_enum, ignore_case = true, default_value_t = FakeCandidateValidationError::InvalidOutputs)]
	pub fake_validation_error: FakeCandidateValidationError,

	#[clap(flatten)]
	cmd: RunCmd,
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
			NemesisVariant::DisputeAncestor(opts) => polkadot_cli::run_node(
				run_cmd(opts.clone().cmd),
				DisputeValidCandidates::new(opts),
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
