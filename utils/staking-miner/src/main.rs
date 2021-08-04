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

use jsonrpsee_ws_client::{WsClient, WsClientBuilder};
use remote_externalities::{Builder, Mode, OnlineConfig};
use sp_runtime::traits::Block as BlockT;
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
					raw_solution: EPM::RawSolution<EPM::CompactOf<Runtime>>,
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

					let local_call = EPMCall::<Runtime>::submit(raw_solution, witness);
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
						target: crate::LOG_TARGET, "constructed extrinsic {}",
						sp_core::hexdisplay::HexDisplay::from(&extrinsic.encode())
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
					use $crate::polkadot_runtime_exports::*;
					$($code)*
				},
				$crate::AnyRuntime::Kusama => {
					use $crate::kusama_runtime_exports::*;
					$($code)*
				},
				$crate::AnyRuntime::Westend => {
					use $crate::westend_runtime_exports::*;
					$($code)*
				}
			}
		}
	}
}

#[derive(Debug, thiserror::Error)]
enum Error {
	Io(#[from] std::io::Error),
	Jsonrpsee(#[from] jsonrpsee_ws_client::types::Error),
	Codec(#[from] codec::Error),
	Crypto(sp_core::crypto::SecretStringError),
	RemoteExternalities(&'static str),
	PalletMiner(EPM::unsigned::MinerError),
	PalletElection(EPM::ElectionError),
	PalletFeasibility(EPM::FeasibilityError),
	AccountDoesNotExists,
	IncorrectPhase,
	AlreadySubmitted,
}

impl From<sp_core::crypto::SecretStringError> for Error {
	fn from(e: sp_core::crypto::SecretStringError) -> Error {
		Error::Crypto(e)
	}
}

impl From<EPM::unsigned::MinerError> for Error {
	fn from(e: EPM::unsigned::MinerError) -> Error {
		Error::PalletMiner(e)
	}
}

impl From<EPM::ElectionError> for Error {
	fn from(e: EPM::ElectionError) -> Error {
		Error::PalletElection(e)
	}
}

impl From<EPM::FeasibilityError> for Error {
	fn from(e: EPM::FeasibilityError) -> Error {
		Error::PalletFeasibility(e)
	}
}

impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		<Error as std::fmt::Debug>::fmt(self, f)
	}
}

#[derive(Debug, Clone, StructOpt)]
enum Command {
	/// Monitor for the phase being signed, then compute.
	Monitor(MonitorConfig),
	/// Just compute a solution now, and don't submit it.
	DryRun(DryRunConfig),
	/// Provide a solution that can be submitted to the chain as an emergency response.
	EmergencySolution,
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

	#[structopt(long, short, default_value = "10")]
	iterations: usize,
}

#[derive(Debug, Clone, StructOpt)]
struct DryRunConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[structopt(long)]
	at: Option<Hash>,

	#[structopt(long, short, default_value = "10")]
	iterations: usize,
}

#[derive(Debug, Clone, StructOpt)]
struct SharedConfig {
	/// The `ws` node to connect to.
	#[structopt(long, default_value = DEFAULT_URI)]
	uri: String,

	/// The file from which we read the account seed.
	///
	/// WARNING: don't use an account with a large stash for this. Based on how the bot is
	/// configured, it might re-try lose funds through transaction fees/deposits.
	#[structopt(long)]
	account_seed: std::path::PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct Opt {
	/// The `ws` node to connect to.
	#[structopt(flatten)]
	shared: SharedConfig,

	#[structopt(subcommand)]
	command: Command,
}

/// Build the `Ext` at `hash` with all the data of `ElectionProviderMultiPhase` and `Staking`
/// stored.
async fn create_election_ext<T: EPM::Config, B: BlockT>(
	uri: String,
	at: Option<B::Hash>,
	with_staking: bool,
) -> Result<Ext, Error> {
	use frame_support::{storage::generator::StorageMap, traits::PalletInfo};
	use sp_core::hashing::twox_128;

	Builder::<B>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: uri.into(),
			at,
			modules: if with_staking {
				vec![
					<T as frame_system::Config>::PalletInfo::name::<EPM::Pallet<T>>()
						.expect("Pallet always has name; qed.")
						.to_string(),
					<T as frame_system::Config>::PalletInfo::name::<pallet_staking::Pallet<T>>()
						.expect("Pallet always has name; qed.")
						.to_string(),
				]
			} else {
				vec![<T as frame_system::Config>::PalletInfo::name::<EPM::Pallet<T>>()
					.expect("Pallet always has name; qed.")
					.to_string()]
			},
			..Default::default()
		}))
		.inject_hashed_prefix(&<frame_system::BlockHash<T>>::prefix_hash())
		.inject_hashed_key(&[twox_128(b"System"), twox_128(b"Number")].concat())
		.build()
		.await
		.map_err(|why| Error::RemoteExternalities(why))
}

/// Compute the election at the given block number. It expects to NOT be `Phase::Off`. In other
/// words, the snapshot must exists on the given externalities.
fn mine_unchecked<T: EPM::Config>(
	ext: &mut Ext,
	iterations: usize,
	do_feasibility: bool,
) -> Result<(EPM::RawSolution<EPM::CompactOf<T>>, u32), Error> {
	ext.execute_with(|| {
		let (solution, _) = <EPM::Pallet<T>>::mine_solution(iterations)?;
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

#[allow(unused)]
fn mine_dpos<T: EPM::Config>(ext: &mut Ext) -> Result<(), Error> {
	ext.execute_with(|| {
		use std::collections::BTreeMap;
		use EPM::RoundSnapshot;
		let RoundSnapshot { voters, .. } = EPM::Snapshot::<T>::get().unwrap();
		let desired_targets = EPM::DesiredTargets::<T>::get().unwrap();
		let mut candidates_and_backing = BTreeMap::<T::AccountId, u128>::new();
		voters.into_iter().for_each(|(who, stake, targets)| {
			if targets.len() == 0 {
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

	let chain = rpc_helpers::rpc::<String>(&client, "system_chain", params! {})
		.await
		.expect("system_chain infallible; qed.");
	match chain.to_lowercase().as_str() {
		"polkadot" | "development" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormat::PolkadotAccount,
			);
			// safety: this program will always be single threaded, thus accessing global static is
			// safe.
			unsafe {
				RUNTIME = AnyRuntime::Polkadot;
			}
		},
		"kusama" | "kusama-dev" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormat::KusamaAccount,
			);
			// safety: this program will always be single threaded, thus accessing global static is
			// safe.
			unsafe {
				RUNTIME = AnyRuntime::Kusama;
			}
		},
		"westend" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormat::PolkadotAccount,
			);
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

	let signer_account = any_runtime! {
		signer::read_signer_uri::<_, Runtime>(&shared.account_seed, &client)
			.await
			.expect("Provided account is invalid, terminating.")
	};

	let outcome = any_runtime! {
		match command.clone() {
			Command::Monitor(c) => monitor_cmd(&client, shared, c, signer_account).await,
			// --------------------^^ comes from the macro prelude, needs no generic.
			Command::DryRun(c) => dry_run_cmd(&client, shared, c, signer_account).await,
			Command::EmergencySolution => emergency_solution_cmd(shared.clone()).await,
		}
	};
	log::info!(target: LOG_TARGET, "round of execution finished. outcome = {:?}", outcome);
}

#[cfg(test)]
mod tests {
	use super::*;

	fn get_version<T: frame_system::Config>() -> sp_version::RuntimeVersion {
		use frame_support::traits::Get;
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
