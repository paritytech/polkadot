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
use polkadot_cli::RunCmd;
use structopt::StructOpt;

pub(crate) mod interceptor;
pub(crate) mod shared;

mod variants;

use variants::*;

/// Define the different variants of behavior.
#[derive(Debug, StructOpt, PartialEq, Eq)]
#[structopt(rename_all = "kebab-case")]
enum NemesisVariant {
	BackGarbageCandidate,
	SuggestGarabageCandidate,
	DisputeAncestor,
}

#[derive(Debug, StructOpt)]
#[allow(missing_docs)]
struct MalusCli {
	#[structopt(subcommand)]
	pub variant: NemesisVariant,

	#[structopt(flatten)]
	pub run: RunCmd,
}

impl MalusCli {
	/// Launch a malus node.
	fn launch(self) -> eyre::Result<()> {
		let Self { variant, run } = self;
		match variant {
			NemesisVariant::BackGarbageCandidate =>
				polkadot_cli::run_node(run, BackGarbageCandidate)?,
			NemesisVariant::SuggestGarabageCandidate =>
				polkadot_cli::run_node(run, SuggestGarbageCandidate)?,
			NemesisVariant::DisputeAncestor => polkadot_cli::run_node(run, DisputeAncestor)?,
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
			"--alice",
		]))
		.unwrap();
		assert_matches::assert_matches!(cli, MalusCli {
			variant,
			..
		} => {
			assert_eq!(variant, NemesisVariant::DisputeAncestor);
		});
	}
}
