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

use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::Arc;
use std::thread;

use log::{error, info, trace, warn};
use sp_blockchain::{Result as ClientResult};
use sp_runtime::traits::{Header as HeaderT, Block as BlockT, HashFor, BlakeTwo256};
use sp_api::{ApiExt, ProvideRuntimeApi};
use client::{
	BlockchainEvents, BlockBackend,
	blockchain::ProvideCache,
};
use consensus_common::{
	self, BlockImport, BlockCheckParams, BlockImportParams, Error as ConsensusError,
	ImportResult,
	import_queue::CacheKeyId,
};
use polkadot_primitives::{Block, BlockId, Hash};
use polkadot_primitives::parachain::{
	ParachainHost, ValidatorId, AbridgedCandidateReceipt, AvailableData,
	ValidatorPair, ErasureChunk,
};
use futures::{prelude::*, future::select, channel::{mpsc, oneshot}, task::{Spawn, SpawnExt}};
use futures::future::AbortHandle;
use keystore::KeyStorePtr;

use tokio::runtime::{Handle, Runtime as LocalRuntime};

use crate::{LOG_TARGET, ErasureNetworking};
use crate::store::Store;

/// Errors that may occur.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub(crate) enum Error {
	#[from]
	StoreError(io::Error),
	#[display(fmt = "Validator's id and number of validators at block with parent {} not found", relay_parent)]
	IdAndNValidatorsNotFound { relay_parent: Hash },
}

/// Used in testing to interact with the worker thread.
#[cfg(test)]
pub(crate) struct WithWorker(Box<dyn FnOnce(&mut Worker) + Send>);

#[cfg(test)]
impl std::fmt::Debug for WithWorker {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "<boxed closure>")
	}
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
	IncludedParachainBlocks(IncludedParachainBlocks),
	Chunks(Chunks),
	CandidatesFinalized(CandidatesFinalized),
	MakeAvailable(MakeAvailable),
	#[cfg(test)]
	WithWorker(WithWorker),
}

/// A notification of a parachain block included in the relay chain.
#[derive(Debug)]
pub(crate) struct IncludedParachainBlock {
	/// The abridged candidate receipt, extracted from a relay-chain block.
	pub candidate: AbridgedCandidateReceipt,
	/// The data to keep available from the candidate, if known.
	pub available_data: Option<AvailableData>,
}

/// The receipts of the heads included into the block with a given parent.
#[derive(Debug)]
pub(crate) struct IncludedParachainBlocks {
	/// The blocks themselves.
	pub blocks: Vec<IncludedParachainBlock>,
	/// A sender to signal the result asynchronously.
	pub result: oneshot::Sender<Result<(), Error>>,
}

/// We have received chunks we requested.
#[derive(Debug)]
pub(crate) struct Chunks {
	/// The hash of the parachain candidate these chunks belong to.
	pub candidate_hash: Hash,
	/// The chunks
	pub chunks: Vec<ErasureChunk>,
	/// The number of validators present at the candidate's relay-parent.
	pub n_validators: u32,
	/// A sender to signal the result asynchronously.
	pub result: oneshot::Sender<Result<(), Error>>,
}

/// These candidates have been finalized, so unneded availability may be now pruned
#[derive(Debug)]
pub(crate) struct CandidatesFinalized {
	/// The relay parent of the block that was finalized.
	relay_parent: Hash,
	/// The hashes of candidates that were finalized in this block.
	included_candidates: HashSet<Hash>,
}

/// The message that corresponds to `make_available` call of the crate API.
#[derive(Debug)]
pub(crate) struct MakeAvailable {
	/// The hash of the candidate for which we are publishing data.
	pub candidate_hash: Hash,
	/// The data to make available.
	pub available_data: AvailableData,
	/// A sender to signal the result asynchronously.
	pub result: oneshot::Sender<Result<(), Error>>,
}

/// Description of a chunk we are listening for.
#[derive(Hash, Debug, PartialEq, Eq)]
struct ListeningKey {
	candidate_hash: Hash,
	index: u32,
}

/// An availability worker with it's inner state.
pub(super) struct Worker {
	availability_store: Store,
	listening_for: HashMap<ListeningKey, AbortHandle>,

	sender: mpsc::UnboundedSender<WorkerMsg>,
}

/// The handle to the `Worker`.
pub(super) struct WorkerHandle {
	thread: Option<thread::JoinHandle<io::Result<()>>>,
	sender: mpsc::UnboundedSender<WorkerMsg>,
	exit_signal: Option<exit_future::Signal>,
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


fn fetch_candidates<P>(client: &P, extrinsics: Vec<<Block as BlockT>::Extrinsic>, parent: &BlockId)
	-> ClientResult<Option<Vec<AbridgedCandidateReceipt>>>
where
	P: ProvideRuntimeApi<Block>,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HashFor<Block>>,
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
	P: ProvideRuntimeApi<Block> + BlockchainEvents<Block> + BlockBackend<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + ApiExt<Block, Error=sp_blockchain::Error>,
	S: Sink<WorkerMsg> + Clone + Send + Sync + Unpin,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HashFor<Block>>,
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

		let included_candidates = match fetch_candidates(
			&*client,
			extrinsics,
			&BlockId::hash(parent_hash),
		) {
			Ok(Some(candidates)) => candidates
				.into_iter()
				.map(|c| c.hash())
				.collect(),
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
			included_candidates
		});

		if let Err(_) = sender.send(msg).await {
			break;
		}
	}
}

impl Worker {

	// Called on startup of the worker to initiate fetch from network for all awaited chunks.
	fn initiate_all_fetches<EN: ErasureNetworking>(
		&mut self,
		runtime_handle: &Handle,
		erasure_network: &EN,
		sender: &mut mpsc::UnboundedSender<WorkerMsg>,
	) {
		if let Some(awaited_chunks) = self.availability_store.awaited_chunks() {
			for awaited_chunk in awaited_chunks {
				if let Err(e) = self.initiate_fetch(
					runtime_handle,
					erasure_network,
					sender,
					awaited_chunk.relay_parent,
					awaited_chunk.candidate_hash,
				) {
					warn!(target: LOG_TARGET, "Failed to register network listener: {}", e);
				}
			}
		}
	}

	// initiates a fetch from network for the described chunk, with our local index.
	fn initiate_fetch<EN: ErasureNetworking>(
		&mut self,
		runtime_handle: &Handle,
		erasure_network: &EN,
		sender: &mut mpsc::UnboundedSender<WorkerMsg>,
		relay_parent: Hash,
		candidate_hash: Hash,
	) -> Result<(), Error> {
		let (local_id, n_validators) = self.availability_store
			.get_validator_index_and_n_validators(&relay_parent)
			.ok_or(Error::IdAndNValidatorsNotFound { relay_parent })?;

		// fast exit for if we already have the chunk.
		if self.availability_store.get_erasure_chunk(&candidate_hash, local_id as _).is_some() {
			return Ok(())
		}

		trace!(
			target: LOG_TARGET,
			"Initiating fetch for erasure-chunk at parent {} with candidate-hash {}",
			relay_parent,
			candidate_hash,
		);

		let fut = erasure_network.fetch_erasure_chunk(&candidate_hash, local_id);
		let mut sender = sender.clone();
		let (fut, signal) = future::abortable(async move {
			let chunk = match fut.await {
				Ok(chunk) => chunk,
				Err(e) => {
					warn!(target: LOG_TARGET, "Unable to fetch erasure-chunk from network: {:?}", e);
					return
				}
			};
			let (s, _) = oneshot::channel();
			let _ = sender.send(WorkerMsg::Chunks(Chunks {
				candidate_hash,
				chunks: vec![chunk],
				n_validators,
				result: s,
			})).await;
		}.map(drop).boxed());


		let key = ListeningKey {
			candidate_hash,
			index: local_id,
		};

		self.listening_for.insert(key, signal);
		let _ = runtime_handle.spawn(fut);

		Ok(())
	}

	fn on_parachain_blocks_received<EN: ErasureNetworking>(
		&mut self,
		runtime_handle: &Handle,
		erasure_network: &EN,
		sender: &mut mpsc::UnboundedSender<WorkerMsg>,
		blocks: Vec<IncludedParachainBlock>,
	) -> Result<(), Error> {
		// First we have to add the receipts themselves.
		for IncludedParachainBlock { candidate, available_data }
			in blocks.into_iter()
		{
			let _ = self.availability_store.add_candidate(&candidate);

			if let Some(_available_data) = available_data {
				// Should we be breaking block into chunks here and gossiping it and so on?
			}

			// This leans on the codebase-wide assumption that the `relay_parent`
			// of all candidates in a block matches the parent hash of that block.
			//
			// In the future this will not always be true.
			let candidate_hash = candidate.hash();
			let _ = self.availability_store.note_candidates_with_relay_parent(
				&candidate.relay_parent,
				&[candidate_hash],
			);

			if let Err(e) = self.initiate_fetch(
				runtime_handle,
				erasure_network,
				sender,
				candidate.relay_parent,
				candidate_hash,
			) {
				warn!(target: LOG_TARGET, "Failed to register chunk listener: {}", e);
			}
		}

		Ok(())
	}

	// Handles chunks that were required.
	fn on_chunks(
		&mut self,
		candidate_hash: Hash,
		chunks: Vec<ErasureChunk>,
		n_validators: u32,
	) -> Result<(), Error> {
		for c in &chunks {
			let key = ListeningKey {
				candidate_hash,
				index: c.index,
			};

			// remove bookkeeping so network does not attempt to fetch
			// any longer.
			if let Some(exit_signal) = self.listening_for.remove(&key) {
				exit_signal.abort();
			}
		}

		self.availability_store.add_erasure_chunks(
			n_validators,
			&candidate_hash,
			chunks,
		)?;

		Ok(())
	}

	/// Starts a worker with a given availability store and a gossip messages provider.
	pub fn start<EN: ErasureNetworking + Send + 'static>(
		availability_store: Store,
		erasure_network: EN,
	) -> WorkerHandle {
		let (sender, mut receiver) = mpsc::unbounded();

		let mut worker = Worker {
			availability_store,
			listening_for: HashMap::new(),
			sender: sender.clone(),
		};

		let sender = sender.clone();
		let (signal, exit) = exit_future::signal();

		let handle = thread::spawn(move || -> io::Result<()> {
			let mut runtime = LocalRuntime::new()?;
			let mut sender = worker.sender.clone();

			let runtime_handle = runtime.handle().clone();

			// On startup, initiates fetch from network for all
			// entries in the awaited frontier.
			worker.initiate_all_fetches(runtime.handle(), &erasure_network, &mut sender);

			let process_notification = async move {
				while let Some(msg) = receiver.next().await {
					trace!(target: LOG_TARGET, "Received message {:?}", msg);

					let res = match msg {
						WorkerMsg::IncludedParachainBlocks(msg) => {
							let IncludedParachainBlocks {
								blocks,
								result,
							} = msg;

							let res = worker.on_parachain_blocks_received(
								&runtime_handle,
								&erasure_network,
								&mut sender,
								blocks,
							);

							let _ = result.send(res);
							Ok(())
						}
						WorkerMsg::Chunks(msg) => {
							let Chunks {
								candidate_hash,
								chunks,
								n_validators,
								result,
							} = msg;

							let res = worker.on_chunks(
								candidate_hash,
								chunks,
								n_validators,
							);

							let _ = result.send(res);
							Ok(())
						}
						WorkerMsg::CandidatesFinalized(msg) => {
							let CandidatesFinalized { relay_parent, included_candidates } = msg;

							worker.availability_store.candidates_finalized(
								relay_parent,
								included_candidates,
							)
						}
						WorkerMsg::MakeAvailable(msg) => {
							let MakeAvailable { candidate_hash, available_data, result } = msg;
							let res = worker.availability_store
								.make_available(candidate_hash, available_data)
								.map_err(|e| e.into());
							let _ = result.send(res);
							Ok(())
						}
						#[cfg(test)]
						WorkerMsg::WithWorker(with_worker) => {
							(with_worker.0)(&mut worker);
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
	inner: I,
	client: Arc<P>,
	keystore: KeyStorePtr,
	to_worker: mpsc::UnboundedSender<WorkerMsg>,
	exit_signal: AbortHandle,
}

impl<I, P> Drop for AvailabilityBlockImport<I, P> {
	fn drop(&mut self) {
		self.exit_signal.abort();
	}
}

impl<I, P> BlockImport<Block> for AvailabilityBlockImport<I, P> where
	I: BlockImport<Block, Transaction = sp_api::TransactionFor<P, Block>> + Send + Sync,
	I::Error: Into<ConsensusError>,
	P: ProvideRuntimeApi<Block> + ProvideCache<Block>,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<BlakeTwo256>
{
	type Error = ConsensusError;
	type Transaction = sp_api::TransactionFor<P, Block>;

	fn import_block(
		&mut self,
		block: BlockImportParams<Block, Self::Transaction>,
		new_cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		trace!(
			target: LOG_TARGET,
			"Importing block #{}, ({})",
			block.header.number(),
			block.post_hash(),
		);

		if let Some(ref extrinsics) = block.body {
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

							let (s, _) = oneshot::channel();

							// Inform the worker about the included parachain blocks.
							let blocks = candidates
								.into_iter()
								.map(|c| IncludedParachainBlock {
									candidate: c,
									available_data: None,
								})
								.collect();

							let msg = WorkerMsg::IncludedParachainBlocks(IncludedParachainBlocks {
								blocks,
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
		client: Arc<P>,
		block_import: I,
		spawner: impl Spawn,
		keystore: KeyStorePtr,
		to_worker: mpsc::UnboundedSender<WorkerMsg>,
	) -> Self
	where
		P: ProvideRuntimeApi<Block> + BlockBackend<Block> + BlockchainEvents<Block> + Send + Sync + 'static,
		P::Api: ParachainHost<Block>,
		P::Api: ApiExt<Block, Error = sp_blockchain::Error>,
		// Rust bug: https://github.com/rust-lang/rust/issues/24159
		sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HashFor<Block>>,
	{
		// This is not the right place to spawn the finality future,
		// it would be more appropriate to spawn it in the `start` method of the `Worker`.
		// However, this would make the type of the `Worker` and the `Store` itself
		// dependent on the types of client and executor, which would prove
		// not not so handy in the testing code.
		let (prune_available, exit_signal) = future::abortable(prune_unneeded_availability(
			client.clone(),
			to_worker.clone(),
		));

		if let Err(_) = spawner.spawn(prune_available.map(drop)) {
			error!(target: LOG_TARGET, "Failed to spawn availability pruning task");
		}

		AvailabilityBlockImport {
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
	use futures::channel::oneshot;
	use std::sync::Arc;
	use std::pin::Pin;
	use tokio::runtime::Runtime;
	use parking_lot::Mutex;
	use crate::store::AwaitedFrontierEntry;

	#[derive(Default, Clone)]
	struct TestErasureNetwork {
		chunk_receivers: Arc<Mutex<HashMap<
			(Hash, u32),
			oneshot::Receiver<ErasureChunk>
		>>>,
	}

	impl TestErasureNetwork {
		// adds a receiver. this returns a sender for the erasure-chunk
		// along with an exit future that fires when the erasure chunk has
		// been fully-processed
		fn add_receiver(&self, candidate_hash: Hash, index: u32)
			-> oneshot::Sender<ErasureChunk>
		{
			let (sender, receiver) = oneshot::channel();
			self.chunk_receivers.lock().insert((candidate_hash, index), receiver);
			sender
		}
	}

	impl ErasureNetworking for TestErasureNetwork {
		type Error = String;

		fn fetch_erasure_chunk(&self, candidate_hash: &Hash, index: u32)
			-> Pin<Box<dyn Future<Output = Result<ErasureChunk, Self::Error>> + Send>>
		{
			match self.chunk_receivers.lock().remove(&(*candidate_hash, index)) {
				Some(receiver) => receiver.then(|x| match x {
					Ok(x) => future::ready(Ok(x)).left_future(),
					Err(_) => future::pending().right_future(),
				}).boxed(),
				None => future::pending().boxed(),
			}
		}

		fn distribute_erasure_chunk(
			&self,
			_candidate_hash: Hash,
			_chunk: ErasureChunk
		) {}
	}

	// This test tests that as soon as the worker receives info about new parachain blocks
	// included it registers gossip listeners for it's own chunks. Upon receiving the awaited
	// chunk messages the corresponding listeners are deregistered and these chunks are removed
	// from the awaited chunks set.
	#[test]
	fn receiving_gossip_chunk_removes_from_frontier() {
		let mut runtime = Runtime::new().unwrap();
		let relay_parent = [1; 32].into();
		let local_id = 2;
		let n_validators = 4;

		let store = Store::new_in_memory();

		let mut candidate = AbridgedCandidateReceipt::default();

		candidate.relay_parent = relay_parent;
		let candidate_hash = candidate.hash();

		// Tell the store our validator's position and the number of validators at given point.
		store.note_validator_index_and_n_validators(&relay_parent, local_id, n_validators).unwrap();

		let network = TestErasureNetwork::default();
		let chunk_sender = network.add_receiver(candidate_hash, local_id);

		// At this point we shouldn't be waiting for any chunks.
		assert!(store.awaited_chunks().is_none());

		let (s, r) = oneshot::channel();

		let msg = WorkerMsg::IncludedParachainBlocks(IncludedParachainBlocks {
			blocks: vec![IncludedParachainBlock {
				candidate,
				available_data: None,
			}],
			result: s,
		});

		let handle = Worker::start(store.clone(), network);

		// Tell the worker that the new blocks have been included into the relay chain.
		// This should trigger the registration of gossip message listeners for the
		// chunk topics.
		handle.sender.unbounded_send(msg).unwrap();

		runtime.block_on(r).unwrap().unwrap();

		// Make sure that at this point we are waiting for the appropriate chunk.
		assert_eq!(
			store.awaited_chunks().unwrap(),
			vec![AwaitedFrontierEntry {
				relay_parent,
				candidate_hash,
				validator_index: local_id,
			}].into_iter().collect()
		);

		// Complete the chunk request.
		chunk_sender.send(ErasureChunk {
			chunk: vec![1, 2, 3],
			index: local_id as u32,
			proof: vec![],
		}).unwrap();

		// wait until worker thread has de-registered the listener for a
		// particular chunk.
		loop {
			let (s, r) = oneshot::channel();
			handle.sender.unbounded_send(WorkerMsg::WithWorker(WithWorker(Box::new(move |worker| {
				let key = ListeningKey {
					candidate_hash,
					index: local_id,
				};

				let is_waiting = worker.listening_for.contains_key(&key);

				s.send(!is_waiting).unwrap(); // tell the test thread `true` if we are not waiting.
			})))).unwrap();

			if runtime.block_on(r).unwrap() {
				break
			}
		}

		// The awaited chunk has been received so at this point we no longer wait for any chunks.
		assert_eq!(store.awaited_chunks().unwrap().len(), 0);
	}

	#[test]
	fn included_parachain_blocks_registers_listener() {
		let mut runtime = Runtime::new().unwrap();
		let relay_parent = [1; 32].into();
		let erasure_root_1 = [2; 32].into();
		let erasure_root_2 = [3; 32].into();
		let pov_block_hash_1 = [4; 32].into();
		let pov_block_hash_2 = [5; 32].into();
		let local_id = 2;
		let n_validators = 4;

		let mut candidate_1 = AbridgedCandidateReceipt::default();
		candidate_1.commitments.erasure_root = erasure_root_1;
		candidate_1.pov_block_hash = pov_block_hash_1;
		candidate_1.relay_parent = relay_parent;
		let candidate_1_hash = candidate_1.hash();

		let mut candidate_2 = AbridgedCandidateReceipt::default();
		candidate_2.commitments.erasure_root = erasure_root_2;
		candidate_2.pov_block_hash = pov_block_hash_2;
		candidate_2.relay_parent = relay_parent;
		let candidate_2_hash = candidate_2.hash();

		let store = Store::new_in_memory();

		// Tell the store our validator's position and the number of validators at given point.
		store.note_validator_index_and_n_validators(&relay_parent, local_id, n_validators).unwrap();

		// Let the store know about the candidates
		store.add_candidate(&candidate_1).unwrap();
		store.add_candidate(&candidate_2).unwrap();

		// And let the store know about the chunk from the second candidate.
		store.add_erasure_chunks(
			n_validators,
			&candidate_2_hash,
			vec![ErasureChunk {
				chunk: vec![1, 2, 3],
				index: local_id,
				proof: Vec::default(),
			}],
		).unwrap();

		let network = TestErasureNetwork::default();
		let _ = network.add_receiver(candidate_1_hash, local_id);
		let _ = network.add_receiver(candidate_2_hash, local_id);

		let handle = Worker::start(store.clone(), network.clone());

		{
			let (s, r) = oneshot::channel();
			// Tell the worker to listen for chunks from candidate 2 (we alredy have a chunk from it).
			let listen_msg_2 = WorkerMsg::IncludedParachainBlocks(IncludedParachainBlocks {
				blocks: vec![IncludedParachainBlock {
					candidate: candidate_2,
					available_data: None,
				}],
				result: s,
			});

			handle.sender.unbounded_send(listen_msg_2).unwrap();

			runtime.block_on(r).unwrap().unwrap();
			// The receiver for this chunk left intact => listener not registered.
			assert!(network.chunk_receivers.lock().contains_key(&(candidate_2_hash, local_id)));

			// more directly:
			let (s, r) = oneshot::channel();
			handle.sender.unbounded_send(WorkerMsg::WithWorker(WithWorker(Box::new(move |worker| {
				let key = ListeningKey {
					candidate_hash: candidate_2_hash,
					index: local_id,
				};
				let _ = s.send(worker.listening_for.contains_key(&key));
			})))).unwrap();

			assert!(!runtime.block_on(r).unwrap());
		}

		{
			let (s, r) = oneshot::channel();

			// Tell the worker to listen for chunks from candidate 1.
			// (we don't have a chunk from it yet).
			let listen_msg_1 = WorkerMsg::IncludedParachainBlocks(IncludedParachainBlocks {
				blocks: vec![IncludedParachainBlock {
					candidate: candidate_1,
					available_data: None,
				}],
				result: s,
			});

			handle.sender.unbounded_send(listen_msg_1).unwrap();
			runtime.block_on(r).unwrap().unwrap();

			// The receiver taken => listener registered.
			assert!(!network.chunk_receivers.lock().contains_key(&(candidate_1_hash, local_id)));


			// more directly:
			let (s, r) = oneshot::channel();
			handle.sender.unbounded_send(WorkerMsg::WithWorker(WithWorker(Box::new(move |worker| {
				let key = ListeningKey {
					candidate_hash: candidate_1_hash,
					index: local_id,
				};
				let _ = s.send(worker.listening_for.contains_key(&key));
			})))).unwrap();

			assert!(runtime.block_on(r).unwrap());
		}
	}
}
