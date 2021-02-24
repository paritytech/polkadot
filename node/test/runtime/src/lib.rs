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

use substrate_test_runner::{Node, ChainInfo, SignatureVerificationOverride};
use grandpa::GrandpaBlockImport;
use sc_service::{TFullBackend, TFullClient, Configuration, TaskManager, new_full_parts};
use sp_runtime::generic::Era;
use std::sync::Arc;
use sp_inherents::InherentDataProviders;
use sc_consensus_babe::BabeBlockImport;
use sp_keystore::SyncCryptoStorePtr;
use sp_keyring::sr25519::Keyring::Alice;
use polkadot_service::chain_spec::polkadot_config;
use polkadot_runtime_common::claims;
use polkadot_runtime::{Event, Runtime};
use sp_consensus_babe::AuthorityId;
use sc_consensus_manual_seal::{ConsensusDataProvider, consensus::babe::BabeConsensusDataProvider};
use sp_runtime::{traits::IdentifyAccount, MultiSigner, AccountId32};
use std::str::FromStr;
use polkadot_runtime::FastTrackVotingPeriod;
use pallet_democracy::{AccountVote, Vote, Conviction};

type BlockImport<B, BE, C, SC> = BabeBlockImport<B, C, GrandpaBlockImport<BE, B, C, SC>>;

sc_executor::native_executor_instance!(
	pub Executor,
	polkadot_runtime::api::dispatch,
	polkadot_runtime::native_version,
	SignatureVerificationOverride,
);

/// ChainInfo implementation.
struct PolkadotChainInfo;

impl ChainInfo for PolkadotChainInfo {
    type Block = polkadot_primitives::v1::Block;
    type Executor = Executor;
    type Runtime = polkadot_runtime::Runtime;
    type RuntimeApi = polkadot_runtime::RuntimeApi;
    type SelectChain = sc_consensus::LongestChain<TFullBackend<Self::Block>, Self::Block>;
    type BlockImport = BlockImport<
        Self::Block,
        TFullBackend<Self::Block>,
        TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>,
        Self::SelectChain,
    >;
    type SignedExtras = polkadot_runtime::SignedExtra;

    fn load_spec() -> Result<Box<dyn sc_service::ChainSpec>, String> {
        Ok(Box::new(polkadot_config()?))
    }

    fn signed_extras(from: <Self::Runtime as frame_system::Config>::AccountId) -> Self::SignedExtras {
        (
            frame_system::CheckSpecVersion::<Self::Runtime>::new(),
            frame_system::CheckTxVersion::<Self::Runtime>::new(),
            frame_system::CheckGenesis::<Self::Runtime>::new(),
            frame_system::CheckMortality::<Self::Runtime>::from(Era::Immortal),
            frame_system::CheckNonce::<Self::Runtime>::from(frame_system::Module::<Self::Runtime>::account_nonce(from)),
            frame_system::CheckWeight::<Self::Runtime>::new(),
            pallet_transaction_payment::ChargeTransactionPayment::<Self::Runtime>::from(0),
            claims::PrevalidateAttests::<Self::Runtime>::new(),
        )
    }

    fn create_client_parts(
        config: &Configuration,
    ) -> Result<
        (
            Arc<TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>>,
            Arc<TFullBackend<Self::Block>>,
            SyncCryptoStorePtr,
            TaskManager,
            InherentDataProviders,
            Option<
                Box<
                    dyn ConsensusDataProvider<
                        Self::Block,
                        Transaction = sp_api::TransactionFor<
                            TFullClient<Self::Block, Self::RuntimeApi, Self::Executor>,
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
            new_full_parts::<Self::Block, Self::RuntimeApi, Self::Executor>(config)?;
        let client = Arc::new(client);

        let inherent_providers = InherentDataProviders::new();
        let select_chain = sc_consensus::LongestChain::new(backend.clone());

        let (grandpa_block_import, ..) =
            grandpa::block_import(client.clone(), &(client.clone() as Arc<_>), select_chain.clone())?;

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

    fn dispatch_with_root(call: <Self::Runtime as frame_system::Config>::Call, node: &mut Node<Self>) {
        // type SystemCall = frame_system::Call<Runtime>;
        // type DemocracyCall = pallet_democracy::Call<Runtime>;
        // type TechnicalCollectiveCall = pallet_collective::Call<Runtime, TechnicalCollective>;
        // type CouncilCollectiveCall = pallet_collective::Call<Runtime, CouncilCollective>;
        //
        // // here lies a black mirror esque copy of on chain whales.
        // let whales = vec![
        //     "1rvXMZpAj9nKLQkPFCymyH7Fg3ZyKJhJbrc7UtHbTVhJm1A",
        //     "15j4dg5GzsL1bw2U2AWgeyAk6QTxq43V7ZPbXdAmbVLjvDCK",
        // ]
        //     .into_iter()
        //     .map(|account| AccountId32::from_str(account).unwrap())
        //     .collect::<Vec<_>>();
        //
        // // and these
        // let (technical_collective, council_collective) = node.with_state(|| (
        //     pallet_collective::Members::<Runtime, TechnicalCollective>::get(),
        //     pallet_collective::Members::<Runtime, CouncilCollective>::get()
        // ));
        //
        // let call = Call::System(SystemCall::set_code(wasm));
        // // note the call (pre-image?) of the runtime upgrade proposal
        // node.submit_extrinsic(DemocracyCall::note_preimage(call.encode()), whales[0].clone());
        // node.seal_blocks(1);
        //
        // // fetch proposal hash from event emitted by the runtime
        // let events = node.with_state(|| frame_system::Module::<Self::Runtime>::events());
        // let proposal_hash = events.into_iter()
        //     .filter_map(|event| match event.event {
        //         Event::pallet_democracy(
        //             pallet_democracy::RawEvent::PreimageNoted(proposal_hash, _, _)
        //         ) => Some(proposal_hash),
        //         _ => None
        //     })
        //     .next()
        //     .unwrap();
        //
        // // submit external_propose call through council
        // let external_propose = DemocracyCall::external_propose_majority(proposal_hash.clone().into());
        // let proposal_length = external_propose.using_encoded(|x| x.len()) as u32 + 1;
        // let proposal_weight = external_propose.get_dispatch_info().weight;
        // let proposal = CouncilCollectiveCall::propose(
        //     council_collective.len() as u32,
        //     Box::new(external_propose.clone().into()),
        //     proposal_length
        // );
        //
        // node.submit_extrinsic(proposal.clone(), council_collective[0].clone());
        // node.seal_blocks(1);
        //
        // // fetch proposal index from event emitted by the runtime
        // let events = node.with_state(|| frame_system::Module::<Self::Runtime>::events());
        // let (council_proposal_index, council_proposal_hash) = events.into_iter()
        //     .filter_map(|event| {
        //         match event.event {
        //             Event::pallet_collective_Instance1(
        //                 pallet_collective::RawEvent::Proposed(_, index, proposal_hash, _)
        //             ) => Some((index, proposal_hash)),
        //             _ => None
        //         }
        //     })
        //     .next()
        //     .unwrap();
        //
        // // vote
        // for member in &council_collective[1..] {
        //     let call = CouncilCollectiveCall::vote(council_proposal_hash.clone(), council_proposal_index, true);
        //     node.submit_extrinsic(call, member.clone());
        // }
        // node.seal_blocks(1);
        //
        // // close vote
        // let call = CouncilCollectiveCall::close(council_proposal_hash, council_proposal_index, proposal_weight, proposal_length);
        // node.submit_extrinsic(call, council_collective[0].clone());
        // node.seal_blocks(1);
        //
        // // assert that proposal has been passed on chain
        // let events = node.with_state(|| frame_system::Module::<Self::Runtime>::events())
        //     .into_iter()
        //     .filter(|event| {
        //         match event.event {
        //             Event::pallet_collective_Instance1(pallet_collective::RawEvent::Closed(_, _, _)) |
        //             Event::pallet_collective_Instance1(pallet_collective::RawEvent::Approved(_,)) |
        //             Event::pallet_collective_Instance1(pallet_collective::RawEvent::Executed(_, Ok(()))) => true,
        //             _ => false,
        //         }
        //     })
        //     .collect::<Vec<_>>();
        //
        // // make sure all 3 events are in state
        // assert_eq!(events.len(), 3);
        //
        // // next technical collective must fast track the proposal.
        // let fast_track = DemocracyCall::fast_track(proposal_hash.into(), FastTrackVotingPeriod::get(), 0);
        // let proposal_weight = fast_track.get_dispatch_info().weight;
        // let fast_track_length = fast_track.using_encoded(|x| x.len()) as u32 + 1;
        // let proposal = TechnicalCollectiveCall::propose(
        //     technical_collective.len() as u32,
        //     Box::new(fast_track.into()),
        //     fast_track_length
        // );
        //
        // node.submit_extrinsic(proposal, technical_collective[0].clone());
        // node.seal_blocks(1);
        //
        // let events = node.with_state(|| frame_system::Module::<Self::Runtime>::events());
        // let (technical_proposal_index, technical_proposal_hash) = events.into_iter()
        //     .filter_map(|event| {
        //         match event.event {
        //             Event::pallet_collective_Instance2(
        //                 pallet_collective::RawEvent::Proposed(_, index, hash, _)
        //             ) => Some((index, hash)),
        //             _ => None
        //         }
        //     })
        //     .next()
        //     .unwrap();
        //
        // // vote
        // for member in &technical_collective[1..] {
        //     let call = TechnicalCollectiveCall::vote(technical_proposal_hash.clone(), technical_proposal_index, true);
        //     node.submit_extrinsic(call, member.clone());
        // }
        // node.seal_blocks(1);
        //
        // // close vote
        // let call = TechnicalCollectiveCall::close(
        //     technical_proposal_hash,
        //     technical_proposal_index,
        //     proposal_weight,
        //     fast_track_length,
        // );
        // node.submit_extrinsic(call, technical_collective[0].clone());
        // node.seal_blocks(1);
        //
        // // assert that fast-track proposal has been passed on chain
        // let events = node.with_state(|| frame_system::Module::<Self::Runtime>::events());
        // let collective_events = events.iter()
        //     .filter(|event| {
        //         match event.event {
        //             Event::pallet_collective_Instance2(pallet_collective::RawEvent::Closed(_, _, _)) |
        //             Event::pallet_collective_Instance2(pallet_collective::RawEvent::Approved(_)) |
        //             Event::pallet_collective_Instance2(pallet_collective::RawEvent::Executed(_, Ok(()))) => true,
        //             _ => false,
        //         }
        //     })
        //     .collect::<Vec<_>>();
        //
        // // make sure all 3 events are in state
        // assert_eq!(collective_events.len(), 3);
        //
        // // now runtime upgrade proposal is a fast-tracked referendum we can vote for.
        // let referendum_index = events.into_iter()
        //     .filter_map(|event| match event.event {
        //         Event::pallet_democracy(pallet_democracy::Event::<Self::Runtime>::Started(index, _)) => Some(index),
        //         _ => None,
        //     })
        //     .next()
        //     .unwrap();
        // let call = DemocracyCall::vote(
        //     referendum_index,
        //     AccountVote::Standard {
        //         vote: Vote { aye: true, conviction: Conviction::Locked1x },
        //         // 10 DOTS
        //         balance: 10_000_000_000_000
        //     }
        // );
        // for whale in whales {
        //     node.submit_extrinsic(call.clone(), whale);
        // }
        //
        // // wait for fast track period.
        // node.seal_blocks(FastTrackVotingPeriod::get() as usize);
        //
        // // assert that the runtime is upgraded by looking at events
        // let events = node.with_state(|| frame_system::Module::<Self::Runtime>::events())
        //     .into_iter()
        //     .filter(|event| {
        //         match event.event {
        //             Event::frame_system(frame_system::Event::CodeUpdated) |
        //             Event::pallet_democracy(pallet_democracy::RawEvent::Passed(_)) |
        //             Event::pallet_democracy(pallet_democracy::RawEvent::PreimageUsed(_, _, _)) |
        //             Event::pallet_democracy(pallet_democracy::RawEvent::Executed(_, true)) => true,
        //             _ => false,
        //         }
        //     })
        //     .collect::<Vec<_>>();
        //
        // // make sure event is in state
        // assert_eq!(events.len(), 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use substrate_test_runner::Node;

    #[test]
    fn it_works() {
        let mut node = Node::<PolkadotChainInfo>::new().unwrap();

        node.seal_blocks(100);
    }
}
