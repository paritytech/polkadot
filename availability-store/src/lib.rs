// Copyright 2018-2020 Parity Technologies (UK) Ltd.
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

//! Persistent database for parachain data: PoV block data, erasure-coding chunks and outgoing messages.
//!
//! This will be written into during the block validation pipeline, and queried
//! by networking code in order to circulate required data and maintain availability
//! of it.

#![warn(missing_docs)]

use futures::prelude::*;
use futures::{channel::{mpsc, oneshot}, task::Spawn};
use keystore::KeyStorePtr;
use polkadot_primitives::{
	Hash, Block,
	parachain::{
		PoVBlock, AbridgedCandidateReceipt, ErasureChunk,
		ParachainHost, AvailableData, OmittedValidationData,
	},
};
use sp_runtime::traits::HashFor;
use sp_blockchain::{Result as ClientResult};
use client::{
	BlockchainEvents, BlockBackend,
};
use sp_api::{ApiExt, ProvideRuntimeApi};
use codec::{Encode, Decode};

use log::warn;

use std::sync::Arc;
use std::collections::HashSet;
use std::path::PathBuf;
use std::io;
use std::pin::Pin;

mod worker;
mod store;

pub use worker::AvailabilityBlockImport;
pub use store::AwaitedFrontierEntry;

use worker::{
	Worker, WorkerHandle, IncludedParachainBlocks, WorkerMsg, MakeAvailable, Chunks
};

use store::{Store as InnerStore};

const LOG_TARGET: &str = "availability";

/// Configuration for the availability store.
pub struct Config {
	/// Cache size in bytes. If `None` default is used.
	pub cache_size: Option<usize>,
	/// Path to the database.
	pub path: PathBuf,
}

/// An abstraction around networking for the availablity-store.
///
/// Currently it is not possible to use the networking code in the availability store
/// core directly due to a number of loop dependencies it requires:
///
/// `availability-store` -> `network` -> `availability-store`
///
/// `availability-store` -> `network` -> `validation` -> `availability-store`
///
/// So we provide this trait that gets implemented for a type in
/// the [`network`] module or a mock in tests.
///
/// [`network`]: ../polkadot_network/index.html
pub trait ErasureNetworking {
	/// Errors that can occur when fetching erasure chunks.
	type Error: std::fmt::Debug + 'static;

	/// Fetch an erasure chunk from the networking service.
	fn fetch_erasure_chunk(
		&self,
		candidate_hash: &Hash,
		index: u32,
	) -> Pin<Box<dyn Future<Output = Result<ErasureChunk, Self::Error>> + Send>>;

	/// Distributes an erasure chunk to the correct validator node.
	fn distribute_erasure_chunk(
		&self,
		candidate_hash: Hash,
		chunk: ErasureChunk,
	);
}

/// Data that, when combined with an `AbridgedCandidateReceipt`, is enough
/// to fully re-execute a block.
#[derive(Debug, Encode, Decode, PartialEq)]
pub struct ExecutionData {
	/// The `PoVBlock`.
	pub pov_block: PoVBlock,
	/// The data omitted from the `AbridgedCandidateReceipt`.
	pub omitted_validation: OmittedValidationData,
}

/// Handle to the availability store.
///
/// This provides a proxying API that
///   * in case of write operations provides async methods that send data to
///     the background worker and resolve when that data is processed by the worker
///   * in case of read opeartions queries the underlying storage synchronously.
#[derive(Clone)]
pub struct Store {
	inner: InnerStore,
	worker: Arc<WorkerHandle>,
	to_worker: mpsc::UnboundedSender<WorkerMsg>,
}

impl Store {
	/// Create a new `Store` with given config on disk.
	///
	/// Creating a store among other things starts a background worker thread that
	/// handles most of the write operations to the storage.
	#[cfg(not(target_os = "unknown"))]
	pub fn new<EN>(config: Config, network: EN) -> io::Result<Self>
		where EN: ErasureNetworking + Send + Sync + Clone + 'static
	{
		let inner = InnerStore::new(config)?;
		let worker = Arc::new(Worker::start(inner.clone(), network));
		let to_worker = worker.to_worker().clone();

		Ok(Self {
			inner,
			worker,
			to_worker,
		})
	}

	/// Create a new in-memory `Store`. Useful for tests.
	///
	/// Creating a store among other things starts a background worker thread
	/// that handles most of the write operations to the storage.
	pub fn new_in_memory<EN>(network: EN) -> Self
		where EN: ErasureNetworking + Send + Sync + Clone + 'static
	{
		let inner = InnerStore::new_in_memory();
		let worker = Arc::new(Worker::start(inner.clone(), network));
		let to_worker = worker.to_worker().clone();

		Self {
			inner,
			worker,
			to_worker,
		}
	}

	/// Obtain a [`BlockImport`] implementation to import blocks into this store.
	///
	/// This block import will act upon all newly imported blocks sending information
	/// about parachain heads included in them to this `Store`'s background worker.
	/// The user may create multiple instances of [`BlockImport`]s with this call.
	///
	/// [`BlockImport`]: https://substrate.dev/rustdocs/v1.0/substrate_consensus_common/trait.BlockImport.html
	pub fn block_import<I, P>(
		&self,
		wrapped_block_import: I,
		client: Arc<P>,
		spawner: impl Spawn,
		keystore: KeyStorePtr,
	) -> ClientResult<AvailabilityBlockImport<I, P>>
	where
		P: ProvideRuntimeApi<Block> + BlockchainEvents<Block> + BlockBackend<Block> + Send + Sync + 'static,
		P::Api: ParachainHost<Block>,
		P::Api: ApiExt<Block, Error=sp_blockchain::Error>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HashFor<Block>>,
	{
		let to_worker = self.to_worker.clone();

		let import = AvailabilityBlockImport::new(
			client,
			wrapped_block_import,
			spawner,
			keystore,
			to_worker,
		);

		Ok(import)
	}

	/// Make some data available provisionally.
	///
	/// Validators with the responsibility of maintaining availability
	/// for a block or collators collating a block will call this function
	/// in order to persist that data to disk and so it can be queried and provided
	/// to other nodes in the network.
	///
	/// Determination of invalidity is beyond the scope of this function.
	///
	/// This method will send the data to the background worker, allowing the caller to
	/// asynchronously wait for the result.
	pub async fn make_available(&self, candidate_hash: Hash, available_data: AvailableData)
		-> io::Result<()>
	{
		let (s, r) = oneshot::channel();
		let msg = WorkerMsg::MakeAvailable(MakeAvailable {
			candidate_hash,
			available_data,
			result: s,
		});

		let _ = self.to_worker.unbounded_send(msg);

		if let Ok(Ok(())) = r.await {
			Ok(())
		} else {
			Err(io::Error::new(io::ErrorKind::Other, format!("adding erasure chunks failed")))
		}

	}

	/// Get a set of all chunks we are waiting for.
	pub fn awaited_chunks(&self) -> Option<HashSet<AwaitedFrontierEntry>> {
		self.inner.awaited_chunks()
	}

	/// Adds an erasure chunk to storage.
	///
	/// The chunk should be checked for validity against the root of encoding
	/// and its proof prior to calling this.
	///
	/// This method will send the chunk to the background worker, allowing the caller to
	/// asynchronously wait for the result.
	pub async fn add_erasure_chunk(
		&self,
		candidate: AbridgedCandidateReceipt,
		n_validators: u32,
		chunk: ErasureChunk,
	) -> io::Result<()> {
		self.add_erasure_chunks(candidate, n_validators, std::iter::once(chunk)).await
	}

	/// Adds a set of erasure chunks to storage.
	///
	/// The chunks should be checked for validity against the root of encoding
	/// and its proof prior to calling this.
	///
	/// This method will send the chunks to the background worker, allowing the caller to
	/// asynchronously wait for the result.
	pub async fn add_erasure_chunks<I>(
		&self,
		candidate: AbridgedCandidateReceipt,
		n_validators: u32,
		chunks: I,
	) -> io::Result<()>
		where I: IntoIterator<Item = ErasureChunk>
	{
		let candidate_hash = candidate.hash();

		self.add_candidate(candidate).await?;

		let (s, r) = oneshot::channel();
		let chunks = chunks.into_iter().collect();

		let msg = WorkerMsg::Chunks(Chunks {
			candidate_hash,
			chunks,
			n_validators,
			result: s,
		});

		let _ = self.to_worker.unbounded_send(msg);

		if let Ok(Ok(())) = r.await {
			Ok(())
		} else {
			Err(io::Error::new(io::ErrorKind::Other, format!("adding erasure chunks failed")))
		}
	}

	/// Queries an erasure chunk by the candidate hash and validator index.
	pub fn get_erasure_chunk(
		&self,
		candidate_hash: &Hash,
		validator_index: usize,
	) -> Option<ErasureChunk> {
		self.inner.get_erasure_chunk(candidate_hash, validator_index)
	}

	/// Note a validator's index and a number of validators at a relay parent in the
	/// store.
	///
	/// This should be done before adding erasure chunks with this relay parent.
	pub fn note_validator_index_and_n_validators(
		&self,
		relay_parent: &Hash,
		validator_index: u32,
		n_validators: u32,
	) -> io::Result<()> {
		self.inner.note_validator_index_and_n_validators(
			relay_parent,
			validator_index,
			n_validators,
		)
	}

	// Stores a candidate receipt.
	async fn add_candidate(
		&self,
		candidate: AbridgedCandidateReceipt,
	) -> io::Result<()> {
		let (s, r) = oneshot::channel();

		let msg = WorkerMsg::IncludedParachainBlocks(IncludedParachainBlocks {
			blocks: vec![crate::worker::IncludedParachainBlock {
				candidate,
				available_data: None,
			}],
			result: s,
		});

		let _ = self.to_worker.unbounded_send(msg);

		if let Ok(Ok(())) = r.await {
			Ok(())
		} else {
			Err(io::Error::new(io::ErrorKind::Other, format!("adding erasure chunks failed")))
		}
	}

	/// Queries a candidate receipt by its hash.
	pub fn get_candidate(&self, candidate_hash: &Hash)
		-> Option<AbridgedCandidateReceipt>
	{
		self.inner.get_candidate(candidate_hash)
	}

	/// Query execution data by pov-block hash.
	pub fn execution_data(&self, candidate_hash: &Hash)
		-> Option<ExecutionData>
	{
		self.inner.execution_data(candidate_hash)
	}
}
