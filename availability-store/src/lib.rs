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
use sp_runtime::traits::{BlakeTwo256, Hash as HashT, HashFor};
use sp_blockchain::{Result as ClientResult};
use client::{
	BlockchainEvents, BlockBody,
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
	Worker, WorkerHandle, Chunks, IncludedParachainBlocks, WorkerMsg, MakeAvailable,
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

/// Compute gossip topic for the erasure chunk messages given the relay parent,
/// root and the chunk index.
///
/// Since at this point we are not able to use [`network`] directly, but both
/// of them need to compute these topics, this lives here and not there.
///
/// [`network`]: ../polkadot_network/index.html
pub fn erasure_coding_topic(relay_parent: Hash, erasure_root: Hash, index: u32) -> Hash {
	let mut v = relay_parent.as_ref().to_vec();
	v.extend(erasure_root.as_ref());
	v.extend(&index.to_le_bytes()[..]);
	v.extend(b"erasure_chunks");

	BlakeTwo256::hash(&v[..])
}

/// A trait that provides a shim for the [`NetworkService`] trait.
///
/// Currently it is not possible to use the networking code in the availability store
/// core directly due to a number of loop dependencies it require:
///
/// `availability-store` -> `network` -> `availability-store`
///
/// `availability-store` -> `network` -> `validation` -> `availability-store`
///
/// So we provide this shim trait that gets implemented for a wrapper newtype in
/// the [`network`] module.
///
/// [`NetworkService`]: ../polkadot_network/trait.NetworkService.html
/// [`network`]: ../polkadot_network/index.html
pub trait ProvideGossipMessages {
	/// Get a stream of gossip erasure chunk messages for a given topic.
	///
	/// Each item is a tuple (relay_parent, candidate_hash, erasure_chunk)
	fn gossip_messages_for(
		&self,
		topic: Hash,
	) -> Pin<Box<dyn Stream<Item = (Hash, Hash, ErasureChunk)> + Send>>;

	/// Gossip an erasure chunk message.
	fn gossip_erasure_chunk(
		&self,
		relay_parent: Hash,
		candidate_hash: Hash,
		erasure_root: Hash,
		chunk: ErasureChunk,
	);
}

/// Data which, when combined with an `AbridgedCandidateReceipt`, is enough
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
	/// Create a new `Store` with given condig on disk.
	///
	/// Creating a store among other things starts a background worker thread which
	/// handles most of the write operations to the storage.
	#[cfg(not(target_os = "unknown"))]
	pub fn new<PGM>(config: Config, gossip: PGM) -> io::Result<Self>
		where PGM: ProvideGossipMessages + Send + Sync + Clone + 'static
	{
		let inner = InnerStore::new(config)?;
		let worker = Arc::new(Worker::start(inner.clone(), gossip));
		let to_worker = worker.to_worker().clone();

		Ok(Self {
			inner,
			worker,
			to_worker,
		})
	}

	/// Create a new `Store` in-memory. Useful for tests.
	///
	/// Creating a store among other things starts a background worker thread
	/// which handles most of the write operations to the storage.
	pub fn new_in_memory<PGM>(gossip: PGM) -> Self
		where PGM: ProvideGossipMessages + Send + Sync + Clone + 'static
	{
		let inner = InnerStore::new_in_memory();
		let worker = Arc::new(Worker::start(inner.clone(), gossip));
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
		P: ProvideRuntimeApi<Block> + BlockchainEvents<Block> + BlockBody<Block> + Send + Sync + 'static,
		P::Api: ParachainHost<Block>,
		P::Api: ApiExt<Block, Error=sp_blockchain::Error>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HashFor<Block>>,
	{
		let to_worker = self.to_worker.clone();

		let import = AvailabilityBlockImport::new(
			self.inner.clone(),
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
	/// This method will send the chunk to the background worker, allowing caller to
	/// asynchrounously wait for the result.
	pub async fn add_erasure_chunk(
		&self,
		candidate: AbridgedCandidateReceipt,
		chunk: ErasureChunk,
	) -> io::Result<()> {
		self.add_erasure_chunks(candidate, vec![chunk]).await
	}

	/// Adds a set of erasure chunks to storage.
	///
	/// The chunks should be checked for validity against the root of encoding
	/// and it's proof prior to calling this.
	///
	/// This method will send the chunks to the background worker, allowing caller to
	/// asynchrounously waiting for the result.
	pub async fn add_erasure_chunks<I>(
		&self,
		candidate: AbridgedCandidateReceipt,
		chunks: I,
	) -> io::Result<()>
		where I: IntoIterator<Item = ErasureChunk>
	{
		let candidate_hash = candidate.hash();
		let relay_parent = candidate.relay_parent;

		self.add_candidate(candidate).await?;
		let (s, r) = oneshot::channel();
		let chunks = chunks.into_iter().collect();
		let msg = WorkerMsg::Chunks(Chunks {
			relay_parent,
			candidate_hash,
			chunks,
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
