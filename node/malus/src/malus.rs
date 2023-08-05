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

//! A malus or nemesis node launch code.

use clap::Parser;
use color_eyre::eyre;

pub(crate) mod interceptor;
pub(crate) mod shared;

mod variants;

use variants::*;

/// Define the different variants of behavior.
#[derive(Debug, Parser)]
#[command(about = "Malus - the nemesis of polkadot.", version, rename_all = "kebab-case")]
enum NemesisVariant {
	/// Suggest a candidate with an invalid proof of validity.
	SuggestGarbageCandidate(SuggestGarbageCandidateOptions),
	/// Back a candidate with a specifically crafted proof of validity.
	BackGarbageCandidate(BackGarbageCandidateOptions),
	/// Delayed disputing of ancestors that are perfectly fine.
	DisputeAncestor(DisputeAncestorOptions),
}

#[derive(Debug, Parser)]
#[allow(missing_docs)]
struct MalusCli {
	#[command(subcommand)]
	pub variant: NemesisVariant,
	/// Sets the minimum delay between the best and finalized block.
	pub finality_delay: Option<u32>,
}

impl MalusCli {
	/// Launch a malus node.
	fn launch(self) -> eyre::Result<()> {
		let finality_delay = self.finality_delay;
		match self.variant {
			NemesisVariant::BackGarbageCandidate(opts) => {
				let BackGarbageCandidateOptions { percentage, cli } = opts;

				polkadot_cli::run_node(cli, BackGarbageCandidates { percentage }, finality_delay)?
			},
			NemesisVariant::SuggestGarbageCandidate(opts) => {
				let SuggestGarbageCandidateOptions { percentage, cli } = opts;

				polkadot_cli::run_node(
					cli,
					SuggestGarbageCandidates { percentage },
					finality_delay,
				)?
			},
			NemesisVariant::DisputeAncestor(opts) => {
				let DisputeAncestorOptions {
					fake_validation,
					fake_validation_error,
					percentage,
					cli,
				} = opts;

				polkadot_cli::run_node(
					cli,
					DisputeValidCandidates { fake_validation, fake_validation_error, percentage },
					finality_delay,
				)?
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
			assert!(run.cli.run.base.bob);
		});
	}

	#[test]
	fn percentage_works_suggest_garbage() {
		let cli = MalusCli::try_parse_from(IntoIterator::into_iter([
			"malus",
			"suggest-garbage-candidate",
			"--percentage",
			"100",
			"--bob",
		]))
		.unwrap();
		assert_matches::assert_matches!(cli, MalusCli {
			variant: NemesisVariant::SuggestGarbageCandidate(run),
			..
		} => {
			assert!(run.cli.run.base.bob);
		});
	}

	#[test]
	fn percentage_works_dispute_ancestor() {
		let cli = MalusCli::try_parse_from(IntoIterator::into_iter([
			"malus",
			"dispute-ancestor",
			"--percentage",
			"100",
			"--bob",
		]))
		.unwrap();
		assert_matches::assert_matches!(cli, MalusCli {
			variant: NemesisVariant::DisputeAncestor(run),
			..
		} => {
			assert!(run.cli.run.base.bob);
		});
	}

	#[test]
	fn percentage_works_back_garbage() {
		let cli = MalusCli::try_parse_from(IntoIterator::into_iter([
			"malus",
			"back-garbage-candidate",
			"--percentage",
			"100",
			"--bob",
		]))
		.unwrap();
		assert_matches::assert_matches!(cli, MalusCli {
			variant: NemesisVariant::BackGarbageCandidate(run),
			..
		} => {
			assert!(run.cli.run.base.bob);
		});
	}

	#[test]
	#[should_panic]
	fn validate_range_for_percentage() {
		let cli = MalusCli::try_parse_from(IntoIterator::into_iter([
			"malus",
			"suggest-garbage-candidate",
			"--percentage",
			"101",
			"--bob",
		]))
		.unwrap();
		assert_matches::assert_matches!(cli, MalusCli {
			variant: NemesisVariant::DisputeAncestor(run),
			..
		} => {
			assert!(run.cli.run.base.bob);
		});
	}
}
