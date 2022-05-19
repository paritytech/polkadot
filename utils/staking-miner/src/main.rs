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

//! # Polkadot Staking Miner.
//!
//! Simple bot capable of monitoring a polkadot (and cousins) chain and submitting solutions to the
//! `pallet-election-provider-multi-phase`. See `--help` for more details.
//!
//! # Implementation Notes:
//!
//! - First draft: Be aware that this is the first draft and there might be bugs, or undefined
//!   behaviors. Don't attach this bot to an account with lots of funds.
//! - Quick to crash: The bot is written so that it only continues to work if everything goes well.
//!   In case of any failure (RPC, logic, IO), it will crash. This was a decision to simplify the
//!   development. It is intended to run this bot with a `restart = true` way, so that it reports it
//!   crash, but resumes work thereafter.

mod chain;
mod dry_run;
mod emergency_solution;
mod error;
mod helpers;
mod monitor;
mod prelude;
mod signer;

use jsonrpsee::ws_client::WsClientBuilder;
pub(crate) use prelude::*;

use clap::Parser;
use sp_npos_elections::ExtendedBalance;
use sp_runtime::Perbill;
use tracing_subscriber::{fmt, EnvFilter};

use std::{str::FromStr, sync::Arc};

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
enum Command {
	/// Monitor for the phase being signed, then compute.
	Monitor(MonitorConfig),
	/// Just compute a solution now, and don't submit it.
	DryRun(DryRunConfig),
	/// Provide a solution that can be submitted to the chain as an emergency response.
	EmergencySolution(EmergencySolutionConfig),
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
enum Solver {
	SeqPhragmen {
		#[clap(long, default_value = "10")]
		iterations: usize,
	},
	PhragMMS {
		#[clap(long, default_value = "10")]
		iterations: usize,
	},
}

#[macro_export]
macro_rules! any_runtime {
	($chain:tt, $($code:tt)*) => {
			match $chain {
				"polkadot" => {
					#[allow(unused)]
					use {$crate::chain::polkadot::RuntimeApi, monitor::run_polkadot as monitor_cmd, dry_run::run_polkadot as dry_run_cmd, emergency_solution::run_polkadot as emergency_cmd};
					$($code)*
				},
				"kusama" => {
					#[allow(unused)]
					use {$crate::chain::kusama::RuntimeApi, monitor::run_kusama as monitor_cmd, dry_run::run_kusama as dry_run_cmd, emergency_solution::run_kusama as emergency_cmd};
					$($code)*
				},
				"westend" => {
					#[allow(unused)]
					use {$crate::chain::westend::RuntimeApi, monitor::run_westend as monitor_cmd, dry_run::run_westend as dry_run_cmd, emergency_solution::run_westend as emergency_cmd};
					$($code)*
				}
				other => Err(Error::Other(format!(
					"expected chain to be polkadot, kusama or westend; got: {}", other
				))),
			}
	}
}

/// Submission strategy to use.
#[derive(Debug, Copy, Clone)]
#[cfg_attr(test, derive(PartialEq))]
enum SubmissionStrategy {
	// Only submit if at the time, we are the best.
	IfLeading,
	// Always submit.
	Always,
	// Submit if we are leading, or if the solution that's leading is more that the given `Perbill`
	// better than us. This helps detect obviously fake solutions and still combat them.
	ClaimBetterThan(Perbill),
}

/// Custom `impl` to parse `SubmissionStrategy` from CLI.
///
/// Possible options:
/// * --submission-strategy if-leading: only submit if leading
/// * --submission-strategy always: always submit
/// * --submission-strategy "percent-better <percent>": submit if submission is `n` percent better.
///
impl FromStr for SubmissionStrategy {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		let s = s.trim();

		let res = if s == "if-leading" {
			Self::IfLeading
		} else if s == "always" {
			Self::Always
		} else if s.starts_with("percent-better ") {
			let percent: u32 = s[15..].parse().map_err(|e| format!("{:?}", e))?;
			Self::ClaimBetterThan(Perbill::from_percent(percent))
		} else {
			return Err(s.into())
		};
		Ok(res)
	}
}

frame_support::parameter_types! {
	/// Number of balancing iterations for a solution algorithm. Set based on the [`Solvers`] CLI
	/// config.
	pub static BalanceIterations: usize = 10;
	pub static Balancing: Option<(usize, ExtendedBalance)> = Some((BalanceIterations::get(), 0));
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
struct MonitorConfig {
	/// They type of event to listen to.
	///
	/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
	/// slower. It is recommended to use finalized, if the duration of the signed phase is longer
	/// than the the finality delay.
	#[clap(long, default_value = "head", possible_values = &["head", "finalized"])]
	listen: String,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	solver: Solver,

	/// Submission strategy to use.
	///
	/// Possible options:
	///
	/// `--submission-strategy if-leading`: only submit if leading.
	///
	/// `--submission-strategy always`: always submit.
	///
	/// `--submission-strategy "percent-better <percent>"`: submit if the submission is `n` percent better.
	#[clap(long, parse(try_from_str), default_value = "if-leading")]
	submission_strategy: SubmissionStrategy,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
struct EmergencySolutionConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[clap(long)]
	at: Option<Hash>,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	solver: Solver,

	/// The number of top backed winners to take. All are taken, if not provided.
	take: Option<usize>,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
struct DryRunConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[clap(long)]
	at: Option<Hash>,

	/// The solver algorithm to use.
	#[clap(subcommand)]
	solver: Solver,

	/// Force create a new snapshot, else expect one to exist onchain.
	#[clap(long)]
	force_snapshot: bool,
}

#[derive(Debug, Clone, Parser)]
#[cfg_attr(test, derive(PartialEq))]
#[clap(author, version, about)]
struct Opt {
	/// The `ws` node to connect to.
	#[clap(long, short, default_value = DEFAULT_URI, env = "URI")]
	uri: String,

	/// The path to a file containing the seed of the account. If the file is not found, the seed is
	/// used as-is.
	///
	/// Can also be provided via the `SEED` environment variable.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[clap(long, short, env = "SEED")]
	seed_or_path: String,

	#[clap(subcommand)]
	command: Command,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
	fmt().with_env_filter(EnvFilter::from_default_env()).init();

	let Opt { uri, seed_or_path, command } = Opt::parse();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", uri);

	// TODO: fix unwrap
	let rpc = WsClientBuilder::default()
		.max_request_body_size(u32::MAX)
		.build(uri)
		.await
		.unwrap();

	let client = subxt::ClientBuilder::new().set_client(rpc).build().await?;
	let runtime_version = client.rpc().runtime_version(None).await?;
	let signer = signer::signer_from_string(&seed_or_path)?;

	// TODO: fix unwrap and cleanup.
	let json = &runtime_version.other["specName"];
	let chain = serde_json::to_string(&json).unwrap().to_lowercase();
	let chain_str = chain.as_str();

	log::info!(target: LOG_TARGET, "Connected to chain: {}", chain);

	let outcome = any_runtime!(chain_str, {
		let api: RuntimeApi = client.to_runtime_api();

		// Start a new tokio task to perform the runtime updates in the background.
		let update_client = api.client.updates();
		tokio::spawn(async move {
			let result = update_client.perform_runtime_updates().await;
			log::error!(target: LOG_TARGET, "Runtime update failed with result: {:?}", result);
		});

		let account_info = api.storage().system().account(signer.account_id(), None).await?;
		log::info!(target: LOG_TARGET, "Loaded account: {:?}", account_info);

		match command {
			Command::Monitor(cfg) => monitor_cmd(api, cfg, Arc::new(signer)).await,
			Command::DryRun(cfg) => dry_run_cmd(api, cfg, signer).await,
			Command::EmergencySolution(cfg) => emergency_cmd(api, cfg, signer).await,
		}
	});

	log::info!(target: LOG_TARGET, "round of execution finished. outcome = {:?}", outcome);
	outcome
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn cli_monitor_works() {
		let opt = Opt::try_parse_from([
			env!("CARGO_PKG_NAME"),
			"--uri",
			"hi",
			"--seed-or-path",
			"//Alice",
			"monitor",
			"--listen",
			"head",
			"seq-phragmen",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				seed_or_path: "//Alice".to_string(),
				command: Command::Monitor(MonitorConfig {
					listen: "head".to_string(),
					solver: Solver::SeqPhragmen { iterations: 10 },
					submission_strategy: SubmissionStrategy::IfLeading,
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
			"--seed-or-path",
			"//Alice",
			"dry-run",
			"phrag-mms",
		])
		.unwrap();

		assert_eq!(
			opt,
			Opt {
				uri: "hi".to_string(),
				seed_or_path: "//Alice".to_string(),
				command: Command::DryRun(DryRunConfig {
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
			"--seed-or-path",
			"//Alice",
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
				seed_or_path: "//Alice".to_string(),
				command: Command::EmergencySolution(EmergencySolutionConfig {
					take: Some(99),
					at: None,
					solver: Solver::PhragMMS { iterations: 1337 }
				}),
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
