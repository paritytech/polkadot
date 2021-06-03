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

// ## Staking Miner
//
// things to look out for:
// 1. weight (already taken care of).
// 2. length (already taken care of).
// 3. Important, but hard to do: memory usage of the chain. For this we need to bring in a substrate
//    wasm executor.

mod dry_run;
mod monitor;
mod prelude;

use jsonrpsee_ws_client::{WsClient, WsClientBuilder};
use prelude::*;
use remote_externalities::{Builder, Mode, OnlineConfig};
use sp_core::crypto::Pair as _;
use sp_runtime::traits::{Block as BlockT, Saturating};
use std::path::{Path, PathBuf};
use structopt::StructOpt;

pub(crate) enum AnyRuntime {
	Polkadot,
	Kusama,
	Westend,
}

pub(crate) static mut RUNTIME: AnyRuntime = AnyRuntime::Polkadot;

macro_rules! construct_runtime_prelude {
	($runtime:ident, $npos:ty) => {
		paste::paste! {
				#[allow(unused_import)]
				pub(crate) mod [<$runtime _runtime_exports>] {
					pub(crate) use crate::prelude::EPM;
					pub(crate) use [<$runtime _runtime>]::*;
					pub(crate) type NposCompactSolution = [<$runtime _runtime>]::$npos;
					pub(crate) use crate::monitor::[<monitor_cmd_ $runtime>] as monitor_cmd;
					pub(crate) use crate::dry_run::[<dry_run_cmd_ $runtime>] as dry_run_cmd;
					pub(crate) use private::{[<create_uxt_ $runtime>] as create_uxt};

					mod private {
						use super::*;
						pub(crate) fn [<create_uxt_ $runtime>](
							raw_solution: EPM::RawSolution<EPM::CompactOf<Runtime>>,
							witness: u32,
							signer: crate::Signer,
							nonce: crate::prelude::Index,
							tip: crate::prelude::Balance,
							era: sp_runtime::generic::Era,
						) -> UncheckedExtrinsic {
							use codec::Encode as _;
							use sp_core::Pair as _;
							use sp_runtime::traits::StaticLookup as _;

							let crate::Signer { account, pair, .. } = signer;

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
							log::debug!(target: crate::LOG_TARGET, "constructed extrinsic {:?}", extrinsic);
							extrinsic
						}
					}
				}
			}
	};
}

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

construct_runtime_prelude!(polkadot, NposCompactSolution16);
construct_runtime_prelude!(kusama, NposCompactSolution24);
construct_runtime_prelude!(westend, NposCompactSolution16);

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

#[derive(codec::Encode, codec::Decode, Clone, Copy, Debug)]
#[allow(unused)]
enum MinerProfile {
	/// seq-phragmen -> balancing(round) -> reduce
	WithBalancing(u32),
	/// seq-phragmen -> reduce
	JustSeqPhragmen,
	/// trim the least staked `perbill%` nominators, then seq-phragmen -> reduce
	TrimmedVoters(sp_runtime::Percent),
	/// Terminate. There's nothing else that we can do about this. Sorry.
	Terminate,
}

#[allow(unused)]
impl MinerProfile {
	/// Get the next miner profile to use, should this one fail.
	fn next(self) -> Self {
		use sp_runtime::Percent;
		match self {
			MinerProfile::WithBalancing(count) => {
				if count > 0 {
					MinerProfile::WithBalancing(count.saturating_sub(3))
				} else {
					MinerProfile::JustSeqPhragmen
				}
			}
			MinerProfile::JustSeqPhragmen => MinerProfile::TrimmedVoters(Percent::from_percent(90)),
			MinerProfile::TrimmedVoters(percent) => {
				if !percent.is_zero() {
					MinerProfile::TrimmedVoters(percent.saturating_sub(Percent::from_percent(10)))
				} else {
					MinerProfile::Terminate
				}
			}
			MinerProfile::Terminate => panic!("miner profile is to terminate."),
		}
	}
}

#[derive(Debug, thiserror::Error)]
enum Error {
	Io(#[from] std::io::Error),
	Jsonrpsee(#[from] jsonrpsee_ws_client::Error),
	Codec(#[from] codec::Error),
	Crypto(sp_core::crypto::SecretStringError),
	RemoteExternalities(&'static str),
	PalletMiner(EPM::unsigned::MinerError),
	PalletElection(EPM::ElectionError),
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

impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		<Error as std::fmt::Debug>::fmt(self, f)
	}
}

/// Some information about the signer. Redundant at this point, but makes life easier.
#[derive(Clone)]
struct Signer {
	/// The account id.
	account: AccountId,
	/// The full crypto key-pair.
	pair: Pair,
	/// The raw uri read from file.
	uri: String,
}

#[derive(Debug, Clone, StructOpt)]
enum Command {
	/// Monitor for the phase being signed, then compute.
	Monitor(MonitorConfig),
	/// Just compute a solution now, and don't submit it.
	DryRun(DryRunConfig),
}

#[derive(Debug, Clone, StructOpt)]
struct MonitorConfig {
	/// They type of event to listen to.
	///
	/// Typically, finalized is safer and there is no chance of anything going wrong, but it can be
	/// slower. It is recommended if the duration of the signed phase is longer than the a
	#[structopt(long, default_value = "head", possible_values = &["head", "finalized"])]
	listen: String,
}

#[derive(Debug, Clone, StructOpt)]
struct DryRunConfig {
	/// The block hash at which scraping happens. If none is provided, the latest head is used.
	#[structopt(long)]
	at: Option<Hash>,
}

#[derive(Debug, Clone, StructOpt)]
struct SharedConfig {
	/// The ws node to connect to.
	#[structopt(long, default_value = DEFAULT_URI)]
	uri: String,

	/// The file from which we read the account seed.
	#[structopt(long)]
	account_seed: PathBuf,
}

#[derive(Debug, Clone, StructOpt)]
struct Opt {
	/// The ws node to connect to.
	#[structopt(flatten)]
	shared: SharedConfig,

	#[structopt(subcommand)]
	command: Command,
}

mod rpc_helpers {
	use super::*;
	use jsonrpsee_ws_client::traits::Client;
	pub(crate) use jsonrpsee_ws_client::v2::params::JsonRpcParams;

	#[macro_export]
	macro_rules! params {
		($($param:expr),*) => {
			{
				let mut __params = vec![];
				$(
					__params.push(serde_json::to_value($param).expect("json serialization infallible; qed."));
				)*
				$crate::rpc_helpers::JsonRpcParams::Array(__params)
			}
		};
		() => {
			$crate::rpc::JsonRpcParams::NoParams,
		}
	}

	/// Make the rpc request, returning `Ret`.
	pub(crate) async fn rpc<'a, Ret: serde::de::DeserializeOwned>(
		client: &WsClient,
		method: &'a str,
		params: JsonRpcParams<'a>,
	) -> Result<Ret, Error> {
		client.request::<Ret>(method, params).await.map_err(Into::into)
	}

	/// Make the rpc request, decode the outcome into `Dec`. Don't use for storage, it will fail for
	/// non-existent storage items.
	pub(crate) async fn rpc_decode<'a, Dec: codec::Decode>(
		client: &WsClient,
		method: &'a str,
		params: JsonRpcParams<'a>,
	) -> Result<Dec, Error> {
		let bytes = rpc::<sp_core::Bytes>(client, method, params).await?;
		<Dec as codec::Decode>::decode(&mut &*bytes.0).map_err(Into::into)
	}

	/// Get the storage item.
	pub(crate) async fn get_storage<'a, T: codec::Decode>(
		client: &WsClient,
		params: JsonRpcParams<'a>,
	) -> Result<Option<T>, Error> {
		let maybe_bytes = rpc::<Option<sp_core::Bytes>>(client, "state_getStorage", params).await?;
		if let Some(bytes) = maybe_bytes {
			let decoded = <T as codec::Decode>::decode(&mut &*bytes.0)?;
			Ok(Some(decoded))
		} else {
			Ok(None)
		}
	}

	use codec::{EncodeLike, FullCodec};
	use frame_support::storage::{StorageMap, StorageValue};
	#[allow(unused)]
	pub(crate) async fn get_storage_value_frame_v2<'a, V: StorageValue<T>, T: FullCodec, Hash>(
		client: &WsClient,
		maybe_at: Option<Hash>,
	) -> Result<Option<V::Query>, Error>
	where
		V::Query: codec::Decode,
		Hash: serde::Serialize,
	{
		let key = <V as StorageValue<T>>::hashed_key();
		get_storage::<V::Query>(&client, params! { key, maybe_at }).await
	}

	#[allow(unused)]
	pub(crate) async fn get_storage_map_frame_v2<
		'a,
		Hash,
		KeyArg: EncodeLike<K>,
		K: FullCodec,
		T: FullCodec,
		M: StorageMap<K, T>,
	>(
		client: &WsClient,
		key: KeyArg,
		maybe_at: Option<Hash>,
	) -> Result<Option<M::Query>, Error>
	where
		M::Query: codec::Decode,
		Hash: serde::Serialize,
	{
		let key = <M as StorageMap<K, T>>::hashed_key_for(key);
		get_storage::<M::Query>(&client, params! { key, maybe_at }).await
	}
}

/// Build the `Ext` at `hash` with all the data of `ElectionProviderMultiPhase` and `Staking`
/// stored.
async fn create_election_ext<T: EPM::Config, B: BlockT>(
	uri: String,
	at: Option<B::Hash>,
	with_staking: bool,
) -> Result<Ext, Error> {
	use frame_support::{storage::generator::StorageMap, traits::PalletInfo};
	let system_block_hash_key = <frame_system::BlockHash<T>>::prefix_hash();

	Builder::<B>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: uri.into(),
			at,
			modules: if with_staking {
				vec![
					<T as frame_system::Config>::PalletInfo::name::<EPM::Pallet<T>>()
						.expect("Pallet always has name; qed.")
						.to_string(),
					// NOTE: change when staking moves to frame v2.
					"Staking".to_owned(),
				]
			} else {
				vec![<T as frame_system::Config>::PalletInfo::name::<EPM::Pallet<T>>()
					.expect("Pallet always has name; qed.")
					.to_string()
				]
			},
			..Default::default()
		}))
		.raw_prefix(&system_block_hash_key)
		.build()
		.await
		.map_err(|why| Error::RemoteExternalities(why))
}

/// Compute the election at the given block number. It expects to NOT be `Phase::Off`. In other
/// words, the snapshot must exists on the given externalities.
fn mine_unchecked<T: EPM::Config>(
	ext: &mut Ext,
) -> Result<(EPM::RawSolution<EPM::CompactOf<T>>, u32), Error> {
	ext.execute_with(|| {
		let (solution, _) = <EPM::Pallet<T>>::mine_solution(10)?;
		let witness = <EPM::SignedSubmissions<T>>::decode_len().unwrap_or_default();
		Ok((solution, witness as u32))
	})
}

fn mine_checked<T: EPM::Config>(
	ext: &mut Ext,
) -> Result<(EPM::RawSolution<EPM::CompactOf<T>>, u32), Error> {
	ext.execute_with(|| {
		let (solution, _) = <EPM::Pallet<T>>::mine_and_check(10)?;
		let witness = <EPM::SignedSubmissions<T>>::decode_len().unwrap_or_default();
		Ok((solution, witness as u32))
	})
}

fn mine_dpos<T: EPM::Config>(
	ext: &mut Ext,
) -> Result<(EPM::RawSolution<EPM::CompactOf<T>>, u32), Error> {
	ext.execute_with(|| {
		use EPM::RoundSnapshot;
		use std::collections::BTreeMap;
		let RoundSnapshot { voters, targets } = EPM::Snapshot::<T>::get().unwrap();
		let desired_targets = EPM::DesiredTargets::<T>::get().unwrap();
		let mut candidates_and_backing = BTreeMap::<T::AccountId, u128>::new();
		voters.into_iter().for_each(|(who, stake, targets)| {
			if targets.len() == 0 {
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

		todo!();
	})
}

pub(crate) async fn get_account_info<T: frame_system::Config>(
	client: &WsClient,
	who: &T::AccountId,
	maybe_at: Option<T::Hash>,
) -> Result<Option<frame_system::AccountInfo<Index, T::AccountData>>, Error> {
	rpc_helpers::get_storage::<frame_system::AccountInfo<Index, T::AccountData>>(
		client,
		params! {
			sp_core::storage::StorageKey(<frame_system::Account<T>>::hashed_key_for(&who)),
			maybe_at
		},
	)
	.await
}

/// Read the signer account's uri from the given `path`.
async fn read_signer_uri<
	P: AsRef<Path>,
	T: frame_system::Config<AccountId = AccountId, Index = Index>,
>(
	path: P,
	client: &WsClient,
) -> Result<Signer, Error> {
	let uri = std::fs::read_to_string(path)?;

	// trim any trailing garbage.
	let uri = uri.trim_end();

	let pair = Pair::from_string(&uri, None)?;
	let account = T::AccountId::from(pair.public());
	let _info =
		get_account_info::<T>(&client, &account, None).await?.ok_or(Error::AccountDoesNotExists)?;
	log::info!(target: LOG_TARGET, "loaded account {:?}, info: {:?}", &account, _info);
	Ok(Signer { account, pair, uri: uri.to_string() })
}

#[tokio::main]
async fn main() {
	env_logger::Builder::from_default_env().format_module_path(true).format_level(true).init();
	let Opt { shared, command } = Opt::from_args();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", shared.uri);

	let client = WsClientBuilder::default()
		.connection_timeout(std::time::Duration::new(20, 0))
		.max_request_body_size(u32::MAX)
		.build(&shared.uri)
		.await
		.unwrap();

	let chain = rpc_helpers::rpc::<String>(&client, "system_chain", params! {})
		.await
		.expect("system_chain infallible; qed.");
	match chain.to_lowercase().as_str() {
		"polkadot" | "development" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormat::PolkadotAccount,
			);
			unsafe {
				RUNTIME = AnyRuntime::Polkadot;
			}
		}
		"kusama" | "kusama-dev" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormat::KusamaAccount,
			);
			unsafe {
				RUNTIME = AnyRuntime::Kusama;
			}
		}
		"westend" => {
			sp_core::crypto::set_default_ss58_version(
				sp_core::crypto::Ss58AddressFormat::PolkadotAccount,
			);
			unsafe {
				RUNTIME = AnyRuntime::Westend;
			}
		}
		_ => panic!("unexpected chain: {:?}", chain),
	}
	log::info!(target: LOG_TARGET, "connected to chain {:?}", chain);

	let outcome = any_runtime! {
		let signer = read_signer_uri::<_, Runtime>(&shared.account_seed, &client)
			.await
			.expect("Provided account is invalid, terminating.");
		match command {
			Command::Monitor(c) => monitor_cmd(client, shared, c, signer).await,
			// --------------------^^ comes from the macro prelude, needs no generic.
			Command::DryRun(c) => dry_run_cmd(client, shared, c, signer).await,
			// ----------------^^ likewise.
		}
	};

	log::info!(target: LOG_TARGET, "execution finished. outcome = {:?}", outcome);
}

#[cfg(test)]
mod tests {
	use super::*;
	const TEST_URI: &'static str = DEFAULT_URI;

	fn get_version<T: frame_system::Config>() -> sp_version::RuntimeVersion {
		use frame_support::traits::Get;
		T::Version::get()
	}

	#[test]
	fn any_runtime_works() {
		unsafe {
			RUNTIME = AnyRuntime::Polkadot;
		}
		let polkadot_version = any_runtime! { get_version<Runtime>() };

		unsafe {
			RUNTIME = AnyRuntime::Kusama;
		}
		let kusama_version = any_runtime! { get_version<Runtime>() };

		assert_eq!(polkadot_version.spec_name, "polkadot".into());
		assert_eq!(kusama_version.spec_name, "kusama".into());
	}

	#[tokio::test]
	async fn can_compute_anytime() {
		env_logger::Builder::from_default_env().format_module_path(true).format_level(true).init();
		let client = WsClientBuilder::default().build(TEST_URI).await.unwrap();

		let hash = rpc!(client<chain_getFinalizedHead, Hash>,).unwrap();
		let mut ext = create_election_ext(TEST_URI.to_owned(), Some(hash), true).await;
		force_create_snapshot(&mut ext);
		mine_unchecked(&mut ext);
	}
}
