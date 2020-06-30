use futures::prelude::*;
use polkadot_overseer::OverseerHandler;
use polkadot_node_messages::{AllMessages, ProvisionableData, ProvisionerMessage};
use polkadot_primitives::{inclusion_inherent, parachain::ParachainHost, Block, Header};
use sc_block_builder::{BlockBuilderApi, BlockBuilderProvider};
use sp_api::{ApiExt, ProvideRuntimeApi};
use sp_blockchain::HeaderBackend;
use sp_consensus::{Proposal, RecordProof};
use sp_inherents::InherentData;
use sp_runtime::traits::{DigestFor, HashFor};
use sp_transaction_pool::TransactionPool;
use std::{pin::Pin, sync::Arc, time};

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
	type CreateProposer = Pin<Box<
		dyn Future<Output = Result<Self::Proposer, Self::Error>> + Send + 'static
	>>;
	type Proposer = Proposer<TxPool, Backend, Client>;
	type Error = sp_blockchain::Error;

	fn init(&mut self, parent_header: &Header) -> Self::CreateProposer {
		unimplemented!()
	}
}

pub struct Proposer<TxPool: TransactionPool<Block = Block>, Backend, Client> {
	inner: sc_basic_authorship::Proposer<Backend, Block, Client, TxPool>,
	overseer: OverseerHandler,
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
	type Proposal = Pin<Box<dyn Future<Output = Result<Proposal<Block, sp_api::TransactionFor<Client, Block>>, Error>> + Send>>;
	type Error = Error;

	fn propose(
		self,
		mut inherent_data: InherentData,
		inherent_digests: DigestFor<Block>,
		max_duration: time::Duration,
		record_proof: RecordProof,
	) -> Self::Proposal {
		// REVIEW: per the guide, block authors must re-send a new request for block authorship data
		// for each block. I'm assuming that this function gets called once per block, but it is not
		// obvious from the documentation that that assumption is in fact true.
		let (sender, receiver) = futures::channel::mpsc::channel(1);
		let mut bitfields = Vec::new();
		let mut candidates = Vec::new();

		async move {
			self.overseer.send_msg(AllMessages::ProvisionerMessage(ProvisionerMessage::RequestBlockAuthorshipData(unimplemented!(), sender)));
			receiver.for_each_concurrent(None, |item| {
				match item {
					ProvisionableData::Bitfield(_, signed_bitfield) => bitfields.push(signed_bitfield),
					ProvisionableData::BackedCandidate(candidate) => candidates.push(candidate),
					_ => {},
				}
			});
			inherent_data.put_data(inclusion_inherent::INHERENT_IDENTIFIER, &(bitfields, candidates))
				.map_err(Error::Inherent)?;
			self.inner.propose(inherent_data, inherent_digests, max_duration, record_proof).await.map_err(Into::into)
		}.boxed()
	}
}

#[derive(Debug)]
pub enum Error {
	Consensus(sp_consensus::Error),
	Blockchain(sp_blockchain::Error),
	Inherent(sp_inherents::Error),
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
