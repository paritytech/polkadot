use futures::prelude::*;
use futures::select;
use polkadot_node_subsystem::{messages::{AllMessages, ProvisionerInherentData, ProvisionerMessage}, SubsystemError};
use polkadot_overseer::OverseerHandler;
use polkadot_primitives::{
	inclusion_inherent,
	parachain::ParachainHost,
	Block, Header,
};
use sc_block_builder::{BlockBuilderApi, BlockBuilderProvider};
use sp_api::{ApiExt, ProvideRuntimeApi};
use sp_blockchain::HeaderBackend;
use sp_consensus::{Proposal, RecordProof};
use sp_inherents::InherentData;
use sp_runtime::traits::{DigestFor, HashFor};
use sp_transaction_pool::TransactionPool;
use std::{pin::Pin, sync::Arc, time};

/// How long proposal can take before we give up and err out
const PROPOSE_TIMEOUT: core::time::Duration = core::time::Duration::from_secs(2);

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
			inner: sc_basic_authorship::ProposerFactory::new(
				client,
				transaction_pool,
				None,
			),
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
	type CreateProposer = Pin<Box<
		dyn Future<Output = Result<Self::Proposer, Self::Error>> + Send + 'static,
	>>;
	type Proposer = Proposer<TxPool, Backend, Client>;
	type Error = Error;

	fn init(&mut self, parent_header: &Header) -> Self::CreateProposer {
		// create the inner proposer
		let proposer = self.inner.init(parent_header).into_inner();

		// clone the overseer handle (lightweight) so it can be moved into the future
		let mut overseer = self.overseer.clone();

		// we know that this function will be called at least once per proposed block,
		// because this is where the parent header is supplied. Therefore, we can send
		// the request for authorship data here.
		let (sender, receiver) = futures::channel::oneshot::channel();
		let parent_header_hash = parent_header.hash();

		async move {
			overseer.send_msg(AllMessages::Provisioner(
				ProvisionerMessage::RequestInherentData(parent_header_hash, sender),
			)).await?;
			Ok(Proposer {
				inner: proposer?,
				provisioner_inherent_data: receiver,
			})
		}.boxed()
	}
}

pub struct Proposer<TxPool: TransactionPool<Block = Block>, Backend, Client> {
	inner: sc_basic_authorship::Proposer<Backend, Block, Client, TxPool>,
	provisioner_inherent_data: futures::channel::oneshot::Receiver<ProvisionerInherentData>,
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
	type Proposal = Pin<Box<
		dyn Future<Output = Result<Proposal<Block, sp_api::TransactionFor<Client, Block>>, Error>> + Send,
	>>;
	type Error = Error;

	fn propose(
		self,
		mut inherent_data: InherentData,
		inherent_digests: DigestFor<Block>,
		max_duration: time::Duration,
		record_proof: RecordProof,
	) -> Self::Proposal {
		let mut proposal = async move {
			let provisioner_inherent_data = self.provisioner_inherent_data.await.map_err(Error::ClosedChannelFromProvisioner)?;

			inherent_data.put_data(
				inclusion_inherent::INHERENT_IDENTIFIER,
				&provisioner_inherent_data,
			)?;

			self.inner
				.propose(inherent_data, inherent_digests, max_duration, record_proof)
				.await
				.map_err(Into::into)
		}
		.boxed()
		.fuse();

		let mut timeout = wasm_timer::Delay::new(PROPOSE_TIMEOUT).fuse();

		async move {
			select! {
				proposal_result = proposal => proposal_result,
				_ = timeout => Err(Error::Timeout),
			}
		}
		.boxed()
	}
}

#[derive(Debug)]
pub enum Error {
	Consensus(sp_consensus::Error),
	Blockchain(sp_blockchain::Error),
	Inherent(sp_inherents::Error),
	Timeout,
	ClosedChannelFromProvisioner(futures::channel::oneshot::Canceled),
	Subsystem(SubsystemError)
}

impl From<sp_consensus::Error> for Error {
	fn from(e: sp_consensus::Error) -> Error {
		Error::Consensus(e)
	}
}

impl From<sp_blockchain::Error> for Error {
	fn from(e: sp_blockchain::Error) -> Error {
		Error::Blockchain(e)
	}
}

impl From<sp_inherents::Error> for Error {
	fn from(e: sp_inherents::Error) -> Error {
		Error::Inherent(e)
	}
}

impl From<SubsystemError> for Error {
	fn from(e: SubsystemError) -> Error {
		Error::Subsystem(e)
	}
}
