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

//! Version 1 of the DB schema.

use kvdb::{DBTransaction, KeyValueDB};
use polkadot_node_primitives::approval::{DelayTranche, AssignmentCert};
use polkadot_primitives::v1::{
	ValidatorIndex, GroupIndex, CandidateReceipt, SessionIndex, CoreIndex,
	BlockNumber, Hash, CandidateHash, ValidatorSignature,
};
use sp_consensus_slots::Slot;
use parity_scale_codec::{Encode, Decode};

use std::collections::{BTreeMap, HashMap};
use std::collections::hash_map::Entry;
use bitvec::{vec::BitVec, order::Lsb0 as BitOrderLsb0};

#[cfg(test)]
pub mod tests;

// slot_duration * 2 + DelayTranche gives the number of delay tranches since the
// unix epoch.
#[derive(Encode, Decode, Clone, Copy, Debug, PartialEq)]
pub struct Tick(u64);

pub type Bitfield = BitVec<BitOrderLsb0, u8>;

const STORED_BLOCKS_KEY: &[u8] = b"Approvals_StoredBlocks";

/// The database config.
#[derive(Debug, Clone, Copy)]
pub struct Config {
	/// The column family in the database where data is stored.
	pub col_data: u32,
}

/// Details pertaining to our assignment on a block.
#[derive(Encode, Decode, Debug, Clone, PartialEq)]
pub struct OurAssignment {
	pub cert: AssignmentCert,
	pub tranche: DelayTranche,
	pub validator_index: ValidatorIndex,
	// Whether the assignment has been triggered already.
	pub triggered: bool,
}

/// Metadata regarding a specific tranche of assignments for a specific candidate.
#[derive(Encode, Decode, Debug, Clone, PartialEq)]
pub struct TrancheEntry {
	pub tranche: DelayTranche,
	// Assigned validators, and the instant we received their assignment, rounded
	// to the nearest tick.
	pub assignments: Vec<(ValidatorIndex, Tick)>,
}

/// Metadata regarding approval of a particular candidate within the context of some
/// particular block.
#[derive(Encode, Decode, Debug, Clone, PartialEq)]
pub struct ApprovalEntry {
	pub tranches: Vec<TrancheEntry>,
	pub backing_group: GroupIndex,
	pub our_assignment: Option<OurAssignment>,
	pub our_approval_sig: Option<ValidatorSignature>,
	// `n_validators` bits.
	pub assignments: Bitfield,
	pub approved: bool,
}

/// Metadata regarding approval of a particular candidate.
#[derive(Encode, Decode, Debug, Clone, PartialEq)]
pub struct CandidateEntry {
	pub candidate: CandidateReceipt,
	pub session: SessionIndex,
	// Assignments are based on blocks, so we need to track assignments separately
	// based on the block we are looking at.
	pub block_assignments: BTreeMap<Hash, ApprovalEntry>,
	pub approvals: Bitfield,
}

/// Metadata regarding approval of a particular block, by way of approval of the
/// candidates contained within it.
#[derive(Encode, Decode, Debug, Clone, PartialEq)]
pub struct BlockEntry {
	pub block_hash: Hash,
	pub block_number: BlockNumber,
	pub parent_hash: Hash,
	pub session: SessionIndex,
	pub slot: Slot,
	/// Random bytes derived from the VRF submitted within the block by the block
	/// author as a credential and used as input to approval assignment criteria.
	pub relay_vrf_story: [u8; 32],
	// The candidates included as-of this block and the index of the core they are
	// leaving. Sorted ascending by core index.
	pub candidates: Vec<(CoreIndex, CandidateHash)>,
	// A bitfield where the i'th bit corresponds to the i'th candidate in `candidates`.
	// The i'th bit is `true` iff the candidate has been approved in the context of this
	// block. The block can be considered approved if the bitfield has all bits set to `true`.
	pub approved_bitfield: Bitfield,
	pub children: Vec<Hash>,
}

/// A range from earliest..last block number stored within the DB.
#[derive(Encode, Decode, Debug, Clone, PartialEq)]
pub struct StoredBlockRange(BlockNumber, BlockNumber);

impl From<crate::Tick> for Tick {
	fn from(tick: crate::Tick) -> Tick {
		Tick(tick)
	}
}

impl From<Tick> for crate::Tick {
	fn from(tick: Tick) -> crate::Tick {
		tick.0
	}
}

/// Errors while accessing things from the DB.
#[derive(Debug, derive_more::From, derive_more::Display)]
pub enum Error {
	Io(std::io::Error),
	InvalidDecoding(parity_scale_codec::Error),
}

impl std::error::Error for Error {}

/// Result alias for DB errors.
pub type Result<T> = std::result::Result<T, Error>;

/// Canonicalize some particular block, pruning everything before it and
/// pruning any competing branches at the same height.
pub(crate) fn canonicalize(
	store: &dyn KeyValueDB,
	config: &Config,
	canon_number: BlockNumber,
	canon_hash: Hash,
)
	-> Result<()>
{
	let range = match load_stored_blocks(store, config)? {
		None => return Ok(()),
		Some(range) => if range.0 >= canon_number {
			return Ok(())
		} else {
			range
		},
	};

	let mut transaction = DBTransaction::new();

	// Storing all candidates in memory is potentially heavy, but should be fine
	// as long as finality doesn't stall for a long while. We could optimize this
	// by keeping only the metadata about which blocks reference each candidate.
	let mut visited_candidates = HashMap::new();

	// All the block heights we visited but didn't necessarily delete everything from.
	let mut visited_heights = HashMap::new();

	let visit_and_remove_block_entry = |
		block_hash: Hash,
		transaction: &mut DBTransaction,
		visited_candidates: &mut HashMap<CandidateHash, CandidateEntry>,
	| -> Result<Vec<Hash>> {
		let block_entry = match load_block_entry(store, config,  &block_hash)? {
			None => return Ok(Vec::new()),
			Some(b) => b,
		};

		transaction.delete(config.col_data, &block_entry_key(&block_hash)[..]);
		for &(_, ref candidate_hash) in &block_entry.candidates {
			let candidate = match visited_candidates.entry(*candidate_hash) {
				Entry::Occupied(e) => e.into_mut(),
				Entry::Vacant(e) => {
					e.insert(match load_candidate_entry(store, config, candidate_hash)? {
						None => continue, // Should not happen except for corrupt DB
						Some(c) => c,
					})
				}
			};

			candidate.block_assignments.remove(&block_hash);
		}

		Ok(block_entry.children)
	};

	// First visit everything before the height.
	for i in range.0..canon_number {
		let at_height = load_blocks_at_height(store, config, i)?;
		transaction.delete(config.col_data, &blocks_at_height_key(i)[..]);

		for b in at_height {
			let _ = visit_and_remove_block_entry(
				b,
				&mut transaction,
				&mut visited_candidates,
			)?;
		}
	}

	// Then visit everything at the height.
	let pruned_branches = {
		let at_height = load_blocks_at_height(store, config, canon_number)?;
		transaction.delete(config.col_data, &blocks_at_height_key(canon_number));

		// Note that while there may be branches descending from blocks at earlier heights,
		// we have already covered them by removing everything at earlier heights.
		let mut pruned_branches = Vec::new();

		for b in at_height {
			let children = visit_and_remove_block_entry(
				b,
				&mut transaction,
				&mut visited_candidates,
			)?;

			if b != canon_hash {
				pruned_branches.extend(children);
			}
		}

		pruned_branches
	};

	// Follow all children of non-canonicalized blocks.
	{
		let mut frontier: Vec<_> = pruned_branches.into_iter().map(|h| (canon_number + 1, h)).collect();
		while let Some((height, next_child)) = frontier.pop() {
			let children = visit_and_remove_block_entry(
				next_child,
				&mut transaction,
				&mut visited_candidates,
			)?;

			// extend the frontier of branches to include the given height.
			frontier.extend(children.into_iter().map(|h| (height + 1, h)));

			// visit the at-height key for this deleted block's height.
			let at_height = match visited_heights.entry(height) {
				Entry::Occupied(e) => e.into_mut(),
				Entry::Vacant(e) => e.insert(load_blocks_at_height(store, config, height)?),
			};

			if let Some(i) = at_height.iter().position(|x| x == &next_child) {
				at_height.remove(i);
			}
		}
	}

	// Update all `CandidateEntry`s, deleting all those which now have empty `block_assignments`.
	for (candidate_hash, candidate) in visited_candidates {
		if candidate.block_assignments.is_empty() {
			transaction.delete(config.col_data, &candidate_entry_key(&candidate_hash)[..]);
		} else {
			transaction.put_vec(
				config.col_data,
				&candidate_entry_key(&candidate_hash)[..],
				candidate.encode(),
			);
		}
	}

	// Update all blocks-at-height keys, deleting all those which now have empty `block_assignments`.
	for (h, at) in visited_heights {
		if at.is_empty() {
			transaction.delete(config.col_data, &blocks_at_height_key(h)[..]);
		} else {
			transaction.put_vec(config.col_data, &blocks_at_height_key(h), at.encode());
		}
	}

	// due to the fork pruning, this range actually might go too far above where our actual highest block is,
	// if a relatively short fork is canonicalized.
	let new_range = StoredBlockRange(
		canon_number + 1,
		std::cmp::max(range.1, canon_number + 2),
	).encode();

	transaction.put_vec(config.col_data, &STORED_BLOCKS_KEY[..], new_range);

	// Update the values on-disk.
	store.write(transaction).map_err(Into::into)
}

fn load_decode<D: Decode>(store: &dyn KeyValueDB, col_data: u32, key: &[u8])
	-> Result<Option<D>>
{
	match store.get(col_data, key)? {
		None => Ok(None),
		Some(raw) => D::decode(&mut &raw[..])
			.map(Some)
			.map_err(Into::into),
	}
}

/// Information about a new candidate necessary to instantiate the requisite
/// candidate and approval entries.
#[derive(Clone)]
pub(crate) struct NewCandidateInfo {
	pub candidate: CandidateReceipt,
	pub backing_group: GroupIndex,
	pub our_assignment: Option<OurAssignment>,
}

/// Record a new block entry.
///
/// This will update the blocks-at-height mapping, the stored block range, if necessary,
/// and add block and candidate entries. It will also add approval entries to existing
/// candidate entries and add this as a child of any block entry corresponding to the
/// parent hash.
///
/// Has no effect if there is already an entry for the block or `candidate_info` returns
/// `None` for any of the candidates referenced by the block entry. In these cases,
/// no information about new candidates will be referred to by this function.
pub(crate) fn add_block_entry(
	store: &dyn KeyValueDB,
	config: &Config,
	entry: BlockEntry,
	n_validators: usize,
	candidate_info: impl Fn(&CandidateHash) -> Option<NewCandidateInfo>,
) -> Result<Vec<(CandidateHash, CandidateEntry)>> {
	let mut transaction = DBTransaction::new();
	let session = entry.session;
	let parent_hash = entry.parent_hash;
	let number = entry.block_number;

	// Update the stored block range.
	{
		let new_range = match load_stored_blocks(store, config)? {
			None => Some(StoredBlockRange(number, number + 1)),
			Some(range) => if range.1 <= number {
				Some(StoredBlockRange(range.0, number + 1))
			} else {
				None
			}
		};

		new_range.map(|n| transaction.put_vec(config.col_data, &STORED_BLOCKS_KEY[..], n.encode()))
	};

	// Update the blocks at height meta key.
	{
		let mut blocks_at_height = load_blocks_at_height(store, config, number)?;
		if blocks_at_height.contains(&entry.block_hash) {
			// seems we already have a block entry for this block. nothing to do here.
			return Ok(Vec::new())
		}

		blocks_at_height.push(entry.block_hash);
		transaction.put_vec(config.col_data, &blocks_at_height_key(number)[..], blocks_at_height.encode())
	};

	let mut candidate_entries = Vec::with_capacity(entry.candidates.len());

	// read and write all updated entries.
	{
		for &(_, ref candidate_hash) in &entry.candidates {
			let NewCandidateInfo {
				candidate,
				backing_group,
				our_assignment,
			} = match candidate_info(candidate_hash) {
				None => return Ok(Vec::new()),
				Some(info) => info,
			};

			let mut candidate_entry = load_candidate_entry(store, config, &candidate_hash)?
				.unwrap_or_else(move || CandidateEntry {
					candidate,
					session,
					block_assignments: BTreeMap::new(),
					approvals: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
				});

			candidate_entry.block_assignments.insert(
				entry.block_hash,
				ApprovalEntry {
					tranches: Vec::new(),
					backing_group,
					our_assignment,
					our_approval_sig: None,
					assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
					approved: false,
				}
			);

			transaction.put_vec(
				config.col_data,
				&candidate_entry_key(&candidate_hash)[..],
				candidate_entry.encode(),
			);

			candidate_entries.push((*candidate_hash, candidate_entry));
		}
	};

	// Update the child index for the parent.
	load_block_entry(store, config, &parent_hash)?.map(|mut e| {
		e.children.push(entry.block_hash);
		transaction.put_vec(config.col_data, &block_entry_key(&parent_hash)[..], e.encode())
	});

	// Put the new block entry in.
	transaction.put_vec(config.col_data, &block_entry_key(&entry.block_hash)[..], entry.encode());

	store.write(transaction)?;
	Ok(candidate_entries)
}

/// Forcibly approve all candidates included at up to the given relay-chain height in the indicated
/// chain.
pub fn force_approve(
	store: &dyn KeyValueDB,
	db_config: Config,
	chain_head: Hash,
	up_to: BlockNumber,
) -> Result<()> {
	enum State {
		WalkTo,
		Approving,
	}

	let mut cur_hash = chain_head;
	let mut state = State::WalkTo;

	let mut tx = Transaction::new(db_config);

	// iterate back to the `up_to` block, and then iterate backwards until all blocks
	// are updated.
	while let Some(mut entry) = load_block_entry(store, &db_config, &cur_hash)? {

		if entry.block_number <= up_to {
			state = State::Approving;
		}

		cur_hash = entry.parent_hash;

		match state {
			State::WalkTo => {},
			State::Approving => {
				entry.approved_bitfield.iter_mut().for_each(|mut b| *b = true);
				tx.put_block_entry(entry);
			}
		}
	}

	tx.write(store)
}

/// Return all blocks which have entries in the DB, ascending, by height.
pub(crate) fn load_all_blocks(store: &dyn KeyValueDB, config: &Config) -> Result<Vec<Hash>> {
	let stored_blocks = load_stored_blocks(store, config)?;

	let mut hashes = Vec::new();
	for height in stored_blocks.into_iter().flat_map(|s| s.0..s.1) {
		hashes.extend(load_blocks_at_height(store, config, height)?);
	}

	Ok(hashes)
}

// An atomic transaction of multiple candidate or block entries.
#[must_use = "Transactions do nothing unless written to a DB"]
pub struct Transaction {
	block_entries: HashMap<Hash, BlockEntry>,
	candidate_entries: HashMap<CandidateHash, CandidateEntry>,
	config: Config,
}

impl Transaction {
	pub(crate) fn new(config: Config) -> Self {
		Transaction {
			block_entries: HashMap::default(),
			candidate_entries: HashMap::default(),
			config,
		}
	}

	/// Put a block entry in the transaction, overwriting any other with the
	/// same hash.
	pub(crate) fn put_block_entry(&mut self, entry: BlockEntry) {
		let hash = entry.block_hash;
		let _ = self.block_entries.insert(hash, entry);
	}

	/// Put a candidate entry in the transaction, overwriting any other with the
	/// same hash.
	pub(crate) fn put_candidate_entry(&mut self, hash: CandidateHash, entry: CandidateEntry) {
		let _ = self.candidate_entries.insert(hash, entry);
	}

	/// Returns true if the transaction contains no actions
	pub(crate) fn is_empty(&self) -> bool {
		self.block_entries.is_empty() && self.candidate_entries.is_empty()
	}

	/// Write the contents of the transaction, atomically, to the DB.
	pub(crate) fn write(self, db: &dyn KeyValueDB) -> Result<()> {
		if self.is_empty() {
			return Ok(())
		}

		let mut db_transaction = DBTransaction::new();

		for (hash, entry) in self.block_entries {
			let k = block_entry_key(&hash);
			let v = entry.encode();

			db_transaction.put_vec(self.config.col_data, &k, v);
		}

		for (hash, entry) in self.candidate_entries {
			let k = candidate_entry_key(&hash);
			let v = entry.encode();

			db_transaction.put_vec(self.config.col_data, &k, v);
		}

		db.write(db_transaction).map_err(Into::into)
	}
}

/// Load the stored-blocks key from the state.
fn load_stored_blocks(store: &dyn KeyValueDB, config: &Config)
	-> Result<Option<StoredBlockRange>>
{
	load_decode(store, config.col_data, STORED_BLOCKS_KEY)
}

/// Load a blocks-at-height entry for a given block number.
pub(crate) fn load_blocks_at_height(
	store: &dyn KeyValueDB,
	config: &Config,
	block_number: BlockNumber,
)
	-> Result<Vec<Hash>> {
	load_decode(store, config.col_data, &blocks_at_height_key(block_number))
		.map(|x| x.unwrap_or_default())
}

/// Load a block entry from the aux store.
pub(crate) fn load_block_entry(store: &dyn KeyValueDB, config: &Config, block_hash: &Hash)
	-> Result<Option<BlockEntry>>
{
	load_decode(store, config.col_data, &block_entry_key(block_hash))
}

/// Load a candidate entry from the aux store.
pub(crate) fn load_candidate_entry(
	store: &dyn KeyValueDB,
	config: &Config,
	candidate_hash: &CandidateHash,
)
	-> Result<Option<CandidateEntry>>
{
	load_decode(store, config.col_data, &candidate_entry_key(candidate_hash))
}

/// The key a given block entry is stored under.
fn block_entry_key(block_hash: &Hash) -> [u8; 46] {
	const BLOCK_ENTRY_PREFIX: [u8; 14] = *b"Approvals_blck";

	let mut key = [0u8; 14 + 32];
	key[0..14].copy_from_slice(&BLOCK_ENTRY_PREFIX);
	key[14..][..32].copy_from_slice(block_hash.as_ref());

	key
}

/// The key a given candidate entry is stored under.
fn candidate_entry_key(candidate_hash: &CandidateHash) -> [u8; 46] {
	const CANDIDATE_ENTRY_PREFIX: [u8; 14] = *b"Approvals_cand";

	let mut key = [0u8; 14 + 32];
	key[0..14].copy_from_slice(&CANDIDATE_ENTRY_PREFIX);
	key[14..][..32].copy_from_slice(candidate_hash.0.as_ref());

	key
}

/// The key a set of block hashes corresponding to a block number is stored under.
fn blocks_at_height_key(block_number: BlockNumber) -> [u8; 16] {
	const BLOCKS_AT_HEIGHT_PREFIX: [u8; 12] = *b"Approvals_at";

	let mut key = [0u8; 12 + 4];
	key[0..12].copy_from_slice(&BLOCKS_AT_HEIGHT_PREFIX);
	block_number.using_encoded(|s| key[12..16].copy_from_slice(s));

	key
}
