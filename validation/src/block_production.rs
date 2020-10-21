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
	time::Duration,
};

use sp_blockchain::HeaderBackend;
use block_builder::{BlockBuilderApi, BlockBuilderProvider};
use consensus::{Proposal, RecordProof};
use primitives::traits::SpawnNamed;
use polkadot_primitives::v0::{NEW_HEADS_IDENTIFIER, Block, Header, AttestedCandidate};
use runtime_primitives::traits::{DigestFor, HashFor};
use txpool_api::TransactionPool;

use futures::prelude::*;
use inherents::InherentData;
use sp_api::{ApiExt, ProvideRuntimeApi};
use prometheus_endpoint::Registry as PrometheusRegistry;

use crate::Error;

// Polkadot proposer factory.
pub struct ProposerFactory<Client, TxPool, Backend> {
	factory: sc_basic_authorship::ProposerFactory<TxPool, Backend, Client>,
}

impl<Client, TxPool, Backend> ProposerFactory<Client, TxPool, Backend> {
	/// Create a new proposer factory.
	pub fn new(
		spawn_handle: Box<dyn SpawnNamed>,
		client: Arc<Client>,
		transaction_pool: Arc<TxPool>,
		prometheus: Option<&PrometheusRegistry>,
	) -> Self {
		let factory = sc_basic_authorship::ProposerFactory::new(
			spawn_handle,
			client,
			transaction_pool,
			prometheus,
		);
		ProposerFactory {
			factory,
		}
	}
}

impl<Client, TxPool, Backend> consensus::Environment<Block>
	for ProposerFactory<Client, TxPool, Backend>
where
	TxPool: TransactionPool<Block=Block> + 'static,
	Client: BlockBuilderProvider<Backend, Block, Client> + ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	Client::Api: BlockBuilderApi<Block>
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
		let proposer = self.factory.init(parent_header)
			.into_inner()
			.map_err(Into::into)
			.map(|proposer| Proposer { proposer });

		Box::pin(future::ready(proposer))
	}
}

/// The Polkadot proposer logic.
pub struct Proposer<Client, TxPool: TransactionPool<Block=Block>, Backend> {
	proposer: sc_basic_authorship::Proposer<Backend, Block, Client, TxPool>,
}

impl<Client, TxPool, Backend> consensus::Proposer<Block> for Proposer<Client, TxPool, Backend> where
	TxPool: TransactionPool<Block=Block> + 'static,
	Client: BlockBuilderProvider<Backend, Block, Client> + ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	Client::Api: BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	Backend: sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>> + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HashFor<Block>> + Send,
{
	type Error = Error;
	type Transaction = sp_api::TransactionFor<Client, Block>;
	type Proposal = Pin<
		Box<
			dyn Future<Output = Result<
				Proposal<Block, sp_api::TransactionFor<Client, Block>>,
				Self::Error,
			>>
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
		async move {
			let mut inherent_data = inherent_data;
			inherent_data.put_data(NEW_HEADS_IDENTIFIER, &Vec::<AttestedCandidate>::new())
				.map_err(Error::InherentError)?;

			self.proposer.propose(
				inherent_data,
				inherent_digests.clone(),
				max_duration,
				record_proof
			).await.map_err(Into::into)
		}.boxed()
	}
}
