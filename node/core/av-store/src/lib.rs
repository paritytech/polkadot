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

#![recursion_limit="256"]
#![warn(missing_docs)]

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH};

use codec::{Encode, Decode};
use futures::{select, channel::oneshot, future::{self, Either}, Future, FutureExt};
use futures_timer::Delay;
use kvdb_rocksdb::{Database, DatabaseConfig};
use kvdb::{KeyValueDB, DBTransaction};

use polkadot_primitives::v1::{
	Hash, AvailableData, BlockNumber, CandidateEvent, ErasureChunk, ValidatorIndex,
};
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, SubsystemError, Subsystem, SubsystemContext, SpawnedSubsystem,
	ActiveLeavesUpdate,
	errors::{ChainApiError, RuntimeApiError},
	metrics::{self, prometheus},
};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityStoreMessage, ChainApiMessage, RuntimeApiMessage, RuntimeApiRequest,
};

const LOG_TARGET: &str = "availability";

mod columns {
	pub const DATA: u32 = 0;
	pub const META: u32 = 1;
	pub const NUM_COLUMNS: u32 = 2;
}

#[derive(Debug, derive_more::From)]
enum Error {
	#[from]
	Chain(ChainApiError),
	#[from]
	Erasure(erasure::Error),
	#[from]
	Io(io::Error),
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	Runtime(RuntimeApiError),
	#[from]
	Subsystem(SubsystemError),
	#[from]
	Time(SystemTimeError),
}

/// A key for chunk pruning records.
const CHUNK_PRUNING_KEY: [u8; 14] = *b"chunks_pruning";

/// A key for PoV pruning records.
const POV_PRUNING_KEY: [u8; 11] = *b"pov_pruning";

/// A key for a cached value of next scheduled PoV pruning.
const NEXT_POV_PRUNING: [u8; 16] = *b"next_pov_pruning";

/// A key for a cached value of next scheduled chunk pruning.
const NEXT_CHUNK_PRUNING: [u8; 18] = *b"next_chunk_pruning";

/// The following constants are used under normal conditions:

/// Stored block is kept available for 1 hour.
const KEEP_STORED_BLOCK_FOR: u64 = 60*60;

/// Finalized block is kept for 1 day.
const KEEP_FINALIZED_BLOCK_FOR: u64 = 24 * 60 * 60;

/// Keep chunk of the finalized block for 1 day + 1 hour.
const KEEP_FINALIZED_CHUNK_FOR: u64 = 25 * 60 * 60;

/// At which point in time we need to wakeup and do next pruning of blocks.
/// Essenially this is the first element in the sorted array of pruning data,
/// we just want to cache it here to avoid lifting the whole array just to look at the head.
///
/// This record exists under `NEXT_POV_PRUNING` key, if it does not either:
///  a) There are no records and nothing has to be pruned.
///  b) There are records but all of them are in `Included` state and do not have exact time to
///     be pruned.
#[derive(Decode, Encode)]
struct NextPoVPruning(u64);

impl NextPoVPruning {
	// After which amount of seconds into the future from `now` this should fire.
	fn should_fire_in(&self) -> Result<u64, Error> {
		Ok(self.0.saturating_sub(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()))
	}
}

/// At which point in time we need to wakeup and do next pruning of chunks.
/// Essenially this is the first element in the sorted array of pruning data,
/// we just want to cache it here to avoid lifting the whole array just to look at the head.
///
/// This record exists under `NEXT_CHUNK_PRUNING` key, if it does not either:
///  a) There are no records and nothing has to be pruned.
///  b) There are records but all of them are in `Included` state and do not have exact time to
///     be pruned.
#[derive(Decode, Encode)]
struct NextChunkPruning(u64);

impl NextChunkPruning {
	// After which amount of seconds into the future from `now` this should fire.
	fn should_fire_in(&self) -> Result<u64, Error> {
		Ok(self.0.saturating_sub(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()))
	}
}

/// Struct holding pruning timing configuration.
/// The only purpose of this structure is to use different timing
/// configurations in production and in testing.
#[derive(Clone)]
struct PruningConfig {
	/// How long should a stored block stay available.
	keep_stored_block_for: u64,

	/// How long should a finalized block stay available.
	keep_finalized_block_for: u64,

	/// How long should a chunk of a finalized block stay available.
	keep_finalized_chunk_for: u64
}

impl Default for PruningConfig {
    fn default() -> Self {
		Self {
			keep_stored_block_for: KEEP_STORED_BLOCK_FOR,
			keep_finalized_block_for: KEEP_FINALIZED_BLOCK_FOR,
			keep_finalized_chunk_for: KEEP_FINALIZED_CHUNK_FOR,
		}
    }
}

#[derive(Debug, Decode, Encode, Eq, PartialEq)]
enum CandidateState {
	Stored,
	Included,
	Finalized,
}

#[derive(Debug, Decode, Encode, Eq)]
struct PoVPruningRecord {
	candidate_hash: Hash,
	block_number: BlockNumber,
	candidate_state: CandidateState,
	prune_at: u64,
}

impl PartialEq for PoVPruningRecord {
	fn eq(&self, other: &Self) -> bool {
		self.candidate_hash == other.candidate_hash
	}
}

impl Ord for PoVPruningRecord {
	fn cmp(&self, other: &Self) -> Ordering {
		self.prune_at.cmp(&other.prune_at)
	}
}

impl PartialOrd for PoVPruningRecord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
    }
}

#[derive(Debug, Decode, Encode, Eq)]
struct ChunkPruningRecord {
	candidate_hash: Hash,
	block_number: BlockNumber,
	candidate_state: CandidateState,
	chunk_index: u32,
	prune_at: u64,
}

impl PartialEq for ChunkPruningRecord {
	fn eq(&self, other: &Self) -> bool {
		self.candidate_hash == other.candidate_hash &&
			self.chunk_index == other.chunk_index
	}
}

impl Ord for ChunkPruningRecord {
	fn cmp(&self, other: &Self) -> Ordering {
		self.prune_at.cmp(&other.prune_at)
	}
}

impl PartialOrd for ChunkPruningRecord {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

/// An implementation of the Availability Store subsystem.
pub struct AvailabilityStoreSubsystem {
	pruning_config: PruningConfig,
	inner: Arc<dyn KeyValueDB>,
	metrics: Metrics,
}

impl AvailabilityStoreSubsystem {
	// Perform pruning of PoVs
	fn prune_povs(&self) -> Result<(), Error> {
		let mut tx = DBTransaction::new();
		let mut pov_pruning = pov_pruning(&self.inner).unwrap_or_default();
		let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

		log::trace!(target: LOG_TARGET, "Pruning PoVs");
		let outdated_records_count = pov_pruning.iter()
			.take_while(|r| r.prune_at <= now)
			.count();

		for record in pov_pruning.drain(..outdated_records_count) {
			log::trace!(target: LOG_TARGET, "Removing record {:?}", record);
			tx.delete(
				columns::DATA,
				available_data_key(&record.candidate_hash).as_slice(),
			);
		}

		put_pov_pruning(&self.inner, Some(tx), pov_pruning)?;

		Ok(())
	}

	// Perform pruning of chunks.
	fn prune_chunks(&self) -> Result<(), Error> {
		let mut tx = DBTransaction::new();
		let mut chunk_pruning = chunk_pruning(&self.inner).unwrap_or_default();
		let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

		log::trace!(target: LOG_TARGET, "Pruning Chunks");
		let outdated_records_count = chunk_pruning.iter()
			.take_while(|r| r.prune_at <= now)
			.count();

		for record in chunk_pruning.drain(..outdated_records_count) {
			log::trace!(target: LOG_TARGET, "Removing record {:?}", record);
			tx.delete(
				columns::DATA,
				erasure_chunk_key(&record.candidate_hash, record.chunk_index).as_slice(),
			);
		}

		put_chunk_pruning(&self.inner, Some(tx), chunk_pruning)?;

		Ok(())
	}

	// Return a `Future` that either resolves when another PoV pruning has to happen
	// or is indefinitely `pending` in case no pruning has to be done.
	// Just a helper to `select` over multiple things at once.
	fn maybe_prune_povs(&self) -> Result<impl Future<Output = ()>, Error> {
		let future = match get_next_pov_pruning_time(&self.inner) {
			Some(pruning) => {
				Either::Left(Delay::new(Duration::from_secs(pruning.should_fire_in()?)))
			}
			None => Either::Right(future::pending::<()>()),
		};

		Ok(future)
	}

	// Return a `Future` that either resolves when another chunk pruning has to happen
	// or is indefinitely `pending` in case no pruning has to be done.
	// Just a helper to `select` over multiple things at once.
	fn maybe_prune_chunks(&self) -> Result<impl Future<Output = ()>, Error> {
		let future = match get_next_chunk_pruning_time(&self.inner) {
			Some(pruning) => {
				Either::Left(Delay::new(Duration::from_secs(pruning.should_fire_in()?)))
			}
			None => Either::Right(future::pending::<()>()),
		};

		Ok(future)
	}
}

fn available_data_key(candidate_hash: &Hash) -> Vec<u8> {
	(candidate_hash, 0i8).encode()
}

fn erasure_chunk_key(candidate_hash: &Hash, index: u32) -> Vec<u8> {
	(candidate_hash, index, 0i8).encode()
}

#[derive(Encode, Decode)]
struct StoredAvailableData {
	data: AvailableData,
	n_validators: u32,
}

/// Configuration for the availability store.
pub struct Config {
	/// Total cache size in megabytes. If `None` the default (128 MiB per column) is used.
	pub cache_size: Option<usize>,
	/// Path to the database.
	pub path: PathBuf,
}

impl AvailabilityStoreSubsystem {
	/// Create a new `AvailabilityStoreSubsystem` with a given config on disk.
	pub fn new_on_disk(config: Config, metrics: Metrics) -> io::Result<Self> {
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
			pruning_config: PruningConfig::default(),
			inner: Arc::new(db),
			metrics,
		})
	}

	#[cfg(test)]
	fn new_in_memory(inner: Arc<dyn KeyValueDB>, pruning_config: PruningConfig) -> Self {
		Self {
			pruning_config,
			inner,
			metrics: Metrics(None),
		}
	}
}

fn get_next_pov_pruning_time(db: &Arc<dyn KeyValueDB>) -> Option<NextPoVPruning> {
	query_inner(db, columns::META, &NEXT_POV_PRUNING)
}

fn get_next_chunk_pruning_time(db: &Arc<dyn KeyValueDB>) -> Option<NextChunkPruning> {
	query_inner(db, columns::META, &NEXT_CHUNK_PRUNING)
}

async fn run<Context>(mut subsystem: AvailabilityStoreSubsystem, mut ctx: Context)
	-> Result<(), Error>
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>,
{
	let ctx = &mut ctx;
	loop {
		let pov_pruning_time = subsystem.maybe_prune_povs()?;
		let chunk_pruning_time = subsystem.maybe_prune_chunks()?;

		let mut pov_pruning_time = pov_pruning_time.fuse();
		let mut chunk_pruning_time = chunk_pruning_time.fuse();

		select! {
			incoming = ctx.recv().fuse() => {
				match incoming {
					Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => break,
					Ok(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
						ActiveLeavesUpdate { activated, .. })
					)) => {
						for activated in activated.into_iter() {
							process_block_activated(ctx, &subsystem.inner, activated).await?;
						}
					}
					Ok(FromOverseer::Signal(OverseerSignal::BlockFinalized(hash))) => {
						process_block_finalized(&subsystem, ctx, &subsystem.inner, hash).await?;
					}
					Ok(FromOverseer::Communication { msg }) => {
						process_message(&mut subsystem, ctx, msg).await?;
					}
					Err(_) => break,
				}
			}
			pov_pruning_time = pov_pruning_time => {
				subsystem.prune_povs()?;
			}
			chunk_pruning_time = chunk_pruning_time => {
				subsystem.prune_chunks()?;
			}
			complete => break,
		}
	}

	Ok(())
}

/// As soon as certain block is finalized its pruning records and records of all
/// blocks that we keep that are `older` than the block in question have to be updated.
///
/// The state of data has to be changed from
/// `CandidateState::Included` to `CandidateState::Finalized` and their pruning times have
/// to be updated to `now` + keep_finalized_{block, chunk}_for`.
async fn process_block_finalized<Context>(
	subsystem: &AvailabilityStoreSubsystem,
	ctx: &mut Context,
	db: &Arc<dyn KeyValueDB>,
	hash: Hash,
) -> Result<(), Error>
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>
{
	let block_number = get_block_number(ctx, hash).await?;

	if let Some(mut pov_pruning) = pov_pruning(db) {
		// Since the records are sorted by time in which they need to be pruned and not by block
		// numbers we have to iterate through the whole collection here.
		for record in pov_pruning.iter_mut() {
			if record.block_number <= block_number {
				log::trace!(
					target: LOG_TARGET,
					"Updating pruning record for finalized block {}",
					record.candidate_hash,
				);

				record.prune_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() +
					subsystem.pruning_config.keep_finalized_block_for
			}
		}

		// has to be kept sorted.
		pov_pruning.sort();

		put_pov_pruning(db, None, pov_pruning)?;
	}

	Ok(())
}

async fn process_block_activated<Context>(
	ctx: &mut Context,
	db: &Arc<dyn KeyValueDB>,
	hash: Hash,
) -> Result<(), Error>
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>
{
	let events = request_candidate_events(ctx, hash).await?;

	log::trace!(target: LOG_TARGET, "block activated {}", hash);
	if let Some(mut pov_pruning) = pov_pruning(db) {
		let mut included = HashSet::new();

		for event in events.into_iter() {
			if let CandidateEvent::CandidateIncluded(receipt, _) = event {
				log::trace!(target: LOG_TARGET, "Candidate {} was included", receipt.hash());
				included.insert(receipt.hash());
			}
		}

		for record in pov_pruning.iter_mut() {
			if included.contains(&record.candidate_hash) {
				record.prune_at = std::u64::MAX;
			}
		}

		pov_pruning.sort();

		put_pov_pruning(db, None, pov_pruning)?;
	}

	Ok(())
}

async fn request_candidate_events<Context>(
	ctx: &mut Context,
	hash: Hash,
) -> Result<Vec<CandidateEvent>, Error>
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>
{
	let (tx, rx) = oneshot::channel();

	let msg = AllMessages::RuntimeApi(RuntimeApiMessage::Request(
		hash,
		RuntimeApiRequest::CandidateEvents(tx),
	));

	ctx.send_message(msg.into()).await?;

	Ok(rx.await??)
}

async fn process_message<Context>(
	subsystem: &mut AvailabilityStoreSubsystem,
	ctx: &mut Context,
	msg: AvailabilityStoreMessage,
) -> Result<(), Error>
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>
{
	use AvailabilityStoreMessage::*;
	match msg {
		QueryAvailableData(hash, tx) => {
			tx.send(available_data(&subsystem.inner, &hash).map(|d| d.data))
				.map_err(|_| oneshot::Canceled)?;
		}
		QueryDataAvailability(hash, tx) => {
			tx.send(available_data(&subsystem.inner, &hash).is_some())
				.map_err(|_| oneshot::Canceled)?;
		}
		QueryChunk(hash, id, tx) => {
			tx.send(get_chunk(subsystem, &hash, id)?)
				.map_err(|_| oneshot::Canceled)?;
		}
		QueryChunkAvailability(hash, id, tx) => {
			tx.send(get_chunk(subsystem, &hash, id)?.is_some())
				.map_err(|_| oneshot::Canceled)?;
		}
		StoreChunk(candidate_hash, relay_parent, id, chunk, tx) => {
			let block_number = get_block_number(ctx, relay_parent).await?;
			match store_chunk(subsystem, &candidate_hash, id, chunk, block_number) {
				Err(e) => {
					tx.send(Err(())).map_err(|_| oneshot::Canceled)?;
					return Err(e);
				}
				Ok(()) => {
					tx.send(Ok(())).map_err(|_| oneshot::Canceled)?;
				}
			}
		}
		StoreAvailableData(hash, id, n_validators, av_data, tx) => {
			match store_available_data(subsystem, &hash, id, n_validators, av_data) {
				Err(e) => {
					tx.send(Err(())).map_err(|_| oneshot::Canceled)?;
					return Err(e);
				}
				Ok(()) => {
					tx.send(Ok(())).map_err(|_| oneshot::Canceled)?;
				}
			}
		}
	}

	Ok(())
}

fn available_data(db: &Arc<dyn KeyValueDB>, candidate_hash: &Hash) -> Option<StoredAvailableData> {
	query_inner(db, columns::DATA, &available_data_key(candidate_hash))
}

fn pov_pruning(db: &Arc<dyn KeyValueDB>) -> Option<Vec<PoVPruningRecord>> {
	query_inner(db, columns::META, &POV_PRUNING_KEY)
}

fn chunk_pruning(db: &Arc<dyn KeyValueDB>) -> Option<Vec<ChunkPruningRecord>> {
	query_inner(db, columns::META, &CHUNK_PRUNING_KEY)
}

fn put_pov_pruning(
	db: &Arc<dyn KeyValueDB>,
	tx: Option<DBTransaction>,
	pov_pruning: Vec<PoVPruningRecord>,
) -> Result<(), Error> {
	let mut tx = tx.unwrap_or_default();

	tx.put_vec(
		columns::META,
		&POV_PRUNING_KEY,
		pov_pruning.encode(),
	);

	match pov_pruning.get(0) {
		// We want to wake up in case we have some records that are not scheduled to be kept
		// indefinitely (data is included and waiting to move to the finalized state) and so
		// the is at least one value that is not `u64::MAX`.
		Some(head) if head.prune_at != std::u64::MAX => {
			tx.put_vec(
				columns::META,
				&NEXT_POV_PRUNING,
				NextPoVPruning(head.prune_at).encode(),
			);
		}
		_ => {
			// If there is no longer any records, delete the cached pruning time record.
			tx.delete(
				columns::META,
				&NEXT_POV_PRUNING,
			);
		}
	}

	db.write(tx)?;

	Ok(())
}

fn put_chunk_pruning(
	db: &Arc<dyn KeyValueDB>,
	tx: Option<DBTransaction>,
	chunk_pruning: Vec<ChunkPruningRecord>,
) -> Result<(), Error> {
	let mut tx = tx.unwrap_or_default();

	tx.put_vec(
		columns::META,
		&CHUNK_PRUNING_KEY,
		chunk_pruning.encode(),
	);

	match chunk_pruning.get(0) {
		Some(head) if head.prune_at != std::u64::MAX => {
			tx.put_vec(
				columns::META,
				&NEXT_CHUNK_PRUNING,
				NextChunkPruning(head.prune_at).encode(),
			);
		}
		_ => {
			tx.delete(
				columns::META,
				&NEXT_CHUNK_PRUNING,
			);
		}
	}

	db.write(tx)?;

	Ok(())
}


async fn get_block_number<Context>(
	ctx: &mut Context,
	relay_parent: Hash,
) -> Result<BlockNumber, Error>
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>,
{
	let (tx, rx) = oneshot::channel();

	ctx.send_message(AllMessages::ChainApi(ChainApiMessage::BlockNumber(relay_parent, tx))).await?;

	Ok(rx.await??.map(|number| number + 1).unwrap_or_default())
}

fn store_available_data(
	subsystem: &mut AvailabilityStoreSubsystem,
	candidate_hash: &Hash,
	id: Option<ValidatorIndex>,
	n_validators: u32,
	available_data: AvailableData,
) -> Result<(), Error> {
	let mut tx = DBTransaction::new();

	let block_number = available_data.validation_data.block_number;

	if let Some(index) = id {
		let chunks = get_chunks(&available_data, n_validators as usize, &subsystem.metrics)?;
		store_chunk(
			subsystem,
			candidate_hash,
			n_validators,
			chunks[index as usize].clone(),
			block_number,
		)?;
	}

	let stored_data = StoredAvailableData {
		data: available_data,
		n_validators,
	};

	let mut pov_pruning = pov_pruning(&subsystem.inner).unwrap_or_default();

	let pruning_record = PoVPruningRecord {
		candidate_hash: candidate_hash.clone(),
		block_number,
		candidate_state: CandidateState::Stored,
		prune_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() +
			subsystem.pruning_config.keep_stored_block_for,
	};

	let idx = pov_pruning.binary_search(&pruning_record).unwrap_or_else(|x| x);

	pov_pruning.insert(idx, pruning_record);
	let next_pruning = pov_pruning[0].prune_at;

	tx.put_vec(
		columns::DATA,
		available_data_key(&candidate_hash).as_slice(),
		stored_data.encode(),
	);

	tx.put_vec(
		columns::META,
		&POV_PRUNING_KEY,
		pov_pruning.encode(),
	);

	tx.put_vec(
		columns::META,
		&NEXT_POV_PRUNING,
		NextPoVPruning(next_pruning).encode(),
	);

	subsystem.inner.write(tx)?;

	Ok(())
}

fn store_chunk(
	subsystem: &mut AvailabilityStoreSubsystem,
	candidate_hash: &Hash,
	_n_validators: u32,
	chunk: ErasureChunk,
	block_number: BlockNumber,
) -> Result<(), Error> {
	let mut tx = DBTransaction::new();

	let dbkey = erasure_chunk_key(candidate_hash, chunk.index);

	let mut chunk_pruning = chunk_pruning(&subsystem.inner).unwrap_or_default();

	let pruning_record = ChunkPruningRecord {
		candidate_hash: candidate_hash.clone(),
		block_number,
		candidate_state: CandidateState::Stored,
		chunk_index: chunk.index,
		prune_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() +
			subsystem.pruning_config.keep_stored_block_for,
	};

	let idx = chunk_pruning.binary_search(&pruning_record).unwrap_or_else(|x| x);

	chunk_pruning.insert(idx, pruning_record);
	let next_pruning = chunk_pruning[0].prune_at;

	tx.put_vec(
		columns::DATA,
		&dbkey,
		chunk.encode(),
	);

	tx.put_vec(
		columns::META,
		&NEXT_CHUNK_PRUNING,
		NextChunkPruning(next_pruning).encode(),
	);

	subsystem.inner.write(tx)?;

	Ok(())
}

fn get_chunk(
	subsystem: &mut AvailabilityStoreSubsystem,
	candidate_hash: &Hash,
	index: u32,
) -> Result<Option<ErasureChunk>, Error> {
	if let Some(chunk) = query_inner(
		&subsystem.inner,
		columns::DATA,
		&erasure_chunk_key(candidate_hash, index)
	) {
		return Ok(Some(chunk));
	}

	if let Some(data) = available_data(&subsystem.inner, candidate_hash) {
		let mut chunks = get_chunks(&data.data, data.n_validators as usize, &subsystem.metrics)?;
		let desired_chunk = chunks.get(index as usize).cloned();
		for chunk in chunks.drain(..) {
			store_chunk(
				subsystem,
				candidate_hash,
				data.n_validators,
				chunk,
				data.data.validation_data.block_number,
			)?;
		}
		return Ok(desired_chunk);
	}

	Ok(None)
}

fn query_inner<D: Decode>(db: &Arc<dyn KeyValueDB>, column: u32, key: &[u8]) -> Option<D> {
	match db.get(column, key) {
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

impl<Context> Subsystem<Context> for AvailabilityStoreSubsystem
	where
		Context: SubsystemContext<Message=AvailabilityStoreMessage>,
{
	type Metrics = Metrics;

	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			if let Err(e) = run(self, ctx).await {
				log::error!(target: LOG_TARGET, "Subsystem exited with an error {:?}", e);
			}
		});

		SpawnedSubsystem {
			name: "availability-store-subsystem",
			future,
		}
	}
}

fn get_chunks(data: &AvailableData, n_validators: usize, metrics: &Metrics) -> Result<Vec<ErasureChunk>, Error> {
	let chunks = erasure::obtain_chunks_v1(n_validators, data)?;
	metrics.on_chunks_received(chunks.len());
	let branches = erasure::branches(chunks.as_ref());

	Ok(chunks
		.iter()
		.zip(branches.map(|(proof, _)| proof))
		.enumerate()
		.map(|(index, (chunk, proof))| ErasureChunk {
			chunk: chunk.clone(),
			proof,
			index: index as u32,
		})
		.collect()
	)
}

#[derive(Clone)]
struct MetricsInner {
	received_availability_chunks_total: prometheus::Counter<prometheus::U64>,
}

/// Availability metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_chunks_received(&self, count: usize) {
		if let Some(metrics) = &self.0 {
			use core::convert::TryFrom as _;
			// assume usize fits into u64
			let by = u64::try_from(count).unwrap_or_default();
			metrics.received_availability_chunks_total.inc_by(by);
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			received_availability_chunks_total: prometheus::register(
				prometheus::Counter::new(
					"parachain_received_availability_chunks_total",
					"Number of availability chunks received.",
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use assert_matches::assert_matches;
	use futures::{
		future,
		channel::oneshot,
		executor,
		Future,
	};
	use smallvec::smallvec;

	use polkadot_primitives::v1::{
		AvailableData, BlockData, CandidateDescriptor, CandidateReceipt, HeadData,
		PersistedValidationData, PoV, Id as ParaId,
	};
	use polkadot_node_subsystem_util::TimeoutExt;
	use polkadot_subsystem::ActiveLeavesUpdate;
	use polkadot_node_subsystem_test_helpers as test_helpers;

	struct TestHarness {
		virtual_overseer: test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	}

	#[derive(Default)]
	struct TestCandidateBuilder {
		para_id: ParaId,
		pov_hash: Hash,
		relay_parent: Hash,
		commitments_hash: Hash,
	}

	impl TestCandidateBuilder {
		fn build(self) -> CandidateReceipt {
			CandidateReceipt {
				descriptor: CandidateDescriptor {
					para_id: self.para_id,
					pov_hash: self.pov_hash,
					relay_parent: self.relay_parent,
					..Default::default()
				},
				commitments_hash: self.commitments_hash,
			}
		}
	}

	struct TestState {
		persisted_validation_data: PersistedValidationData,
		pruning_config: PruningConfig,
	}

	impl Default for TestState {
		fn default() -> Self {
			let persisted_validation_data = PersistedValidationData {
				parent_head: HeadData(vec![7, 8, 9]),
				block_number: 5,
				hrmp_mqc_heads: Vec::new(),
			};

			let pruning_config = PruningConfig {
				keep_stored_block_for: 1,
				keep_finalized_block_for: 2,
				keep_finalized_chunk_for: 2,
			};

			Self {
				persisted_validation_data,
				pruning_config,
			}
		}
	}

	fn test_harness<T: Future<Output=()>>(
		pruning_config: PruningConfig,
		store: Arc<dyn KeyValueDB>,
		test: impl FnOnce(TestHarness) -> T,
	) {
		let _ = env_logger::builder()
			.is_test(true)
			.filter(
				Some("polkadot_node_core_av_store"),
				log::LevelFilter::Trace,
			)
			.filter(
				Some(LOG_TARGET),
				log::LevelFilter::Trace,
			)
			.try_init();

		let pool = sp_core::testing::TaskExecutor::new();
		let (context, virtual_overseer) = test_helpers::make_subsystem_context(pool.clone());

		let subsystem = AvailabilityStoreSubsystem::new_in_memory(store, pruning_config);
		let subsystem = run(subsystem, context);

		let test_fut = test(TestHarness {
			virtual_overseer,
		});

		futures::pin_mut!(test_fut);
		futures::pin_mut!(subsystem);

		executor::block_on(future::select(test_fut, subsystem));
	}

	const TIMEOUT: Duration = Duration::from_millis(100);

	async fn overseer_send(
		overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
		msg: AvailabilityStoreMessage,
	) {
		log::trace!("Sending message:\n{:?}", &msg);
		overseer
			.send(FromOverseer::Communication { msg })
			.timeout(TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough for sending messages.", TIMEOUT));
	}

	async fn overseer_recv(
		overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
	) -> AllMessages {
		let msg = overseer_recv_with_timeout(overseer, TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough to receive messages", TIMEOUT));

		log::trace!("Received message:\n{:?}", &msg);

		msg
	}

	async fn overseer_recv_with_timeout(
		overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
		timeout: Duration,
	) -> Option<AllMessages> {
		log::trace!("Waiting for message...");
		overseer
			.recv()
			.timeout(timeout)
			.await
	}

	async fn overseer_signal(
		overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
		signal: OverseerSignal,
	) {
		overseer
			.send(FromOverseer::Signal(signal))
			.timeout(TIMEOUT)
			.await
			.expect(&format!("{:?} is more than enough for sending signals.", TIMEOUT));
	}

	#[test]
	fn store_chunk_works() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		test_harness(PruningConfig::default(), store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let relay_parent = Hash::repeat_byte(32);
			let candidate_hash = Hash::repeat_byte(33);
			let validator_index = 5;

			let chunk = ErasureChunk {
				chunk: vec![1, 2, 3],
				index: validator_index,
				proof: vec![vec![3, 4, 5]],
			};

			let (tx, rx) = oneshot::channel();

			let chunk_msg = AvailabilityStoreMessage::StoreChunk(
				candidate_hash,
				relay_parent,
				validator_index,
				chunk.clone(),
				tx,
			);

			overseer_send(&mut virtual_overseer, chunk_msg.into()).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::ChainApi(ChainApiMessage::BlockNumber(
					hash,
					tx,
				)) => {
					assert_eq!(hash, relay_parent);
					tx.send(Ok(Some(4))).unwrap();
				}
			);

			assert_eq!(rx.await.unwrap(), Ok(()));

			let (tx, rx) = oneshot::channel();
			let query_chunk = AvailabilityStoreMessage::QueryChunk(
				candidate_hash,
				validator_index,
				tx,
			);

			overseer_send(&mut virtual_overseer, query_chunk.into()).await;

			assert_eq!(rx.await.unwrap().unwrap(), chunk);
		});
	}

	#[test]
	fn store_block_works() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		let test_state = TestState::default();
		test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let candidate_hash = Hash::from([1; 32]);
			let validator_index = 5;
			let n_validators = 10;

			let pov = PoV {
				block_data: BlockData(vec![4, 5, 6]),
			};

			let available_data = AvailableData {
				pov,
				validation_data: test_state.persisted_validation_data,
			};


			let (tx, rx) = oneshot::channel();
			let block_msg = AvailabilityStoreMessage::StoreAvailableData(
				candidate_hash,
				Some(validator_index),
				n_validators,
				available_data.clone(),
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;
			assert_eq!(rx.await.unwrap(), Ok(()));

			let pov = query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap();
			assert_eq!(pov, available_data);

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
	fn store_pov_and_query_chunk_works() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		let test_state = TestState::default();

		test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let candidate_hash = Hash::from([1; 32]);
			let n_validators = 10;

			let pov = PoV {
				block_data: BlockData(vec![4, 5, 6]),
			};

			let available_data = AvailableData {
				pov,
				validation_data: test_state.persisted_validation_data,
			};

			let no_metrics = Metrics(None);
			let chunks_expected = get_chunks(&available_data, n_validators as usize, &no_metrics).unwrap();

			let (tx, rx) = oneshot::channel();
			let block_msg = AvailabilityStoreMessage::StoreAvailableData(
				candidate_hash,
				None,
				n_validators,
				available_data,
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

			assert_eq!(rx.await.unwrap(), Ok(()));

			for validator_index in 0..n_validators {
				let chunk = query_chunk(&mut virtual_overseer, candidate_hash, validator_index).await.unwrap();

				assert_eq!(chunk, chunks_expected[validator_index as usize]);
			}
		});
	}

	#[test]
	fn stored_but_not_included_data_is_pruned() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		let test_state = TestState::default();

		test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let candidate_hash = Hash::repeat_byte(1);
			let n_validators = 10;

			let pov = PoV {
				block_data: BlockData(vec![4, 5, 6]),
			};

			let available_data = AvailableData {
				pov,
				validation_data: test_state.persisted_validation_data,
			};

			let (tx, rx) = oneshot::channel();
			let block_msg = AvailabilityStoreMessage::StoreAvailableData(
				candidate_hash,
				None,
				n_validators,
				available_data.clone(),
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

			rx.await.unwrap().unwrap();

			// At this point data should be in the store.
			assert_eq!(
				query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
				available_data,
			);

			// Wait for twice as long as the stored block kept for.
			Delay::new(Duration::from_secs(
					test_state.pruning_config.keep_stored_block_for * 2
				)
			).await;

			// The block was not included by this point so it should be pruned now.
			assert!(query_available_data(&mut virtual_overseer, candidate_hash).await.is_none());
		});
	}

	#[test]
	fn stored_data_kept_until_finalized() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		let test_state = TestState::default();

		test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let n_validators = 10;

			let pov = PoV {
				block_data: BlockData(vec![4, 5, 6]),
			};

			let pov_hash = pov.hash();

			let candidate = TestCandidateBuilder {
				pov_hash,
				..Default::default()
			}.build();

			let candidate_hash = candidate.hash();

			let available_data = AvailableData {
				pov,
				validation_data: test_state.persisted_validation_data,
			};

			let (tx, rx) = oneshot::channel();
			let block_msg = AvailabilityStoreMessage::StoreAvailableData(
				candidate_hash,
				None,
				n_validators,
				available_data.clone(),
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg: block_msg }).await;

			rx.await.unwrap().unwrap();

			// At this point data should be in the store.
			assert_eq!(
				query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
				available_data,
			);

			let new_leaf = Hash::repeat_byte(2);
			overseer_signal(
				&mut virtual_overseer,
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated: smallvec![new_leaf.clone()],
					deactivated: smallvec![],
				}),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					relay_parent,
					RuntimeApiRequest::CandidateEvents(tx),
				)) => {
					assert_eq!(relay_parent, new_leaf);
					tx.send(Ok(vec![
						CandidateEvent::CandidateIncluded(candidate, HeadData::default()),
					])).unwrap();
				}
			);

			Delay::new(Duration::from_secs(
				test_state.pruning_config.keep_stored_block_for * 10
			)).await;

			// At this point data should _still_ be in the store.
			assert_eq!(
				query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
				available_data,
			);

			overseer_signal(
				&mut virtual_overseer,
				OverseerSignal::BlockFinalized(new_leaf)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::ChainApi(ChainApiMessage::BlockNumber(
					hash,
					tx,
				)) => {
					assert_eq!(hash, new_leaf);
					tx.send(Ok(Some(10))).unwrap();
				}
			);

			// Wait for a half of the time finalized data should be available for
			Delay::new(Duration::from_secs(
				test_state.pruning_config.keep_finalized_block_for / 2
			)).await;

			// At this point data should _still_ be in the store.
			assert_eq!(
				query_available_data(&mut virtual_overseer, candidate_hash).await.unwrap(),
				available_data,
			);

			// Wait until it is should be gone.
			Delay::new(Duration::from_secs(
				test_state.pruning_config.keep_finalized_block_for
			)).await;

			// At this point data should be gone from the store.
			assert!(
				query_available_data(&mut virtual_overseer, candidate_hash).await.is_none(),
			);
		});
	}

	#[test]
	fn forkfullness_works() {
		let store = Arc::new(kvdb_memorydb::create(columns::NUM_COLUMNS));
		let test_state = TestState::default();

		test_harness(test_state.pruning_config.clone(), store.clone(), |test_harness| async move {
			let TestHarness { mut virtual_overseer } = test_harness;
			let n_validators = 10;

			let pov_1 = PoV {
				block_data: BlockData(vec![1, 2, 3]),
			};

			let pov_1_hash = pov_1.hash();

			let pov_2 = PoV {
				block_data: BlockData(vec![4, 5, 6]),
			};

			let pov_2_hash = pov_2.hash();

			let candidate_1 = TestCandidateBuilder {
				pov_hash: pov_1_hash,
				..Default::default()
			}.build();

			let candidate_1_hash = candidate_1.hash();

			let candidate_2 = TestCandidateBuilder {
				pov_hash: pov_2_hash,
				..Default::default()
			}.build();

			let candidate_2_hash = candidate_2.hash();

			let available_data_1 = AvailableData {
				pov: pov_1,
				validation_data: test_state.persisted_validation_data.clone(),
			};

			let available_data_2 = AvailableData {
				pov: pov_2,
				validation_data: test_state.persisted_validation_data,
			};

			let (tx, rx) = oneshot::channel();
			let msg = AvailabilityStoreMessage::StoreAvailableData(
				candidate_1_hash,
				None,
				n_validators,
				available_data_1.clone(),
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg }).await;

			rx.await.unwrap().unwrap();

			let (tx, rx) = oneshot::channel();
			let msg = AvailabilityStoreMessage::StoreAvailableData(
				candidate_2_hash,
				None,
				n_validators,
				available_data_2.clone(),
				tx,
			);

			virtual_overseer.send(FromOverseer::Communication{ msg }).await;

			rx.await.unwrap().unwrap();

			assert_eq!(
				query_available_data(&mut virtual_overseer, candidate_1_hash).await.unwrap(),
				available_data_1,
			);

			assert_eq!(
				query_available_data(&mut virtual_overseer, candidate_2_hash).await.unwrap(),
				available_data_2,
			);


			let new_leaf_1 = Hash::repeat_byte(2);
			let new_leaf_2 = Hash::repeat_byte(3);

			overseer_signal(
				&mut virtual_overseer,
				OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
					activated: smallvec![new_leaf_1.clone(), new_leaf_2.clone()],
					deactivated: smallvec![],
				}),
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					leaf,
					RuntimeApiRequest::CandidateEvents(tx),
				)) => {
					assert_eq!(leaf, new_leaf_1);
					tx.send(Ok(vec![
						CandidateEvent::CandidateIncluded(candidate_1, HeadData::default()),
					])).unwrap();
				}
			);

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					leaf,
					RuntimeApiRequest::CandidateEvents(tx),
				)) => {
					assert_eq!(leaf, new_leaf_2);
					tx.send(Ok(vec![
						CandidateEvent::CandidateIncluded(candidate_2, HeadData::default()),
					])).unwrap();
				}
			);

			overseer_signal(
				&mut virtual_overseer,
				OverseerSignal::BlockFinalized(new_leaf_1)
			).await;

			assert_matches!(
				overseer_recv(&mut virtual_overseer).await,
				AllMessages::ChainApi(ChainApiMessage::BlockNumber(
					hash,
					tx,
				)) => {
					assert_eq!(hash, new_leaf_1);
					tx.send(Ok(Some(5))).unwrap();
				}
			);


			// Data of both candidates should be still present in the DB.
			assert_eq!(
				query_available_data(&mut virtual_overseer, candidate_1_hash).await.unwrap(),
				available_data_1,
			);

			assert_eq!(
				query_available_data(&mut virtual_overseer, candidate_2_hash).await.unwrap(),
				available_data_2,
			);
			// Wait for longer than finalized blocks should be kept for
			Delay::new(Duration::from_secs(
				test_state.pruning_config.keep_finalized_block_for + 1
			)).await;

			// Data of both candidates should be gone now.
			assert!(
				query_available_data(&mut virtual_overseer, candidate_1_hash).await.is_none(),
			);

			assert!(
				query_available_data(&mut virtual_overseer, candidate_2_hash).await.is_none(),
			);
		});
	}

	async fn query_available_data(
		virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
		candidate_hash: Hash,
	) -> Option<AvailableData> {
		let (tx, rx) = oneshot::channel();

		let query = AvailabilityStoreMessage::QueryAvailableData(candidate_hash, tx);
		virtual_overseer.send(FromOverseer::Communication{ msg: query }).await;

		rx.await.unwrap()
	}

	async fn query_chunk(
		virtual_overseer: &mut test_helpers::TestSubsystemContextHandle<AvailabilityStoreMessage>,
		candidate_hash: Hash,
		index: u32,
	) -> Option<ErasureChunk> {
		let (tx, rx) = oneshot::channel();

		let query = AvailabilityStoreMessage::QueryChunk(candidate_hash, index, tx);
		virtual_overseer.send(FromOverseer::Communication{ msg: query }).await;

		rx.await.unwrap()
	}
}
