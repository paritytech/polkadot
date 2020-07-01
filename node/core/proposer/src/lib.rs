use futures::lock::Mutex;
use futures::prelude::*;
use polkadot_node_subsystem::messages::{AllMessages, ProvisionableData, ProvisionerMessage};
use polkadot_overseer::OverseerHandler;
use polkadot_primitives::{
	inclusion_inherent,
	parachain::{ParachainHost, SignedAvailabilityBitfields},
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

/// Custom Proposer factory for Polkadot
pub struct ProposerFactory<TxPool, Backend, Client> {
	inner: sc_basic_authorship::ProposerFactory<TxPool, Backend, Client>,
	client: Arc<Client>,
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
				client.clone(),
				transaction_pool,
				None,
			),
			client,
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
	type CreateProposer =
		Pin<Box<dyn Future<Output = Result<Self::Proposer, Self::Error>> + Send + 'static>>;
	type Proposer = Proposer<TxPool, Backend, Client>;
	type Error = sp_blockchain::Error;

	fn init(&mut self, parent_header: &Header) -> Self::CreateProposer {
		// we know that this function will be called at least once per proposed block,
		// because this is where the parent header is supplied. Therefore, we can send
		// the request for authorship data here.

		// note that the buffer of 1 here is actually an _overflow_ bound; every sender also
		// has a guaranteed slot in the channel:
		// https://docs.rs/futures/0.3.5/futures/channel/mpsc/fn.channel.html
		let (sender, receiver) = futures::channel::mpsc::channel(1);
		self.overseer.send_msg(AllMessages::Provisioner(
			ProvisionerMessage::RequestBlockAuthorshipData(parent_header.hash(), sender),
		));
		unimplemented!()
	}
}

pub struct Proposer<TxPool: TransactionPool<Block = Block>, Backend, Client> {
	inner: sc_basic_authorship::Proposer<Backend, Block, Client, TxPool>,
	client: Arc<Client>,
	overseer: OverseerHandler,
	provisionable_data: futures::channel::mpsc::Receiver<ProvisionableData>,
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
		let bitfields = Mutex::new(Vec::new());
		let candidates = Mutex::new(Vec::new());

		async move {
			// At most two items can be simultaneously processed: one bitfield, one candidate
			// allowing more concurrent tasks to be spawned just wastes resources
			// This is not strictly true in the case of the ignored variants,
			// but those should be discarded quickly enough not to produce backpressure.
			self.provisionable_data
				.for_each_concurrent(2, |item| async {
					match item {
						ProvisionableData::Bitfield(_, signed_bitfield) => {
							bitfields.lock().await.push(signed_bitfield)
						}
						ProvisionableData::BackedCandidate(candidate) => {
							candidates.lock().await.push(candidate)
						}
						_ => {}
					}
				})
				.await;
			inherent_data.put_data(
				inclusion_inherent::INHERENT_IDENTIFIER,
				&(
					SignedAvailabilityBitfields(bitfields.into_inner()),
					candidates.into_inner(),
				),
			)?;
			self.inner
				.propose(inherent_data, inherent_digests, max_duration, record_proof)
				.await
				.map_err(Into::into)
		}
		.boxed()
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

impl From<sp_inherents::Error> for Error {
	fn from(e: sp_inherents::Error) -> Error {
		Error::Inherent(e)
	}
}
