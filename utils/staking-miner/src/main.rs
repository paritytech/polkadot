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
mod rpc_helpers;
mod signer;

pub(crate) use prelude::*;
pub(crate) use signer::get_account_info;

use frame_election_provider_support::NposSolver;
use frame_support::traits::Get;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use remote_externalities::{Builder, Mode, OnlineConfig};
use sp_npos_elections::ExtendedBalance;
use sp_runtime::{traits::Block as BlockT, DeserializeOwned};
use structopt::StructOpt;

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
					witness: u32,
					signer: crate::signer::Signer,
					nonce: crate::prelude::Index,
					tip: crate::prelude::Balance,
					era: sp_runtime::generic::Era,
				) -> UncheckedExtrinsic {
					use codec::Encode as _;
					use sp_core::Pair as _;
					use sp_runtime::traits::StaticLookup as _;

					let crate::signer::Signer { account, pair, .. } = signer;

					let local_call = EPMCall::<Runtime>::submit { raw_solution: Box::new(raw_solution), num_signed_submissions: witness };
					let call: Call = <EPMCall<Runtime> as std::convert::TryInto<Call>>::try_into(local_call)
						.expect("election provider pallet must exist in the runtime, thus \
							inner call can be converted, qed."
						);

					let extra: SignedExtra = crate::[<signed_ext_builder_ $runtime>](nonce, tip, era);
					let raw_payload = SignedPayload::new(call, extra).expect("creating signed payload infallible; qed.");
					let signature = raw_payload.using_encoded(|payload| {
						pair.clone().sign(payload)
					});
					let (call, extra, _) = raw_payload.deconstruct();
					let address = <Runtime as frame_system::Config>::Lookup::unlookup(account.clone());
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
	JsonRpsee(#[from] jsonrpsee::types::Error),
	RpcHelperError(#[from] rpc_helpers::RpcHelperError),
	Codec(#[from] codec::Error),
	Crypto(sp_core::crypto::SecretStringError),
	RemoteExternalities(&'static str),
	PalletMiner(EPM::unsigned::MinerError<T>),
	PalletElection(EPM::ElectionError<T>),
	PalletFeasibility(EPM::FeasibilityError),
	AccountDoesNotExists,
	IncorrectPhase,
	AlreadySubmitted,
	VersionMismatch,
}

impl<T: EPM::Config> From<sp_core::crypto::SecretStringError> for Error<T> {
	fn from(e: sp_core::crypto::SecretStringError) -> Error<T> {
		Error::Crypto(e)
	}
}

impl<T: EPM::Config> From<EPM::unsigned::MinerError<T>> for Error<T> {
	fn from(e: EPM::unsigned::MinerError<T>) -> Error<T> {
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

#[derive(Debug, Clone, StructOpt)]
enum Command {
	/// Monitor for the phase being signed, then compute.
	Monitor(MonitorConfig),
	/// Just compute a solution now, and don't submit it.
	DryRun(DryRunConfig),
	/// Provide a solution that can be submitted to the chain as an emergency response.
	EmergencySolution(EmergencySolutionConfig),
}

#[derive(Debug, Clone, StructOpt)]
enum Solvers {
	SeqPhragmen {
		#[structopt(long, default_value = "10")]
		iterations: usize,
	},
	PhragMMS {
		#[structopt(long, default_value = "10")]
		iterations: usize,
	},
}

frame_support::parameter_types! {
	/// Number of balancing iterations for a solution algorithm. Set based on the [`Solvers`] CLI
	/// config.
	pub static BalanceIterations: usize = 10;
	pub static Balancing: Option<(usize, ExtendedBalance)> = Some((BalanceIterations::get(), 0));
}

#[derive(Debug, Clone, StructOpt)]
struct MonitorConfig {
	/// They type of event to listen to.
	///
	/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
	/// slower. It is recommended to use finalized, if the duration of the signed phase is longer
	/// than the the finality delay.
	#[structopt(long, default_value = "head", possible_values = &["head", "finalized"])]
	listen: String,

	/// The solver algorithm to use.
	#[structopt(subcommand)]
	solver: Solvers,
}

#[derive(Debug, Clone, StructOpt)]
struct EmergencySolutionConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[structopt(long)]
	at: Option<Hash>,

	/// The solver algorithm to use.
	#[structopt(subcommand)]
	solver: Solvers,

	/// The number of top backed winners to take. All are taken, if not provided.
	take: Option<usize>,
}

#[derive(Debug, Clone, StructOpt)]
struct DryRunConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[structopt(long)]
	at: Option<Hash>,

	/// The solver algorithm to use.
	#[structopt(subcommand)]
	solver: Solvers,
}

#[derive(Debug, Clone, StructOpt)]
struct SharedConfig {
	/// The `ws` node to connect to.
	#[structopt(long, short, default_value = DEFAULT_URI, env = "URI")]
	uri: String,

	/// The seed of a funded account in hex.
	///
	/// WARNING: Don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try and lose funds through transaction fees/deposits.
	#[structopt(long, short, env = "SEED")]
	seed: String,
}

#[derive(Debug, Clone, StructOpt)]
struct Opt {
	/// The `ws` node to connect to.
	#[structopt(flatten)]
	shared: SharedConfig,

	#[structopt(subcommand)]
	command: Command,
}

/// Build the Ext at hash with all the data of `ElectionProviderMultiPhase` and any additional
/// pallets.
async fn create_election_ext<T: EPM::Config, B: BlockT + DeserializeOwned>(
	uri: String,
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
			transport: uri.into(),
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
) -> Result<(EPM::RawSolution<EPM::SolutionOf<T>>, u32), Error<T>>
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
		let witness = <EPM::SignedSubmissions<T>>::decode_len().unwrap_or_default();
		Ok((solution, witness as u32))
	})
}

/// Mine a solution with the given `solver`.
fn mine_with<T>(
	solver: &Solvers,
	ext: &mut Ext,
	do_feasibility: bool,
) -> Result<(EPM::RawSolution<EPM::SolutionOf<T>>, u32), Error<T>>
where
	T: EPM::Config,
	T::Solver: NposSolver<Error = sp_npos_elections::Error>,
{
	use frame_election_provider_support::{PhragMMS, SequentialPhragmen};

	match solver {
		Solvers::SeqPhragmen { iterations } => {
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
		Solvers::PhragMMS { iterations } => {
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
	client: &WsClient,
) -> Result<(), Error<T>> {
	let linked_version = T::Version::get();
	let on_chain_version =
		rpc_helpers::rpc::<sp_version::RuntimeVersion>(client, "state_getRuntimeVersion", None)
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
	env_logger::Builder::from_default_env()
		.format_module_path(true)
		.format_level(true)
		.init();
	let Opt { shared, command } = Opt::from_args();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", shared.uri);

	let client = loop {
		let maybe_client = WsClientBuilder::default()
			.connection_timeout(std::time::Duration::new(20, 0))
			.max_request_body_size(u32::MAX)
			.build(&shared.uri)
			.await;
		match maybe_client {
			Ok(client) => break client,
			Err(why) => {
				log::warn!(
					target: LOG_TARGET,
					"failed to connect to client due to {:?}, retrying soon..",
					why
				);
				std::thread::sleep(std::time::Duration::from_millis(2500));
			},
		}
	};

	let chain = rpc_helpers::rpc::<String>(&client, "system_chain", None)
		.await
		.expect("system_chain infallible; qed.");
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
		check_versions::<Runtime>(&client).await
	};

	let signer_account = any_runtime! {
		signer::signer_uri_from_string::<Runtime>(&shared.seed, &client)
			.await
			.expect("Provided account is invalid, terminating.")
	};

	let outcome = any_runtime! {
		match command.clone() {
			Command::Monitor(c) => monitor_cmd(&client, shared, c, signer_account).await
				.map_err(|e| {
					log::error!(target: LOG_TARGET, "Monitor error: {:?}", e);
				}),
			Command::DryRun(c) => dry_run_cmd(&client, shared, c, signer_account).await
				.map_err(|e| {
					log::error!(target: LOG_TARGET, "DryRun error: {:?}", e);
				}),
			Command::EmergencySolution(c) => emergency_solution_cmd(shared.clone(), c).await
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
}
