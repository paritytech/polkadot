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

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::thread;

use log::{error, info, trace, warn};
use sp_blockchain::{Result as ClientResult};
use sp_runtime::traits::{Header as HeaderT, ProvideRuntimeApi, Block as BlockT};
use sp_api::ApiExt;
use client::{
	BlockchainEvents, BlockBody,
	blockchain::ProvideCache,
};
use consensus_common::{
	self, BlockImport, BlockCheckParams, BlockImportParams, Error as ConsensusError,
	ImportResult,
	import_queue::CacheKeyId,
};
use polkadot_primitives::{Block, BlockId, Hash};
use polkadot_primitives::parachain::{
	CandidateReceipt, ParachainHost, ValidatorId,
	ValidatorPair, AvailableMessages, BlockData, ErasureChunk,
};
use futures::{prelude::*, future::select, channel::{mpsc, oneshot}, task::SpawnExt};
use keystore::KeyStorePtr;

use tokio::runtime::{Handle, Runtime as LocalRuntime};

use crate::{LOG_TARGET, Data, TaskExecutor, ProvideGossipMessages, erasure_coding_topic};
use crate::store::Store;

/// Errors that may occur.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub(crate) enum Error {
	#[from]
	StoreError(io::Error),
	#[display(fmt = "Validator's id and number of validators at block with parent {} not found", relay_parent)]
	IdAndNValidatorsNotFound { relay_parent: Hash },
	#[display(fmt = "Candidate receipt with hash {} not found", candidate_hash)]
	CandidateNotFound { candidate_hash: Hash },
}

/// Messages sent to the `Worker`.
///
/// Messages are sent in a number of different scenarios,
/// for instance, when:
///   * importing blocks in `BlockImport` implementation,
///   * recieving finality notifications,
///   * when the `Store` api is used by outside code.
#[derive(Debug)]
pub(crate) enum WorkerMsg {
	ErasureRoots(ErasureRoots),
	ParachainBlocks(ParachainBlocks),
	ListenForChunks(ListenForChunks),
	Chunks(Chunks),
	CandidatesFinalized(CandidatesFinalized),
	MakeAvailable(MakeAvailable),
}

/// The erasure roots of the heads included in the block with a given parent.
#[derive(Debug)]
pub(crate) struct ErasureRoots {
	/// The relay parent of the block these roots belong to.
	pub relay_parent: Hash,
	/// The roots themselves.
	pub erasure_roots: Vec<Hash>,
	/// A sender to signal the result asynchronously.
	pub result: oneshot::Sender<Result<(), Error>>,
}

/// The receipts of the heads included into the block with a given parent.
#[derive(Debug)]
pub(crate) struct ParachainBlocks {
	/// The relay parent of the block these parachain blocks belong to.
	pub relay_parent: Hash,
	/// The blocks themselves.
	pub blocks: Vec<(CandidateReceipt, Option<(BlockData, AvailableMessages)>)>,
	/// A sender to signal the result asynchronously.
	pub result: oneshot::Sender<Result<(), Error>>,
}

/// Listen gossip for these chunks.
#[derive(Debug)]
pub(crate) struct ListenForChunks {
	/// The relay parent of the block the chunks from we want to listen to.
	pub relay_parent: Hash,
	/// The hash of the candidate chunk belongs to.
	pub candidate_hash: Hash,
	/// The index of the chunk we need.
	pub index: u32,
	/// A sender to signal the result asynchronously.
	pub result: Option<oneshot::Sender<Result<(), Error>>>,
}

/// We have received some chunks.
#[derive(Debug)]
pub(crate) struct Chunks {
	/// The relay parent of the block these chunks belong to.
	pub relay_parent: Hash,
	/// The hash of the parachain candidate these chunks belong to.
	pub candidate_hash: Hash,
	/// The chunks.
	pub chunks: Vec<ErasureChunk>,
	/// A sender to signal the result asynchronously.
	pub result: oneshot::Sender<Result<(), Error>>,
}

/// These candidates have been finalized, so unneded availability may be now pruned
#[derive(Debug)]
pub(crate) struct CandidatesFinalized {
	/// The relay parent of the block that was finalized.
	relay_parent: Hash,
	/// The parachain heads that were finalized in this block.
	candidate_hashes: Vec<Hash>,
}

/// The message that corresponds to `make_available` call of the crate API.
#[derive(Debug)]
pub(crate) struct MakeAvailable {
	/// The data being made available.
	pub data: Data,
	/// A sender to signal the result asynchronously.
	pub result: oneshot::Sender<Result<(), Error>>,
}

/// An availability worker with it's inner state.
pub(super) struct Worker<PGM> {
	availability_store: Store,
	provide_gossip_messages: PGM,
	registered_gossip_streams: HashMap<Hash, exit_future::Signal>,

	sender: mpsc::UnboundedSender<WorkerMsg>,
}

/// The handle to the `Worker`.
pub(super) struct WorkerHandle {
	exit_signal: Option<exit_future::Signal>,
	thread: Option<thread::JoinHandle<io::Result<()>>>,
	sender: mpsc::UnboundedSender<WorkerMsg>,
}

impl WorkerHandle {
	pub(crate) fn to_worker(&self) -> &mpsc::UnboundedSender<WorkerMsg> {
		&self.sender
	}
}

impl Drop for WorkerHandle {
	fn drop(&mut self) {
		if let Some(signal) = self.exit_signal.take() {
			let _ = signal.fire();
		}

		if let Some(thread) = self.thread.take() {
			if let Err(_) = thread.join() {
				error!(target: LOG_TARGET, "Errored stopping the thread");
			}
		}
	}
}

async fn listen_for_chunks<PGM, S>(
	p: PGM,
	topic: Hash,
	mut sender: S
)
where
	PGM: ProvideGossipMessages,
	S: Sink<WorkerMsg> + Unpin,
{
	trace!(target: LOG_TARGET, "Registering gossip listener for topic {}", topic);
	let mut chunks_stream = p.gossip_messages_for(topic);

	while let Some(item) = chunks_stream.next().await {
		let (s, _) = oneshot::channel();
		trace!(target: LOG_TARGET, "Received for {:?}", item);
		let chunks = Chunks {
			relay_parent: item.0,
			candidate_hash: item.1,
			chunks: vec![item.2],
			result: s,
		};

		if let Err(_) = sender.send(WorkerMsg::Chunks(chunks)).await {
			break;
		}
	}
}


fn fetch_candidates<P>(client: &P, extrinsics: Vec<<Block as BlockT>::Extrinsic>, parent: &BlockId)
	-> ClientResult<Option<Vec<CandidateReceipt>>>
where
	P: ProvideRuntimeApi,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	let api = client.runtime_api();

	let candidates = if api.has_api_with::<dyn ParachainHost<Block, Error = ()>, _>(
		parent,
		|version| version >= 2,
	).map_err(|e| ConsensusError::ChainLookup(e.to_string()))? {
		api.get_heads(&parent, extrinsics)
			.map_err(|e| ConsensusError::ChainLookup(e.to_string()))?
	} else {
		None
	};

	Ok(candidates)
}

/// Creates a task to prune entries in availability store upon block finalization.
async fn prune_unneeded_availability<P, S>(client: Arc<P>, mut sender: S)
where
	P: ProvideRuntimeApi + BlockchainEvents<Block> + BlockBody<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + ApiExt<Block, Error=sp_blockchain::Error>,
	S: Sink<WorkerMsg> + Clone + Send + Sync + Unpin,
{
	let mut finality_notification_stream = client.finality_notification_stream();

	while let Some(notification) = finality_notification_stream.next().await {
		let hash = notification.hash;
		let parent_hash = notification.header.parent_hash;
		let extrinsics = match client.block_body(&BlockId::hash(hash)) {
			Ok(Some(extrinsics)) => extrinsics,
			Ok(None) => {
				error!(
					target: LOG_TARGET,
					"No block body found for imported block {:?}",
					hash,
				);
				continue;
			}
			Err(e) => {
				error!(
					target: LOG_TARGET,
					"Failed to get block body for imported block {:?}: {:?}",
					hash,
					e,
				);
				continue;
			}
		};

		let candidate_hashes = match fetch_candidates(
			&*client,
			extrinsics,
			&BlockId::hash(parent_hash)
		) {
			Ok(Some(candidates)) => candidates.into_iter().map(|c| c.hash()).collect(),
			Ok(None) => {
				warn!(
					target: LOG_TARGET,
					"Failed to extract candidates from block body of imported block {:?}", hash
				);
				continue;
			}
			Err(e) => {
				warn!(
					target: LOG_TARGET,
					"Failed to fetch block body for imported block {:?}: {:?}", hash, e
				);
				continue;
			}
		};

		let msg = WorkerMsg::CandidatesFinalized(CandidatesFinalized {
			relay_parent: parent_hash,
			candidate_hashes
		});

		if let Err(_) = sender.send(msg).await {
			break;
		}
	}
}

impl<PGM> Drop for Worker<PGM> {
	fn drop(&mut self) {
		for (_, signal) in self.registered_gossip_streams.drain() {
			let _ = signal.fire();
		}
	}
}

impl<PGM> Worker<PGM>
where
	PGM: ProvideGossipMessages + Clone + Send + 'static,
{

	// Called on startup of the worker to register listeners for all awaited chunks.
	fn register_listeners(
		&mut self,
		runtime_handle: &Handle,
		sender: &mut mpsc::UnboundedSender<WorkerMsg>,
	) {
		if let Some(awaited_chunks) = self.availability_store.awaited_chunks() {
			for chunk in awaited_chunks {
				if let Err(e) = self.register_chunks_listener(
					runtime_handle,
					sender,
					chunk.0,
					chunk.1,
				) {
					warn!(target: LOG_TARGET, "Failed to register gossip listener: {}", e);
				}
			}
		}
	}

	fn register_chunks_listener(
		&mut self,
		runtime_handle: &Handle,
		sender: &mut mpsc::UnboundedSender<WorkerMsg>,
		relay_parent: Hash,
		erasure_root: Hash,
	) -> Result<(), Error> {
		let (local_id, _) = self.availability_store
			.get_validator_index_and_n_validators(&relay_parent)
			.ok_or(Error::IdAndNValidatorsNotFound { relay_parent })?;
		let topic = erasure_coding_topic(relay_parent, erasure_root, local_id);
		trace!(
			target: LOG_TARGET,
			"Registering listener for erasure chunks topic {} for ({}, {})",
			topic,
			relay_parent,
			erasure_root,
		);

		let (signal, exit) = exit_future::signal();

		let fut = listen_for_chunks(
			self.provide_gossip_messages.clone(),
			topic,
			sender.clone(),
		);

		self.registered_gossip_streams.insert(topic, signal);

		let _ = runtime_handle.spawn(select(fut.boxed(), exit).map(drop));

		Ok(())
	}

	fn on_parachain_blocks_received(
		&mut self,
		runtime_handle: &Handle,
		sender: &mut mpsc::UnboundedSender<WorkerMsg>,
		relay_parent: Hash,
		blocks: Vec<(CandidateReceipt, Option<(BlockData, AvailableMessages)>)>,
	) -> Result<(), Error> {
		let hashes: Vec<_> = blocks.iter().map(|(c, _)| c.hash()).collect();

		// First we have to add the receipts themselves.
		for (candidate, block) in blocks.into_iter() {
			let _ = self.availability_store.add_candidate(&candidate);

			if let Some((_block, _msgs)) = block {
				// Should we be breaking block into chunks here and gossiping it and so on?
			}

			if let Err(e) = self.register_chunks_listener(
				runtime_handle,
				sender,
				relay_parent,
				candidate.erasure_root
			) {
				warn!(target: LOG_TARGET, "Failed to register chunk listener: {}", e);
			}
		}

		let _ = self.availability_store.add_candidates_in_relay_block(
			&relay_parent,
			hashes
		);

		Ok(())
	}

	// Processes chunks messages that contain awaited items.
	//
	// When an awaited item is received, it is placed into the availability store
	// and removed from the frontier. Listener de-registered.
	fn on_chunks_received(
		&mut self,
		relay_parent: Hash,
		candidate_hash: Hash,
		chunks: Vec<ErasureChunk>,
	) -> Result<(), Error> {
		let (_, n_validators) = self.availability_store
			.get_validator_index_and_n_validators(&relay_parent)
			.ok_or(Error::IdAndNValidatorsNotFound { relay_parent })?;

		let receipt = self.availability_store.get_candidate(&candidate_hash)
			.ok_or(Error::CandidateNotFound { candidate_hash })?;

		for chunk in &chunks {
			let topic = erasure_coding_topic(relay_parent, receipt.erasure_root, chunk.index);
			// need to remove gossip listener and stop it.
			if let Some(signal) = self.registered_gossip_streams.remove(&topic) {
				let _ = signal.fire();
			}
		}

		self.availability_store.add_erasure_chunks(
			n_validators,
			&relay_parent,
			&candidate_hash,
			chunks,
		)?;

		Ok(())
	}

	// Adds the erasure roots into the store.
	fn on_erasure_roots_received(
		&mut self,
		relay_parent: Hash,
		erasure_roots: Vec<Hash>
	) -> Result<(), Error> {
		self.availability_store.add_erasure_roots_in_relay_block(&relay_parent, erasure_roots)?;

		Ok(())
	}

	// Processes the `ListenForChunks` message.
	//
	// When the worker receives a `ListenForChunk` message, it double-checks that
	// we don't have that piece, and then it registers a listener.
	fn on_listen_for_chunks_received(
		&mut self,
		runtime_handle: &Handle,
		sender: &mut mpsc::UnboundedSender<WorkerMsg>,
		relay_parent: Hash,
		candidate_hash: Hash,
		id: usize
	) -> Result<(), Error> {
		let candidate = self.availability_store.get_candidate(&candidate_hash)
			.ok_or(Error::CandidateNotFound { candidate_hash })?;

		if self.availability_store
			.get_erasure_chunk(&relay_parent, candidate.block_data_hash, id)
			.is_none() {
			if let Err(e) = self.register_chunks_listener(
				runtime_handle,
				sender,
				relay_parent,
				candidate.erasure_root
			) {
				warn!(target: LOG_TARGET, "Failed to register a gossip listener: {}", e);
			}
		}

		Ok(())
	}

	/// Starts a worker with a given availability store and a gossip messages provider.
	pub fn start(
		availability_store: Store,
		provide_gossip_messages: PGM,
	) -> WorkerHandle {
		let (sender, mut receiver) = mpsc::unbounded();

		let mut worker = Self {
			availability_store,
			provide_gossip_messages,
			registered_gossip_streams: HashMap::new(),
			sender: sender.clone(),
		};

		let sender = sender.clone();
		let (signal, exit) = exit_future::signal();

		let handle = thread::spawn(move || -> io::Result<()> {
			let mut runtime = LocalRuntime::new()?;
			let mut sender = worker.sender.clone();

			let runtime_handle = runtime.handle().clone();

			// On startup, registers listeners (gossip streams) for all
			// (relay_parent, erasure-root, i) in the awaited frontier.
			worker.register_listeners(runtime.handle(), &mut sender);

			let process_notification = async move {
				while let Some(msg) = receiver.next().await {
					trace!(target: LOG_TARGET, "Received message {:?}", msg);

					let res = match msg {
						WorkerMsg::ErasureRoots(msg) => {
							let ErasureRoots { relay_parent, erasure_roots, result} = msg;
							let res = worker.on_erasure_roots_received(
								relay_parent,
								erasure_roots,
							);
							let _ = result.send(res);
							Ok(())
						}
						WorkerMsg::ListenForChunks(msg) => {
							let ListenForChunks {
								relay_parent,
								candidate_hash,
								index,
								result,
							} = msg;

							let res = worker.on_listen_for_chunks_received(
								&runtime_handle,
								&mut sender,
								relay_parent,
								candidate_hash,
								index as usize,
							);

							if let Some(result) = result {
								let _ = result.send(res);
							}
							Ok(())
						}
						WorkerMsg::ParachainBlocks(msg) => {
							let ParachainBlocks {
								relay_parent,
								blocks,
								result,
							} = msg;

							let res = worker.on_parachain_blocks_received(
								&runtime_handle,
								&mut sender,
								relay_parent,
								blocks,
							);

							let _ = result.send(res);
							Ok(())
						}
						WorkerMsg::Chunks(msg) => {
							let Chunks { relay_parent, candidate_hash, chunks, result } = msg;
							let res = worker.on_chunks_received(
								relay_parent,
								candidate_hash,
								chunks,
							);

							let _ = result.send(res);
							Ok(())
						}
						WorkerMsg::CandidatesFinalized(msg) => {
							let CandidatesFinalized { relay_parent, candidate_hashes } = msg;

							worker.availability_store.candidates_finalized(
								relay_parent,
								candidate_hashes.into_iter().collect(),
							)
						}
						WorkerMsg::MakeAvailable(msg) => {
							let MakeAvailable { data, result } = msg;
							let res = worker.availability_store.make_available(data)
								.map_err(|e| e.into());
							let _ = result.send(res);
							Ok(())
						}
					};

					if let Err(_) = res {
						warn!(target: LOG_TARGET, "An error occured while processing a message");
					}
				}

			};

			runtime.spawn(select(process_notification.boxed(), exit.clone()).map(drop));

			runtime.block_on(exit);

			info!(target: LOG_TARGET, "Availability worker exiting");

			Ok(())
		});

		WorkerHandle {
			thread: Some(handle),
			sender,
			exit_signal: Some(signal),
		}
	}
}

/// Implementor of the [`BlockImport`] trait.
///
/// Used to embed `availability-store` logic into the block imporing pipeline.
///
/// [`BlockImport`]: https://substrate.dev/rustdocs/v1.0/substrate_consensus_common/trait.BlockImport.html
pub struct AvailabilityBlockImport<I, P> {
	availability_store: Store,
	inner: I,
	client: Arc<P>,
	keystore: KeyStorePtr,
	to_worker: mpsc::UnboundedSender<WorkerMsg>,
	exit_signal: Option<exit_future::Signal>,
}

impl<I, P> Drop for AvailabilityBlockImport<I, P> {
	fn drop(&mut self) {
		if let Some(signal) = self.exit_signal.take() {
			let _ = signal.fire();
		}
	}
}

impl<I, P> BlockImport<Block> for AvailabilityBlockImport<I, P> where
	I: BlockImport<Block> + Send + Sync,
	I::Error: Into<ConsensusError>,
	P: ProvideRuntimeApi + ProvideCache<Block>,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	type Error = ConsensusError;

	fn import_block(
		&mut self,
		block: BlockImportParams<Block>,
		new_cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		trace!(
			target: LOG_TARGET,
			"Importing block #{}, ({})",
			block.header.number(),
			block.post_header().hash()
		);

		if let Some(ref extrinsics) = block.body {
			let relay_parent = *block.header.parent_hash();
			let parent_id = BlockId::hash(*block.header.parent_hash());
			// Extract our local position i from the validator set of the parent.
			let validators = self.client.runtime_api().validators(&parent_id)
				.map_err(|e| ConsensusError::ChainLookup(e.to_string()))?;

			let our_id = self.our_id(&validators);

			// Use a runtime API to extract all included erasure-roots from the imported block.
			let candidates = fetch_candidates(&*self.client, extrinsics.clone(), &parent_id)
				.map_err(|e| ConsensusError::ChainLookup(e.to_string()))?;

			match candidates {
				Some(candidates) => {
					match our_id {
						Some(our_id) => {
							trace!(
								target: LOG_TARGET,
								"Our validator id is {}, the candidates included are {:?}",
								our_id,
								candidates,
							);

							for candidate in &candidates {
								// If we don't yet have our chunk of this candidate,
								// tell the worker to listen for one.
								if self.availability_store.get_erasure_chunk(
									&relay_parent,
									candidate.block_data_hash,
									our_id as usize,
								).is_none() {
									let msg = WorkerMsg::ListenForChunks(ListenForChunks {
										relay_parent,
										candidate_hash: candidate.hash(),
										index: our_id as u32,
										result: None,
									});

									let _ = self.to_worker.unbounded_send(msg);
								}
							}

							let erasure_roots: Vec<_> = candidates
								.iter()
								.map(|c| c.erasure_root)
								.collect();

							// Inform the worker about new (relay_parent, erasure_roots) pairs
							let (s, _) = oneshot::channel();
							let msg = WorkerMsg::ErasureRoots(ErasureRoots {
								relay_parent,
								erasure_roots,
								result: s,
							});

							let _ = self.to_worker.unbounded_send(msg);

							let (s, _) = oneshot::channel();

							// Inform the worker about the included parachain blocks.
							let msg = WorkerMsg::ParachainBlocks(ParachainBlocks {
								relay_parent,
								blocks: candidates.into_iter().map(|c| (c, None)).collect(),
								result: s,
							});

							let _ = self.to_worker.unbounded_send(msg);
						}
						None => (),
					}
				}
				None => {
					trace!(
						target: LOG_TARGET,
						"No parachain heads were included in block {}", block.header.hash()
					);
				},
			}
		}

		self.inner.import_block(block, new_cache).map_err(Into::into)
	}

	fn check_block(
		&mut self,
		block: BlockCheckParams<Block>,
	) -> Result<ImportResult, Self::Error> {
		self.inner.check_block(block).map_err(Into::into)
	}
}

impl<I, P> AvailabilityBlockImport<I, P> {
	pub(crate) fn new(
		availability_store: Store,
		client: Arc<P>,
		block_import: I,
		thread_pool: TaskExecutor,
		keystore: KeyStorePtr,
		to_worker: mpsc::UnboundedSender<WorkerMsg>,
	) -> Self
	where
		P: ProvideRuntimeApi + BlockBody<Block> + BlockchainEvents<Block> + Send + Sync + 'static,
		P::Api: ParachainHost<Block>,
		P::Api: ApiExt<Block, Error = sp_blockchain::Error>,
	{
		let (signal, exit) = exit_future::signal();

		// This is not the right place to spawn the finality future,
		// it would be more appropriate to spawn it in the `start` method of the `Worker`.
		// However, this would make the type of the `Worker` and the `Store` itself
		// dependent on the types of client and executor, which would prove
		// not not so handy in the testing code.
		let mut exit_signal = Some(signal);
		let prune_available = select(
			prune_unneeded_availability(client.clone(), to_worker.clone()).boxed(),
			exit.clone()
		).map(drop);

		if let Err(_) = thread_pool.spawn(Box::new(prune_available)) {
			error!(target: LOG_TARGET, "Failed to spawn availability pruning task");
			exit_signal = None;
		}

		AvailabilityBlockImport {
			availability_store,
			client,
			inner: block_import,
			to_worker,
			keystore,
			exit_signal,
		}
	}

	fn our_id(&self, validators: &[ValidatorId]) -> Option<u32> {
		let keystore = self.keystore.read();
		validators
			.iter()
			.enumerate()
			.find_map(|(i, v)| {
				keystore.key_pair::<ValidatorPair>(&v).map(|_| i as u32).ok()
			})
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use std::time::Duration;
	use futures::{stream, channel::mpsc, Stream};
	use std::sync::{Arc, Mutex, Condvar};
	use std::pin::Pin;
	use tokio::runtime::Runtime;

	// Just contains topic->channel mapping to give to outer code on `gossip_messages_for` calls.
	struct TestGossipMessages {
		messages: Arc<Mutex<HashMap<
			Hash,
			(
				Arc<(Mutex<bool>, Condvar)>,
				mpsc::UnboundedReceiver<(Hash, Hash, ErasureChunk)>,
			),
		>>>,
	}

	impl ProvideGossipMessages for TestGossipMessages {
		fn gossip_messages_for(&self, topic: Hash)
			-> Pin<Box<dyn Stream<Item = (Hash, Hash, ErasureChunk)> + Send>>
		{
			match self.messages.lock().unwrap().remove(&topic) {
				Some((pair, receiver)) => {
					let (lock, cvar) = &*pair;
					let mut consumed = lock.lock().unwrap();
					*consumed = true;
					cvar.notify_one();
					receiver.boxed()
				},
				None => stream::iter(vec![]).boxed(),
			}
		}

		fn gossip_erasure_chunk(
			&self,
			_relay_parent: Hash,
			_candidate_hash: Hash,
			_erasure_root: Hash,
			_chunk: ErasureChunk
		) {}
	}

	impl Clone for TestGossipMessages {
		fn clone(&self) -> Self {
			TestGossipMessages {
				messages: self.messages.clone(),
			}
		}
	}

	// This test tests that as soon as the worker receives info about new parachain blocks
	// included it registers gossip listeners for it's own chunks. Upon receiving the awaited
	// chunk messages the corresponding listeners are deregistered and these chunks are removed
	// from the awaited chunks set.
	#[test]
	fn receiving_gossip_chunk_removes_from_frontier() {
		let mut runtime = Runtime::new().unwrap();
		let relay_parent = [1; 32].into();
		let erasure_root = [2; 32].into();
		let local_id = 2;
		let n_validators = 4;

		let store = Store::new_in_memory();

		// Tell the store our validator's position and the number of validators at given point.
		store.add_validator_index_and_n_validators(&relay_parent, local_id, n_validators).unwrap();

		let (gossip_sender, gossip_receiver) = mpsc::unbounded();

		let topic = erasure_coding_topic(relay_parent, erasure_root, local_id);

		let pair = Arc::new((Mutex::new(false), Condvar::new()));
		let messages = TestGossipMessages {
			messages: Arc::new(Mutex::new(vec![
						  (topic, (pair.clone(), gossip_receiver))
			].into_iter().collect()))
		};

		let mut candidate = CandidateReceipt::default();

		candidate.erasure_root = erasure_root;
		let candidate_hash = candidate.hash();

		// At this point we shouldn't be waiting for any chunks.
		assert!(store.awaited_chunks().is_none());

		let (s, r) = oneshot::channel();

		let msg = WorkerMsg::ParachainBlocks(ParachainBlocks {
			relay_parent,
			blocks: vec![(candidate, None)],
			result: s,
		});

		let handle = Worker::start(store.clone(), messages);

		// Tell the worker that the new blocks have been included into the relay chain.
		// This should trigger the registration of gossip message listeners for the
		// chunk topics.
		handle.sender.unbounded_send(msg).unwrap();

		runtime.block_on(r).unwrap().unwrap();

		// Make sure that at this point we are waiting for the appropriate chunk.
		assert_eq!(
			store.awaited_chunks().unwrap(),
			vec![(relay_parent, erasure_root, candidate_hash, local_id)].into_iter().collect()
		);

		let msg = (
			relay_parent,
			candidate_hash,
			ErasureChunk {
				chunk: vec![1, 2, 3],
				index: local_id as u32,
				proof: vec![],
			}
		);

		// Send a gossip message with an awaited chunk
		gossip_sender.unbounded_send(msg).unwrap();

		// At the point the needed piece is received, the gossip listener for
		// this topic is deregistered and it's receiver side is dropped.
		// Wait for the sender side to become closed.
		while !gossip_sender.is_closed() {
			// Probably we can just .wait this somehow?
			thread::sleep(Duration::from_millis(100));
		}

		// The awaited chunk has been received so at this point we no longer wait for any chunks.
		assert_eq!(store.awaited_chunks().unwrap().len(), 0);
	}

	#[test]
	fn listen_for_chunk_registers_listener() {
		let mut runtime = Runtime::new().unwrap();
		let relay_parent = [1; 32].into();
		let erasure_root_1 = [2; 32].into();
		let erasure_root_2 = [3; 32].into();
		let block_data_hash_1 = [4; 32].into();
		let block_data_hash_2 = [5; 32].into();
		let local_id = 2;
		let n_validators = 4;

		let mut candidate_1 = CandidateReceipt::default();
		candidate_1.erasure_root = erasure_root_1;
		candidate_1.block_data_hash = block_data_hash_1;
		let candidate_1_hash = candidate_1.hash();

		let mut candidate_2 = CandidateReceipt::default();
		candidate_2.erasure_root = erasure_root_2;
		candidate_2.block_data_hash = block_data_hash_2;
		let candidate_2_hash = candidate_2.hash();

		let store = Store::new_in_memory();

		// Tell the store our validator's position and the number of validators at given point.
		store.add_validator_index_and_n_validators(&relay_parent, local_id, n_validators).unwrap();

		// Let the store know about the candidates
		store.add_candidate(&candidate_1).unwrap();
		store.add_candidate(&candidate_2).unwrap();

		// And let the store know about the chunk from the second candidate.
		store.add_erasure_chunks(
			n_validators,
			&relay_parent,
			&candidate_2_hash,
			vec![ErasureChunk {
				chunk: vec![1, 2, 3],
				index: local_id,
				proof: Vec::default(),
			}],
		).unwrap();

		let (_, gossip_receiver_1) = mpsc::unbounded();
		let (_, gossip_receiver_2) = mpsc::unbounded();

		let topic_1 = erasure_coding_topic(relay_parent, erasure_root_1, local_id);
		let topic_2 = erasure_coding_topic(relay_parent, erasure_root_2, local_id);

		let cvar_pair1 = Arc::new((Mutex::new(false), Condvar::new()));
		let cvar_pair2 = Arc::new((Mutex::new(false), Condvar::new()));

		let messages = TestGossipMessages {
			messages: Arc::new(Mutex::new(
			vec![
				(topic_1, (cvar_pair1.clone(), gossip_receiver_1)),
				(topic_2, (cvar_pair2, gossip_receiver_2)),
			].into_iter().collect()))
		};

		let handle = Worker::start(store.clone(), messages.clone());

		let (s2, r2) = oneshot::channel();
		// Tell the worker to listen for chunks from candidate 2 (we alredy have a chunk from it).
		let listen_msg_2 = WorkerMsg::ListenForChunks(ListenForChunks {
			relay_parent,
			candidate_hash: candidate_2_hash,
			index: local_id as u32,
			result: Some(s2),
		});

		handle.sender.unbounded_send(listen_msg_2).unwrap();

		runtime.block_on(r2).unwrap().unwrap();
		// The gossip sender for this topic left intact => listener not registered.
		assert!(messages.messages.lock().unwrap().contains_key(&topic_2));

		let (s1, r1) = oneshot::channel();

		// Tell the worker to listen for chunks from candidate 1.
		// (we don't have a chunk from it yet).
		let listen_msg_1 = WorkerMsg::ListenForChunks(ListenForChunks {
			relay_parent,
			candidate_hash: candidate_1_hash,
			index: local_id as u32,
			result: Some(s1),
		});

		handle.sender.unbounded_send(listen_msg_1).unwrap();
		runtime.block_on(r1).unwrap().unwrap();

		// Here, we are racing against the worker thread that might have not yet
		// reached the point when it requests the gossip messages for `topic_2`
		// which will get them removed from `TestGossipMessages`. Therefore, the
		// `Condvar` is used to wait for that event.
		let (lock, cvar1) = &*cvar_pair1;
		let mut gossip_stream_consumed = lock.lock().unwrap();
		while !*gossip_stream_consumed {
			gossip_stream_consumed = cvar1.wait(gossip_stream_consumed).unwrap();
		}

		// The gossip sender taken => listener registered.
		assert!(!messages.messages.lock().unwrap().contains_key(&topic_1));
	}
}
