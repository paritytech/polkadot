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

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
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

fn load_meta(
	db: &Arc<dyn KeyValueDB>,
	hash: &CandidateHash,
) -> Option<CandidateMeta> {
	let key = (META_PREFIX, hash).encode();

	query_inner(db, columns::META, &key)
}

fn delete_unfinalized_block(
	tx: &mut DBTransaction,
	block_number: BlockNumber,
) {
	let prefix = (UNFINALIZED_PREFIX, BEBlockNumber(block_number)).encode();
	tx.delete_prefix(columns::META, &prefix);
}

fn delete_pruning_key(tx: &mut DBTransaction, t: Duration, h: &CandidateHash) {
	let key = (PRUNE_BY_TIME_PREFIX, BETimestamp::from(t), h).encode();
	tx.delete(columns::META, &key);
}

fn write_pruning_key(tx: &mut DBTransaction, t: Duration, h: &CandidateHash) {
	let key = (PRUNE_BY_TIME_PREFIX, BETimestamp::from(t), h).encode();
	tx.put(columns::META, &key, TOMBSTONE_VALUE);
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
	loop {
		let res = run_iteration(&mut subsystem, &mut ctx).await;
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
async fn run_iteration<Context>(subsystem: &mut AvailabilityStoreSubsystem, ctx: &mut Context)
	-> Result<bool, Error>
where
	Context: SubsystemContext<Message=AvailabilityStoreMessage>,
{
	let mut next_pruning = Delay::new(PRUNING_INTERVAL).fuse();

	select! {
		incoming = ctx.recv().fuse() => {
			match incoming? {
				FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(true),
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate { activated, .. })
				) => {
					for (activated, _span) in activated.into_iter() {
						// TODO [now]
						// process_block_activated(ctx, subsystem, activated).await?;
					}
				}
				FromOverseer::Signal(OverseerSignal::BlockFinalized(_hash, number)) => {
					// TODO [now]
					// process_block_finalized(subsystem, &subsystem.inner, number).await?;
				}
				FromOverseer::Communication { msg } => {
					// TODO [now]
					// process_message(subsystem, ctx, msg).await?;
				}
			}
		}
		_ = next_pruning => {
			// TODO [now]: pruning.
			next_pruning = Delay::new(PRUNING_INTERVAL).fuse();
		}
	}

	Ok(false)
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
