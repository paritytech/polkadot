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

use parity_scale_codec::{Encode, Decode, Input, Error as CodecError};
use futures::{select, channel::oneshot, future::{self, Either}, Future, FutureExt};
use futures_timer::Delay;
use kvdb_rocksdb::{Database, DatabaseConfig};
use kvdb::{KeyValueDB, DBTransaction};

use polkadot_primitives::v1::{
	Hash, AvailableData, BlockNumber, CandidateEvent, ErasureChunk, ValidatorIndex, CandidateHash,
	CandidateReceipt,
};
use polkadot_subsystem::{
	FromOverseer, OverseerSignal, SubsystemError, Subsystem, SubsystemContext, SpawnedSubsystem,
	ActiveLeavesUpdate,
	errors::{ChainApiError, RuntimeApiError},
};
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_subsystem::messages::{
	AllMessages, AvailabilityStoreMessage, ChainApiMessage, RuntimeApiMessage, RuntimeApiRequest,
};
use bitvec::{vec::BitVec, order::Lsb0 as BitOrderLsb0};

// #[cfg(test)]
// mod tests;

const LOG_TARGET: &str = "availability";

mod columns {
	pub const DATA: u32 = 0;
	pub const META: u32 = 1;
	pub const NUM_COLUMNS: u32 = 2;
}

/// The following constants are used under normal conditions:

const LAST_FINALIZED_KEY: &[u8] = &*b"LastFinalized";

const AVAILABLE_PREFIX: &[u8] = &*b"available";
const CHUNK_PREFIX: &[u8] = &*b"chunk";
const META_PREFIX: &[u8] = &*b"meta";
const UNFINALIZED_PREFIX: &[u8] = &*b"unfinalized";
const PRUNE_BY_TIME_PREFIX: &[u8] = &*b"prune_by_time";

// We have some keys we want to map to empty values because existence of the key is enough. We use this because
// rocksdb doesn't support empty values.
const TOMBSTONE_VALUE: &[u8] = &*b" ";

/// Unavailable blocks are kept for 1 hour.
const KEEP_UNAVAILABLE_FOR: Duration = Duration::from_secs(60 * 60);

/// Finalized data is kept for 25 hours.
const KEEP_FINALIZED_FOR: Duration = Duration::from_secs(25 * 60 * 60);

/// The pruning interval.
const PRUNING_INTERVAL: Duration = Duration::from_secs(60 * 5);

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord)]
struct BETimestamp(u64);

impl Encode for BETimestamp {
	fn size_hint(&self) -> usize {
		std::mem::size_of::<u64>()
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		f(&self.0.to_be_bytes())
	}
}

impl Decode for BETimestamp {
	fn decode<I: Input>(value: &mut I) -> Result<Self, CodecError> {
		<[u8; 8]>::decode(value).map(u64::from_be_bytes).map(Self)
	}
}

impl From<Duration> for BETimestamp {
	fn from(d: Duration) -> Self {
		BETimestamp(d.as_secs())
	}
}

impl Into<Duration> for BETimestamp {
	fn into(self) -> Duration {
		Duration::from_secs(self.0)
	}
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
struct BEBlockNumber(BlockNumber);

impl Encode for BEBlockNumber {
	fn size_hint(&self) -> usize {
		std::mem::size_of::<BlockNumber>()
	}

	fn using_encoded<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
		f(&self.0.to_be_bytes())
	}
}

impl Decode for BEBlockNumber {
	fn decode<I: Input>(value: &mut I) -> Result<Self, CodecError> {
		<[u8; std::mem::size_of::<BlockNumber>()]>::decode(value).map(BlockNumber::from_be_bytes).map(Self)
	}
}

#[derive(Debug, Encode, Decode)]
enum State {
	/// Candidate data was first observed at the given time but is not available in any block.
	Unavailable(BETimestamp),
	/// The candidate was first observed at the given time and was included in the given list of unfinalized blocks, which may be
	/// empty. The timestamp here is not used for pruning. Either one of these blocks will be finalized or the state will regress to
	/// `State::Unavailable`, in which case the same timestamp will be reused.
	Unfinalized(BETimestamp, Vec<(BEBlockNumber, Hash)>),
	/// Candidate data has appeared in a finalized block and did so at the given time.
	Finalized(BETimestamp)
}

// Meta information about a candidate.
#[derive(Debug, Encode, Decode)]
struct CandidateMeta {
	state: State,
	data_available: bool,
	chunks_stored: BitVec<BitOrderLsb0, u8>,
}

fn query_inner<D: Decode>(
	db: &Arc<dyn KeyValueDB>,
	column: u32,
	key: &[u8],
) -> Option<D> {
	match db.get(column, key) {
		Ok(Some(raw)) => {
			let res = D::decode(&mut &raw[..]).expect("all stored data serialized correctly; qed");
			Some(res)
		}
		Ok(None) => None,
		Err(e) => {
			tracing::warn!(target: LOG_TARGET, err = ?e, "Error reading from the availability store");
			None
		}
	}
}

fn load_last_finalized(
	db: &Arc<dyn KeyValueDB>,
) -> Option<BlockNumber> {
	query_inner(db, columns::META, LAST_FINALIZED_KEY)
}

fn write_last_finalized(
	tx: &mut DBTransaction,
	finalized: BlockNumber,
) {
	tx.put_vec(columns::META, LAST_FINALIZED_KEY, finalized.encode());
}

fn load_available_data(
	db: &Arc<dyn KeyValueDB>,
	hash: &CandidateHash,
) -> Option<AvailableData> {
	let key = (AVAILABLE_PREFIX, hash).encode();

	query_inner(db, columns::DATA, &key)
}

fn load_chunk(
	db: &Arc<dyn KeyValueDB>,
	hash: &CandidateHash,
	chunk_index: ValidatorIndex,
) -> Option<ErasureChunk> {
	let key = (CHUNK_PREFIX, hash, chunk_index).encode();

	query_inner(db, columns::DATA, &key)
}

fn write_chunk(
	tx: &mut DBTransaction,
	candidate_hash: &CandidateHash,
	chunk_index: ValidatorIndex,
	erasure_chunk: &ErasureChunk,
) {
	let key = (CHUNK_PREFIX, candidate_hash, chunk_index, erasure_chunk).encode();

	tx.put_vec(columns::DATA, &key, erasure_chunk.encode());
}

fn load_meta(
	db: &Arc<dyn KeyValueDB>,
	hash: &CandidateHash,
) -> Option<CandidateMeta> {
	let key = (META_PREFIX, hash).encode();

	query_inner(db, columns::META, &key)
}

fn write_meta(
	tx: &mut DBTransaction,
	hash: &CandidateHash,
	meta: &CandidateMeta,
) {
	let key = (META_PREFIX, hash) .encode();

	tx.put_vec(columns::META, &key, meta.encode());
}

fn delete_unfinalized_height(
	tx: &mut DBTransaction,
	block_number: BlockNumber,
) {
	let prefix = (UNFINALIZED_PREFIX, BEBlockNumber(block_number)).encode();
	tx.delete_prefix(columns::META, &prefix);
}

fn delete_unfinalized_inclusion(
	tx: &mut DBTransaction,
	block_number: BlockNumber,
	block_hash: &Hash,
	candidate_hash: &CandidateHash,
) {
	let key = (
		UNFINALIZED_PREFIX,
		BEBlockNumber(block_number),
		block_hash,
		candidate_hash,
	).encode();

	tx.delete(columns::META, &key[..]);
}

fn delete_pruning_key(tx: &mut DBTransaction, t: impl Into<BETimestamp>, h: &CandidateHash) {
	let key = (PRUNE_BY_TIME_PREFIX, t.into(), h).encode();
	tx.delete(columns::META, &key);
}

fn write_pruning_key(tx: &mut DBTransaction, t: impl Into<BETimestamp>, h: &CandidateHash) {
	let key = (PRUNE_BY_TIME_PREFIX, t.into(), h).encode();
	tx.put(columns::META, &key, TOMBSTONE_VALUE);
}

fn finalized_block_range(last_finalized: BlockNumber, finalized: BlockNumber) -> (Vec<u8>, Vec<u8>) {
	let start = (UNFINALIZED_PREFIX, BEBlockNumber(last_finalized)).encode();
	let end = (UNFINALIZED_PREFIX, BEBlockNumber(finalized + 1)).encode();

	(start, end)
}

fn write_unfinalized_block_contains(
	tx: &mut DBTransaction,
	n: BlockNumber,
	h: &Hash,
	ch: &CandidateHash,
) {
	let key = (UNFINALIZED_PREFIX, BEBlockNumber(n), h, ch).encode();
	tx.put(columns::META, &key, TOMBSTONE_VALUE);
}

fn decode_unfinalized_key(s: &[u8]) -> Result<(BlockNumber, Hash, CandidateHash), CodecError> {
	if !s.starts_with(UNFINALIZED_PREFIX) {
		return Err("missing magic string".into());
	}

	<(BEBlockNumber, Hash, CandidateHash)>::decode(&mut &s[UNFINALIZED_PREFIX.len()..])
		.map(|(b, h, ch)| (b.0, h, ch))
}

fn decode_pruning_key(s: &[u8]) -> Result<(Duration, CandidateHash), CodecError> {
	if !s.starts_with(PRUNE_BY_TIME_PREFIX) {
		return Err("missing magic string".into());
	}

	<(BETimestamp, CandidateHash)>::decode(&mut &s[PRUNE_BY_TIME_PREFIX.len()..])
		.map(|(t, ch)| (t.into(), ch))
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Erasure(#[from] erasure::Error),

	#[error(transparent)]
	Io(#[from] io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),

	#[error(transparent)]
	Time(#[from] SystemTimeError),

	#[error("Custom databases are not supported")]
	CustomDatabase,
}

impl Error {
	fn trace(&self) {
		match self {
			// don't spam the log with spurious errors
			Self::RuntimeApi(_) |
			Self::Oneshot(_) => tracing::debug!(target: LOG_TARGET, err = ?self),
			// it's worth reporting otherwise
			_ => tracing::warn!(target: LOG_TARGET, err = ?self),
		}
	}
}

/// Struct holding pruning timing configuration.
/// The only purpose of this structure is to use different timing
/// configurations in production and in testing.
#[derive(Clone)]
struct PruningConfig {
	/// How long unavailable data should be kept.
	keep_unavailable_for: Duration,

	/// How long finalized data should be kept.
	keep_finalized_for: Duration,
}

impl Default for PruningConfig {
	fn default() -> Self {
		Self {
			keep_unavailable_for: KEEP_UNAVAILABLE_FOR,
			keep_finalized_for: KEEP_FINALIZED_FOR,
		}
	}
}

/// Configuration for the availability store.
pub struct Config {
	/// Total cache size in megabytes. If `None` the default (128 MiB per column) is used.
	pub cache_size: Option<usize>,
	/// Path to the database.
	pub path: PathBuf,
}

impl std::convert::TryFrom<sc_service::config::DatabaseConfig> for Config {
	type Error = Error;

	fn try_from(config: sc_service::config::DatabaseConfig) -> Result<Self, Self::Error> {
		let path = config.path().ok_or(Error::CustomDatabase)?;

		Ok(Self {
			// substrate cache size is improper here; just use the default
			cache_size: None,
			// DB path is a sub-directory of substrate db path to give two properties:
			// 1: column numbers don't conflict with substrate
			// 2: commands like purge-chain work without further changes
			path: path.join("parachains").join("av-store"),
		})
	}
}

/// An implementation of the Availability Store subsystem.
pub struct AvailabilityStoreSubsystem {
	pruning_config: PruningConfig,
	inner: Arc<dyn KeyValueDB>,
	chunks_cache: HashMap<CandidateHash, HashMap<u32, ErasureChunk>>,
	metrics: Metrics,
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

		std::fs::create_dir_all(&path)?;
		let db = Database::open(&db_config, &path)?;

		Ok(Self {
			pruning_config: PruningConfig::default(),
			inner: Arc::new(db),
			chunks_cache: HashMap::new(),
			metrics,
		})
	}

	#[cfg(test)]
	fn new_in_memory(inner: Arc<dyn KeyValueDB>, pruning_config: PruningConfig) -> Self {
		Self {
			pruning_config,
			inner,
			chunks_cache: HashMap::new(),
			metrics: Metrics(None),
		}
	}
}

#[tracing::instrument(skip(subsystem, ctx), fields(subsystem = LOG_TARGET))]
async fn run<Context>(mut subsystem: AvailabilityStoreSubsystem, mut ctx: Context)
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>,
{
	let mut next_pruning = Delay::new(PRUNING_INTERVAL).fuse();

	loop {
		let res = run_iteration(&mut ctx, &mut subsystem, &mut next_pruning).await;
		match res {
			Err(e) => {
				e.trace();

				if let Error::Subsystem(SubsystemError::Context(_)) = e {
					break;
				}
			}
			Ok(true) => {
				tracing::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
				break;
			},
			Ok(false) => continue,
		}
	}
}

#[tracing::instrument(level = "trace", skip(subsystem, ctx), fields(subsystem = LOG_TARGET))]
async fn run_iteration<Context>(
	ctx: &mut Context,
	subsystem: &mut AvailabilityStoreSubsystem,
	mut next_pruning: &mut future::Fuse<Delay>,
)
	-> Result<bool, Error>
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>,
{
	select! {
		incoming = ctx.recv().fuse() => {
			match incoming? {
				FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(true),
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate { activated, .. })
				) => {
					for (activated, _span) in activated.into_iter() {
						process_block_activated(ctx, subsystem, activated).await?;
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(hash, number)) => {
					process_block_finalized(
						ctx,
						&subsystem.inner,
						&subsystem.pruning_config,
						hash,
						number,
					).await?;
				}
				FromOverseer::Communication { msg } => {
					process_message(subsystem, msg)?;
				}
			}
		}
		_ = next_pruning => {
			// It's important to set the delay before calling `prune_all` because an error in `prune_all`
			// could lead to the delay not being set again. Then we would never prune anything anymore.
			*next_pruning = Delay::new(PRUNING_INTERVAL).fuse();
			prune_all(&subsystem.inner)?;
		}
	}

	Ok(false)
}

async fn process_block_activated(
	ctx: &mut impl SubsystemContext,
	subsystem: &mut AvailabilityStoreSubsystem,
	activated: Hash,
) -> Result<(), Error> {
	let now = SystemTime::now().duration_since(UNIX_EPOCH)?;

	let candidate_events = {
		let (tx, rx) = oneshot::channel();
		ctx.send_message(
			RuntimeApiMessage::Request(activated, RuntimeApiRequest::CandidateEvents(tx)).into()
		).await;

		rx.await??
	};

	let block_number = {
		let (tx, rx) = oneshot::channel();
		ctx.send_message(
			ChainApiMessage::BlockNumber(activated, tx).into()
		).await;

		match rx.await?? {
			None => return Ok(()),
			Some(n) => n,
		}
	};

	let n_validators = {
		let (tx, rx) = oneshot::channel();
		ctx.send_message(
			RuntimeApiMessage::Request(activated, RuntimeApiRequest::Validators(tx)).into()
		).await;

		rx.await??.len()
	};

	let mut tx = DBTransaction::new();

	for event in candidate_events {
		match event {
			CandidateEvent::CandidateBacked(receipt, head) => {
				note_block_backed(
					&subsystem.inner,
					&mut tx,
					&subsystem.pruning_config,
					now,
					n_validators,
					receipt,
				)?;
			}
			CandidateEvent::CandidateIncluded(receipt, head) => {
				note_block_included(
					&subsystem.inner,
					&mut tx,
					&subsystem.pruning_config,
					now,
					(block_number, activated),
					receipt,
				)?;
			}
			_ => {}
		}
	}

	subsystem.inner.write(tx)?;

	Ok(())
}

fn note_block_backed(
	db: &Arc<dyn KeyValueDB>,
	db_transaction: &mut DBTransaction,
	pruning_config: &PruningConfig,
	now: Duration,
	n_validators: usize,
	candidate: CandidateReceipt,
) -> Result<(), Error> {
	let candidate_hash = candidate.hash();

	if load_meta(db, &candidate_hash).is_none() {
		let meta = CandidateMeta {
			state: State::Unavailable(now.into()),
			data_available: false,
			chunks_stored: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
		};

		let prune_at = now + pruning_config.keep_unavailable_for;

		write_pruning_key(db_transaction, prune_at, &candidate_hash);
		write_meta(db_transaction, &candidate_hash, &meta);
	}

	Ok(())
}

fn note_block_included(
	db: &Arc<dyn KeyValueDB>,
	db_transaction: &mut DBTransaction,
	pruning_config:&PruningConfig,
	now: Duration,
	block: (BlockNumber, Hash),
	candidate: CandidateReceipt,
) -> Result<(), Error> {
	let candidate_hash = candidate.hash();

	let be_block = (BEBlockNumber(block.0), block.1);
	match load_meta(db, &candidate_hash) {
		None => {
			// This is alarming. We've observed a block being included without ever seeing it backed.
			// Warn and ignore.
			tracing::warn!(
				target: LOG_TARGET,
				"Candidate {}, included without being backed?",
				candidate_hash,
			);
		}
		Some(mut meta) => {
			meta.state = match meta.state {
				State::Unavailable(at) => {
					let at_d: Duration = at.into();
					let prune_at = at_d + pruning_config.keep_unavailable_for;
					delete_pruning_key(db_transaction, prune_at, &candidate_hash);

					State::Unfinalized(at, vec![be_block])
				}
				State::Unfinalized(at, mut within) => {
					if let Err(i) = within.binary_search(&be_block) {
						within.insert(i, be_block);
					}

					State::Unfinalized(at, within)
				}
				State::Finalized(at) => {
					// This should never happen as a candidate would have to be included after
					// finality.
					return Ok(())
				}
			};

			write_unfinalized_block_contains(db_transaction, block.0, &block.1, &candidate_hash);
			write_meta(db_transaction, &candidate_hash, &meta);
		}
	}

	Ok(())
}

async fn process_block_finalized(
	ctx: &mut impl SubsystemContext,
	db: &Arc<dyn KeyValueDB>,
	pruning_config: &PruningConfig,
	finalized_hash: Hash,
	finalized_number: BlockNumber,
) -> Result<(), Error> {
	let mut db_transaction = DBTransaction::new();
	let now = SystemTime::now().duration_since(UNIX_EPOCH)?;

	let last_finalized = load_last_finalized(db).unwrap_or(0);

	let (start_prefix, end_prefix) = finalized_block_range(last_finalized, finalized_number);

	let mut iter = db.iter_with_prefix(columns::META, &start_prefix)
		.take_while(|(k, _)| &k[..] < &end_prefix[..])
		.peekable();

	macro_rules! peek_num {
		() => {
			match iter.peek() {
				Some((k, _)) => decode_unfinalized_key(&k[..]).ok().map(|(b, _, _)| b),
				None => None
			}
		}
	}

	loop {
		let batch_num = match peek_num!() {
			None => break, // end of iterator.
			Some(n) => n,
		};

		let batch_finalized_hash = if batch_num == finalized_number {
			finalized_hash
		} else {
			let (tx, rx) = oneshot::channel();
			ctx.send_message(ChainApiMessage::FinalizedBlockHash(batch_num, tx).into()).await;

			match rx.await?? {
				None => {
					tracing::warn!(target: LOG_TARGET,
						"Availability store was informed that block #{} is finalized, \
						but chain API has no finalized hash.",
						batch_num,
					);

					break
				}
				Some(h) => h,
			}
		};

		// maps candidate hashes to true if finalized, false otherwise.
		let mut candidates = HashMap::new();

		// Load all candidates that were included at this height.
		loop {
			match peek_num!() {
				None => break, // end of iterator.
				Some(n) if n != batch_num => break, // end of batch.
				_ => {}
			}

			let (k, _v) = iter.next().expect("`peek` used to check non-empty; qed");
			let (_, block_hash, candidate_hash) = decode_unfinalized_key(&k[..])
				.expect("`peek_num` checks validity of key; qed");

			if block_hash == batch_finalized_hash {
				candidates.insert(candidate_hash, true);
			} else {
				candidates.entry(candidate_hash).or_insert(false);
			}
		}

		// Now that we've iterated over the entire batch at this finalized height,
		// update the meta.

		delete_unfinalized_height(&mut db_transaction, batch_num);

		for (candidate_hash, is_finalized) in candidates {
			let mut meta = match load_meta(db, &candidate_hash) {
				None => {
					tracing::warn!(
						target: LOG_TARGET,
						"Dangling candidate metadata for {}",
						candidate_hash,
					);

					continue;
				}
				Some(c) => c,
			};

			if is_finalized {
				// Clear everything else related to this block. We're finalized now!
				match meta.state {
					State::Finalized(_) => continue, // sanity
					State::Unavailable(at) => {
						delete_pruning_key(&mut db_transaction, at, &candidate_hash);
					}
					State::Unfinalized(_, blocks) => {
						for (block_num, block_hash) in blocks.iter().cloned() {
							// this exact height is all getting cleared out anyway.
							if block_num.0 != batch_num {
								delete_unfinalized_inclusion(
									&mut db_transaction,
									block_num.0,
									&block_hash,
									&candidate_hash,
								);
							}
						}
					}
				}

				meta.state = State::Finalized(now.into());

				// Write the meta and a pruning record.
				write_meta(&mut db_transaction, &candidate_hash, &meta);
				write_pruning_key(
					&mut db_transaction,
					now + pruning_config.keep_finalized_for,
					&candidate_hash,
				);
			} else {
				meta.state = match meta.state {
					State::Finalized(_) => continue, // sanity.
					State::Unavailable(_) => continue, // sanity.
					State::Unfinalized(at, mut blocks) => {
						// Clear out everything at this height.
						blocks.retain(|(n, _)| n.0 != batch_num);

						// If empty, we need to go back to being unavailable as we aren't
						// aware of any blocks this is included in.
						if blocks.is_empty() {
							let at_d: Duration = at.into();
							let prune_at = at_d + pruning_config.keep_unavailable_for;
							write_pruning_key(&mut db_transaction, prune_at, &candidate_hash);
							State::Unavailable(at)
						} else {
							State::Unfinalized(at, blocks)
						}
					}
				};

				// Update the meta entry.
				write_meta(&mut db_transaction, &candidate_hash, &meta)
			}
		}
	}

	write_last_finalized(&mut db_transaction, finalized_number);
	db.write(db_transaction)?;

	Ok(())
}

fn process_message(
	subsystem: &mut AvailabilityStoreSubsystem,
	msg: AvailabilityStoreMessage,
) -> Result<(), Error> {
	match msg {
		AvailabilityStoreMessage::QueryAvailableData(candidate, tx) => {
			let _ = tx.send(load_available_data(&subsystem.inner, &candidate));
		}
		AvailabilityStoreMessage::QueryDataAvailability(candidate, tx) => {
			let a = load_meta(&subsystem.inner, &candidate).map_or(false, |m| m.data_available);
			let _ = tx.send(a);
		}
		AvailabilityStoreMessage::QueryChunk(candidate, validator_index, tx) => {
			let _ = tx.send(load_chunk(&subsystem.inner, &candidate, validator_index));
		}
		AvailabilityStoreMessage::QueryChunkAvailability(candidate, validator_index, tx) => {
			let a = load_meta(&subsystem.inner, &candidate)
				.map_or(false, |m| *m.chunks_stored.get(validator_index as usize).unwrap_or(&false));
			let _ = tx.send(a);
		}
		AvailabilityStoreMessage::StoreChunk {
			candidate_hash,
			relay_parent,
			chunk,
			tx,
		} => {
			match store_chunk(&subsystem.inner, candidate_hash, chunk) {
				Ok(true) => {
					let _ = tx.send(Ok(()));
				}
				Ok(false) => {
					let _ = tx.send(Err(()));
				}
				Err(e) => {
					let _ = tx.send(Err(()));
					return Err(e)
				}
			}
		}
		AvailabilityStoreMessage::StoreAvailableData(candidate, our_index, n_validators, available_data, tx) => {
			let res = store_available_data(
				&subsystem.inner,
				&subsystem.pruning_config,
				candidate,
				n_validators as _,
				available_data,
			);

			match res {
				Ok(()) => {
					let _ = tx.send(Ok(()));
				}
				Err(e) => {
					let _ = tx.send(Err(()));
					return Err(e)
				}
			}
		}
	}

	Ok(())
}

// Ok(true) on success, Ok(false) on failure, and Err on internal error.
fn store_chunk(
	db: &Arc<dyn KeyValueDB>,
	candidate_hash: CandidateHash,
	chunk: ErasureChunk,
) -> Result<bool, Error> {
	let mut tx = DBTransaction::new();

	let mut meta = match load_meta(db, &candidate_hash) {
		Some(m) => m,
		None => return Ok(false), // we weren't informed of this candidate by import events.
	};

	match meta.chunks_stored.get(chunk.index as usize).map(|b| *b) {
		Some(true) => return Ok(true), // already stored.
		Some(false) => {
			meta.chunks_stored.set(chunk.index as usize, true);

			write_chunk(&mut tx, &candidate_hash, chunk.index, &chunk);
			write_meta(&mut tx, &candidate_hash, &meta);
		}
		None => return Ok(false), // out of bounds.
	}

	db.write(tx)?;
	Ok(true)
}

// Ok(true) on success, Ok(false) on failure, and Err on internal error.
fn store_available_data(
	db: &Arc<dyn KeyValueDB>,
	pruning_config: &PruningConfig,
	candidate_hash: CandidateHash,
	n_validators: usize,
	available_data: AvailableData,
) -> Result<(), Error> {
	let mut tx = DBTransaction::new();

	let mut meta = match load_meta(db, &candidate_hash) {
		Some(m) => {
			if m.data_available {
				return Ok(()); // already stored.
			}

			m
		},
		None => {
			let now = SystemTime::now().duration_since(UNIX_EPOCH)?;

			// Write a pruning record.
			write_pruning_key(&mut tx, now + pruning_config.keep_unavailable_for, &candidate_hash);

			CandidateMeta {
				state: State::Unavailable(now.into()),
				data_available: false,
				chunks_stored: BitVec::new(),
			}
		}
	};

	let chunks = erasure::obtain_chunks_v1(n_validators, &available_data)?;
	let branches = erasure::branches(chunks.as_ref());

	let erasure_chunks = chunks.iter()
		.zip(branches.map(|(proof, _)| proof))
		.enumerate()
		.map(|(index, (chunk, proof))| ErasureChunk {
			chunk: chunk.clone(),
			proof,
			index: index as u32,
		});

	for chunk in erasure_chunks {
		write_chunk(&mut tx, &candidate_hash, chunk.index, &chunk);
	}

	meta.data_available = true;
	meta.chunks_stored = bitvec::bitvec![BitOrderLsb0, u8; 1; n_validators];

	write_meta(&mut tx, &candidate_hash, &meta);

	db.write(tx)?;
	Ok(())
}

fn prune_all(db: &Arc<dyn KeyValueDB>) -> Result<(), Error> {
	unimplemented!()
}

#[derive(Clone)]
struct MetricsInner {
	received_availability_chunks_total: prometheus::Counter<prometheus::U64>,
	chunk_pruning_records_total: prometheus::Gauge<prometheus::U64>,
	block_pruning_records_total: prometheus::Gauge<prometheus::U64>,
	prune_povs: prometheus::Histogram,
	prune_chunks: prometheus::Histogram,
	process_block_finalized: prometheus::Histogram,
	block_activated: prometheus::Histogram,
	process_message: prometheus::Histogram,
	store_available_data: prometheus::Histogram,
	store_chunks: prometheus::Histogram,
	get_chunk: prometheus::Histogram,
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

	fn chunk_pruning_records_size(&self, count: usize) {
		if let Some(metrics) = &self.0 {
			use core::convert::TryFrom as _;
			let total = u64::try_from(count).unwrap_or_default();
			metrics.chunk_pruning_records_total.set(total);
		}
	}

	fn block_pruning_records_size(&self, count: usize) {
		if let Some(metrics) = &self.0 {
			use core::convert::TryFrom as _;
			let total = u64::try_from(count).unwrap_or_default();
			metrics.block_pruning_records_total.set(total);
		}
	}

	/// Provide a timer for `prune_povs` which observes on drop.
	fn time_prune_povs(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.prune_povs.start_timer())
	}

	/// Provide a timer for `prune_chunks` which observes on drop.
	fn time_prune_chunks(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.prune_chunks.start_timer())
	}

	/// Provide a timer for `process_block_finalized` which observes on drop.
	fn time_process_block_finalized(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_block_finalized.start_timer())
	}

	/// Provide a timer for `block_activated` which observes on drop.
	fn time_block_activated(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.block_activated.start_timer())
	}

	/// Provide a timer for `process_message` which observes on drop.
	fn time_process_message(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.process_message.start_timer())
	}

	/// Provide a timer for `store_available_data` which observes on drop.
	fn time_store_available_data(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.store_available_data.start_timer())
	}

	/// Provide a timer for `store_chunk` which observes on drop.
	fn time_store_chunks(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.store_chunks.start_timer())
	}

	/// Provide a timer for `get_chunk` which observes on drop.
	fn time_get_chunk(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.get_chunk.start_timer())
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
			chunk_pruning_records_total: prometheus::register(
				prometheus::Gauge::new(
					"parachain_chunk_pruning_records_total",
					"Number of chunk pruning records kept by the storage.",
				)?,
				registry,
			)?,
			block_pruning_records_total: prometheus::register(
				prometheus::Gauge::new(
					"parachain_block_pruning_records_total",
					"Number of block pruning records kept by the storage.",
				)?,
				registry,
			)?,
			prune_povs: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_av_store_prune_povs",
						"Time spent within `av_store::prune_povs`",
					)
				)?,
				registry,
			)?,
			prune_chunks: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_av_store_prune_chunks",
						"Time spent within `av_store::prune_chunks`",
					)
				)?,
				registry,
			)?,
			process_block_finalized: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_av_store_process_block_finalized",
						"Time spent within `av_store::block_finalized`",
					)
				)?,
				registry,
			)?,
			block_activated: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_av_store_block_activated",
						"Time spent within `av_store::block_activated`",
					)
				)?,
				registry,
			)?,
			process_message: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_av_store_process_message",
						"Time spent within `av_store::process_message`",
					)
				)?,
				registry,
			)?,
			store_available_data: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_av_store_store_available_data",
						"Time spent within `av_store::store_available_data`",
					)
				)?,
				registry,
			)?,
			store_chunks: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_av_store_store_chunks",
						"Time spent within `av_store::store_chunks`",
					)
				)?,
				registry,
			)?,
			get_chunk: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_av_store_get_chunk",
						"Time spent within `av_store::get_chunk`",
					)
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}
