use polkadot_cli::Cli;
use sc_cli::{SubstrateCli, CliConfiguration};
use structopt::StructOpt;
use polkadot_runtime_test::{PolkadotChainInfo, Block, Executor, SelectChain, BlockImport, dispatch_with_pallet_democracy};
use test_runner::{Node, ChainInfo};
use sc_service::{TFullBackend, TFullClient, Configuration, TaskManager, TaskExecutor};
use polkadot_runtime::{Runtime, RuntimeApi};
use std::sync::Arc;
use sp_keystore::SyncCryptoStorePtr;
use sp_inherents::CreateInherentDataProviders;
use sc_consensus_manual_seal::ConsensusDataProvider;
use sc_consensus_manual_seal::consensus::babe::SlotTimestampProvider;

pub struct PolkadotSimnetChainInfo;

impl ChainInfo for PolkadotSimnetChainInfo {
    type Block = Block;
    type Executor = Executor;
    type Runtime = Runtime;
    type RuntimeApi = RuntimeApi;
    type SelectChain = SelectChain;
    type BlockImport = BlockImport<
        Self::Block,
        TFullBackend<Self::Block>,
        TFullClient<Self::Block, RuntimeApi, Self::Executor>,
        Self::SelectChain,
    >;
    type SignedExtras = polkadot_runtime::SignedExtra;
    type InherentDataProviders = (SlotTimestampProvider, sp_consensus_babe::inherents::InherentDataProvider);

    fn signed_extras(from: <Runtime as frame_system::Config>::AccountId) -> Self::SignedExtras {
        PolkadotChainInfo::signed_extras(from)
    }

    fn config(task_executor: TaskExecutor) -> Configuration {
        let cmd = <Cli as StructOpt>::from_args();
        let config = cmd.create_configuration(&cmd.run.base, task_executor).unwrap();
        // print_node_infos(&config);

        let filters = cmd.run.base.log_filters().unwrap();
        let logger = sc_tracing::logging::LoggerBuilder::new(filters);
        logger.init().unwrap();

        config
    }

    fn create_client_parts(config: &Configuration) -> Result<
        (
            Arc<TFullClient<Self::Block, RuntimeApi, Self::Executor>>,
            Arc<TFullBackend<Self::Block>>,
            SyncCryptoStorePtr,
            TaskManager,
            Box<
                dyn CreateInherentDataProviders<
                    Self::Block,
                    (),
                    InherentDataProviders = Self::InherentDataProviders
                >
            >,
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
        PolkadotChainInfo::create_client_parts(config)
    }

    fn dispatch_with_root(call: <Runtime as frame_system::Config>::Call, node: &mut Node<Self>) {
        dispatch_with_pallet_democracy(call, node)
    }
}
