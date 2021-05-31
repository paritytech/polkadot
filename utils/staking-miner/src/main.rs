// ## Staking Miner
//
// things to look out for:
// 1. weight (already taken care of).
// 2. length (already taken care of).
// 3. Important, but hard to do: memory usage of the chain. For this we need to bring in a substrate
//    wasm executor.
//
// ### Monitor
//
// 1. wait for a block where phase is signed.
// 2. ensure we don't have any previously submitted blocks.
// 3. store some global data about `MinerProfile` of the miners.
// 4. run an action appropriate to the current `MinerProfile`.
//
//
// ##### Miner States
//
// ```rust
// enum MinerProfile {
//   // seq-phragmen -> balancing(round) -> reduce
//   WithBalancing(round),
//   // seq-phragmen -> reduce
//   JustSeqPhragmen,
//   // trim the least staked `perbill%` nominators, then seq-phragmen -> reduce
//   TrimmedState(Perbill),
// }
// }

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
							let raw_payload = SignedPayload::new(call, extra).unwrap();
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

#[macro_export]
macro_rules! rpc {
	($client:ident<$method:tt, $ret:ty>, $($params:expr),*) => {
		{
			let mut _params = vec![];
			$(
				let param = serde_json::to_value($params).unwrap();
				_params.push(param);
			)*
			<
				jsonrpsee_ws_client::WsClient
				as
				jsonrpsee_ws_client::traits::Client
			>::request::<$ret>(
				&$client,
				stringify!($method),
				jsonrpsee_ws_client::v2::params::JsonRpcParams::Array(_params)
			).await
		}
	}
}

// TODO: these are pretty reusable. Maybe move to remote-ext.
#[macro_export]
macro_rules! rpc_decode {
	($client:ident<$method:tt, $ret:ty, $dec:ty>, $($params:expr),*) => {
		{
			let data = rpc!($client<$method, $ret>, $( $params ),* ).map(|d| d.0).unwrap();
			<$dec as codec::Decode>::decode(&mut &*data).unwrap()
		}
	}
}

#[macro_export]
macro_rules! storage_key {
	($storage:ty) => {{
			let __key = <$storage>::hashed_key();
			sp_core::storage::StorageKey(__key.to_vec())
		}};
}

/// Build the `Ext` at `hash` with all the data of `ElectionProviderMultiPhase` and `Staking`
/// stored.
async fn create_election_ext<T: EPM::Config + frame_system::Config, B: BlockT>(
	uri: String,
	at: B::Hash,
	with_staking: bool,
) -> Ext {
	use frame_support::storage::generator::StorageMap;
	let system_block_hash_key = <frame_system::BlockHash<T>>::prefix_hash();

	Builder::<B>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: uri.into(),
			at: Some(at),
			modules: if with_staking {
				// TODO: would save us some time, if we can also command remote-ext to scrape
				// certain maps from staking, not all of it.
				// TODO: fancy not hard-coding the name of these. They can be different in different
				// runtimes.
				// vec![<EPMPalletOf<T> as frame_system::Config>::PalletInfo::name(),
				// "Staking".to_owned()]
				vec!["ElectionProviderMultiPhase".to_owned(), "Staking".to_owned()]
			} else {
				vec!["ElectionProviderMultiPhase".to_owned()]
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
		let (solution, _) = <EPMPalletOf<T>>::mine_solution(10).unwrap();
		let witness = <EPM::SignedSubmissions<T>>::decode_len().unwrap_or_default();
		(solution, witness as u32)
	})
}

async fn read_key_from<
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
	log::info!(target: LOG_TARGET, "loaded account {:?}", &account);

	let key = sp_core::storage::StorageKey(<frame_system::Account<T>>::hashed_key_for(&account));
	let info = rpc_decode!(
		client<
			state_getStorage,
			sp_core::storage::StorageData,
			frame_system::AccountInfo<T::Index, T::AccountData>
		>,
		key
	);
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

	let chain = rpc!(client<system_chain, String>,).unwrap();
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
		let signer = read_key_from::<_, Runtime>(&shared.account_seed, &client).await.unwrap();
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
