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

//! The block production pipeline of Polkadot.
//!
//! The `ProposerFactory` exported by this module will be wrapped by some
//! consensus engine, and triggered when it is time to create a block.

use std::{
	pin::Pin,
	sync::Arc,
	time::{self, Duration, Instant},
};

use sp_blockchain::HeaderBackend;
use block_builder::BlockBuilderApi;
use codec::Encode;
use consensus::{Proposal, RecordProof};
use polkadot_primitives::{Hash, Block, BlockId, Header};
use polkadot_primitives::parachain::{
	ParachainHost, AttestedCandidate, NEW_HEADS_IDENTIFIER,
};
use runtime_primitives::traits::{DigestFor, HashFor};
use futures_timer::Delay;
use txpool_api::{TransactionPool, InPoolTransaction};

use futures::prelude::*;
use inherents::InherentData;
use sp_timestamp::TimestampInherentData;
use log::{info, debug, trace};
use sp_api::{ApiExt, ProvideRuntimeApi};

use crate::validation_service::ServiceHandle;
use crate::dynamic_inclusion::DynamicInclusion;
use crate::Error;

// block size limit.
pub(crate) const MAX_TRANSACTIONS_SIZE: usize = 4 * 1024 * 1024;

// Polkadot proposer factory.
pub struct ProposerFactory<Client, TxPool, Backend> {
	client: Arc<Client>,
	transaction_pool: Arc<TxPool>,
	service_handle: ServiceHandle,
	babe_slot_duration: u64,
	backend: Arc<Backend>,
}

impl<Client, TxPool, Backend> ProposerFactory<Client, TxPool, Backend> {
	/// Create a new proposer factory.
	pub fn new(
		client: Arc<Client>,
		transaction_pool: Arc<TxPool>,
		service_handle: ServiceHandle,
		babe_slot_duration: u64,
		backend: Arc<Backend>,
	) -> Self {
		ProposerFactory {
			client,
			transaction_pool,
			service_handle: service_handle,
			babe_slot_duration,
			backend,
		}
	}
}

impl<Client, TxPool, Backend> consensus::Environment<Block>
	for ProposerFactory<Client, TxPool, Backend>
where
	TxPool: TransactionPool<Block=Block> + 'static,
	Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	Client::Api: ParachainHost<Block> + BlockBuilderApi<Block>
		+ ApiExt<Block, Error = sp_blockchain::Error>,
	Backend: sc_client_api::Backend<
		Block,
		State = sp_api::StateBackendFor<Client, Block>
	> + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HashFor<Block>> + Send,
{
	type CreateProposer = Pin<Box<
		dyn Future<Output = Result<Self::Proposer, Self::Error>> + Send + 'static
	>>;
	type Proposer = Proposer<Client, TxPool, Backend>;
	type Error = Error;

	fn init(
		&mut self,
		parent_header: &Header,
	) -> Self::CreateProposer {
		let parent_hash = parent_header.hash();
		let parent_id = BlockId::hash(parent_hash);

		let client = self.client.clone();
		let transaction_pool = self.transaction_pool.clone();
		let backend = self.backend.clone();
		let slot_duration = self.babe_slot_duration.clone();

		let maybe_proposer = self.service_handle
			.clone()
			.get_validation_instance(parent_hash)
			.and_then(move |tracker| future::ready(Ok(Proposer {
				client,
				tracker,
				parent_id,
				transaction_pool,
				slot_duration,
				backend,
			})));

		Box::pin(maybe_proposer)
	}
}

/// The Polkadot proposer logic.
pub struct Proposer<Client, TxPool, Backend> {
	client: Arc<Client>,
	parent_id: BlockId,
	tracker: crate::validation_service::ValidationInstanceHandle,
	transaction_pool: Arc<TxPool>,
	slot_duration: u64,
	backend: Arc<Backend>,
}

impl<Client, TxPool, Backend> consensus::Proposer<Block> for Proposer<Client, TxPool, Backend> where
	TxPool: TransactionPool<Block=Block> + 'static,
	Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	Client::Api: ParachainHost<Block> + BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	Backend: sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>> + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HashFor<Block>> + Send,
{
	type Error = Error;
	type Transaction = sp_api::TransactionFor<Client, Block>;
	type Proposal = Pin<
		Box<
			dyn Future<Output = Result<Proposal<Block, sp_api::TransactionFor<Client, Block>>, Error>>
				+ Send
		>
	>;

	fn propose(&mut self,
		inherent_data: InherentData,
		inherent_digests: DigestFor<Block>,
		max_duration: Duration,
		record_proof: RecordProof,
	) -> Self::Proposal {
		const SLOT_DURATION_DENOMINATOR: u64 = 3; // wait up to 1/3 of the slot for candidates.

		let initial_included = self.tracker.table().includable_count();
		let now = Instant::now();

		let dynamic_inclusion = DynamicInclusion::new(
			self.tracker.table().num_parachains(),
			self.tracker.started(),
			Duration::from_millis(self.slot_duration / SLOT_DURATION_DENOMINATOR),
		);

		let parent_id = self.parent_id.clone();
		let client = self.client.clone();
		let transaction_pool = self.transaction_pool.clone();
		let table = self.tracker.table().clone();
		let backend = self.backend.clone();

		async move {
			let enough_candidates = dynamic_inclusion.acceptable_in(
				now,
				initial_included,
			).unwrap_or_else(|| Duration::from_millis(1));

			let believed_timestamp = match inherent_data.timestamp_inherent_data() {
				Ok(timestamp) => timestamp,
				Err(e) => return Err(Error::InherentError(e)),
			};

			let deadline_diff = max_duration - max_duration / 3;
			let deadline = match Instant::now().checked_add(deadline_diff) {
				None => return Err(Error::DeadlineComputeFailure(deadline_diff)),
				Some(d) => d,
			};

			let data = CreateProposalData {
				parent_id,
				client,
				transaction_pool,
				table,
				inherent_data: Some(inherent_data),
				inherent_digests,
				// leave some time for the proposal finalisation
				deadline,
				record_proof,
				backend,
			};

			// set up delay until next allowed timestamp.
			let current_timestamp = current_timestamp();
			if current_timestamp < believed_timestamp {
				Delay::new(Duration::from_millis(current_timestamp - believed_timestamp))
					.await;
			}

			Delay::new(enough_candidates).await;

			tokio::task::spawn_blocking(move || {
				let proposed_candidates = data.table.proposed_set();
				data.propose_with(proposed_candidates)
			})
				.await?
		}.boxed()
	}
}

fn current_timestamp() -> u64 {
	time::SystemTime::now().duration_since(time::UNIX_EPOCH)
		.expect("now always later than unix epoch; qed")
		.as_millis() as u64
}

/// Inner data of the create proposal.
struct CreateProposalData<Client, TxPool, Backend> {
	parent_id: BlockId,
	client: Arc<Client>,
	transaction_pool: Arc<TxPool>,
	table: Arc<crate::SharedTable>,
	inherent_data: Option<InherentData>,
	inherent_digests: DigestFor<Block>,
	deadline: Instant,
	record_proof: RecordProof,
	backend: Arc<Backend>,
}

impl<Client, TxPool, Backend> CreateProposalData<Client, TxPool, Backend> where
	TxPool: TransactionPool<Block=Block>,
	Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
	Client::Api: ParachainHost<Block> + BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	Backend: sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>> + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HashFor<Block>> + Send,
{
	fn propose_with(
		mut self,
		candidates: Vec<AttestedCandidate>,
	) -> Result<Proposal<Block, sp_api::TransactionFor<Client, Block>>, Error> {
		use runtime_primitives::traits::{Hash as HashT, BlakeTwo256};

		const MAX_TRANSACTIONS: usize = 40;

		let mut inherent_data = self.inherent_data
			.take()
			.expect("CreateProposal is not polled after finishing; qed");
		inherent_data.put_data(NEW_HEADS_IDENTIFIER, &candidates)
			.map_err(Error::InherentError)?;

		let runtime_api = self.client.runtime_api();

		let mut block_builder = block_builder::BlockBuilder::new(
			&*self.client,
			self.client.expect_block_hash_from_id(&self.parent_id)?,
			self.client.expect_block_number_from_id(&self.parent_id)?,
			self.record_proof,
			self.inherent_digests.clone(),
			&*self.backend,
		)?;

		{
			let inherents = runtime_api.inherent_extrinsics(&self.parent_id, inherent_data)?;
			for inherent in inherents {
				block_builder.push(inherent)?;
			}

			let mut unqueue_invalid = Vec::new();
			let mut pending_size = 0;

			let ready_iter = self.transaction_pool.ready();
			for ready in ready_iter.take(MAX_TRANSACTIONS) {
				let encoded_size = ready.data().encode().len();
				if pending_size + encoded_size >= MAX_TRANSACTIONS_SIZE {
					break;
				}
				if Instant::now() > self.deadline {
					debug!("Consensus deadline reached when pushing block transactions, proceeding with proposing.");
					break;
				}

				match block_builder.push_trusted(ready.data().clone()) {
					Ok(()) => {
						debug!("[{:?}] Pushed to the block.", ready.hash());
						pending_size += encoded_size;
					}
					Err(sp_blockchain::Error::ApplyExtrinsicFailed(sp_blockchain::ApplyExtrinsicFailed::Validity(e)))
						if e.exhausted_resources() =>
					{
						debug!("Block is full, proceed with proposing.");
						break;
					}
					Err(e) => {
						trace!(target: "transaction-pool", "Invalid transaction: {}", e);
						unqueue_invalid.push(ready.hash().clone());
					}
				}
			}

			self.transaction_pool.remove_invalid(&unqueue_invalid);
		}

		let (new_block, storage_changes, proof) = block_builder.build()?.into_inner();

		info!("Prepared block for proposing at {} [hash: {:?}; parent_hash: {}; extrinsics: [{}]]",
			new_block.header.number,
			Hash::from(new_block.header.hash()),
			new_block.header.parent_hash,
			new_block.extrinsics.iter()
				.map(|xt| format!("{}", BlakeTwo256::hash_of(xt)))
				.collect::<Vec<_>>()
				.join(", ")
		);

		Ok(Proposal { block: new_block, storage_changes, proof })
	}
}
