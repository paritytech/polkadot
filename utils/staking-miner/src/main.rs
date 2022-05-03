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

mod dry_run;
mod emergency_solution;
mod monitor;
mod prelude;
mod rpc;
mod signer;

use std::str::FromStr;

pub(crate) use prelude::*;

use clap::Parser;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use remote_externalities::{Builder, Mode, OnlineConfig};
use sp_npos_elections::ExtendedBalance;
use sp_runtime::Perbill;
use subxt::{DefaultConfig, PolkadotExtrinsicParams};
use tracing_subscriber::{fmt, EnvFilter};

use std::{ops::Deref, sync::Arc};

#[subxt::subxt(
	runtime_metadata_path = "metadata.scale",
	derive_for_all_types = "Clone, PartialEq",
	derive_for_type(type = "sp_core::crypto::AccountId32", derive = "Eq, Ord, PartialOrd")
)]
pub mod runtime {}

pub type RuntimeApi = runtime::RuntimeApi<DefaultConfig, PolkadotExtrinsicParams<DefaultConfig>>;

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

/// Build the Ext at hash with all the data of `ElectionProviderMultiPhase` and any additional
/// pallets.
async fn create_election_ext(
	api: &RuntimeApi,
	at: Option<Hash>,
	additional: Vec<String>,
) -> Result<Ext, anyhow::Error> {
	use crate::runtime::{
		election_provider_multi_phase::storage::Round,
		system::storage::{BlockHash, Number},
	};
	use subxt::storage::{StorageEntry, StorageKeyPrefix};

	// dummy way to get the name of the pallet.
	let mut pallets = vec![<Round as StorageEntry>::PALLET.to_string()];

	// <frame_system::BlockHash<T>>::prefix_hash()
	// TODO(niklasad1): this is probably wrong.
	let block_hash_storage = {
		let prefix = StorageKeyPrefix::new::<BlockHash>();
		let store = BlockHash(&0);
		store.key().final_key(prefix)
	};

	let system_number = {
		let prefix = StorageKeyPrefix::new::<Number>();
		let store = Number;
		store.key().final_key(prefix)
	};

	let client = api.client.rpc().client.clone();

	pallets.extend(additional);
	Builder::<Block>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: client.into(),
			at,
			pallets,
			..Default::default()
		}))
		.inject_hashed_prefix(block_hash_storage.as_ref())
		.inject_hashed_key(system_number.as_ref())
		.build()
		.await
		.map_err(|why| anyhow::anyhow!("{}", why))
}

/// Compute the election. It expects to NOT be `Phase::Off`. In other words, the snapshot must
/// exists on the given externalities.
/*fn mine_solution<T, S>(
	ext: &mut Ext,
	do_feasibility: bool,
) -> Result<EPM::RawSolution<EPM::SolutionOf<T>>, Error<T>>
where
	T: EPM::Config,
	S: NposSolver<
		Error = <<T as EPM::Config>::Solver as NposSolver>::Error,
		AccountId = <<T as EPM::Config>::Solver as NposSolver>::AccountId,
	>,
{
	ext.execute_with(|| {
		let (solution, _) =
			<EPM::Pallet<T>>::mine_solution::<S>().map_err::<Error<T>, _>(Into::into)?;
		if do_feasibility {
			let _ = <EPM::Pallet<T>>::feasibility_check(
				solution.clone(),
				EPM::ElectionCompute::Signed,
			)?;
		}
		Ok(solution)
	})
}*/

/// Mine a solution with the given `solver`.
/*fn mine_with<T>(
	solver: &Solver,
	ext: &mut Ext,
	do_feasibility: bool,
) -> Result<EPM::RawSolution<EPM::SolutionOf<T>>, Error<T>>
where
	T: EPM::Config,
	T::Solver: NposSolver<Error = sp_npos_elections::Error>,
{
	use frame_election_provider_support::{PhragMMS, SequentialPhragmen};

	match solver {
		Solver::SeqPhragmen { iterations } => {
			BalanceIterations::set(*iterations);
			mine_solution::<
				T,
				SequentialPhragmen<
					<T as frame_system::Config>::AccountId,
					sp_runtime::Perbill,
					Balancing,
				>,
			>(ext, do_feasibility)
		},
		Solver::PhragMMS { iterations } => {
			BalanceIterations::set(*iterations);
			mine_solution::<
				T,
				PhragMMS<<T as frame_system::Config>::AccountId, sp_runtime::Perbill, Balancing>,
			>(ext, do_feasibility)
		},
	}
}*/

/*#[allow(unused)]
fn mine_dpos<T: EPM::Config>(ext: &mut Ext) -> Result<(), Error<T>> {
	ext.execute_with(|| {
		use std::collections::BTreeMap;
		use EPM::RoundSnapshot;
		let RoundSnapshot { voters, .. } = EPM::Snapshot::<T>::get().unwrap();
		let desired_targets = EPM::DesiredTargets::<T>::get().unwrap();
		let mut candidates_and_backing = BTreeMap::<T::AccountId, u128>::new();
		voters.into_iter().for_each(|(who, stake, targets)| {
			if targets.is_empty() {
				println!("target = {:?}", (who, stake, targets));
				return;
			}
			let share: u128 = (stake as u128) / (targets.len() as u128);
			for target in targets {
				*candidates_and_backing.entry(target.clone()).or_default() += share
			}
		});

		let mut candidates_and_backing =
			candidates_and_backing.into_iter().collect::<Vec<(_, _)>>();
		candidates_and_backing.sort_by_key(|(_, total_stake)| *total_stake);
		let winners = candidates_and_backing
			.into_iter()
			.rev()
			.take(desired_targets as usize)
			.collect::<Vec<_>>();
		let score = {
			let min_staker = *winners.last().map(|(_, stake)| stake).unwrap();
			let sum_stake = winners.iter().fold(0u128, |acc, (_, stake)| acc + stake);
			let sum_squared = winners.iter().fold(0u128, |acc, (_, stake)| acc + stake);
			[min_staker, sum_stake, sum_squared]
		};
		println!("mined a dpos-like solution with score = {:?}", score);
		Ok(())
	})
}*/

#[tokio::main]
async fn main() {
	fmt().with_env_filter(EnvFilter::from_default_env()).init();

	let Opt { uri, seed_or_path, command } = Opt::parse();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", uri);

	let client: subxt::Client<DefaultConfig> =
		subxt::ClientBuilder::new().set_url(uri.clone()).build().await.unwrap();

	let signer = signer::signer_from_string(&seed_or_path);

	let outcome = match command {
		Command::Monitor(cmd) => monitor::run_cmd(client, cmd, Arc::new(signer)).await,
		_ => panic!("oo"),
	};

	log::info!(target: LOG_TARGET, "round of execution finished. outcome = {:?}", outcome);
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
