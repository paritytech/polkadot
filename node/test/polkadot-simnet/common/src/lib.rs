// Copyright 2020 Parity Technologies (UK) Ltd.
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

#![deny(unused_extern_crates, missing_docs)]

//! Utilities for End to end runtime tests

use codec::Encode;
use democracy::{AccountVote, Conviction, Vote};
use grandpa::GrandpaBlockImport;
use polkadot_runtime::{
	CouncilCollective, Event, FastTrackVotingPeriod, Runtime, RuntimeApi, TechnicalCollective,
};
use polkadot_runtime_common::claims;
use sc_consensus_babe::BabeBlockImport;
use sc_consensus_manual_seal::consensus::babe::SlotTimestampProvider;
use sc_executor::NativeElseWasmExecutor;
use sc_service::{TFullBackend, TFullClient};
use sp_runtime::{app_crypto::sp_core::H256, generic::Era, AccountId32};
use std::{error::Error, future::Future, str::FromStr};
use support::{weights::Weight, StorageValue};
use test_runner::{
	build_runtime, client_parts, task_executor, ChainInfo, ConfigOrChainSpec, Node,
	SignatureVerificationOverride,
};

type BlockImport<B, BE, C, SC> = BabeBlockImport<B, C, GrandpaBlockImport<BE, B, C, SC>>;
type Block = polkadot_primitives::v1::Block;
type SelectChain = sc_consensus::LongestChain<TFullBackend<Block>, Block>;

/// Declare an instance of the native executor named `ExecutorDispatch`. Include the wasm binary as the
/// equivalent wasm code.
pub struct ExecutorDispatch;

impl sc_executor::NativeExecutionDispatch for ExecutorDispatch {
	type ExtendHostFunctions =
		(benchmarking::benchmarking::HostFunctions, SignatureVerificationOverride);

	fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
		polkadot_runtime::api::dispatch(method, data)
	}

	fn native_version() -> sc_executor::NativeVersion {
		polkadot_runtime::native_version()
	}
}

/// `ChainInfo` implementation.
pub struct PolkadotChainInfo;

impl ChainInfo for PolkadotChainInfo {
	type Block = Block;
	type ExecutorDispatch = ExecutorDispatch;
	type Runtime = Runtime;
	type RuntimeApi = RuntimeApi;
	type SelectChain = SelectChain;
	type BlockImport = BlockImport<
		Self::Block,
		TFullBackend<Self::Block>,
		TFullClient<Self::Block, RuntimeApi, NativeElseWasmExecutor<ExecutorDispatch>>,
		Self::SelectChain,
	>;
	type SignedExtras = polkadot_runtime::SignedExtra;
	type InherentDataProviders =
		(SlotTimestampProvider, sp_consensus_babe::inherents::InherentDataProvider);

	fn signed_extras(from: <Runtime as system::Config>::AccountId) -> Self::SignedExtras {
		(
			system::CheckSpecVersion::<Runtime>::new(),
			system::CheckTxVersion::<Runtime>::new(),
			system::CheckGenesis::<Runtime>::new(),
			system::CheckMortality::<Runtime>::from(Era::Immortal),
			system::CheckNonce::<Runtime>::from(system::Pallet::<Runtime>::account_nonce(from)),
			system::CheckWeight::<Runtime>::new(),
			transaction_payment::ChargeTransactionPayment::<Runtime>::from(0),
			claims::PrevalidateAttests::<Runtime>::new(),
		)
	}
}

/// Dispatch with root origin, via pallet-democracy
pub async fn dispatch_with_root<T>(
	call: impl Into<<T::Runtime as system::Config>::Call>,
	node: &Node<T>,
) -> Result<(), Box<dyn Error>>
where
	T: ChainInfo<
		Block = Block,
		ExecutorDispatch = ExecutorDispatch,
		Runtime = Runtime,
		RuntimeApi = RuntimeApi,
		SelectChain = SelectChain,
		BlockImport = BlockImport<
			Block,
			TFullBackend<Block>,
			TFullClient<Block, RuntimeApi, NativeElseWasmExecutor<ExecutorDispatch>>,
			SelectChain,
		>,
		SignedExtras = polkadot_runtime::SignedExtra,
	>,
{
	type DemocracyCall = democracy::Call<Runtime>;
	type CouncilCollectiveEvent = collective::Event<Runtime, CouncilCollective>;
	type CouncilCollectiveCall = collective::Call<Runtime, CouncilCollective>;
	type TechnicalCollectiveCall = collective::Call<Runtime, TechnicalCollective>;
	type TechnicalCollectiveEvent = collective::Event<Runtime, TechnicalCollective>;

	// here lies a black mirror esque copy of on chain whales.
	let whales = vec![
		"1rvXMZpAj9nKLQkPFCymyH7Fg3ZyKJhJbrc7UtHbTVhJm1A",
		"15j4dg5GzsL1bw2U2AWgeyAk6QTxq43V7ZPbXdAmbVLjvDCK",
	]
	.into_iter()
	.map(|account| AccountId32::from_str(account).unwrap())
	.collect::<Vec<_>>();

	// and these
	let (technical_collective, council_collective) = node.with_state(|| {
		(
			collective::Members::<Runtime, TechnicalCollective>::get(),
			collective::Members::<Runtime, CouncilCollective>::get(),
		)
	});

	// hash of the proposal in democracy
	let proposal_hash = {
		// note the call (pre-image?) of the call.
		node.submit_extrinsic(
			DemocracyCall::note_preimage(call.into().encode()),
			Some(whales[0].clone()),
		)
		.await?;
		node.seal_blocks(1).await;

		// fetch proposal hash from event emitted by the runtime
		let events = node.events();
		events
			.iter()
			.filter_map(|event| match event.event {
				Event::Democracy(democracy::Event::PreimageNoted(ref proposal_hash, _, _)) =>
					Some(proposal_hash.clone()),
				_ => None,
			})
			.next()
			.ok_or_else(|| {
				format!("democracy::Event::PreimageNoted not found in events: {:#?}", events)
			})?
	};

	// submit external_propose call through council collective
	{
		let external_propose =
			DemocracyCall::external_propose_majority(proposal_hash.clone().into());
		let length = external_propose.using_encoded(|x| x.len()) as u32 + 1;
		let weight = Weight::MAX / 100_000_000;
		let proposal = CouncilCollectiveCall::propose(
			council_collective.len() as u32,
			Box::new(external_propose.clone().into()),
			length,
		);

		node.submit_extrinsic(proposal.clone(), Some(council_collective[0].clone()))
			.await?;
		node.seal_blocks(1).await;

		// fetch proposal index from event emitted by the runtime
		let events = node.events();
		let (index, hash): (u32, H256) = events
			.iter()
			.filter_map(|event| match event.event {
				Event::Council(CouncilCollectiveEvent::Proposed(_, index, ref hash, _)) =>
					Some((index, hash.clone())),
				_ => None,
			})
			.next()
			.ok_or_else(|| {
				format!("CouncilCollectiveEvent::Proposed not found in events: {:#?}", events)
			})?;

		// vote
		for member in &council_collective[1..] {
			let call = CouncilCollectiveCall::vote(hash.clone(), index, true);
			node.submit_extrinsic(call, Some(member.clone())).await?;
		}
		node.seal_blocks(1).await;

		// close vote
		let call = CouncilCollectiveCall::close(hash, index, weight, length);
		node.submit_extrinsic(call, Some(council_collective[0].clone())).await?;
		node.seal_blocks(1).await;

		// assert that proposal has been passed on chain
		let events = node
			.events()
			.into_iter()
			.filter(|event| match event.event {
				Event::Council(CouncilCollectiveEvent::Closed(_hash, _, _)) if hash == _hash =>
					true,
				Event::Council(CouncilCollectiveEvent::Approved(_hash)) if hash == _hash => true,
				Event::Council(CouncilCollectiveEvent::Executed(_hash, Ok(())))
					if hash == _hash =>
					true,
				_ => false,
			})
			.collect::<Vec<_>>();

		// make sure all 3 events are in state
		assert_eq!(
			events.len(),
			3,
			"CouncilCollectiveEvent::{{Closed, Approved, Executed}} not found in events: {:#?}",
			node.events(),
		);
	}

	// next technical collective must fast track the proposal.
	{
		let fast_track =
			DemocracyCall::fast_track(proposal_hash.into(), FastTrackVotingPeriod::get(), 0);
		let weight = Weight::MAX / 100_000_000;
		let length = fast_track.using_encoded(|x| x.len()) as u32 + 1;
		let proposal = TechnicalCollectiveCall::propose(
			technical_collective.len() as u32,
			Box::new(fast_track.into()),
			length,
		);

		node.submit_extrinsic(proposal, Some(technical_collective[0].clone())).await?;
		node.seal_blocks(1).await;

		let events = node.events();
		let (index, hash) = events
			.iter()
			.filter_map(|event| match event.event {
				Event::TechnicalCommittee(TechnicalCollectiveEvent::Proposed(
					_,
					index,
					ref hash,
					_,
				)) => Some((index, hash.clone())),
				_ => None,
			})
			.next()
			.ok_or_else(|| {
				format!("TechnicalCollectiveEvent::Proposed not found in events: {:#?}", events)
			})?;

		// vote
		for member in &technical_collective[1..] {
			let call = TechnicalCollectiveCall::vote(hash.clone(), index, true);
			node.submit_extrinsic(call, Some(member.clone())).await?;
		}
		node.seal_blocks(1).await;

		// close vote
		let call = TechnicalCollectiveCall::close(hash, index, weight, length);
		node.submit_extrinsic(call, Some(technical_collective[0].clone())).await?;
		node.seal_blocks(1).await;

		// assert that fast-track proposal has been passed on chain
		let events = node
			.events()
			.into_iter()
			.filter(|event| match event.event {
				Event::TechnicalCommittee(TechnicalCollectiveEvent::Closed(_hash, _, _))
					if hash == _hash =>
					true,
				Event::TechnicalCommittee(TechnicalCollectiveEvent::Approved(_hash))
					if hash == _hash =>
					true,
				Event::TechnicalCommittee(TechnicalCollectiveEvent::Executed(_hash, Ok(())))
					if hash == _hash =>
					true,
				_ => false,
			})
			.collect::<Vec<_>>();

		// make sure all 3 events are in state
		assert_eq!(
			events.len(),
			3,
			"TechnicalCollectiveEvent::{{Closed, Approved, Executed}} not found in events: {:#?}",
			node.events(),
		);
	}

	// now runtime upgrade proposal is a fast-tracked referendum we can vote for.
	let ref_index = node
		.events()
		.into_iter()
		.filter_map(|event| match event.event {
			Event::Democracy(democracy::Event::Started(index, _)) => Some(index),
			_ => None,
		})
		.next()
		.ok_or_else(|| {
			format!("democracy::Event::Started not found in events: {:#?}", node.events())
		})?;

	let call = DemocracyCall::vote(
		ref_index,
		AccountVote::Standard {
			vote: Vote { aye: true, conviction: Conviction::Locked1x },
			// 10 DOTS
			balance: 10_000_000_000_000,
		},
	);
	for whale in whales {
		node.submit_extrinsic(call.clone(), Some(whale)).await?;
	}

	// wait for fast track period.
	node.seal_blocks(FastTrackVotingPeriod::get() as usize).await;

	// assert that the proposal is passed by looking at events
	let events = node
		.events()
		.into_iter()
		.filter(|event| match event.event {
			Event::Democracy(democracy::Event::Passed(_index)) if _index == ref_index => true,
			Event::Democracy(democracy::Event::PreimageUsed(_hash, _, _))
				if _hash == proposal_hash =>
				true,
			Event::Democracy(democracy::Event::Executed(_index, Ok(()))) if _index == ref_index =>
				true,
			_ => false,
		})
		.collect::<Vec<_>>();

	// make sure all events were emitted
	assert_eq!(
		events.len(),
		3,
		"democracy::Event::{{Passed, PreimageUsed, Executed}} not found in events: {:#?}",
		node.events(),
	);
	Ok(())
}

/// Runs the test-runner as a binary.
pub fn run<F, Fut>(callback: F) -> Result<(), Box<dyn Error>>
where
	F: FnOnce(Node<PolkadotChainInfo>) -> Fut,
	Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
	use sc_cli::{CliConfiguration, SubstrateCli};
	use structopt::StructOpt;

	let mut tokio_runtime = build_runtime()?;
	let task_executor = task_executor(tokio_runtime.handle().clone());
	// parse cli args
	let cmd = <polkadot_cli::Cli as StructOpt>::from_args();
	// set up logging
	let filters = cmd.run.base.log_filters()?;
	let logger = sc_tracing::logging::LoggerBuilder::new(filters);
	logger.init()?;

	// set up the test-runner
	let config = cmd.create_configuration(&cmd.run.base, task_executor)?;
	sc_cli::print_node_infos::<polkadot_cli::Cli>(&config);
	let (rpc, task_manager, client, pool, command_sink, backend) =
		client_parts::<PolkadotChainInfo>(ConfigOrChainSpec::Config(config))?;
	let node =
		Node::<PolkadotChainInfo>::new(rpc, task_manager, client, pool, command_sink, backend);

	// hand off node.
	tokio_runtime.block_on(callback(node))?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_service::chain_spec::polkadot_development_config;
	use sp_keyring::sr25519::Keyring::Alice;
	use sp_runtime::{traits::IdentifyAccount, MultiSigner};

	#[test]
	fn test_runner() {
		let mut runtime = build_runtime().unwrap();
		let task_executor = task_executor(runtime.handle().clone());
		let (rpc, task_manager, client, pool, command_sink, backend) =
			client_parts::<PolkadotChainInfo>(ConfigOrChainSpec::ChainSpec(
				Box::new(polkadot_development_config().unwrap()),
				task_executor,
			))
			.unwrap();
		let node =
			Node::<PolkadotChainInfo>::new(rpc, task_manager, client, pool, command_sink, backend);

		runtime.block_on(async {
			// seals blocks
			node.seal_blocks(1).await;
			// submit extrinsics
			let alice = MultiSigner::from(Alice.public()).into_account();
			node.submit_extrinsic(system::Call::remark((b"hello world").to_vec()), Some(alice))
				.await
				.unwrap();

			// look ma, I can read state.
			let _events = node.with_state(|| system::Pallet::<Runtime>::events());
			// get access to the underlying client.
			let _client = node.client();
		});
	}
}
