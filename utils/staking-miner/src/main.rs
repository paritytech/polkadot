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
pub(crate) use signer::get_account_info;

use clap::Parser;
use frame_election_provider_support::NposSolver;
use frame_support::traits::Get;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use remote_externalities::{Builder, Mode, OnlineConfig};
use rpc::{RpcApiClient, SharedRpcClient};
use sp_npos_elections::BalancingConfig;
use sp_runtime::{traits::Block as BlockT, DeserializeOwned, Perbill};
use tracing_subscriber::{fmt, EnvFilter};

use std::{ops::Deref, sync::Arc};

pub(crate) enum AnyRuntime {
	Polkadot,
	Kusama,
	Westend,
}

pub(crate) static mut RUNTIME: AnyRuntime = AnyRuntime::Polkadot;

macro_rules! construct_runtime_prelude {
	($runtime:ident) => { paste::paste! {
		#[allow(unused_import)]
		pub(crate) mod [<$runtime _runtime_exports>] {
			pub(crate) use crate::prelude::EPM;
			pub(crate) use [<$runtime _runtime>]::*;
			pub(crate) use crate::monitor::[<monitor_cmd_ $runtime>] as monitor_cmd;
			pub(crate) use crate::dry_run::[<dry_run_cmd_ $runtime>] as dry_run_cmd;
			pub(crate) use crate::emergency_solution::[<emergency_solution_cmd_ $runtime>] as emergency_solution_cmd;
			pub(crate) use private::{[<create_uxt_ $runtime>] as create_uxt};

			mod private {
				use super::*;
				pub(crate) fn [<create_uxt_ $runtime>](
					raw_solution: EPM::RawSolution<EPM::SolutionOf<Runtime>>,
					signer: crate::signer::Signer,
					nonce: crate::prelude::Index,
					tip: crate::prelude::Balance,
					era: sp_runtime::generic::Era,
				) -> UncheckedExtrinsic {
					use codec::Encode as _;
					use sp_core::Pair as _;
					use sp_runtime::traits::StaticLookup as _;

					let crate::signer::Signer { account, pair, .. } = signer;

					let local_call = EPMCall::<Runtime>::submit { raw_solution: Box::new(raw_solution) };
					let call: Call = <EPMCall<Runtime> as std::convert::TryInto<Call>>::try_into(local_call)
						.expect("election provider pallet must exist in the runtime, thus \
							inner call can be converted, qed."
						);

					let extra: SignedExtra = crate::[<signed_ext_builder_ $runtime>](nonce, tip, era);
					let raw_payload = SignedPayload::new(call, extra).expect("creating signed payload infallible; qed.");
					let signature = raw_payload.using_encoded(|payload| {
						pair.sign(payload)
					});
					let (call, extra, _) = raw_payload.deconstruct();
					let address = <Runtime as frame_system::Config>::Lookup::unlookup(account);
					let extrinsic = UncheckedExtrinsic::new_signed(call, address, signature.into(), extra);
					log::debug!(
						target: crate::LOG_TARGET, "constructed extrinsic {} with length {}",
						sp_core::hexdisplay::HexDisplay::from(&extrinsic.encode()),
						extrinsic.encode().len(),
					);
					extrinsic
				}
			}
		}}
	};
}

// NOTE: we might be able to use some code from the bridges repo here.
fn signed_ext_builder_polkadot(
	nonce: Index,
	tip: Balance,
	era: sp_runtime::generic::Era,
) -> polkadot_runtime_exports::SignedExtra {
	use polkadot_runtime_exports::Runtime;
	(
		frame_system::CheckNonZeroSender::<Runtime>::new(),
		frame_system::CheckSpecVersion::<Runtime>::new(),
		frame_system::CheckTxVersion::<Runtime>::new(),
		frame_system::CheckGenesis::<Runtime>::new(),
		frame_system::CheckMortality::<Runtime>::from(era),
		frame_system::CheckNonce::<Runtime>::from(nonce),
		frame_system::CheckWeight::<Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
		runtime_common::claims::PrevalidateAttests::<Runtime>::new(),
	)
}

fn signed_ext_builder_kusama(
	nonce: Index,
	tip: Balance,
	era: sp_runtime::generic::Era,
) -> kusama_runtime_exports::SignedExtra {
	use kusama_runtime_exports::Runtime;
	(
		frame_system::CheckNonZeroSender::<Runtime>::new(),
		frame_system::CheckSpecVersion::<Runtime>::new(),
		frame_system::CheckTxVersion::<Runtime>::new(),
		frame_system::CheckGenesis::<Runtime>::new(),
		frame_system::CheckMortality::<Runtime>::from(era),
		frame_system::CheckNonce::<Runtime>::from(nonce),
		frame_system::CheckWeight::<Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
	)
}

fn signed_ext_builder_westend(
	nonce: Index,
	tip: Balance,
	era: sp_runtime::generic::Era,
) -> westend_runtime_exports::SignedExtra {
	use westend_runtime_exports::Runtime;
	(
		frame_system::CheckNonZeroSender::<Runtime>::new(),
		frame_system::CheckSpecVersion::<Runtime>::new(),
		frame_system::CheckTxVersion::<Runtime>::new(),
		frame_system::CheckGenesis::<Runtime>::new(),
		frame_system::CheckMortality::<Runtime>::from(era),
		frame_system::CheckNonce::<Runtime>::from(nonce),
		frame_system::CheckWeight::<Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(tip),
	)
}

construct_runtime_prelude!(polkadot);
construct_runtime_prelude!(kusama);
construct_runtime_prelude!(westend);

// NOTE: this is no longer used extensively, most of the per-runtime stuff us delegated to
// `construct_runtime_prelude` and macro's the import directly from it. A part of the code is also
// still generic over `T`. My hope is to still make everything generic over a `Runtime`, but sadly
// that is not currently possible as each runtime has its unique `Call`, and all Calls are not
// sharing any generic trait. In other words, to create the `UncheckedExtrinsic` of each chain, you
// need the concrete `Call` of that chain as well.
#[macro_export]
macro_rules! any_runtime {
	($($code:tt)*) => {
		unsafe {
			match $crate::RUNTIME {
				$crate::AnyRuntime::Polkadot => {
					#[allow(unused)]
					use $crate::polkadot_runtime_exports::*;
					$($code)*
				},
				$crate::AnyRuntime::Kusama => {
					#[allow(unused)]
					use $crate::kusama_runtime_exports::*;
					$($code)*
				},
				$crate::AnyRuntime::Westend => {
					#[allow(unused)]
					use $crate::westend_runtime_exports::*;
					$($code)*
				}
			}
		}
	}
}

/// Same as [`any_runtime`], but instead of returning a `Result`, this simply returns `()`. Useful
/// for situations where the result is not useful and un-ergonomic to handle.
#[macro_export]
macro_rules! any_runtime_unit {
	($($code:tt)*) => {
		unsafe {
			match $crate::RUNTIME {
				$crate::AnyRuntime::Polkadot => {
					#[allow(unused)]
					use $crate::polkadot_runtime_exports::*;
					let _ = $($code)*;
				},
				$crate::AnyRuntime::Kusama => {
					#[allow(unused)]
					use $crate::kusama_runtime_exports::*;
					let _ = $($code)*;
				},
				$crate::AnyRuntime::Westend => {
					#[allow(unused)]
					use $crate::westend_runtime_exports::*;
					let _ = $($code)*;
				}
			}
		}
	}
}

#[derive(frame_support::DebugNoBound, thiserror::Error)]
enum Error<T: EPM::Config> {
	Io(#[from] std::io::Error),
	JsonRpsee(#[from] jsonrpsee::core::Error),
	RpcHelperError(#[from] rpc::RpcHelperError),
	Codec(#[from] codec::Error),
	Crypto(sp_core::crypto::SecretStringError),
	RemoteExternalities(&'static str),
	PalletMiner(EPM::unsigned::MinerError),
	PalletElection(EPM::ElectionError<T>),
	PalletFeasibility(EPM::FeasibilityError),
	AccountDoesNotExists,
	IncorrectPhase,
	AlreadySubmitted,
	VersionMismatch,
	StrategyNotSatisfied,
}

impl<T: EPM::Config> From<sp_core::crypto::SecretStringError> for Error<T> {
	fn from(e: sp_core::crypto::SecretStringError) -> Error<T> {
		Error::Crypto(e)
	}
}

impl<T: EPM::Config> From<EPM::unsigned::MinerError> for Error<T> {
	fn from(e: EPM::unsigned::MinerError) -> Error<T> {
		Error::PalletMiner(e)
	}
}

impl<T: EPM::Config> From<EPM::ElectionError<T>> for Error<T> {
	fn from(e: EPM::ElectionError<T>) -> Error<T> {
		Error::PalletElection(e)
	}
}

impl<T: EPM::Config> From<EPM::FeasibilityError> for Error<T> {
	fn from(e: EPM::FeasibilityError) -> Error<T> {
		Error::PalletFeasibility(e)
	}
}

impl<T: EPM::Config> std::fmt::Display for Error<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		<Error<T> as std::fmt::Debug>::fmt(self, f)
	}
}

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
	pub static Balancing: Option<BalancingConfig> = Some( BalancingConfig { iterations: BalanceIterations::get(), tolerance: 0 } );
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
async fn create_election_ext<T: EPM::Config, B: BlockT + DeserializeOwned>(
	client: SharedRpcClient,
	at: Option<B::Hash>,
	additional: Vec<String>,
) -> Result<Ext, Error<T>> {
	use frame_support::{storage::generator::StorageMap, traits::PalletInfo};
	use sp_core::hashing::twox_128;

	let mut pallets = vec![<T as frame_system::Config>::PalletInfo::name::<EPM::Pallet<T>>()
		.expect("Pallet always has name; qed.")
		.to_string()];
	pallets.extend(additional);
	Builder::<B>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: client.into_inner().into(),
			at,
			pallets,
			..Default::default()
		}))
		.inject_hashed_prefix(&<frame_system::BlockHash<T>>::prefix_hash())
		.inject_hashed_key(&[twox_128(b"System"), twox_128(b"Number")].concat())
		.build()
		.await
		.map_err(|why| Error::RemoteExternalities(why))
}

/// Compute the election. It expects to NOT be `Phase::Off`. In other words, the snapshot must
/// exists on the given externalities.
fn mine_solution<T, S>(
	ext: &mut Ext,
	do_feasibility: bool,
) -> Result<EPM::RawSolution<EPM::SolutionOf<T::MinerConfig>>, Error<T>>
where
	T: EPM::Config,
	S: NposSolver<
		Error = <<T as EPM::Config>::Solver as NposSolver>::Error,
		AccountId = <<T as EPM::Config>::Solver as NposSolver>::AccountId,
	>,
{
	ext.execute_with(|| {
		let (solution, _) = <EPM::Pallet<T>>::mine_solution().map_err::<Error<T>, _>(Into::into)?;
		if do_feasibility {
			let _ = <EPM::Pallet<T>>::feasibility_check(
				solution.clone(),
				EPM::ElectionCompute::Signed,
			)?;
		}
		Ok(solution)
	})
}

/// Mine a solution with the given `solver`.
fn mine_with<T>(
	solver: &Solver,
	ext: &mut Ext,
	do_feasibility: bool,
) -> Result<EPM::RawSolution<EPM::SolutionOf<T::MinerConfig>>, Error<T>>
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
}

#[allow(unused)]
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
				return
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
}

pub(crate) async fn check_versions<T: frame_system::Config + EPM::Config>(
	rpc: &SharedRpcClient,
) -> Result<(), Error<T>> {
	let linked_version = T::Version::get();
	let on_chain_version = rpc
		.runtime_version(None)
		.await
		.expect("runtime version RPC should always work; qed");

	log::debug!(target: LOG_TARGET, "linked version {:?}", linked_version);
	log::debug!(target: LOG_TARGET, "on-chain version {:?}", on_chain_version);

	if linked_version != on_chain_version {
		log::error!(
			target: LOG_TARGET,
			"VERSION MISMATCH: any transaction will fail with bad-proof"
		);
		Err(Error::VersionMismatch)
	} else {
		Ok(())
	}
}

#[tokio::main]
async fn main() {
	fmt().with_env_filter(EnvFilter::from_default_env()).init();

	let Opt { uri, seed_or_path, command } = Opt::parse();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", uri);

	let rpc = loop {
		match SharedRpcClient::new(&uri).await {
			Ok(client) => break client,
			Err(why) => {
				log::warn!(
					target: LOG_TARGET,
					"failed to connect to client due to {:?}, retrying soon..",
					why
				);
				tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
			},
		}
	};

	let chain: String = rpc.system_chain().await.expect("system_chain infallible; qed.");
	match chain.to_lowercase().as_str() {
		"polkadot" | "development" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormatRegistry::PolkadotAccount.into(),
			);
			sub_tokens::dynamic::set_name("DOT");
			sub_tokens::dynamic::set_decimal_points(10_000_000_000);
			// safety: this program will always be single threaded, thus accessing global static is
			// safe.
			unsafe {
				RUNTIME = AnyRuntime::Polkadot;
			}
		},
		"kusama" | "kusama-dev" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormatRegistry::KusamaAccount.into(),
			);
			sub_tokens::dynamic::set_name("KSM");
			sub_tokens::dynamic::set_decimal_points(1_000_000_000_000);
			// safety: this program will always be single threaded, thus accessing global static is
			// safe.
			unsafe {
				RUNTIME = AnyRuntime::Kusama;
			}
		},
		"westend" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormatRegistry::PolkadotAccount.into(),
			);
			sub_tokens::dynamic::set_name("WND");
			sub_tokens::dynamic::set_decimal_points(1_000_000_000_000);
			// safety: this program will always be single threaded, thus accessing global static is
			// safe.
			unsafe {
				RUNTIME = AnyRuntime::Westend;
			}
		},
		_ => {
			eprintln!("unexpected chain: {:?}", chain);
			return
		},
	}
	log::info!(target: LOG_TARGET, "connected to chain {:?}", chain);

	any_runtime_unit! {
		check_versions::<Runtime>(&rpc).await
	};

	let signer_account = any_runtime! {
		signer::signer_uri_from_string::<Runtime>(&seed_or_path, &rpc)
			.await
			.expect("Provided account is invalid, terminating.")
	};

	let outcome = any_runtime! {
		match command {
			Command::Monitor(cmd) => monitor_cmd(rpc, cmd, signer_account).await
				.map_err(|e| {
					log::error!(target: LOG_TARGET, "Monitor error: {:?}", e);
				}),
			Command::DryRun(cmd) => dry_run_cmd(rpc, cmd, signer_account).await
				.map_err(|e| {
					log::error!(target: LOG_TARGET, "DryRun error: {:?}", e);
				}),
			Command::EmergencySolution(cmd) => emergency_solution_cmd(rpc, cmd).await
				.map_err(|e| {
					log::error!(target: LOG_TARGET, "EmergencySolution error: {:?}", e);
				}),
		}
	};
	log::info!(target: LOG_TARGET, "round of execution finished. outcome = {:?}", outcome);
}

#[cfg(test)]
mod tests {
	use super::*;

	fn get_version<T: frame_system::Config>() -> sp_version::RuntimeVersion {
		T::Version::get()
	}

	#[test]
	fn any_runtime_works() {
		unsafe {
			RUNTIME = AnyRuntime::Polkadot;
		}
		let polkadot_version = any_runtime! { get_version::<Runtime>() };

		unsafe {
			RUNTIME = AnyRuntime::Kusama;
		}
		let kusama_version = any_runtime! { get_version::<Runtime>() };

		assert_eq!(polkadot_version.spec_name, "polkadot".into());
		assert_eq!(kusama_version.spec_name, "kusama".into());
	}

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
