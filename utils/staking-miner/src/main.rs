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

use prelude::*;
use sp_runtime::traits::{Saturating, Block as BlockT};
use structopt::StructOpt;
use jsonrpsee_ws_client::{WsClientBuilder, WsClient};
use remote_externalities::{Builder, OnlineConfig, Mode};
use std::{
	path::{Path, PathBuf},
};
use sp_core::crypto::Pair as _;

pub(crate) enum AnyRuntime {
	Polkadot,
	Kusama,
	Westend,
}

pub(crate) static mut RUNTIME: AnyRuntime = AnyRuntime::Polkadot;

macro_rules! construct_runtime_prelude {
	($runtime:ident, $npos:ty, $signed_extra:expr) => {
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
						) -> UncheckedExtrinsic {
							use codec::Encode as _;
							use sp_core::Pair as _;
							use sp_runtime::traits::StaticLookup as _;

							let crate::Signer { account, pair, .. } = signer;

							let local_call = EPMCall::<Runtime>::submit(raw_solution, witness);
							// this one is a bit tricky.. for now I just "hope" it does not change.
							let call = Call::ElectionProviderMultiPhase(local_call);

							let extra: SignedExtra = $signed_extra;
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

construct_runtime_prelude!(
	polkadot,
	NposCompactSolution16,
	(
		frame_system::CheckSpecVersion::<Runtime>::new(),
		frame_system::CheckTxVersion::<Runtime>::new(),
		frame_system::CheckGenesis::<Runtime>::new(),
		frame_system::CheckMortality::<Runtime>::from(sp_runtime::generic::Era::Immortal),
		frame_system::CheckNonce::<Runtime>::from(0),
		frame_system::CheckWeight::<Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(0),
		runtime_common::claims::PrevalidateAttests::<Runtime>::new(),
	)
);
construct_runtime_prelude!(
	kusama,
	NposCompactSolution24,
	(
		frame_system::CheckSpecVersion::<Runtime>::new(),
		frame_system::CheckTxVersion::<Runtime>::new(),
		frame_system::CheckGenesis::<Runtime>::new(),
		frame_system::CheckMortality::<Runtime>::from(sp_runtime::generic::Era::Immortal),
		frame_system::CheckNonce::<Runtime>::from(0),
		frame_system::CheckWeight::<Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(0),
	)
);
construct_runtime_prelude!(
	westend,
	NposCompactSolution16,
	(
		frame_system::CheckSpecVersion::<Runtime>::new(),
		frame_system::CheckTxVersion::<Runtime>::new(),
		frame_system::CheckGenesis::<Runtime>::new(),
		frame_system::CheckMortality::<Runtime>::from(sp_runtime::generic::Era::Immortal),
		frame_system::CheckNonce::<Runtime>::from(0),
		frame_system::CheckWeight::<Runtime>::new(),
		pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(0),
	)
);

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
					use $crate::polkadot_runtime_exports::*;
					$($code)*
				},
				$crate::AnyRuntime::Westend => {
					use $crate::polkadot_runtime_exports::*;
					$($code)*
				}
			}
		}
	}
}

#[derive(codec::Encode, codec::Decode, Clone, Copy, Debug)]
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
				if percent.is_zero() {
					MinerProfile::TrimmedVoters(percent.saturating_sub(Percent::from_percent(10)))
				} else {
					MinerProfile::Terminate
				}
			}
			MinerProfile::Terminate => panic!("miner profile is to terminate."),
		}
	}
}

#[derive(Debug, Clone)]
enum Error {
	KeyFileNotFound,
	KeyFileCorrupt,
	IncorrectPhase,
	AlreadySubmitted,
	SnapshotUnavailable,
	RpcError,
	CodecError,
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
	/// The current account nonce.
	// TODO: this could be made generic or sth.
	nonce: u32,
}

#[derive(Debug, Clone, StructOpt)]
enum Command {
	/// Monitor for the phase being signed, then compute.
	Monitor(MonitorConfig),
	/// Just compute a solution now, and don't submit it.
	DryRun,
}

#[derive(Debug, Clone, StructOpt)]
struct MonitorConfig {
	#[structopt(long, default_value = "head", possible_values = &["head", "finalized"])]
	listen: String,
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
#[structopt(name = "staking-miner")]
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
		client.request::<Ret>(method, params).await.map_err(|err| {
			log::error!(
				target: LOG_TARGET,
				"rpc error in {}: {:?}",
				method,
				err
			);
			Error::RpcError
		})
	}

	/// Make the rpc request, decode the outcome into `Dec`. Don't use for storage, it will fail for
	/// non-existent storage items.
	pub(crate) async fn rpc_decode<
		'a,
		Dec: codec::Decode,
	>(
		client: &WsClient,
		method: &'a str,
		params: JsonRpcParams<'a>,
	) -> Result<Dec, Error> {
		let bytes = rpc::<sp_core::Bytes>(client, method, params).await?;
		<Dec as codec::Decode>::decode(&mut &*bytes.0).map_err(|err| {
			log::error!(target: LOG_TARGET, "decode error in {:?} with data: {:?}", err, bytes);
			Error::CodecError
		})
	}

	/// Get the storage item.
	pub(crate) async fn get_storage<'a, T: codec::Decode>(
		client: &WsClient,
		params: JsonRpcParams<'a>,
	) -> Result<Option<T>, Error> {
		let maybe_bytes = rpc::<Option<sp_core::Bytes>>(client, "state_getStorage", params).await?;
		if let Some(bytes) = maybe_bytes {
			let decoded = <T as codec::Decode>::decode(&mut &*bytes.0).map_err(|err| {
				log::error!(target: LOG_TARGET, "decode error in {:?} with data: {:?}", err, bytes);
				Error::CodecError
			})?;
			Ok(Some(decoded))
		} else {
			Ok(None)
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;

		#[test]
		fn can_read_finalized_head() {
			todo!()
		}

		fn can_read_storage_value() {
			todo!()
		}

		fn can_read_storage_map() {
			todo!()
		}

		fn can_submit_dry_run() {
			todo!()
		}
	}
}

pub use rpc_helpers::*;

/// Build the `Ext` at `hash` with all the data of `ElectionProviderMultiPhase` and `Staking`
/// stored.
async fn create_election_ext<T: EPM::Config, B: BlockT>(
	uri: String,
	at: B::Hash,
	with_staking: bool,
) -> Ext {
	use frame_support::storage::generator::StorageMap;
	use frame_support::traits::PalletInfo;
	let system_block_hash_key = <frame_system::BlockHash<T>>::prefix_hash();

	Builder::<B>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: uri.into(),
			at: Some(at),
			modules: if with_staking {
				vec![
					<T as frame_system::Config>::PalletInfo::name::<EPM::Pallet<T>>().expect("Pallet always has name; qed.").to_string(),
					// NOTE: change when staking moves to frame v2.
					"Staking".to_owned(),
				]
			} else {
				vec![
					<T as frame_system::Config>::PalletInfo::name::<EPM::Pallet<T>>().expect("Pallet always has name; qed.").to_string(),
				]
			},
			..Default::default()
		}))
		.raw_prefix(&system_block_hash_key)
		.build()
		.await
		.unwrap()
}

/// Compute the election at the given block number. It expects to NOT be `Phase::Off`. In other
/// words, the snapshot must exists on the given externalities.
fn mine_unchecked<T: EPM::Config>(ext: &mut Ext) -> (EPM::RawSolution<EPM::CompactOf<T>>, u32) {
	ext.execute_with(|| {
		let (solution, _) = <EPM::Pallet<T>>::mine_solution(10).unwrap();
		let witness = <EPM::SignedSubmissions<T>>::decode_len().unwrap_or_default();
		(solution, witness as u32)
	})
}

/// Read the signer account's uri from the given `path`.
async fn read_signer_uri<
	P: AsRef<Path>,
	T: frame_system::Config<AccountId = AccountId, Index = Index>,
>(
	path: P,
	client: &WsClient,
) -> Result<Signer, Error> {
	let mut uri = std::fs::read_to_string(path).map_err(|_| Error::KeyFileNotFound)?;

	// trim any trailing garbage.
	let len = uri.trim_end_matches(&['\r', '\n'][..]).len();
	uri.truncate(len);

	let pair = Pair::from_string(&uri, None).map_err(|e| {
		log::error!(target: LOG_TARGET, "failed to prase key file: {:?}", e);
		Error::KeyFileCorrupt
	})?;
	let account = T::AccountId::from(pair.public());

	let info = crate::get_storage::<frame_system::AccountInfo<T::Index, T::AccountData>>(
		client,
		params! {sp_core::storage::StorageKey(<frame_system::Account<T>>::hashed_key_for(&account))},
	)
	.await?.expect("provided account does not exist.");
	log::info!(target: LOG_TARGET, "loaded account {:?}, info: {:?}", &account, info);
	Ok(Signer { account, pair, uri, nonce: info.nonce })
}

#[tokio::main]
async fn main() {
	env_logger::Builder::from_default_env().format_module_path(true).format_level(true).init();
	let Opt { shared, command } = Opt::from_args();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", shared.uri);

	let client = WsClientBuilder::default()
		.connection_timeout(std::time::Duration::new(20, 0))
		.build(&shared.uri)
		.await
		.unwrap();

	let chain = rpc::<String>(&client, "system_chain", params! {})
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
		let signer = read_signer_uri::<_, Runtime>(&shared.account_seed, &client).await.unwrap();
		match command {
			Command::Monitor(c) => monitor_cmd(client, shared, c, signer).await,
			// --------------------^^ comes from the macro prelude, needs no generic.
			Command::DryRun => dry_run_cmd(client, shared, signer).await,
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
		let mut ext = create_election_ext(TEST_URI.to_owned(), hash, true).await;
		force_create_snapshot(&mut ext);
		mine_unchecked(&mut ext);
	}
}
