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

//! End to end runtime tests

use test_runner::{Node, ChainInfo, SignatureVerificationOverride};
use grandpa::GrandpaBlockImport;
use sc_service::{TFullBackend, TFullClient, Configuration, TaskManager, new_full_parts};
use sp_runtime::generic::Era;
use std::sync::Arc;
use sp_inherents::InherentDataProviders;
use sc_consensus_babe::BabeBlockImport;
use sp_keystore::SyncCryptoStorePtr;
use sp_keyring::sr25519::Keyring::Alice;
use polkadot_runtime_common::claims;
use sp_consensus_babe::AuthorityId;
use sc_consensus_manual_seal::{ConsensusDataProvider, consensus::babe::BabeConsensusDataProvider};
use sp_runtime::AccountId32;
use frame_support::{weights::Weight, StorageValue};
use pallet_democracy::{AccountVote, Conviction, Vote};
use polkadot_runtime::{FastTrackVotingPeriod, Runtime, RuntimeApi, Event, TechnicalCollective, CouncilCollective};
use std::str::FromStr;
use codec::Encode;

type BlockImport<B, BE, C, SC> = BabeBlockImport<B, C, GrandpaBlockImport<BE, B, C, SC>>;

sc_executor::native_executor_instance!(
	pub Executor,
	polkadot_runtime::api::dispatch,
	polkadot_runtime::native_version,
	(frame_benchmarking::benchmarking::HostFunctions, SignatureVerificationOverride),
);

/// ChainInfo implementation.
struct PolkadotChainInfo;

impl ChainInfo for PolkadotChainInfo {
    type Block = polkadot_primitives::v1::Block;
    type Executor = Executor;
    type Runtime = Runtime;
    type RuntimeApi = RuntimeApi;
    type SelectChain = sc_consensus::LongestChain<TFullBackend<Self::Block>, Self::Block>;
    type BlockImport = BlockImport<
        Self::Block,
        TFullBackend<Self::Block>,
        TFullClient<Self::Block, RuntimeApi, Self::Executor>,
        Self::SelectChain,
    >;
    type SignedExtras = polkadot_runtime::SignedExtra;

    fn signed_extras(from: <Runtime as frame_system::Config>::AccountId) -> Self::SignedExtras {
        (
            frame_system::CheckSpecVersion::<Runtime>::new(),
            frame_system::CheckTxVersion::<Runtime>::new(),
            frame_system::CheckGenesis::<Runtime>::new(),
            frame_system::CheckMortality::<Runtime>::from(Era::Immortal),
            frame_system::CheckNonce::<Runtime>::from(frame_system::Pallet::<Runtime>::account_nonce(from)),
            frame_system::CheckWeight::<Runtime>::new(),
            pallet_transaction_payment::ChargeTransactionPayment::<Runtime>::from(0),
            claims::PrevalidateAttests::<Runtime>::new(),
        )
    }

    fn create_client_parts(
        config: &Configuration,
    ) -> Result<
        (
            Arc<TFullClient<Self::Block, RuntimeApi, Self::Executor>>,
            Arc<TFullBackend<Self::Block>>,
            SyncCryptoStorePtr,
            TaskManager,
            InherentDataProviders,
            Option<
                Box<
                    dyn ConsensusDataProvider<
                        Self::Block,
                        Transaction = sp_api::TransactionFor<
                            TFullClient<Self::Block, RuntimeApi, Self::Executor>,
                            Self::Block,
                        >,
                    >,
                >,
            >,
            Self::SelectChain,
            Self::BlockImport,
        ),
        sc_service::Error,
    > {
        let (client, backend, keystore, task_manager) =
            new_full_parts::<Self::Block, RuntimeApi, Self::Executor>(config, None)?;
        let client = Arc::new(client);

        let inherent_providers = InherentDataProviders::new();
        let select_chain = sc_consensus::LongestChain::new(backend.clone());

        let (grandpa_block_import, ..) =
            grandpa::block_import(client.clone(), &(client.clone() as Arc<_>), select_chain.clone(), None)?;

        let (block_import, babe_link) = sc_consensus_babe::block_import(
            sc_consensus_babe::Config::get_or_compute(&*client)?,
            grandpa_block_import,
            client.clone(),
        )?;

        let consensus_data_provider = BabeConsensusDataProvider::new(
            client.clone(),
            keystore.sync_keystore(),
            &inherent_providers,
            babe_link.epoch_changes().clone(),
            vec![(AuthorityId::from(Alice.public()), 1000)],
        )
            .expect("failed to create ConsensusDataProvider");

        Ok((
            client,
            backend,
            keystore.sync_keystore(),
            task_manager,
            inherent_providers,
            Some(Box::new(consensus_data_provider)),
            select_chain,
            block_import,
        ))
    }

    fn dispatch_with_root(call: <Runtime as frame_system::Config>::Call, node: &mut Node<Self>) {
        type DemocracyCall = pallet_democracy::Call<Runtime>;
        type TechnicalCollectiveCall = pallet_collective::Call<Runtime, TechnicalCollective>;
        type CouncilCollectiveCall = pallet_collective::Call<Runtime, CouncilCollective>;

        // here lies a black mirror esque copy of on chain whales.
        let whales = vec![
            "1rvXMZpAj9nKLQkPFCymyH7Fg3ZyKJhJbrc7UtHbTVhJm1A",
            "15j4dg5GzsL1bw2U2AWgeyAk6QTxq43V7ZPbXdAmbVLjvDCK",
        ]
            .into_iter()
            .map(|account| AccountId32::from_str(account).unwrap())
            .collect::<Vec<_>>();

        // and these
        let (technical_collective, council_collective) = node.with_state(|| (
            pallet_collective::Members::<Runtime, TechnicalCollective>::get(),
            pallet_collective::Members::<Runtime, CouncilCollective>::get()
        ));

        // note the call (pre-image?) of the call.
        node.submit_extrinsic(DemocracyCall::note_preimage(call.encode()), whales[0].clone());
        node.seal_blocks(1);

        // fetch proposal hash from event emitted by the runtime
        let events = node.events();
        let proposal_hash = events.into_iter()
            .filter_map(|event| match event.event {
                Event::pallet_democracy(
                    pallet_democracy::RawEvent::PreimageNoted(proposal_hash, _, _)
                ) => Some(proposal_hash),
                _ => None
            })
            .next()
            .unwrap();

        // submit external_propose call through council
        let external_propose = DemocracyCall::external_propose_majority(proposal_hash.clone().into());
        let proposal_length = external_propose.using_encoded(|x| x.len()) as u32 + 1;
        let proposal_weight = Weight::MAX / 100_000_000;
        let proposal = CouncilCollectiveCall::propose(
            council_collective.len() as u32,
            Box::new(external_propose.clone().into()),
            proposal_length
        );

        node.submit_extrinsic(proposal.clone(), council_collective[0].clone());
        node.seal_blocks(1);

        // fetch proposal index from event emitted by the runtime
        let events = node.events();
        let (council_proposal_index, council_proposal_hash) = events.into_iter()
            .filter_map(|event| {
                match event.event {
                    Event::pallet_collective_Instance1(
                        pallet_collective::RawEvent::Proposed(_, index, proposal_hash, _)
                    ) => Some((index, proposal_hash)),
                    _ => None
                }
            })
            .next()
            .unwrap();

        // vote
        for member in &council_collective[1..] {
            let call = CouncilCollectiveCall::vote(council_proposal_hash.clone(), council_proposal_index, true);
            node.submit_extrinsic(call, member.clone());
        }
        node.seal_blocks(1);

        // close vote
        let call = CouncilCollectiveCall::close(council_proposal_hash, council_proposal_index, proposal_weight, proposal_length);
        node.submit_extrinsic(call, council_collective[0].clone());
        node.seal_blocks(1);

        // assert that proposal has been passed on chain
        let events = node.events()
            .into_iter()
            .filter(|event| {
                match event.event {
                    Event::pallet_collective_Instance1(pallet_collective::RawEvent::Closed(_, _, _)) |
                    Event::pallet_collective_Instance1(pallet_collective::RawEvent::Approved(_,)) |
                    Event::pallet_collective_Instance1(pallet_collective::RawEvent::Executed(_, Ok(()))) => true,
                    _ => false,
                }
            })
            .collect::<Vec<_>>();

        // make sure all 3 events are in state
        assert_eq!(events.len(), 3);

        // next technical collective must fast track the proposal.
        let fast_track = DemocracyCall::fast_track(proposal_hash.into(), FastTrackVotingPeriod::get(), 0);
        let proposal_weight = Weight::MAX / 100_000_000;
        let fast_track_length = fast_track.using_encoded(|x| x.len()) as u32 + 1;
        let proposal = TechnicalCollectiveCall::propose(
            technical_collective.len() as u32,
            Box::new(fast_track.into()),
            fast_track_length
        );

        node.submit_extrinsic(proposal, technical_collective[0].clone());
        node.seal_blocks(1);

        let (technical_proposal_index, technical_proposal_hash) = node.events()
            .into_iter()
            .filter_map(|event| {
                match event.event {
                    Event::pallet_collective_Instance2(
                        pallet_collective::RawEvent::Proposed(_, index, hash, _)
                    ) => Some((index, hash)),
                    _ => None
                }
            })
            .next()
            .unwrap();

        // vote
        for member in &technical_collective[1..] {
            let call = TechnicalCollectiveCall::vote(technical_proposal_hash.clone(), technical_proposal_index, true);
            node.submit_extrinsic(call, member.clone());
        }
        node.seal_blocks(1);

        // close vote
        let call = TechnicalCollectiveCall::close(
            technical_proposal_hash,
            technical_proposal_index,
            proposal_weight,
            fast_track_length,
        );
        node.submit_extrinsic(call, technical_collective[0].clone());
        node.seal_blocks(1);

        // assert that fast-track proposal has been passed on chain
        let collective_events = node.events()
            .into_iter()
            .filter(|event| {
                match event.event {
                    Event::pallet_collective_Instance2(pallet_collective::RawEvent::Closed(_, _, _)) |
                    Event::pallet_collective_Instance2(pallet_collective::RawEvent::Approved(_)) |
                    Event::pallet_collective_Instance2(pallet_collective::RawEvent::Executed(_, Ok(()))) => true,
                    _ => false,
                }
            })
            .collect::<Vec<_>>();

        // make sure all 3 events are in state
        assert_eq!(collective_events.len(), 3);

        // now runtime upgrade proposal is a fast-tracked referendum we can vote for.
        let referendum_index = events.into_iter()
            .filter_map(|event| match event.event {
                Event::pallet_democracy(pallet_democracy::Event::<Runtime>::Started(index, _)) => Some(index),
                _ => None,
            })
            .next()
            .unwrap();
        let call = DemocracyCall::vote(
            referendum_index,
            AccountVote::Standard {
                vote: Vote { aye: true, conviction: Conviction::Locked1x },
                // 10 DOTS
                balance: 10_000_000_000_000
            }
        );
        for whale in whales {
            node.submit_extrinsic(call.clone(), whale);
        }

        // wait for fast track period.
        node.seal_blocks(FastTrackVotingPeriod::get() as usize);

        // assert that the proposal is passed by looking at events
        let events = node.events()
            .into_iter()
            .filter(|event| {
                match event.event {
                    Event::pallet_democracy(pallet_democracy::RawEvent::Passed(_)) |
                    Event::pallet_democracy(pallet_democracy::RawEvent::PreimageUsed(_, _, _)) |
                    Event::pallet_democracy(pallet_democracy::RawEvent::Executed(_, true)) => true,
                    _ => false,
                }
            })
            .collect::<Vec<_>>();

        // make sure all events were emitted
        assert_eq!(events.len(), 3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_runner::NodeConfig;
    use log::LevelFilter;
    use sc_client_api::execution_extensions::ExecutionStrategies;
    use polkadot_service::chain_spec::polkadot_development_config;
    use sp_runtime::{MultiSigner, traits::IdentifyAccount};

    #[test]
    fn test_runner() {
        let config = NodeConfig {
            execution_strategies: ExecutionStrategies {
                syncing: sc_client_api::ExecutionStrategy::AlwaysWasm,
                importing: sc_client_api::ExecutionStrategy::AlwaysWasm,
                block_construction: sc_client_api::ExecutionStrategy::AlwaysWasm,
                offchain_worker: sc_client_api::ExecutionStrategy::AlwaysWasm,
                other: sc_client_api::ExecutionStrategy::AlwaysWasm,
            },
            // NOTE: when we have the polkadot db on CI we can change this to the
            // actual polkadot chain spec
            chain_spec: Box::new(polkadot_development_config().unwrap()),
            log_targets: vec![
                ("yamux", LevelFilter::Off),
                ("multistream_select", LevelFilter::Off),
                ("libp2p", LevelFilter::Off),
                ("jsonrpc_client_transports", LevelFilter::Off),
                ("sc_network", LevelFilter::Off),
                ("tokio_reactor", LevelFilter::Off),
                ("parity-db", LevelFilter::Off),
                ("sub-libp2p", LevelFilter::Off),
                ("sync", LevelFilter::Off),
                ("peerset", LevelFilter::Off),
                ("ws", LevelFilter::Off),
                ("sc_network", LevelFilter::Off),
                ("sc_service", LevelFilter::Off),
                ("sc_basic_authorship", LevelFilter::Off),
                ("telemetry-logger", LevelFilter::Off),
                ("sc_peerset", LevelFilter::Off),
                ("rpc", LevelFilter::Off),

                ("runtime", LevelFilter::Trace),
                ("babe", LevelFilter::Debug)
            ],
        };
        let mut node = Node::<PolkadotChainInfo>::new(config).unwrap();
        // seals blocks
        node.seal_blocks(1);
        // submit extrinsics
        let alice = MultiSigner::from(Alice.public()).into_account();
        node.submit_extrinsic(frame_system::Call::remark((b"hello world").to_vec()), alice);

        // look ma, I can read state.
        let _events = node.with_state(|| frame_system::Pallet::<Runtime>::events());
        // get access to the underlying client.
        let _client = node.client();
    }
}
