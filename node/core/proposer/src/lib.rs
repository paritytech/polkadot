use futures::prelude::*;
use futures::select;
use polkadot_node_subsystem::{messages::{AllMessages, ProvisionerMessage}, SubsystemError};
use polkadot_overseer::OverseerHandler;
use polkadot_primitives::{
	inclusion_inherent,
	parachain::ParachainHost,
	Block, Hash, Header,
};
use sc_block_builder::{BlockBuilderApi, BlockBuilderProvider};
use sp_api::{ApiExt, ProvideRuntimeApi};
use sp_blockchain::HeaderBackend;
use sp_consensus::{Proposal, RecordProof};
use sp_inherents::InherentData;
use sp_runtime::traits::{DigestFor, HashFor};
use sp_transaction_pool::TransactionPool;
use std::{fmt, pin::Pin, sync::Arc, time};

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

		// data to be moved into the future
		let overseer = self.overseer.clone();
		let parent_header_hash = parent_header.hash();

		async move {
			Ok(Proposer {
				inner: proposer?,
				overseer,
				parent_header_hash,
			})
		}.boxed()
	}
}

pub struct Proposer<TxPool: TransactionPool<Block = Block>, Backend, Client> {
	inner: sc_basic_authorship::Proposer<Backend, Block, Client, TxPool>,
	overseer: OverseerHandler,
	parent_header_hash: Hash,
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
		// clone this (lightweight) data because we're going to move it into the future
		let mut overseer = self.overseer.clone();
		let parent_header_hash = self.parent_header_hash.clone();

		let mut proposal = async move {
			let (sender, receiver) = futures::channel::oneshot::channel();

			// strictly speaking, we don't _have_ to .await this send_msg before opening the
			// receiver; it's possible that the response there would be ready slightly before
			// this call completes. IMO it's not worth the hassle or overhead of spawning a
			// distinct task for that kind of miniscule efficiency improvement.
			overseer.send_msg(AllMessages::Provisioner(
				ProvisionerMessage::RequestInherentData(parent_header_hash, sender),
			)).await?;

			let provisioner_inherent_data = receiver.await.map_err(Error::ClosedChannelFromProvisioner)?;

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

// It would have been more ergonomic to use thiserror to derive the
// From implementations, Display, and std::error::Error, but unfortunately
// two of the wrapped errors (sp_inherents::Error, SubsystemError) also
// don't impl std::error::Error, which breaks the thiserror derive.
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

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self {
			Self::Consensus(err) => write!(f, "consensus error: {}", err),
			Self::Blockchain(err) => write!(f, "blockchain error: {}", err),
			Self::Inherent(err) => write!(f, "inherent error: {:?}", err),
			Self::Timeout => write!(f, "timeout: provisioner did not return inherent data after {:?}", PROPOSE_TIMEOUT),
			Self::ClosedChannelFromProvisioner(err) => write!(f, "provisioner closed inherent data channel before sending: {}", err),
			Self::Subsystem(err) => write!(f, "subsystem error: {:?}", err),
		}
	}
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::Consensus(err) => Some(err),
			Self::Blockchain(err) => Some(err),
			Self::ClosedChannelFromProvisioner(err) => Some(err),
			_ => None
		}
	}
}
