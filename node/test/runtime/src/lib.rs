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
use polkadot_runtime::{SignedExtra, Runtime, Event};
use sp_runtime::generic::Era;
use std::sync::Arc;
use sp_inherents::InherentDataProviders;
use sc_consensus_babe::BabeBlockImport;
use sp_keystore::SyncCryptoStorePtr;
use sp_keyring::sr25519::Keyring::{Alice, Bob};
use node_cli::chain_spec::development_config;
use sp_consensus_babe::AuthorityId;
use sc_consensus_manual_seal::{ConsensusDataProvider, consensus::babe::BabeConsensusDataProvider};
use sp_runtime::{traits::IdentifyAccount, MultiSigner};

type BlockImport<B, BE, C, SC> = BabeBlockImport<B, C, GrandpaBlockImport<BE, B, C, SC>>;

sc_executor::native_executor_instance!(
	pub Executor,
	polkadot_runtime::api::dispatch,
	polkadot_runtime::native_version,
	SignatureVerificationOverride,
);

/// ChainInfo implementation.
struct NodeTemplateChainInfo;

impl ChainInfo for NodeTemplateChainInfo {
    type Block = node_primitives::Block;
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
    type SignedExtras = SignedExtra;

    fn load_spec() -> Result<Box<dyn sc_service::ChainSpec>, String> {
        Ok(Box::new(development_config()))
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
        let alice = MultiSigner::from(Alice.public()).into_account();
        let call = pallet_sudo::Call::sudo(Box::new(call)); // :D
        node.submit_extrinsic(call, alice);
        node.seal_blocks(1);
    }
}
