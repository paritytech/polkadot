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

mod common;

use common::*;
use sp_runtime::traits::Saturating;
use structopt::StructOpt;
use jsonrpsee_ws_client::{
	traits::{SubscriptionClient, Client},
	v2::params::JsonRpcParams,
	Subscription, WsClientBuilder, WsClient,
};
use remote_externalities::{Builder, OnlineConfig, Mode};
use polkadot_runtime::ElectionProviderMultiPhase as EPMPolkadot;
use pallet_election_provider_multi_phase as EPM;
use std::{
	path::{Path, PathBuf},
};
use sp_core::crypto::Pair as _;

const DEFAULT_URI: &'static str = "wss://rpc.polkadot.io";
const LOG_TARGET: &'static str = "staking-miner";

type EPMPalletPolkadot = EPM::Pallet<polkadot_runtime::Runtime>;
type Ext = sp_io::TestExternalities;
type Pair = sp_core::sr25519::Pair;

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
	KeyFileNotCorrupt,
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
#[structopt(name = "staking-miner")]
struct Opt {
	/// The ws node to connect to.
	#[structopt(long, default_value = DEFAULT_URI)]
	uri: String,

	#[structopt(subcommand)]
	command: Command,
}

macro_rules! rpc {
	($client:ident<$method:tt, $ret:ty>, $($params:expr),*) => {
		{
			let mut _params = vec![];
			$(
				let param = serde_json::to_value($params).unwrap();
				_params.push(param);
			)*
			$client.request::<$ret>(stringify!($method), JsonRpcParams::Array(_params)).await
		}
	}
}

macro_rules! rpc_decode {
	($client:ident<$method:tt, $ret:ty, $dec:ty>, $($params:expr),*) => {
		{
			let data = rpc!($client<$method, $ret>, $( $params ),* ).map(|d| d.0).unwrap();
			<$dec as codec::Decode>::decode(&mut &*data).unwrap()
		}
	}
}

macro_rules! storage_key {
	($storage:ty) => {{
		let __key = <$storage>::hashed_key();
		sp_core::storage::StorageKey(__key.to_vec())
	}};
}

/// Build the `Ext` at `hash` with all the data of `ElectionProviderMultiPhase` and `Staking`
/// stored.
async fn create_election_ext(uri: String, at: Hash) -> Ext {
	Builder::<polkadot_runtime::Block>::new()
		.mode(Mode::Online(OnlineConfig {
			transport: uri.into(),
			at: Some(at),
			modules: vec!["ElectionProviderMultiPhase".to_owned(), "Staking".to_owned()],
			..Default::default()
		}))
		.build()
		.await
		.unwrap()
}

/// Compute the election at the given block number. It expects to NOT be `Phase::Off`. In other
/// words, the snapshot must exists on the given externalities.
fn mine_unchecked(ext: &mut Ext) {
	ext.execute_with(|| {
		let (raw, _size) = EPMPalletPolkadot::mine_solution(10).unwrap();
		println!("raw score = {:?}", raw.score);
	})
}

/// Forcefully create the snapshot. This can be used to compute the election at anytime.
fn force_create_snapshot(ext: &mut Ext) {
	ext.execute_with(|| {
		if <EPM::Snapshot<polkadot_runtime::Runtime>>::exists() {
			log::info!(target: LOG_TARGET, "snapshot already exists.");
		} else {
			log::info!(target: LOG_TARGET, "creating a fake snapshot now.");
		}
		EPMPalletPolkadot::create_snapshot().unwrap();
	});
}

async fn ensure_signed_phase(client: &WsClient, at: Hash) -> Result<(), &'static str> {
	let key = storage_key!(
		pallet_election_provider_multi_phase::CurrentPhase::<polkadot_runtime::Runtime>
	);

	let phase = rpc_decode!(
		client<
			state_getStorage,
			sp_core::storage::StorageData,
			pallet_election_provider_multi_phase::Phase<BlockNumber>
		>,
		key,
		at
	);

	if phase.is_signed() {
		Ok(())
	} else {
		Err("Phase is not signed at this point.")
	}
}

/// Some information about the signer. Redundant at this point, but makes life easier.
struct Signer {
	account: AccountId,
	pair: Pair,
	raw_seed_hex: String,
}

fn read_key_from<P: AsRef<Path>>(path: P) -> Result<Signer, Error> {
	let uri = std::fs::read_to_string(path).map_err(|_| Error::KeyFileNotFound)?;
	let uri = if uri.starts_with("0x") { &uri[2..] } else { &uri };
	let raw_seed_hex = if uri.ends_with("\n") { &uri[..uri.len() - 1] } else { &uri };

	let bytes = hex::decode(raw_seed_hex.clone()).map_err(|_| Error::KeyFileNotCorrupt)?;
	assert_eq!(bytes.len(), 32, "key must be 32 bytes hex; qed.");
	let mut seed = [0u8; 32];
	seed.copy_from_slice(&bytes);
	let pair = Pair::from_seed(&seed);
	let account = AccountId::from(pair.public());
	Ok(Signer { account, pair, raw_seed_hex: raw_seed_hex.to_string() })
}

async fn ensure_no_previous_solution(
	client: &WsClient,
	at: Hash,
	us: AccountId,
) -> Result<(), &'static str> {
	use pallet_election_provider_multi_phase::{SignedSubmissions, signed::SignedSubmission};
	use polkadot_runtime::NposCompactSolution16;
	let key = storage_key!(SignedSubmissions::<polkadot_runtime::Runtime>);

	let queue = rpc_decode!(
		client<
			state_getStorage,
			sp_core::storage::StorageData,
			Vec<SignedSubmission<AccountId, Balance, NposCompactSolution16>>
		>,
		key,
		at
	);

	// if we have a solution in the queue, then don't do anything.
	if queue.iter().any(|ss| ss.who == us) {
		Err("We have already submitted a solution for this round.")
	} else {
		Ok(())
	}
}

async fn monitor(
	client: WsClient,
	config: MonitorConfig,
	signer: Signer,
) -> Result<(), &'static str> {
	let subscription_method = if config.listen == "heads" {
		"chain_subscribeNewHeads"
	} else {
		"chain_subscribeFinalizedHeads"
	};

	log::info!(target: LOG_TARGET, "subscribing to {:?}", subscription_method);
	let mut subscription: Subscription<Header> = client
		.subscribe(&subscription_method, JsonRpcParams::NoParams, "unsubscribe")
		.await
		.unwrap();

	loop {
		let now = subscription.next().await.unwrap();
		let hash = now.hash();
		log::debug!(target: LOG_TARGET, "new event at #{:?} ({:?})", now.number, hash);

		if ensure_signed_phase(&client, hash).await.is_err() {
			log::debug!(target: LOG_TARGET, "phase closed, not interested in this block at all.");
			continue;
		};

		if ensure_no_previous_solution(&client, hash, signer.account.clone()).await.is_err() {
			log::debug!(target: LOG_TARGET, "We already have a solution in this phase, skipping.");
			continue;
		}

		// mine a new solution, with the exact configuration of the on-chain stuff.
		let e
		EPMPalletPolkadot
	}
}

async fn dry_run(client: WsClient, _signer: Signer, ws_uri: &str) -> Result<(), &'static str> {
	let hash = rpc!(client<chain_getFinalizedHead, Hash>,).unwrap();
	let mut ext = create_election_ext(ws_uri.to_owned(), hash).await;
	force_create_snapshot(&mut ext);
	mine_unchecked(&mut ext);
	Ok(())
}

#[tokio::main]
async fn main() {
	env_logger::Builder::from_default_env().format_module_path(true).format_level(true).init();
	let opt = Opt::from_args();
	log::debug!(target: LOG_TARGET, "attempting to connect to {:?}", opt.uri);
	sp_core::crypto::set_default_ss58_version(sp_core::crypto::Ss58AddressFormat::PolkadotAccount);

	let client = WsClientBuilder::default()
		.connection_timeout(std::time::Duration::new(20, 0))
		.build(&opt.uri)
		.await
		.unwrap();

	let signer = read_key_from("./utils/staking-miner/key").unwrap();
	let outcome = match opt.command {
		Command::Monitor(c) => monitor(client, c, signer).await,
		Command::DryRun => dry_run(client, signer, &opt.uri).await,
	};

	log::info!(target: LOG_TARGET, "execution finished. outcome = {:?}", outcome);
}

#[cfg(test)]
mod tests {
	use super::*;
	const TEST_URI: &'static str = DEFAULT_URI;

	#[tokio::test]
	async fn can_compute_anytime() {
		env_logger::Builder::from_default_env().format_module_path(true).format_level(true).init();
		let client = WsClientBuilder::default().build(TEST_URI).await.unwrap();

		let hash = rpc!(client<chain_getFinalizedHead, Hash>,).unwrap();
		let mut ext = create_election_ext(TEST_URI.to_owned(), hash).await;
		force_create_snapshot(&mut ext);
		mine_unchecked(&mut ext);
	}
}
