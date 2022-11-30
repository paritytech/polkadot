// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Implements the Chain Selection Subsystem.

use polkadot_node_primitives::BlockWeight;
use polkadot_node_subsystem::{
	errors::ChainApiError,
	messages::{ChainApiMessage, ChainSelectionMessage},
	overseer::{self, SubsystemSender},
	FromOrchestra, OverseerSignal, SpawnedSubsystem, SubsystemError,
};
use polkadot_node_subsystem_util::database::Database;
use polkadot_primitives::v2::{BlockNumber, ConsensusLog, Hash, Header};

use futures::{channel::oneshot, future::Either, prelude::*};
use parity_scale_codec::Error as CodecError;

use std::{
	sync::Arc,
	time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::backend::{Backend, BackendWriteOp, OverlayedBackend};

mod backend;
mod db_backend;
mod tree;

#[cfg(test)]
mod tests;

const LOG_TARGET: &str = "parachain::chain-selection";
/// Timestamp based on the 1 Jan 1970 UNIX base, which is persistent across node restarts and OS reboots.
type Timestamp = u64;

// If a block isn't approved in 120 seconds, nodes will abandon it
// and begin building on another chain.
const STAGNANT_TIMEOUT: Timestamp = 120;
// Delay prunning of the stagnant keys in prune only mode by 25 hours to avoid interception with the finality
const STAGNANT_PRUNE_DELAY: Timestamp = 25 * 60 * 60;
// Maximum number of stagnant entries cleaned during one `STAGNANT_TIMEOUT` iteration
const MAX_STAGNANT_ENTRIES: usize = 1000;

#[derive(Debug, Clone)]
enum Approval {
	// Approved
	Approved,
	// Unapproved but not stagnant
	Unapproved,
	// Unapproved and stagnant.
	Stagnant,
}

impl Approval {
	fn is_stagnant(&self) -> bool {
		matches!(*self, Approval::Stagnant)
	}
}

#[derive(Debug, Clone)]
struct ViabilityCriteria {
	// Whether this block has been explicitly reverted by one of its descendants.
	explicitly_reverted: bool,
	// The approval state of this block specifically.
	approval: Approval,
	// The earliest unviable ancestor - the hash of the earliest unfinalized
	// block in the ancestry which is explicitly reverted or stagnant.
	earliest_unviable_ancestor: Option<Hash>,
}

impl ViabilityCriteria {
	fn is_viable(&self) -> bool {
		self.is_parent_viable() && self.is_explicitly_viable()
	}

	// Whether the current block is explicitly viable.
	// That is, whether the current block is neither reverted nor stagnant.
	fn is_explicitly_viable(&self) -> bool {
		!self.explicitly_reverted && !self.approval.is_stagnant()
	}

	// Whether the parent is viable. This assumes that the parent
	// descends from the finalized chain.
	fn is_parent_viable(&self) -> bool {
		self.earliest_unviable_ancestor.is_none()
	}
}

// Light entries describing leaves of the chain.
//
// These are ordered first by weight and then by block number.
#[derive(Debug, Clone, PartialEq)]
struct LeafEntry {
	weight: BlockWeight,
	block_number: BlockNumber,
	block_hash: Hash,
}

impl PartialOrd for LeafEntry {
	fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
		let ord = self.weight.cmp(&other.weight).then(self.block_number.cmp(&other.block_number));

		if !matches!(ord, std::cmp::Ordering::Equal) {
			Some(ord)
		} else {
			None
		}
	}
}

#[derive(Debug, Default, Clone)]
struct LeafEntrySet {
	inner: Vec<LeafEntry>,
}

impl LeafEntrySet {
	fn remove(&mut self, hash: &Hash) -> bool {
		match self.inner.iter().position(|e| &e.block_hash == hash) {
			None => false,
			Some(i) => {
				self.inner.remove(i);
				true
			},
		}
	}

	fn insert(&mut self, new: LeafEntry) {
		let mut pos = None;
		for (i, e) in self.inner.iter().enumerate() {
			if e == &new {
				return
			}
			if e < &new {
				pos = Some(i);
				break
			}
		}

		match pos {
			None => self.inner.push(new),
			Some(i) => self.inner.insert(i, new),
		}
	}

	fn into_hashes_descending(self) -> impl Iterator<Item = Hash> {
		self.inner.into_iter().map(|e| e.block_hash)
	}
}

#[derive(Debug, Clone)]
struct BlockEntry {
	block_hash: Hash,
	block_number: BlockNumber,
	parent_hash: Hash,
	children: Vec<Hash>,
	viability: ViabilityCriteria,
	weight: BlockWeight,
}

impl BlockEntry {
	fn leaf_entry(&self) -> LeafEntry {
		LeafEntry {
			block_hash: self.block_hash,
			block_number: self.block_number,
			weight: self.weight,
		}
	}

	fn non_viable_ancestor_for_child(&self) -> Option<Hash> {
		if self.viability.is_viable() {
			None
		} else {
			self.viability.earliest_unviable_ancestor.or(Some(self.block_hash))
		}
	}
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),

	#[error(transparent)]
	Codec(#[from] CodecError),
}

impl Error {
	fn trace(&self) {
		match self {
			// don't spam the log with spurious errors
			Self::Oneshot(_) => gum::debug!(target: LOG_TARGET, err = ?self),
			// it's worth reporting otherwise
			_ => gum::warn!(target: LOG_TARGET, err = ?self),
		}
	}
}

/// A clock used for fetching the current timestamp.
pub trait Clock {
	/// Get the current timestamp.
	fn timestamp_now(&self) -> Timestamp;
}

struct SystemClock;

impl Clock for SystemClock {
	fn timestamp_now(&self) -> Timestamp {
		// `SystemTime` is notoriously non-monotonic, so our timers might not work
		// exactly as expected. Regardless, stagnation is detected on the order of minutes,
		// and slippage of a few seconds in either direction won't cause any major harm.
		//
		// The exact time that a block becomes stagnant in the local node is always expected
		// to differ from other nodes due to network asynchrony and delays in block propagation.
		// Non-monotonicity exarcerbates that somewhat, but not meaningfully.

		match SystemTime::now().duration_since(UNIX_EPOCH) {
			Ok(d) => d.as_secs(),
			Err(e) => {
				gum::warn!(
					target: LOG_TARGET,
					err = ?e,
					"Current time is before unix epoch. Validation will not work correctly."
				);

				0
			},
		}
	}
}

/// The interval, in seconds to check for stagnant blocks.
#[derive(Debug, Clone)]
pub struct StagnantCheckInterval(Option<Duration>);

impl Default for StagnantCheckInterval {
	fn default() -> Self {
		// 5 seconds is a reasonable balance between avoiding DB reads and
		// ensuring validators are generally in agreement on stagnant blocks.
		//
		// Assuming a network delay of D, the longest difference in view possible
		// between 2 validators is D + 5s.
		const DEFAULT_STAGNANT_CHECK_INTERVAL: Duration = Duration::from_secs(5);

		StagnantCheckInterval(Some(DEFAULT_STAGNANT_CHECK_INTERVAL))
	}
}

impl StagnantCheckInterval {
	/// Create a new stagnant-check interval wrapping the given duration.
	pub fn new(interval: Duration) -> Self {
		StagnantCheckInterval(Some(interval))
	}

	/// Create a `StagnantCheckInterval` which never triggers.
	pub fn never() -> Self {
		StagnantCheckInterval(None)
	}

	fn timeout_stream(&self) -> impl Stream<Item = ()> {
		match self.0 {
			Some(interval) => Either::Left({
				let mut delay = futures_timer::Delay::new(interval);

				futures::stream::poll_fn(move |cx| {
					let poll = delay.poll_unpin(cx);
					if poll.is_ready() {
						delay.reset(interval)
					}

					poll.map(Some)
				})
			}),
			None => Either::Right(futures::stream::pending()),
		}
	}
}

/// Mode of the stagnant check operations: check and prune or prune only
#[derive(Debug, Clone)]
pub enum StagnantCheckMode {
	CheckAndPrune,
	PruneOnly,
}

impl Default for StagnantCheckMode {
	fn default() -> Self {
		StagnantCheckMode::PruneOnly
	}
}

/// Configuration for the chain selection subsystem.
#[derive(Debug, Clone)]
pub struct Config {
	/// The column in the database that the storage should use.
	pub col_data: u32,
	/// How often to check for stagnant blocks.
	pub stagnant_check_interval: StagnantCheckInterval,
	/// Mode of stagnant checks
	pub stagnant_check_mode: StagnantCheckMode,
}

/// The chain selection subsystem.
pub struct ChainSelectionSubsystem {
	config: Config,
	db: Arc<dyn Database>,
}

impl ChainSelectionSubsystem {
	/// Create a new instance of the subsystem with the given config
	/// and key-value store.
	pub fn new(config: Config, db: Arc<dyn Database>) -> Self {
		ChainSelectionSubsystem { config, db }
	}

	/// Revert to the block corresponding to the specified `hash`.
	/// The operation is not allowed for blocks older than the last finalized one.
	pub fn revert_to(&self, hash: Hash) -> Result<(), Error> {
		let config = db_backend::v1::Config { col_data: self.config.col_data };
		let mut backend = db_backend::v1::DbBackend::new(self.db.clone(), config);

		let ops = tree::revert_to(&backend, hash)?.into_write_ops();

		backend.write(ops)
	}
}

#[overseer::subsystem(ChainSelection, error = SubsystemError, prefix = self::overseer)]
impl<Context> ChainSelectionSubsystem {
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let backend = db_backend::v1::DbBackend::new(
			self.db,
			db_backend::v1::Config { col_data: self.config.col_data },
		);

		SpawnedSubsystem {
			future: run(
				ctx,
				backend,
				self.config.stagnant_check_interval,
				self.config.stagnant_check_mode,
				Box::new(SystemClock),
			)
			.map(Ok)
			.boxed(),
			name: "chain-selection-subsystem",
		}
	}
}

#[overseer::contextbounds(ChainSelection, prefix = self::overseer)]
async fn run<Context, B>(
	mut ctx: Context,
	mut backend: B,
	stagnant_check_interval: StagnantCheckInterval,
	stagnant_check_mode: StagnantCheckMode,
	clock: Box<dyn Clock + Send + Sync>,
) where
	B: Backend,
{
	#![allow(clippy::all)]
	loop {
		let res = run_until_error(
			&mut ctx,
			&mut backend,
			&stagnant_check_interval,
			&stagnant_check_mode,
			&*clock,
		)
		.await;
		match res {
			Err(e) => {
				e.trace();
				// All errors are considered fatal right now:
				break
			},
			Ok(()) => {
				gum::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
				break
			},
		}
	}
}

// Run the subsystem until an error is encountered or a `conclude` signal is received.
// Most errors are non-fatal and should lead to another call to this function.
//
// A return value of `Ok` indicates that an exit should be made, while non-fatal errors
// lead to another call to this function.
#[overseer::contextbounds(ChainSelection, prefix = self::overseer)]
async fn run_until_error<Context, B>(
	ctx: &mut Context,
	backend: &mut B,
	stagnant_check_interval: &StagnantCheckInterval,
	stagnant_check_mode: &StagnantCheckMode,
	clock: &(dyn Clock + Sync),
) -> Result<(), Error>
where
	B: Backend,
{
	let mut stagnant_check_stream = stagnant_check_interval.timeout_stream();
	loop {
		futures::select! {
			msg = ctx.recv().fuse() => {
				let msg = msg?;
				match msg {
					FromOrchestra::Signal(OverseerSignal::Conclude) => {
						return Ok(())
					}
					FromOrchestra::Signal(OverseerSignal::ActiveLeaves(update)) => {
						if let Some(leaf) = update.activated {
							let write_ops = handle_active_leaf(
								ctx.sender(),
								&*backend,
								clock.timestamp_now() + STAGNANT_TIMEOUT,
								leaf.hash,
							).await?;

							backend.write(write_ops)?;
						}
					}
					FromOrchestra::Signal(OverseerSignal::BlockFinalized(h, n)) => {
						handle_finalized_block(backend, h, n)?
					}
					FromOrchestra::Communication { msg } => match msg {
						ChainSelectionMessage::Approved(hash) => {
							handle_approved_block(backend, hash)?
						}
						ChainSelectionMessage::Leaves(tx) => {
							let leaves = load_leaves(ctx.sender(), &*backend).await?;
							let _ = tx.send(leaves);
						}
						ChainSelectionMessage::BestLeafContaining(required, tx) => {
							let best_containing = backend::find_best_leaf_containing(
								&*backend,
								required,
							)?;

							// note - this may be none if the finalized block is
							// a leaf. this is fine according to the expected usage of the
							// function. `None` responses should just `unwrap_or(required)`,
							// so if the required block is the finalized block, then voilá.

							let _ = tx.send(best_containing);
						}
					}
				}
			}
			_ = stagnant_check_stream.next().fuse() => {
				match stagnant_check_mode {
					StagnantCheckMode::CheckAndPrune => detect_stagnant(backend, clock.timestamp_now(), MAX_STAGNANT_ENTRIES),
					StagnantCheckMode::PruneOnly => {
						let now_timestamp = clock.timestamp_now();
						prune_only_stagnant(backend, now_timestamp - STAGNANT_PRUNE_DELAY, MAX_STAGNANT_ENTRIES)
					},
				}?;
			}
		}
	}
}

async fn fetch_finalized(
	sender: &mut impl SubsystemSender<ChainApiMessage>,
) -> Result<Option<(Hash, BlockNumber)>, Error> {
	let (number_tx, number_rx) = oneshot::channel();

	sender.send_message(ChainApiMessage::FinalizedBlockNumber(number_tx)).await;

	let number = match number_rx.await? {
		Ok(number) => number,
		Err(err) => {
			gum::warn!(target: LOG_TARGET, ?err, "Fetching finalized number failed");
			return Ok(None)
		},
	};

	let (hash_tx, hash_rx) = oneshot::channel();

	sender.send_message(ChainApiMessage::FinalizedBlockHash(number, hash_tx)).await;

	match hash_rx.await? {
		Err(err) => {
			gum::warn!(target: LOG_TARGET, number, ?err, "Fetching finalized block number failed");
			Ok(None)
		},
		Ok(None) => {
			gum::warn!(target: LOG_TARGET, number, "Missing hash for finalized block number");
			Ok(None)
		},
		Ok(Some(h)) => Ok(Some((h, number))),
	}
}

async fn fetch_header(
	sender: &mut impl SubsystemSender<ChainApiMessage>,
	hash: Hash,
) -> Result<Option<Header>, Error> {
	let (tx, rx) = oneshot::channel();
	sender.send_message(ChainApiMessage::BlockHeader(hash, tx)).await;

	Ok(rx.await?.unwrap_or_else(|err| {
		gum::warn!(target: LOG_TARGET, ?hash, ?err, "Missing hash for finalized block number");
		None
	}))
}

async fn fetch_block_weight(
	sender: &mut impl overseer::SubsystemSender<ChainApiMessage>,
	hash: Hash,
) -> Result<Option<BlockWeight>, Error> {
	let (tx, rx) = oneshot::channel();
	sender.send_message(ChainApiMessage::BlockWeight(hash, tx)).await;

	let res = rx.await?;

	Ok(res.unwrap_or_else(|err| {
		gum::warn!(target: LOG_TARGET, ?hash, ?err, "Missing hash for finalized block number");
		None
	}))
}

// Handle a new active leaf.
async fn handle_active_leaf(
	sender: &mut impl overseer::ChainSelectionSenderTrait,
	backend: &impl Backend,
	stagnant_at: Timestamp,
	hash: Hash,
) -> Result<Vec<BackendWriteOp>, Error> {
	let lower_bound = match backend.load_first_block_number()? {
		Some(l) => {
			// We want to iterate back to finalized, and first block number
			// is assumed to be 1 above finalized - the implicit root of the
			// tree.
			l.saturating_sub(1)
		},
		None => fetch_finalized(sender).await?.map_or(1, |(_, n)| n),
	};

	let header = match fetch_header(sender, hash).await? {
		None => {
			gum::warn!(target: LOG_TARGET, ?hash, "Missing header for new head");
			return Ok(Vec::new())
		},
		Some(h) => h,
	};

	let new_blocks = polkadot_node_subsystem_util::determine_new_blocks(
		sender,
		|h| backend.load_block_entry(h).map(|b| b.is_some()),
		hash,
		&header,
		lower_bound,
	)
	.await?;

	let mut overlay = OverlayedBackend::new(backend);

	// determine_new_blocks gives blocks in descending order.
	// for this, we want ascending order.
	for (hash, header) in new_blocks.into_iter().rev() {
		let weight = match fetch_block_weight(sender, hash).await? {
			None => {
				gum::warn!(
					target: LOG_TARGET,
					?hash,
					"Missing block weight for new head. Skipping chain.",
				);

				// If we don't know the weight, we can't import the block.
				// And none of its descendants either.
				break
			},
			Some(w) => w,
		};

		let reversion_logs = extract_reversion_logs(&header);
		tree::import_block(
			&mut overlay,
			hash,
			header.number,
			header.parent_hash,
			reversion_logs,
			weight,
			stagnant_at,
		)?;
	}

	Ok(overlay.into_write_ops().collect())
}

// Extract all reversion logs from a header in ascending order.
//
// Ignores logs with number >= the block header number.
fn extract_reversion_logs(header: &Header) -> Vec<BlockNumber> {
	let number = header.number;
	let mut logs = header
		.digest
		.logs()
		.iter()
		.enumerate()
		.filter_map(|(i, d)| match ConsensusLog::from_digest_item(d) {
			Err(e) => {
				gum::warn!(
					target: LOG_TARGET,
					err = ?e,
					index = i,
					block_hash = ?header.hash(),
					"Digest item failed to encode"
				);

				None
			},
			Ok(Some(ConsensusLog::Revert(b))) if b < number => Some(b),
			Ok(Some(ConsensusLog::Revert(b))) => {
				gum::warn!(
					target: LOG_TARGET,
					revert_target = b,
					block_number = number,
					block_hash = ?header.hash(),
					"Block issued invalid revert digest targeting itself or future"
				);

				None
			},
			Ok(_) => None,
		})
		.collect::<Vec<_>>();

	logs.sort();

	logs
}

/// Handle a finalized block event.
fn handle_finalized_block(
	backend: &mut impl Backend,
	finalized_hash: Hash,
	finalized_number: BlockNumber,
) -> Result<(), Error> {
	let ops = tree::finalize_block(&*backend, finalized_hash, finalized_number)?.into_write_ops();

	backend.write(ops)
}

// Handle an approved block event.
fn handle_approved_block(backend: &mut impl Backend, approved_block: Hash) -> Result<(), Error> {
	let ops = {
		let mut overlay = OverlayedBackend::new(&*backend);

		tree::approve_block(&mut overlay, approved_block)?;

		overlay.into_write_ops()
	};

	backend.write(ops)
}

fn detect_stagnant(
	backend: &mut impl Backend,
	now: Timestamp,
	max_elements: usize,
) -> Result<(), Error> {
	let ops = {
		let overlay = tree::detect_stagnant(&*backend, now, max_elements)?;

		overlay.into_write_ops()
	};

	backend.write(ops)
}

fn prune_only_stagnant(
	backend: &mut impl Backend,
	up_to: Timestamp,
	max_elements: usize,
) -> Result<(), Error> {
	let ops = {
		let overlay = tree::prune_only_stagnant(&*backend, up_to, max_elements)?;

		overlay.into_write_ops()
	};

	backend.write(ops)
}

// Load the leaves from the backend. If there are no leaves, then return
// the finalized block.
async fn load_leaves(
	sender: &mut impl overseer::SubsystemSender<ChainApiMessage>,
	backend: &impl Backend,
) -> Result<Vec<Hash>, Error> {
	let leaves: Vec<_> = backend.load_leaves()?.into_hashes_descending().collect();

	if leaves.is_empty() {
		Ok(fetch_finalized(sender).await?.map_or(Vec::new(), |(h, _)| vec![h]))
	} else {
		Ok(leaves)
	}
}
