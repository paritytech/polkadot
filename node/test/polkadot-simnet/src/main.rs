use polkadot_cli::Cli;
use sc_cli::SubstrateCli;
use structopt::StructOpt;
use std::error::Error;
use polkadot_runtime_test::{PolkadotChainInfo, Block, Executor, SelectChain, BlockImport, dispatch_with_root};
use test_runner::{Node, ChainInfo, NodeConfig};
use sc_service::{TFullBackend, TFullClient, Configuration, TaskManager, TaskExecutor};
use polkadot_runtime::{Runtime, RuntimeApi};
use std::sync::Arc;
use sp_keystore::SyncCryptoStorePtr;
use sp_inherents::InherentDataProviders;
use sc_consensus_manual_seal::ConsensusDataProvider;
use log::LevelFilter;

struct PolkadotSimnetChainInfo;

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

    fn signed_extras(from: <Runtime as frame_system::Config>::AccountId) -> Self::SignedExtras {
        PolkadotChainInfo::signed_extras(from)
    }

    fn config(task_executor: TaskExecutor) -> Configuration {
        let cmd = <Cli as StructOpt>::from_args();
        cmd.create_configuration(&cmd.run.base, task_executor).unwrap()
    }

    fn create_client_parts(config: &Configuration) -> Result<
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
        PolkadotChainInfo::create_client_parts(config)
    }

    fn dispatch_with_root(call: <Runtime as frame_system::Config>::Call, node: &mut Node<Self>) {
        dispatch_with_root(call, node)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = NodeConfig {
        log_targets: vec![
            ("yamux", LevelFilter::Off),
            ("multistream_select", LevelFilter::Off),
            ("libp2p", LevelFilter::Off),
            ("jsonrpc_client_transports", LevelFilter::Off),
            ("sc_network", LevelFilter::Off),
            ("tokio_reactor", LevelFilter::Off),
            ("parity-db", LevelFilter::Off),
            ("sub-libp2p", LevelFilter::Off),
            ("peerset", LevelFilter::Off),
            ("ws", LevelFilter::Off),
            ("sc_service", LevelFilter::Off),
            ("sc_basic_authorship", LevelFilter::Off),
            ("telemetry-logger", LevelFilter::Off),
            ("sc_peerset", LevelFilter::Off),
            ("rpc", LevelFilter::Off),

            ("sync", LevelFilter::Debug),
            ("sc_network", LevelFilter::Debug),
            ("runtime", LevelFilter::Trace),
            ("babe", LevelFilter::Debug)
        ],
    };
    let node = Node::<PolkadotChainInfo>::new(config)?;

    tokio::signal::ctrl_c().await?;

    drop(node);

    Ok(())
}
