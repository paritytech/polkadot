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
use block_builder::{BlockBuilderApi, BlockBuilderProvider};
use consensus::{Proposal, RecordProof};
use polkadot_primitives::{Block, Header};
use polkadot_primitives::parachain::{
	ParachainHost, NEW_HEADS_IDENTIFIER,
};
use runtime_primitives::traits::{DigestFor, HashFor};
use futures_timer::Delay;
use txpool_api::TransactionPool;

use futures::prelude::*;
use inherents::InherentData;
use sp_timestamp::TimestampInherentData;
use sp_api::{ApiExt, ProvideRuntimeApi};
use prometheus_endpoint::Registry as PrometheusRegistry;

use crate::{
	Error,
	dynamic_inclusion::DynamicInclusion,
	validation_service::ServiceHandle,
};

// Polkadot proposer factory.
pub struct ProposerFactory<Client, TxPool, Backend> {
	service_handle: ServiceHandle,
	babe_slot_duration: u64,
	factory: sc_basic_authorship::ProposerFactory<TxPool, Backend, Client>,
}

impl<Client, TxPool, Backend> ProposerFactory<Client, TxPool, Backend> {
	/// Create a new proposer factory.
	pub fn new(
		client: Arc<Client>,
		transaction_pool: Arc<TxPool>,
		service_handle: ServiceHandle,
		babe_slot_duration: u64,
		prometheus: Option<&PrometheusRegistry>,
	) -> Self {
		let factory = sc_basic_authorship::ProposerFactory::new(
			client,
			transaction_pool,
			prometheus,
		);
		ProposerFactory {
			service_handle,
			babe_slot_duration,
			factory,
		}
	}
}

impl<Client, TxPool, Backend> consensus::Environment<Block>
	for ProposerFactory<Client, TxPool, Backend>
where
	TxPool: TransactionPool<Block=Block> + 'static,
	Client: BlockBuilderProvider<Backend, Block, Client> + ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
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
		let slot_duration = self.babe_slot_duration.clone();
		let proposer = self.factory.init(parent_header).into_inner();

		let maybe_proposer = self.service_handle
			.clone()
			.get_validation_instance(parent_hash)
			.and_then(move |tracker| future::ready(proposer
				.map_err(Into::into)
				.map(|proposer| Proposer {
					tracker,
					slot_duration,
					proposer,
				})
			));

		Box::pin(maybe_proposer)
	}
}

/// The Polkadot proposer logic.
pub struct Proposer<Client, TxPool: TransactionPool<Block=Block>, Backend> {
	tracker: crate::validation_service::ValidationInstanceHandle,
	slot_duration: u64,
	proposer: sc_basic_authorship::Proposer<Backend, Block, Client, TxPool>,
}

impl<Client, TxPool, Backend> consensus::Proposer<Block> for Proposer<Client, TxPool, Backend> where
	TxPool: TransactionPool<Block=Block> + 'static,
	Client: BlockBuilderProvider<Backend, Block, Client> + ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
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

	fn propose(
		self,
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

			// set up delay until next allowed timestamp.
			let current_timestamp = current_timestamp();
			if current_timestamp < believed_timestamp {
				Delay::new(Duration::from_millis(current_timestamp - believed_timestamp))
					.await;
			}

			Delay::new(enough_candidates).await;

			let proposed_candidates = self.tracker.table().proposed_set();

			let mut inherent_data = inherent_data;
			inherent_data.put_data(NEW_HEADS_IDENTIFIER, &proposed_candidates)
				.map_err(Error::InherentError)?;

			self.proposer.propose(
				inherent_data,
				inherent_digests.clone(),
				deadline_diff,
				record_proof
			).await.map_err(Into::into)
		}.boxed()
	}
}

fn current_timestamp() -> u64 {
	time::SystemTime::now().duration_since(time::UNIX_EPOCH)
		.expect("now always later than unix epoch; qed")
		.as_millis() as u64
}
