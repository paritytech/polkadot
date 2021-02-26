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

//! The proposer proposes new blocks to include

#![deny(unused_crate_dependencies, unused_results)]

use futures::prelude::*;
use futures::select;
use polkadot_node_subsystem::{
	jaeger,
	messages::{AllMessages, ProvisionerInherentData, ProvisionerMessage}, SubsystemError,
};
use polkadot_overseer::OverseerHandler;
use polkadot_primitives::v1::{
	Block, Hash, Header,
};
use sc_block_builder::{BlockBuilderApi, BlockBuilderProvider};
use sp_core::traits::SpawnNamed;
use sp_api::{ApiExt, ProvideRuntimeApi};
use sp_blockchain::HeaderBackend;
use sp_consensus::{Proposal, DisableProofRecording};
use sp_inherents::InherentData;
use sp_runtime::traits::{DigestFor, HashFor};
use sp_transaction_pool::TransactionPool;
use prometheus_endpoint::Registry as PrometheusRegistry;
use std::{fmt, pin::Pin, sync::Arc, time};

/// How long proposal can take before we give up and err out
const PROPOSE_TIMEOUT: core::time::Duration = core::time::Duration::from_millis(2500);

/// Custom Proposer factory for Polkadot
pub struct ProposerFactory<TxPool, Backend, Client> {
	inner: sc_basic_authorship::ProposerFactory<TxPool, Backend, Client, DisableProofRecording>,
	overseer: OverseerHandler,
}

impl<TxPool, Backend, Client> ProposerFactory<TxPool, Backend, Client> {
	pub fn new(
		spawn_handle: impl SpawnNamed + 'static,
		client: Arc<Client>,
		transaction_pool: Arc<TxPool>,
		overseer: OverseerHandler,
		prometheus: Option<&PrometheusRegistry>,
	) -> Self {
		ProposerFactory {
			inner: sc_basic_authorship::ProposerFactory::new(
				spawn_handle,
				client,
				transaction_pool,
				prometheus,
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
		BlockBuilderApi<Block> + ApiExt<Block>,
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
		let parent_header = parent_header.clone();

		async move {
			Ok(Proposer {
				inner: proposer?,
				overseer,
				parent_header,
				parent_header_hash,
			})
		}.boxed()
	}
}

/// Custom Proposer for Polkadot.
///
/// This proposer gets the ProvisionerInherentData and injects it into the wrapped
/// proposer's inherent data, then delegates the actual proposal generation.
pub struct Proposer<TxPool: TransactionPool<Block = Block>, Backend, Client> {
	inner: sc_basic_authorship::Proposer<Backend, Block, Client, TxPool, DisableProofRecording>,
	overseer: OverseerHandler,
	parent_header: Header,
	parent_header_hash: Hash,
}

// This impl has the same generic bounds as the Proposer impl.
impl<TxPool, Backend, Client> Proposer<TxPool, Backend, Client>
where
	TxPool: 'static + TransactionPool<Block = Block>,
	Client: 'static
		+ BlockBuilderProvider<Backend, Block, Client>
		+ ProvideRuntimeApi<Block>
		+ HeaderBackend<Block>
		+ Send
		+ Sync,
	Client::Api:
		BlockBuilderApi<Block> + ApiExt<Block>,
	Backend:
		'static + sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>>,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HashFor<Block>> + Send,
{
	/// Get provisioner inherent data
	///
	/// This function has a constant timeout: `PROPOSE_TIMEOUT`.
	async fn get_provisioner_data(&self) -> Result<ProvisionerInherentData, Error> {
		// clone this (lightweight) data because we're going to move it into the future
		let mut overseer = self.overseer.clone();
		let parent_header_hash = self.parent_header_hash.clone();

		let pid = async {
			let (sender, receiver) = futures::channel::oneshot::channel();
			overseer.wait_for_activation(parent_header_hash, sender).await;
			receiver.await.map_err(|_| Error::ClosedChannelAwaitingActivation)??;

			let (sender, receiver) = futures::channel::oneshot::channel();
			overseer.send_msg(AllMessages::Provisioner(
				ProvisionerMessage::RequestInherentData(parent_header_hash, sender),
			)).await;

			receiver.await.map_err(|_| Error::ClosedChannelAwaitingInherentData)
		};

		let mut timeout = futures_timer::Delay::new(PROPOSE_TIMEOUT).fuse();

		select! {
			pid = pid.fuse() => pid,
			_ = timeout => Err(Error::Timeout),
		}
	}
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
		BlockBuilderApi<Block> + ApiExt<Block>,
	Backend:
		'static + sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>>,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HashFor<Block>> + Send,
{
	type Transaction = sc_client_api::TransactionFor<Backend, Block>;
	type Proposal = Pin<Box<
		dyn Future<Output = Result<Proposal<Block, sp_api::TransactionFor<Client, Block>, ()>, Error>> + Send,
	>>;
	type Error = Error;
	type ProofRecording = DisableProofRecording;
	type Proof = ();

	fn propose(
		self,
		mut inherent_data: InherentData,
		inherent_digests: DigestFor<Block>,
		max_duration: time::Duration,
	) -> Self::Proposal {
		async move {
			let span = jaeger::hash_span(&self.parent_header_hash, "propose");
			let _span = span.child("get-provisioner");

			let provisioner_data = match self.get_provisioner_data().await {
				Ok(pd) => pd,
				Err(err) => {
					tracing::warn!(err = ?err, "could not get provisioner inherent data; injecting default data");
					Default::default()
				}
			};

			drop(_span);

			let inclusion_inherent_data = (
				provisioner_data.0,
				provisioner_data.1,
				self.parent_header,
			);
			inherent_data.put_data(
				polkadot_primitives::v1::INCLUSION_INHERENT_IDENTIFIER,
				&inclusion_inherent_data,
			)?;

			let _span = span.child("authorship-propose");
			self.inner
				.propose(inherent_data, inherent_digests, max_duration)
				.await
				.map_err(Into::into)
		}
		.boxed()
	}
}

// It would have been more ergonomic to use thiserror to derive the
// From implementations, Display, and std::error::Error, but unfortunately
// one of the wrapped errors (sp_inherents::Error) also
// don't impl std::error::Error, which breaks the thiserror derive.
#[derive(Debug)]
pub enum Error {
	Consensus(sp_consensus::Error),
	Blockchain(sp_blockchain::Error),
	Inherent(sp_inherents::Error),
	Timeout,
	ClosedChannelAwaitingActivation,
	ClosedChannelAwaitingInherentData,
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
			Self::ClosedChannelAwaitingActivation => write!(f, "closed channel from overseer when awaiting activation"),
			Self::ClosedChannelAwaitingInherentData => write!(f, "closed channel from provisioner when awaiting inherent data"),
			Self::Subsystem(err) => write!(f, "subsystem error: {:?}", err),
		}
	}
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::Consensus(err) => Some(err),
			Self::Blockchain(err) => Some(err),
			Self::Subsystem(err) => Some(err),
			_ => None
		}
	}
}
