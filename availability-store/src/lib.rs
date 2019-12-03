// Copyright 2018 Parity Technologies (UK) Ltd.
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
use futures::channel::{mpsc, oneshot};
use keystore::KeyStorePtr;
use polkadot_primitives::{
	Hash, Block,
	parachain::{
		Id as ParaId, BlockData, CandidateReceipt, Message, AvailableMessages, ErasureChunk,
		ParachainHost,
	},
};
use sp_runtime::traits::{BlakeTwo256, Hash as HashT, ProvideRuntimeApi};
use sp_blockchain::{Result as ClientResult};
use client::{
	BlockchainEvents, BlockBody,
};
use sp_api::ApiExt;

use log::warn;

use std::sync::Arc;
use std::collections::HashSet;
use std::path::PathBuf;
use std::io;

mod worker;
mod store;

pub use worker::AvailabilityBlockImport;

use worker::{
	Worker, WorkerHandle, Chunks, ParachainBlocks, WorkerMsg, MakeAvailable,
};

use store::{Store as InnerStore};

/// Abstraction over an executor that lets you spawn tasks in the background.
pub(crate) type TaskExecutor =
	Arc<dyn futures01::future::Executor<
		Box<dyn futures01::Future<Item = (), Error = ()> + Send>
	> + Send + Sync>;

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
	) -> Box<dyn Stream<Item = (Hash, Hash, ErasureChunk)> + Send + Unpin>;

	/// Gossip an erasure chunk message.
	fn gossip_erasure_chunk(
		&self,
		relay_parent: Hash,
		candidate_hash: Hash,
		erasure_root: Hash,
		chunk: ErasureChunk,
	);
}

/// Some data to keep available about a parachain block candidate.
#[derive(Debug)]
pub struct Data {
	/// The relay chain parent hash this should be localized to.
	pub relay_parent: Hash,
	/// The parachain index for this candidate.
	pub parachain_id: ParaId,
	/// Block data.
	pub block_data: BlockData,
	/// Outgoing message queues from execution of the block, if any.
	///
	/// The tuple pairs the message queue root and the queue data.
	pub outgoing_queues: Option<AvailableMessages>,
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
		thread_pool: TaskExecutor,
		keystore: KeyStorePtr,
	) -> ClientResult<(AvailabilityBlockImport<I, P>)>
	where
		P: ProvideRuntimeApi + BlockchainEvents<Block> + BlockBody<Block> + Send + Sync + 'static,
		P::Api: ParachainHost<Block>,
		P::Api: ApiExt<Block, Error=sp_blockchain::Error>,
	{
		let to_worker = self.to_worker.clone();

		let import = AvailabilityBlockImport::new(
			self.inner.clone(),
			client,
			wrapped_block_import,
			thread_pool,
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
	/// The message data of `Data` is optional but is expected
	/// to be present with the exception of the case where there is no message data
	/// due to the block's invalidity. Determination of invalidity is beyond the
	/// scope of this function.
	///
	/// This method will send the `Data` to the background worker, allowing caller to
	/// asynchrounously wait for the result.
	pub async fn make_available(&self, data: Data) -> io::Result<()> {
		let (s, r) = oneshot::channel();
		let msg = WorkerMsg::MakeAvailable(MakeAvailable {
			data,
			result: s,
		});

		let _ = self.to_worker.unbounded_send(msg);

		if let Ok(Ok(())) = r.await {
			Ok(())
		} else {
			Err(io::Error::new(io::ErrorKind::Other, format!("adding erasure chunks failed")))
		}

	}

	/// Get a set of all chunks we are waiting for grouped by
	/// `(relay_parent, erasure_root, candidate_hash, our_id)`.
	pub fn awaited_chunks(&self) -> Option<HashSet<(Hash, Hash, Hash, u32)>> {
		self.inner.awaited_chunks()
	}

	/// Qery which candidates were included in the relay chain block by block's parent.
	pub fn get_candidates_in_relay_block(&self, relay_block: &Hash) -> Option<Vec<Hash>> {
		self.inner.get_candidates_in_relay_block(relay_block)
	}

	/// Make a validator's index and a number of validators at a relay parent available.
	///
	/// This information is needed before the `add_candidates_in_relay_block` is called
	/// since that call forms the awaited frontier of chunks.
	/// In the current implementation this function is called in the `get_or_instantiate` at
	/// the start of the parachain agreement process on top of some parent hash.
	pub fn add_validator_index_and_n_validators(
		&self,
		relay_parent: &Hash,
		validator_index: u32,
		n_validators: u32,
	) -> io::Result<()> {
		self.inner.add_validator_index_and_n_validators(
			relay_parent,
			validator_index,
			n_validators,
		)
	}

	/// Query a validator's index and n_validators by relay parent.
	pub fn get_validator_index_and_n_validators(&self, relay_parent: &Hash) -> Option<(u32, u32)> {
		self.inner.get_validator_index_and_n_validators(relay_parent)
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
		relay_parent: Hash,
		receipt: CandidateReceipt,
		chunk: ErasureChunk,
	) -> io::Result<()> {
		self.add_erasure_chunks(relay_parent, receipt, vec![chunk]).await
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
		relay_parent: Hash,
		receipt: CandidateReceipt,
		chunks: I,
	) -> io::Result<()>
		where I: IntoIterator<Item = ErasureChunk>
	{
		self.add_candidate(relay_parent, receipt.clone()).await?;
		let (s, r) = oneshot::channel();
		let chunks = chunks.into_iter().collect();
		let candidate_hash = receipt.hash();
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

	/// Queries an erasure chunk by its block's parent and hash and index.
	pub fn get_erasure_chunk(
		&self,
		relay_parent: &Hash,
		block_data_hash: Hash,
		index: usize,
	) -> Option<ErasureChunk> {
		self.inner.get_erasure_chunk(relay_parent, block_data_hash, index)
	}

	/// Stores a candidate receipt.
	pub async fn add_candidate(
		&self,
		relay_parent: Hash,
		receipt: CandidateReceipt,
	) -> io::Result<()> {
		let (s, r) = oneshot::channel();

		let msg = WorkerMsg::ParachainBlocks(ParachainBlocks {
			relay_parent,
			blocks: vec![(receipt, None)],
			result: s,
		});

		let _ = self.to_worker.unbounded_send(msg);

		if let Ok(Ok(())) = r.await {
			Ok(())
		} else {
			Err(io::Error::new(io::ErrorKind::Other, format!("adding erasure chunks failed")))
		}
	}

	/// Queries a candidate receipt by it's hash.
	pub fn get_candidate(&self, candidate_hash: &Hash) -> Option<CandidateReceipt> {
		self.inner.get_candidate(candidate_hash)
	}

	/// Query block data.
	pub fn block_data(&self, relay_parent: Hash, block_data_hash: Hash) -> Option<BlockData> {
		self.inner.block_data(relay_parent, block_data_hash)
	}

	/// Query block data by corresponding candidate receipt's hash.
	pub fn block_data_by_candidate(&self, relay_parent: Hash, candidate_hash: Hash)
		-> Option<BlockData>
	{
		self.inner.block_data_by_candidate(relay_parent, candidate_hash)
	}

	/// Query message queue data by message queue root hash.
	pub fn queue_by_root(&self, queue_root: &Hash) -> Option<Vec<Message>> {
		self.inner.queue_by_root(queue_root)
	}
}
