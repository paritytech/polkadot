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

use crate::prelude::*;
use clap::Parser;
use sp_runtime::Perbill;
use std::str::FromStr;

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
#[command(author, version, about)]
pub(crate) struct Opt {
	/// The `ws` node to connect to.
	#[arg(long, short, default_value = DEFAULT_URI, env = "URI", global = true)]
	pub uri: String,

	/// WS connection timeout in number of seconds.
	#[arg(long, default_value_t = 60)]
	pub connection_timeout: usize,

	/// WS request timeout in number of seconds.
	#[arg(long, default_value_t = 60 * 10)]
	pub request_timeout: usize,

	#[command(subcommand)]
	pub command: Command,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum Command {
	/// Monitor for the phase being signed, then compute.
	Monitor(MonitorConfig),

	/// Just compute a solution now, and don't submit it.
	DryRun(DryRunConfig),

	/// Provide a solution that can be submitted to the chain as an emergency response.
	EmergencySolution(EmergencySolutionConfig),

	/// Return information about the current version
	Info(InfoOpts),
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) struct MonitorConfig {
	/// The path to a file containing the seed of the account. If the file is not found, the seed is
	/// used as-is.
	///
	/// Can also be provided via the `SEED` environment variable.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[arg(long, short, env = "SEED")]
	pub seed_or_path: String,

	/// They type of event to listen to.
	///
	/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
	/// slower. It is recommended to use finalized, if the duration of the signed phase is longer
	/// than the the finality delay.
	#[arg(long, default_value = "head", value_parser = ["head", "finalized"])]
	pub listen: String,

	/// The solver algorithm to use.
	#[command(subcommand)]
	pub solver: Solver,

	/// Submission strategy to use.
	///
	/// Possible options:
	///
	/// `--submission-strategy if-leading`: only submit if leading.
	///
	/// `--submission-strategy always`: always submit.
	///
	/// `--submission-strategy "percent-better <percent>"`: submit if the submission is `n` percent better.
	///
	/// `--submission-strategy "no-worse-than  <percent>"`: submit if submission is no more than `n` percent worse.
	#[clap(long, default_value = "if-leading")]
	pub submission_strategy: SubmissionStrategy,

	/// Delay in number seconds to wait until starting mining a solution.
	///
	/// At every block when a solution is attempted
	/// a delay can be enforced to avoid submitting at
	/// "same time" and risk potential races with other miners.
	///
	/// When this is enabled and there are competing solutions, your solution might not be submitted
	/// if the scores are equal.
	#[arg(long, default_value_t = 0)]
	pub delay: usize,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) struct DryRunConfig {
	/// The path to a file containing the seed of the account. If the file is not found, the seed is
	/// used as-is.
	///
	/// Can also be provided via the `SEED` environment variable.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[arg(long, short, env = "SEED")]
	pub seed_or_path: String,

	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[arg(long)]
	pub at: Option<Hash>,

	/// The solver algorithm to use.
	#[command(subcommand)]
	pub solver: Solver,

	/// Force create a new snapshot, else expect one to exist onchain.
	#[arg(long)]
	pub force_snapshot: bool,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) struct EmergencySolutionConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[arg(long)]
	pub at: Option<Hash>,

	/// The solver algorithm to use.
	#[command(subcommand)]
	pub solver: Solver,

	/// The number of top backed winners to take. All are taken, if not provided.
	pub take: Option<usize>,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) struct InfoOpts {
	/// Serialize the output as json
	#[arg(long, short)]
	pub json: bool,
}

/// Submission strategy to use.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(test, derive(PartialEq))]
pub enum SubmissionStrategy {
	/// Always submit.
	Always,
	/// Only submit if at the time, we are the best (or equal to it).
	IfLeading,
	/// Submit if we are no worse than `Perbill` worse than the best.
	ClaimNoWorseThan(Perbill),
	/// Submit if we are leading, or if the solution that's leading is more that the given `Perbill`
	/// better than us. This helps detect obviously fake solutions and still combat them.
	ClaimBetterThan(Perbill),
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum Solver {
	SeqPhragmen {
		#[arg(long, default_value_t = 10)]
		iterations: usize,
	},
	PhragMMS {
		#[arg(long, default_value_t = 10)]
		iterations: usize,
	},
}

/// Custom `impl` to parse `SubmissionStrategy` from CLI.
///
/// Possible options:
/// * --submission-strategy if-leading: only submit if leading
/// * --submission-strategy always: always submit
/// * --submission-strategy "percent-better <percent>": submit if submission is `n` percent better.
/// * --submission-strategy "no-worse-than<percent>": submit if submission is no more than `n` percent worse.
///
impl FromStr for SubmissionStrategy {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let s = s.trim();

		let res = if s == "if-leading" {
			Self::IfLeading
		} else if s == "always" {
			Self::Always
		} else if let Some(percent) = s.strip_prefix("no-worse-than ") {
			let percent: u32 = percent.parse().map_err(|e| format!("{:?}", e))?;
			Self::ClaimNoWorseThan(Perbill::from_percent(percent))
		} else if let Some(percent) = s.strip_prefix("percent-better ") {
			let percent: u32 = percent.parse().map_err(|e| format!("{:?}", e))?;
			Self::ClaimBetterThan(Perbill::from_percent(percent))
		} else {
			return Err(s.into())
		};
		Ok(res)
	}
}

#[cfg(test)]
mod test_super {
	use super::*;

	#[test]
	fn cli_monitor_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"monitor",
			"--seed-or-path",
			"//Alice",
			"--listen",
			"head",
			"--delay",
			"12",
			"seq-phragmen",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				connection_timeout: 60,
				request_timeout: 10 * 60,
				command: Command::Monitor(MonitorConfig {
					seed_or_path: "//Alice".to_string(),
					listen: "head".to_string(),
					solver: Solver::SeqPhragmen { iterations: 10 },
					submission_strategy: SubmissionStrategy::IfLeading,
					delay: 12,
				}),
			}
		);
	}

	#[test]
	fn cli_dry_run_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"dry-run",
			"--seed-or-path",
			"//Alice",
			"phrag-mms",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				connection_timeout: 60,
				request_timeout: 10 * 60,
				command: Command::DryRun(DryRunConfig {
					seed_or_path: "//Alice".to_string(),
					at: None,
					solver: Solver::PhragMMS { iterations: 10 },
					force_snapshot: false,
				}),
			}
		);
	}

	#[test]
	fn cli_emergency_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"emergency-solution",
			"99",
			"phrag-mms",
			"--iterations",
			"1337",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				connection_timeout: 60,
				request_timeout: 10 * 60,
				command: Command::EmergencySolution(EmergencySolutionConfig {
					take: Some(99),
					at: None,
					solver: Solver::PhragMMS { iterations: 1337 }
				}),
			}
		);
	}

	#[test]
	fn cli_info_works() {
		let opt = Opt::try_parse_from([env!("CARGO_PKG_NAME"), "--uri", "hi", "info"]).unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				connection_timeout: 60,
				request_timeout: 10 * 60,
				command: Command::Info(InfoOpts { json: false })
			}
		);
	}

	#[test]
	fn cli_request_conn_timeout_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"--request-timeout",
			"10",
			"--connection-timeout",
			"9",
			"info",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				connection_timeout: 9,
				request_timeout: 10,
				command: Command::Info(InfoOpts { json: false })
			}
		);
	}

	#[test]
	fn submission_strategy_from_str_works() {
		use std::str::FromStr;

		assert_eq!(SubmissionStrategy::from_str("if-leading"), Ok(SubmissionStrategy::IfLeading));
		assert_eq!(SubmissionStrategy::from_str("always"), Ok(SubmissionStrategy::Always));
		assert_eq!(
			SubmissionStrategy::from_str("  percent-better 99   "),
			Ok(SubmissionStrategy::ClaimBetterThan(Perbill::from_percent(99)))
		);
	}
}
