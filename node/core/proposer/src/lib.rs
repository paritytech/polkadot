use futures::future;
use polkadot_overseer::OverseerHandler;
use polkadot_primitives::{parachain::ParachainHost, Block, Header};
use sc_block_builder::{BlockBuilderApi, BlockBuilderProvider};
use sp_api::{ApiExt, ProvideRuntimeApi};
use sp_blockchain::HeaderBackend;
use sp_consensus::{Proposal, RecordProof};
use sp_inherents::InherentData;
use sp_runtime::traits::{DigestFor, HashFor};
use sp_transaction_pool::TransactionPool;
use std::{sync::Arc, time};

/// Custom Proposer factory for Polkadot
pub struct ProposerFactory<TxPool, Backend, Client> {
	inner: sc_basic_authorship::ProposerFactory<TxPool, Backend, Client>,
	overseer: OverseerHandler,
}

impl<TxPool, Backend, Client> ProposerFactory<TxPool, Backend, Client> {
	pub fn new(
		client: Arc<Client>,
		transaction_pool: Arc<TxPool>,
		overseer: OverseerHandler,
	) -> Self {
		ProposerFactory {
			inner: sc_basic_authorship::ProposerFactory::new(client, transaction_pool, None),
			overseer,
		}
	}
}

impl<TxPool, Backend, Client> sp_consensus::Environment<Block>
	for ProposerFactory<TxPool, Backend, Client>
where
	TxPool: 'static + TransactionPool<Block = Block>,
	Client: 'static
		+ BlockBuilderProvider<Backend, Block, Client>
		+ ProvideRuntimeApi<Block>
		+ HeaderBackend<Block>
		+ Send
		+ Sync,
	Client::Api:
		ParachainHost<Block> + BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	Backend:
		'static + sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>>,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HashFor<Block>> + Send,
{
	type CreateProposer = future::Ready<Result<Self::Proposer, Self::Error>>;
	type Proposer = Proposer<TxPool, Backend, Client>;
	type Error = sp_blockchain::Error;

	fn init(&mut self, parent_header: &Header) -> Self::CreateProposer {
		unimplemented!()
	}
}

pub struct Proposer<TxPool: TransactionPool<Block = Block>, Backend, Client> {
	inner: sc_basic_authorship::Proposer<Backend, Block, Client, TxPool>,
}

impl<TxPool, Backend, Client> sp_consensus::Proposer<Block> for Proposer<TxPool, Backend, Client>
where
	TxPool: 'static + TransactionPool<Block = Block>,
	Client: 'static
		+ BlockBuilderProvider<Backend, Block, Client>
		+ ProvideRuntimeApi<Block>
		+ HeaderBackend<Block>
		+ Send
		+ Sync,
	Client::Api:
		ParachainHost<Block> + BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	Backend:
		'static + sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>>,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HashFor<Block>> + Send,
{
	type Transaction = sc_client_api::TransactionFor<Backend, Block>;
	type Proposal =
		tokio_executor::blocking::Blocking<Result<Proposal<Block, Self::Transaction>, Self::Error>>;
	type Error = sp_blockchain::Error;

	fn propose(
		self,
		inherent_data: InherentData,
		inherent_digests: DigestFor<Block>,
		max_duration: time::Duration,
		record_proof: RecordProof,
	) -> Self::Proposal {
		unimplemented!()
	}
}
