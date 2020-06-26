use futures::{
	executor,
	future::{self, Either},
};
use log::{debug, error, info, trace, warn};
use parity_scale_codec::Decode;
use sc_block_builder::{BlockBuilderApi, BlockBuilderProvider};
use sc_client_api::backend;
use sc_telemetry::{telemetry, CONSENSUS_INFO};
use sp_api::{ApiExt, ProvideRuntimeApi};
use sp_blockchain::{ApplyExtrinsicFailed::Validity, Error::ApplyExtrinsicFailed, HeaderBackend};
use sp_consensus::{evaluation, Proposal, RecordProof};
use sp_core::ExecutionContext;
use sp_inherents::InherentData;
use sp_runtime::{
	generic::BlockId,
	traits::{BlakeTwo256, Block as BlockT, DigestFor, Hash, Header as HeaderT},
};
use sp_transaction_pool::{InPoolTransaction, TransactionPool};
use std::{marker::PhantomData, sync::Arc, time};

/// Custom Proposer factory for Polkadot
pub struct ProposerFactory<TxPool, Backend, Client> {
	/// The client instance.
	client: Arc<Client>,
	/// The transaction pool.
	transaction_pool: Arc<TxPool>,
	/// phantom member to pin the `Backend` type.
	_phantom: PhantomData<Backend>,
}

impl<TxPool, Backend, Client> ProposerFactory<TxPool, Backend, Client> {
	pub fn new(client: Arc<Client>, transaction_pool: Arc<TxPool>) -> Self {
		ProposerFactory {
			client,
			transaction_pool,
			_phantom: PhantomData,
		}
	}
}

impl<Backend, Block, Client, TxPool> ProposerFactory<TxPool, Backend, Client>
where
	TxPool: TransactionPool<Block = Block> + 'static,
	Backend: backend::Backend<Block> + Send + Sync + 'static,
	Block: BlockT,
	Client: BlockBuilderProvider<Backend, Block, Client>
		+ HeaderBackend<Block>
		+ ProvideRuntimeApi<Block>
		+ Send
		+ Sync
		+ 'static,
	Client::Api: ApiExt<Block, StateBackend = backend::StateBackendFor<Backend, Block>>
		+ BlockBuilderApi<Block, Error = sp_blockchain::Error>,
{
	pub fn init_with_now(
		&mut self,
		parent_header: &<Block as BlockT>::Header,
		now: Box<dyn Fn() -> time::Instant + Send + Sync>,
	) -> Proposer<TxPool, Backend, Client, Block> {
		let parent_hash = parent_header.hash();

		let id = BlockId::hash(parent_hash);

		info!(
			"🙌 Starting consensus session on top of parent {:?}",
			parent_hash
		);

		let proposer = Proposer(Arc::new(ProposerInner {
			client: self.client.clone(),
			parent_hash,
			parent_id: id,
			parent_number: *parent_header.number(),
			transaction_pool: self.transaction_pool.clone(),
			now,
			_phantom: PhantomData,
		}));

		proposer
	}
}

impl<TxPool, Backend, Client, Block> sp_consensus::Environment<Block>
	for ProposerFactory<TxPool, Backend, Client>
where
	TxPool: TransactionPool<Block = Block> + 'static,
	Backend: backend::Backend<Block> + Send + Sync + 'static,
	Block: BlockT,
	Client: BlockBuilderProvider<Backend, Block, Client>
		+ HeaderBackend<Block>
		+ ProvideRuntimeApi<Block>
		+ Send
		+ Sync
		+ 'static,
	Client::Api: ApiExt<Block, StateBackend = backend::StateBackendFor<Backend, Block>>
		+ BlockBuilderApi<Block, Error = sp_blockchain::Error>,
{
	type CreateProposer = future::Ready<Result<Self::Proposer, Self::Error>>;
	type Proposer = Proposer<TxPool, Backend, Client, Block>;
	type Error = sp_blockchain::Error;

	fn init(&mut self, parent_header: &<Block as BlockT>::Header) -> Self::CreateProposer {
		future::ready(Ok(
			self.init_with_now(parent_header, Box::new(time::Instant::now))
		))
	}
}

pub struct Proposer<TxPool, Backend, Client, Block: BlockT>(
	Arc<ProposerInner<TxPool, Backend, Client, Block>>,
);

struct ProposerInner<TxPool, Backend, Client, Block: BlockT> {
	client: Arc<Client>,
	parent_hash: <Block as BlockT>::Hash,
	parent_id: BlockId<Block>,
	parent_number: <<Block as BlockT>::Header as HeaderT>::Number,
	transaction_pool: Arc<TxPool>,
	now: Box<dyn Fn() -> time::Instant + Send + Sync>,
	_phantom: PhantomData<Backend>,
}

impl<TxPool, Backend, Client, Block> sp_consensus::Proposer<Block>
	for Proposer<TxPool, Backend, Client, Block>
where
	TxPool: 'static + TransactionPool<Block = Block>,
	Backend: 'static + backend::Backend<Block> + Send + Sync,
	Client: 'static
		+ BlockBuilderProvider<Backend, Block, Client>
		+ HeaderBackend<Block>
		+ ProvideRuntimeApi<Block>
		+ Send
		+ Sync,
	Client::Api: ApiExt<Block, StateBackend = backend::StateBackendFor<Backend, Block>>
		+ BlockBuilderApi<Block, Error = sp_blockchain::Error>,
	Block: BlockT,
{
	type Transaction = backend::TransactionFor<Backend, Block>;
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
		let inner = self.0.clone();
		tokio_executor::blocking::run(move || {
			// leave some time for evaluation and block finalization (33%)
			let deadline = (inner.now)() + max_duration - max_duration / 3;
			inner.propose_with(inherent_data, inherent_digests, deadline, record_proof)
		})
	}
}

impl<TxPool, Backend, Client, Block> ProposerInner<TxPool, Backend, Client, Block>
where
	TxPool: TransactionPool<Block = Block>,
	Backend: backend::Backend<Block> + Send + Sync + 'static,
	Block: BlockT,
	Client: BlockBuilderProvider<Backend, Block, Client>
		+ HeaderBackend<Block>
		+ ProvideRuntimeApi<Block>
		+ Send
		+ Sync
		+ 'static,
	Client::Api: ApiExt<Block, StateBackend = backend::StateBackendFor<Backend, Block>>
		+ BlockBuilderApi<Block, Error = sp_blockchain::Error>,
{
	fn propose_with(
		&self,
		inherent_data: InherentData,
		inherent_digests: DigestFor<Block>,
		deadline: time::Instant,
		record_proof: RecordProof,
	) -> Result<Proposal<Block, backend::TransactionFor<Backend, Block>>, sp_blockchain::Error>
	{
		/// If the block is full we will attempt to push at most
		/// this number of transactions before quitting for real.
		/// It allows us to increase block utilization.
		const MAX_SKIPPED_TRANSACTIONS: usize = 8;

		let mut block_builder =
			self.client
				.new_block_at(&self.parent_id, inherent_digests, record_proof)?;

		// We don't check the API versions any further here since the dispatch compatibility
		// check should be enough.
		for inherent in self.client.runtime_api().inherent_extrinsics_with_context(
			&self.parent_id,
			ExecutionContext::BlockConstruction,
			inherent_data,
		)? {
			match block_builder.push(inherent) {
				Err(ApplyExtrinsicFailed(Validity(e))) if e.exhausted_resources() => {
					warn!("⚠️  Dropping non-mandatory inherent from overweight block.")
				}
				Err(ApplyExtrinsicFailed(Validity(e))) if e.was_mandatory() => {
					error!(
						"❌️ Mandatory inherent extrinsic returned error. Block cannot be produced."
					);
					Err(ApplyExtrinsicFailed(Validity(e)))?
				}
				Err(e) => {
					warn!(
						"❗️ Inherent extrinsic returned unexpected error: {}. Dropping.",
						e
					);
				}
				Ok(_) => {}
			}
		}

		// proceed with transactions
		let mut is_first = true;
		let mut skipped = 0;
		let mut unqueue_invalid = Vec::new();
		let pending_iterator = match executor::block_on(future::select(
			self.transaction_pool.ready_at(self.parent_number),
			futures_timer::Delay::new(deadline.saturating_duration_since((self.now)()) / 8),
		)) {
			Either::Left((iterator, _)) => iterator,
			Either::Right(_) => {
				log::warn!(
					"Timeout fired waiting for transaction pool to be ready. Proceeding to block production anyway.",
				);
				self.transaction_pool.ready()
			}
		};

		debug!("Attempting to push transactions from the pool.");
		debug!("Pool status: {:?}", self.transaction_pool.status());
		for pending_tx in pending_iterator {
			if (self.now)() > deadline {
				debug!(
					"Consensus deadline reached when pushing block transactions, \
					proceeding with proposing."
				);
				break;
			}

			let pending_tx_data = pending_tx.data().clone();
			let pending_tx_hash = pending_tx.hash().clone();
			trace!("[{:?}] Pushing to the block.", pending_tx_hash);
			match sc_block_builder::BlockBuilder::push(&mut block_builder, pending_tx_data) {
				Ok(()) => {
					debug!("[{:?}] Pushed to the block.", pending_tx_hash);
				}
				Err(ApplyExtrinsicFailed(Validity(e))) if e.exhausted_resources() => {
					if is_first {
						debug!(
							"[{:?}] Invalid transaction: FullBlock on empty block",
							pending_tx_hash
						);
						unqueue_invalid.push(pending_tx_hash);
					} else if skipped < MAX_SKIPPED_TRANSACTIONS {
						skipped += 1;
						debug!(
							"Block seems full, but will try {} more transactions before quitting.",
							MAX_SKIPPED_TRANSACTIONS - skipped,
						);
					} else {
						debug!("Block is full, proceed with proposing.");
						break;
					}
				}
				Err(e) if skipped > 0 => {
					trace!(
						"[{:?}] Ignoring invalid transaction when skipping: {}",
						pending_tx_hash,
						e
					);
				}
				Err(e) => {
					debug!("[{:?}] Invalid transaction: {}", pending_tx_hash, e);
					unqueue_invalid.push(pending_tx_hash);
				}
			}

			is_first = false;
		}

		self.transaction_pool.remove_invalid(&unqueue_invalid);

		let (block, storage_changes, proof) = block_builder.build()?.into_inner();

		info!("🎁 Prepared block for proposing at {} [hash: {:?}; parent_hash: {}; extrinsics ({}): [{}]]",
			block.header().number(),
			<Block as BlockT>::Hash::from(block.header().hash()),
			block.header().parent_hash(),
			block.extrinsics().len(),
			block.extrinsics()
				.iter()
				.map(|xt| format!("{}", BlakeTwo256::hash_of(xt)))
				.collect::<Vec<_>>()
				.join(", ")
		);
		telemetry!(CONSENSUS_INFO; "prepared_block_for_proposing";
			"number" => ?block.header().number(),
			"hash" => ?<Block as BlockT>::Hash::from(block.header().hash()),
		);

		if Decode::decode(&mut block.encode().as_slice()).as_ref() != Ok(&block) {
			error!("Failed to verify block encoding/decoding");
		}

		if let Err(err) =
			evaluation::evaluate_initial(&block, &self.parent_hash, self.parent_number)
		{
			error!("Failed to evaluate authored block: {:?}", err);
		}

		Ok(Proposal {
			block,
			proof,
			storage_changes,
		})
	}
}
