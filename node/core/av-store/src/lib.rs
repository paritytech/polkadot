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

//! Implements a `AvailabilityStoreSubsystem`.
//!

#![recursion_limit="256"]

use std::collections::{HashSet, HashMap};
use std::io;
use std::iter::FromIterator;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};

use codec::{Encode, Decode};
use futures::{select, channel::oneshot, FutureExt};
use kvdb_rocksdb::{Database, DatabaseConfig};
use kvdb::{KeyValueDB, DBTransaction};

use polkadot_primitives::v1::{
	Hash, AvailableData, ErasureChunk, ValidatorIndex, CommittedCandidateReceipt,
};
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, SubsystemError, Subsystem, SubsystemContext, SpawnedSubsystem,
};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityStoreMessage, RuntimeApiMessage, RuntimeApiRequest,
};

const LOG_TARGET: &str = "availability";

const CHUNKS_PRUNING_KEY: [u8; 14] = *b"chunks_pruning";

const POV_PRUNING_KEY: [u8; 11] = *b"pov_pruning";

// Stored block is kept for 1 hour.
const KEEP_STORED_BLOCK_FOR: u64 = 1000;

// Finalized block is kept for 1 day.
const KEEP_FINALIZED_BLOCK_FOR: u64 = 1_000_000;

// Keep chunk for 1 day + 1 hour.
const KEEP_FINALIZED_CHUNK_FOR: u64 = 1_000_001;

mod columns {
	pub const DATA: u32 = 0;
	pub const META: u32 = 1;
	pub const NUM_COLUMNS: u32 = 2;
}

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	Erasure(erasure::Error),
	#[from]
	Io(io::Error),
	#[from]
	Time(SystemTimeError),
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	Subsystem(SubsystemError),
}

#[derive(Decode, Encode, Debug, Clone, Copy)]
enum CandidateState {
	// We commited to storing this data but the candidate it relates to
	// hasn't been included yet. If it remains not included this data has
	// to be pruned in 1 hour.
	Stored,
	// The block was included and is now awaiting to be finalized.
	Included,
	// The block was finalized, from now on the
	//  PoV is kept for 1 day
	//  Chunk is kept for 1 day + 1 hour
	Finalized,
}

fn prune_pov_at(now: u64, state: CandidateState) -> u64 {
	use CandidateState::*;

	match state {
		Stored => now + KEEP_STORED_BLOCK_FOR,
		// Keep until finalized.
		Included => u64::MAX,
		Finalized => now + KEEP_FINALIZED_BLOCK_FOR,
	}
}

fn prune_chunk_at(now: u64, state: CandidateState) -> u64 {
	use CandidateState::*;

	match state {
		// The same for chunks and for blocks.
		Stored | Included => prune_pov_at(now, state),
		// Keep chunk for longer than the block.
		Finalized => now + KEEP_FINALIZED_CHUNK_FOR,
	}
}

#[derive(Encode, Decode, Debug)]
struct PoVPruning {
	candidate_hash: Hash,
	prune_at: u64,
	state: CandidateState,
}

#[derive(Encode, Decode, Debug)]
struct ChunkPruning {
	candidate_hash: Hash,
	prune_at: u64,
	state: CandidateState,
}

pub struct AvailabilityStoreSubsystem<Context, Time=SystemTime> {
	inner: Arc<dyn KeyValueDB>,
	_context: std::marker::PhantomData<Context>,
	_time: std::marker::PhantomData<Time>,
}

fn available_data_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 0i8).encode()
}

fn erasure_chunks_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 1i8).encode()
}

pub trait Time {
	fn now_as_secs() -> Result<u64, SystemTimeError>;
}

impl Time for SystemTime {
	fn now_as_secs() -> Result<u64, SystemTimeError> {
		Ok(
			SystemTime::now()
			.duration_since(UNIX_EPOCH)?
			.as_secs()
		)
	}
}

impl<Context, T> AvailabilityStoreSubsystem<Context, T>
	where
		Context: SubsystemContext<Message=AvailabilityStoreMessage>,
		T: Time,
{
	pub fn new(config: Config) -> io::Result<Self> {
		let mut db_config = DatabaseConfig::with_columns(columns::NUM_COLUMNS);

		if let Some(cache_size) = config.cache_size {
			let mut memory_budget = HashMap::new();

			for i in 0..columns::NUM_COLUMNS {
				memory_budget.insert(i, cache_size / columns::NUM_COLUMNS as usize);
			}
			db_config.memory_budget = memory_budget;
		}

		let path = config.path.to_str().ok_or_else(|| io::Error::new(
			io::ErrorKind::Other,
			format!("Bad database path: {:?}", config.path),
		))?;

		let db = Database::open(&db_config, &path)?;

		Ok(Self {
			inner: Arc::new(db),
			_context: std::marker::PhantomData,
			_time: std::marker::PhantomData,
		})
	}

	#[cfg(test)]
	fn new_in_memory(inner: Arc<dyn KeyValueDB>) -> Self {
		Self {
			inner,
			_context: std::marker::PhantomData,
			_time: std::marker::PhantomData,
		}
	}

	async fn request_included_candidates(hash: Hash, ctx: &mut Context)
		-> Result<Option<Vec<CommittedCandidateReceipt>>, Error>
	{
		let (tx, rx) = oneshot::channel();

		let msg = AllMessages::RuntimeApi(RuntimeApiMessage::Request(hash,
				RuntimeApiRequest::GetCandidates(hash, tx))
		);

		ctx.send_message(msg.into()).await?;

		Ok(rx.await?)
	}

	async fn run(mut self, mut ctx: Context) -> Result<(), Error> {
		loop {
			select! {
				incoming = ctx.recv().fuse() => {
					match incoming {
						Ok(FromOverseer::Signal(OverseerSignal::StartWork(hash))) => {
							let included = match Self::request_included_candidates(hash, &mut ctx).await? {
								Some(included) => HashSet::from_iter(included.iter().map(|c| c.hash())),
								None => HashSet::new(),
							};

							// for every candidate in relay block
							let _ = self.update_pruning(
								&included,
								CandidateState::Included,
							);
						}
						Ok(FromOverseer::Signal(OverseerSignal::BlockFinalized(hash))) => {
							let included = match Self::request_included_candidates(hash, &mut ctx).await? {
								Some(included) => HashSet::from_iter(included.iter().map(|c| c.hash())),
								None => HashSet::new(),
							};

							let _ = self.update_pruning(
								&included,
								CandidateState::Finalized,
							);
						}
						Ok(FromOverseer::Signal(OverseerSignal::StopWork(_))) => (),
						Ok(FromOverseer::Signal(Conclude)) => break,
						Ok(FromOverseer::Communication { msg }) => {
							let _ = self.process_message(msg);
						}
						Err(_) => break,
					}
				}
				complete => break,
			}
		}

		Ok(())
	}

	fn update_pruning(
		&mut self,
		candidates: &HashSet<Hash>,
		state: CandidateState,
	) -> Result<(), Error> {
		let _ = self.update_pruning_pov(candidates, state);
		let _ = self.update_pruning_chunks(candidates, state);

		Ok(())
	}

	fn update_pruning_pov(
		&mut self,
		candidates: &HashSet<Hash>,
		state: CandidateState,
	) -> Result<(), Error> {
		let mut pruning_pov = match self.query_inner::<Vec<PoVPruning>>(
			columns::META,
			&POV_PRUNING_KEY,
		) {
			Some(pruning) => pruning,
			None => return Ok(()),
		};

		let mut tx = DBTransaction::new();

		let now = T::now_as_secs()?;

		let mut prune_these_pov = Vec::new();
		let mut keep_these_pov = Vec::new();

		for mut record in pruning_pov.drain(..) {
			if candidates.contains(&record.candidate_hash) {
				record.prune_at = prune_pov_at(now, state);
				keep_these_pov.push(record);
			} else if record.prune_at > now {
				keep_these_pov.push(record);
			} else {
				prune_these_pov.push(record);
			}
		}

		tx.put_vec(
			columns::META,
			&POV_PRUNING_KEY,
			keep_these_pov.encode(),
		);

		for pruned in prune_these_pov.drain(..) {
			tx.delete(columns::DATA, available_data_key(&pruned.candidate_hash).as_slice());
		}

		self.inner.write(tx)?;

		Ok(())
	}

	fn update_pruning_chunks(
		&mut self,
		candidates: &HashSet<Hash>,
		state: CandidateState,
	) -> Result<(), Error> {
		let mut pruning_chunks = match self.query_inner::<Vec<ChunkPruning>>(
			columns::META,
			&CHUNKS_PRUNING_KEY,
		) {
			Some(pruning) => pruning,
			None => return Ok(()),
		};
		let mut tx = DBTransaction::new();

		let now = T::now_as_secs()?;

		let mut prune_these_chunks = Vec::new();
		let mut keep_these_chunks = Vec::new();

		for mut record in pruning_chunks.drain(..) {
			if candidates.contains(&record.candidate_hash) {
				record.prune_at = prune_chunk_at(now, state);
				keep_these_chunks.push(record);
			} else if record.prune_at > now {
				keep_these_chunks.push(record);
			} else {
				prune_these_chunks.push(record);
			}
		}

		tx.put_vec(
			columns::META,
			&CHUNKS_PRUNING_KEY,
			keep_these_chunks.encode(),
		);

		for pruned in prune_these_chunks.drain(..) {
			tx.delete(columns::DATA, erasure_chunks_key(&pruned.candidate_hash).as_slice());
		}

		self.inner.write(tx)?;

		Ok(())
	}

	fn process_message(&mut self, msg: AvailabilityStoreMessage) -> Result<(), Error> {
		use AvailabilityStoreMessage::*;
		match msg {
			QueryPoV(hash, tx) => {
				let _ = tx.send(self.available_data(&hash));
			}
			QueryChunk(hash, id, tx) => {
				let _ = tx.send(self.get_chunk(&hash, id));
			}
			StoreChunk(hash, id, chunk) => {
				self.store_chunk(hash, id, chunk)?;
			}
			StorePoV(hash, id, n_validators, av_data) => {
				self.store_pov(hash, id, n_validators, av_data)?;
			}
		}

		Ok(())
	}

	fn available_data(&self, candidate_hash: &Hash) -> Option<AvailableData> {
		self.query_inner(columns::DATA, &available_data_key(candidate_hash))
	}

	fn store_pov(
		&self,
		candidate_hash: Hash,
		id: Option<ValidatorIndex>,
		n_validators: u32,
		available_data: AvailableData,
	) -> Result<(), Error> {
		let mut tx = DBTransaction::new();

		let mut pruning = self.query_inner(columns::META, &POV_PRUNING_KEY).unwrap_or(Vec::new());

		if let None = pruning.iter().position(|c: &PoVPruning| c.candidate_hash == candidate_hash) {
			let now = T::now_as_secs()?; 
			let prune_at = prune_pov_at(now, CandidateState::Stored);

			pruning.push(PoVPruning {
				candidate_hash,
				prune_at,
				state: CandidateState::Stored,
			});
		}

		if let Some(index) = id {
			let chunks = erasure::obtain_chunks_v1(n_validators as usize, &available_data)?;
			let mut branches = erasure::branches(chunks.as_ref());
			if let Some(branch) = branches.nth(index as usize) {
				let chunk = ErasureChunk {
					chunk: branch.1.to_vec(),
					index,
					proof: branch.0
				};
				self.store_chunk(candidate_hash, n_validators, chunk)?;
			}
		}

		tx.put_vec(
			columns::DATA,
			available_data_key(&candidate_hash).as_slice(),
			available_data.encode(),
		);

		tx.put_vec(
			columns::META,
			&POV_PRUNING_KEY,
			pruning.encode(),
		);

		self.inner.write(tx)?;

		Ok(())
	}

	fn store_chunk(&self, candidate_hash: Hash, n_validators: u32, chunk: ErasureChunk)
		-> Result<(), Error>
	{
		let mut tx = DBTransaction::new();

		let dbkey = erasure_chunks_key(&candidate_hash);

		let mut pruning = self.query_inner(columns::META, &CHUNKS_PRUNING_KEY).unwrap_or(Vec::new());
		let mut v = self.query_inner(columns::DATA, &dbkey).unwrap_or(Vec::new());

		if let None = pruning.iter().position(|c: &ChunkPruning| c.candidate_hash == candidate_hash) {
			let now = T::now_as_secs()?;
			let prune_at = prune_chunk_at(now, CandidateState::Stored);
			pruning.push(ChunkPruning {
				candidate_hash,
				prune_at,
				state: CandidateState::Stored,
			});
		}

		if let None = v.iter().position(|c: &ErasureChunk| c.index == chunk.index) {
			v.push(chunk);
		}

		if self.available_data(&candidate_hash).is_none() {
			if let Ok(available_data) = erasure::reconstruct_v1(
				n_validators as usize,
				v.iter().map(|chunk| (chunk.chunk.as_ref(), chunk.index as usize)),
			) {
				self.store_pov(candidate_hash, None, n_validators, available_data)?;
			}
		}

		tx.put_vec(columns::DATA, &dbkey, v.encode());
		tx.put_vec(columns::META, &CHUNKS_PRUNING_KEY, pruning.encode());
		self.inner.write(tx)?;

		Ok(())
	}

	fn get_chunk(&self, candidate_hash: &Hash, index: u32) -> Option<ErasureChunk> {
		self.query_inner(columns::DATA, &erasure_chunks_key(candidate_hash))
			.and_then(|chunks: Vec<ErasureChunk>| {
				chunks
					.iter()
					.find(|chunk: &&ErasureChunk| chunk.index == index)
					.map(|chunk| chunk.clone())
			})
	}

	fn query_inner<D: Decode>(&self, column: u32, key: &[u8]) -> Option<D> {
		match self.inner.get(column, key) {
			Ok(Some(raw)) => {
				let res = D::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed");
				Some(res)
			}
			Ok(None) => None,
			Err(e) => {
				log::warn!(target: LOG_TARGET, "Error reading from the availability store: {:?}", e);
				None
			}
		}
	}
}

/// Configuration for the availability store.
pub struct Config {
	/// Cache size in bytes. If `None` default is used.
	pub cache_size: Option<usize>,
	/// Path to the database.
	pub path: PathBuf,
}

impl<Context, T> Subsystem<Context> for AvailabilityStoreSubsystem<Context, T>
	where
		Context: SubsystemContext<Message=AvailabilityStoreMessage>,
		T: Time + Send + 'static
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem(Box::pin(async move {
			if let Err(e) = self.run(ctx).await {
				log::error!(target: "availabilitystore", "Subsystem exited with an error {:?}", e);
			}
		}))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::{
		future,
		channel::oneshot,
		executor::{self, ThreadPool},
		Future,
	};
	use std::cell::RefCell;
	use polkadot_primitives::v1::{
		AvailableData, BlockData, HeadData, GlobalValidationSchedule, LocalValidationData, PoV,
		OmittedValidationData, CandidateDescriptor, CandidateCommitments, Id as ParaId,
		ValidationCode,
	};
	use assert_matches::assert_matches;

	struct TestHarness {
		virtual_overseer: subsystem_test::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	}

	thread_local! {
		static TIME_NOW: RefCell<Option<u64>> = RefCell::new(None);
	}

	struct TestState {
		global_validation_schedule: GlobalValidationSchedule,
		local_validation_data: LocalValidationData,
		relay_parent: Hash,
	}

	impl Default for TestState {
		fn default() -> Self {

			let local_validation_data = LocalValidationData {
				parent_head: HeadData(vec![7, 8, 9]),
				balance: Default::default(),
				code_upgrade_allowed: None,
				validation_code_hash: Default::default(),
			};

			let global_validation_schedule = GlobalValidationSchedule {
				max_code_size: 1000,
				max_head_data_size: 1000,
				block_number: Default::default(),
			};

			let relay_parent = Hash::from([1; 32]);

			Self {
				relay_parent,
				local_validation_data,
				global_validation_schedule,
			}
		}
	}

	struct TestTime;

	impl TestTime {
		fn set(time: u64) {
			TIME_NOW.with(|cell| *cell.borrow_mut() = Some(time))
		}

		fn get() -> Result<u64, SystemTimeError> {
			Ok(TIME_NOW.with(|cell| cell
					.borrow()
					.as_ref()
					.cloned()
					.unwrap_or(0)
				)
			)
		}

		fn add(t: u64) {
			TIME_NOW.with(|cell| {
				if let Some(time) = cell.borrow_mut().as_mut() {
					*time += t;
				}
			});
		}
	}

	impl TestCandidateBuilder {
		fn build(self) -> CommittedCandidateReceipt {
			CommittedCandidateReceipt {
				descriptor: CandidateDescriptor {
					para_id: self.para_id,
					pov_hash: self.pov_hash,
					relay_parent: self.relay_parent,
					..Default::default()
				},
				commitments: CandidateCommitments {
					head_data: self.head_data,
					new_validation_code: self.new_validation_code,
					..Default::default()
				},
			}
		}
	}

	impl Time for TestTime {
		fn now_as_secs() -> Result<u64, SystemTimeError> {
			Self::get()
		}
	}

	#[derive(Default)]
	struct TestCandidateBuilder {
		para_id: ParaId,
		head_data: HeadData,
		pov_hash: Hash,
		relay_parent: Hash,
		new_validation_code: Option<ValidationCode>,
	}

	fn test_harness<T: Future<Output=()>>(
		store: Arc<dyn KeyValueDB>,
		test: impl FnOnce(TestHarness) -> T,
	) {
		let pool = ThreadPool::new().unwrap();

		let (context, virtual_overseer) = subsystem_test::make_subsystem_context(pool.clone());
		TestTime::set(0);//SystemTime::now_as_secs().unwrap());

		// TODO: why the type?
		let subsystem: AvailabilityStoreSubsystem<_, TestTime> =
			AvailabilityStoreSubsystem::new_in_memory(store);
		let subsystem = subsystem.run(context);

		let test_fut = test(TestHarness {
			virtual_overseer,
		});

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	#[test]
	fn store_chunk_works() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		test_harness(store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let relay_parent = Hash::from([1; 32]);
			let validator_index = 5;

			let chunk = ErasureChunk {
				chunk: vec![1, 2, 3],
				index: validator_index,
				proof: vec![vec![3, 4, 5]],
			};

			let chunk_msg = AvailabilityStoreMessage::StoreChunk(
				relay_parent,
				validator_index, 
				chunk.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: chunk_msg }).await;

			let (tx, rx) = oneshot::channel();
			let query_chunk = AvailabilityStoreMessage::QueryChunk(
				relay_parent,
				validator_index,
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: query_chunk }).await;

			assert_eq!(rx.await.unwrap().unwrap(), chunk);
		});
	}

	#[test]
	fn store_block_works() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		let test_state = TestState::default();
		test_harness(store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let candidate_hash = Hash::from([1; 32]);
			let validator_index = 5;
			let n_validators = 10;

			let pov = PoV {
				block_data: BlockData(vec![4, 5, 6]),
			};

			let global_validation = test_state.global_validation_schedule;
			let local_validation = test_state.local_validation_data;

			let omitted_validation = OmittedValidationData {
				global_validation,
				local_validation,
			};
				
			let available_data = AvailableData {
				pov,
				omitted_validation,
			};

			let block_msg = AvailabilityStoreMessage::StorePoV(
				candidate_hash,
				Some(validator_index),
				n_validators,
				available_data.clone(),
			);
			virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

			let (tx, rx) = oneshot::channel();
			let query_pov = AvailabilityStoreMessage::QueryPoV(
				candidate_hash,
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: query_pov }).await;

			assert_eq!(rx.await.unwrap().unwrap(), available_data);

			let chunk = query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.unwrap();

			let chunks = erasure::obtain_chunks_v1(10, &available_data).unwrap();

			let mut branches = erasure::branches(chunks.as_ref());

			let branch = branches.nth(5).unwrap();
			let expected_chunk = ErasureChunk {
				chunk: branch.1.to_vec(),
				index: 5,
				proof: branch.0,
			};

			assert_eq!(chunk, expected_chunk);
		});
	}

	#[test]
	fn store_not_included_candidate_is_pruned() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		test_harness(store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let validator_index = 5;
			let n_validators = 10;

			TestTime::set(100000);
			let pov = PoV {
				block_data: BlockData(vec![4, 5, 6]),
			};

			let global_validation = GlobalValidationSchedule {
				max_code_size: 1024,
				max_head_data_size: 1024,
				block_number: 2,
			};

			let local_validation = LocalValidationData {
				parent_head: HeadData(vec![6, 7, 8]),
				balance: 9000,
				validation_code_hash: Hash::from([2; 32]),
				code_upgrade_allowed: None,
			};

			let omitted_validation = OmittedValidationData {
				global_validation,
				local_validation,
			};
				
			let available_data = AvailableData {
				pov,
				omitted_validation,
			};

			let candidate_hash = Hash::from([7; 32]);

			let block_msg = AvailabilityStoreMessage::StorePoV(
				candidate_hash,
				Some(validator_index),
				n_validators,
				available_data.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

			let av_data = query_pov(&mut virtual_overseer, candidate_hash).await.unwrap();

			assert_eq!(av_data, available_data);

			TestTime::set(100000 + KEEP_STORED_BLOCK_FOR * 2);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::StartWork(Hash::from([6; 32])))
			).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					hash,
					RuntimeApiRequest::GetCandidates(relay_parent, tx),
				)) => {
					tx.send(None).unwrap();
					assert_eq!(hash, relay_parent);
				}
			);

			assert!(query_pov(&mut virtual_overseer, candidate_hash).await.is_none());
		});
	}

	#[test]
	fn store_finalized_block_and_chunk_are_pruned_in_time() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		let test_state = TestState::default();
		test_harness(store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let validator_index = 5;
			let n_validators = 10;

			let pov = PoV {
				block_data: BlockData(vec![42, 43, 44]),
			};

			let pov_hash = pov.hash();

			let candidate = TestCandidateBuilder {
				pov_hash,
				relay_parent: test_state.relay_parent,
				..Default::default()
			}.build();

			let candidate_hash = candidate.hash();

			let omitted_validation = OmittedValidationData {
				global_validation: test_state.global_validation_schedule,
				local_validation: test_state.local_validation_data,
			};

			let available_data = AvailableData {
				pov,
				omitted_validation,
			};

			let block_msg = AvailabilityStoreMessage::StorePoV(
				candidate_hash,
				Some(validator_index),
				n_validators,
				available_data.clone(),
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

			assert_eq!(
				query_pov(&mut virtual_overseer, candidate_hash).await.unwrap(),
				available_data,
			);

			TestTime::add(KEEP_STORED_BLOCK_FOR / 2);

			assert_eq!(
				query_pov(&mut virtual_overseer, candidate_hash).await.unwrap(),
				available_data,
			);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::StartWork(Hash::from([6; 32])))
			).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					hash,
					RuntimeApiRequest::GetCandidates(relay_parent, tx),
				)) => {
					assert_eq!(hash, relay_parent);
					tx.send(Some(vec![candidate.clone()])).unwrap();
				}
			);

			// whatever time passes by the block should remain there until finalized.
			TestTime::add(KEEP_STORED_BLOCK_FOR * 5);

			// Still there?
			assert_eq!(
				query_pov(&mut virtual_overseer, candidate_hash).await.unwrap(),
				available_data,
			);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::BlockFinalized(Hash::from([6; 32])))
			).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					hash,
					RuntimeApiRequest::GetCandidates(relay_parent, tx),
				)) => {
					assert_eq!(hash, relay_parent);
					tx.send(Some(vec![candidate])).unwrap();
				}
			);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::StartWork(Hash::from([7; 32])))
			).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_,
					RuntimeApiRequest::GetCandidates(_, tx),
				)) => {
					tx.send(None).unwrap();
				}
			);

			// Still there?
			assert_eq!(
				query_pov(&mut virtual_overseer, candidate_hash).await.unwrap(),
				available_data,
			);

			TestTime::add(KEEP_FINALIZED_BLOCK_FOR + 10);

			virtual_overseer.send(
				FromOverseer::Signal(OverseerSignal::StartWork(Hash::from([8; 32])))
			).await;

			assert_matches!(
				virtual_overseer.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					_,
					RuntimeApiRequest::GetCandidates(_, tx),
				)) => {
					tx.send(None).unwrap();
				}
			);

			// Should be gone by now
			assert!(query_pov(&mut virtual_overseer, candidate_hash).await.is_none());
		});
	}

	async fn query_pov(
		virtual_overseer: &mut subsystem_test::TestSubsystemContextHandle<AvailabilityStoreMessage>,
		candidate_hash: Hash,
	) -> Option<AvailableData> {
		let (tx, rx) = oneshot::channel();

		let query = AvailabilityStoreMessage::QueryPoV(candidate_hash, tx);
		virtual_overseer.send(FromOverseer::Communication{ msg: query }).await;

		rx.await.unwrap()
	}

	async fn query_chunk(
		virtual_overseer: &mut subsystem_test::TestSubsystemContextHandle<AvailabilityStoreMessage>,
		candidate_hash: Hash,
		index: u32,
	) -> Option<ErasureChunk> {
		let (tx, rx) = oneshot::channel();

		let query = AvailabilityStoreMessage::QueryChunk(candidate_hash, index, tx);
		virtual_overseer.send(FromOverseer::Communication{ msg: query }).await;

		rx.await.unwrap()
	}
}
