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

use sc_client_api::backend::AuxStore;
use polkadot_node_primitives::approval::{DelayTranche, AssignmentCert};
use polkadot_primitives::v1::{
	ValidatorIndex, GroupIndex, CandidateReceipt, SessionIndex, CoreIndex,
	BlockNumber, Hash, CandidateHash,
};
use sp_consensus_slots::Slot;
use parity_scale_codec::{Encode, Decode};

use std::collections::{BTreeMap, HashMap};
use std::collections::hash_map::Entry;
use bitvec::{vec::BitVec, order::Lsb0 as BitOrderLsb0};

#[cfg(test)]
mod tests;

// slot_duration * 2 + DelayTranche gives the number of delay tranches since the
// unix epoch.
#[derive(Encode, Decode, Clone, Copy, Debug, PartialEq)]
pub struct Tick(u64);

pub type Bitfield = BitVec<BitOrderLsb0, u8>;

const STORED_BLOCKS_KEY: &[u8] = b"Approvals_StoredBlocks";

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

/// Canonicalize some particular block, pruning everything before it and
/// pruning any competing branches at the same height.
pub(crate) fn canonicalize(
	store: &impl AuxStore,
	canon_number: BlockNumber,
	canon_hash: Hash,
)
	-> sp_blockchain::Result<()>
{
	let range = match load_stored_blocks(store)? {
		None => return Ok(()),
		Some(range) => if range.0 >= canon_number {
			return Ok(())
		} else {
			range
		},
	};

	let mut deleted_height_keys = Vec::new();
	let mut deleted_block_keys = Vec::new();

	// Storing all candidates in memory is potentially heavy, but should be fine
	// as long as finality doesn't stall for a long while. We could optimize this
	// by keeping only the metadata about which blocks reference each candidate.
	let mut visited_candidates = HashMap::new();

	// All the block heights we visited but didn't necessarily delete everything from.
	let mut visited_heights = HashMap::new();

	let visit_and_remove_block_entry = |
		block_hash: Hash,
		deleted_block_keys: &mut Vec<_>,
		visited_candidates: &mut HashMap<CandidateHash, CandidateEntry>,
	| -> sp_blockchain::Result<Vec<Hash>> {
		let block_entry = match load_block_entry(store, &block_hash)? {
			None => return Ok(Vec::new()),
			Some(b) => b,
		};

		deleted_block_keys.push(block_entry_key(&block_hash));
		for &(_, ref candidate_hash) in &block_entry.candidates {
			let candidate = match visited_candidates.entry(*candidate_hash) {
				Entry::Occupied(e) => e.into_mut(),
				Entry::Vacant(e) => {
					e.insert(match load_candidate_entry(store, candidate_hash)? {
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
		let at_height = load_blocks_at_height(store, i)?;
		deleted_height_keys.push(blocks_at_height_key(i));

		for b in at_height {
			let _ = visit_and_remove_block_entry(
				b,
				&mut deleted_block_keys,
				&mut visited_candidates,
			)?;
		}
	}

	// Then visit everything at the height.
	let pruned_branches = {
		let at_height = load_blocks_at_height(store, canon_number)?;
		deleted_height_keys.push(blocks_at_height_key(canon_number));

		// Note that while there may be branches descending from blocks at earlier heights,
		// we have already covered them by removing everything at earlier heights.
		let mut pruned_branches = Vec::new();

		for b in at_height {
			let children = visit_and_remove_block_entry(
				b,
				&mut deleted_block_keys,
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
				&mut deleted_block_keys,
				&mut visited_candidates,
			)?;

			// extend the frontier of branches to include the given height.
			frontier.extend(children.into_iter().map(|h| (height + 1, h)));

			// visit the at-height key for this deleted block's height.
			let at_height = match visited_heights.entry(height) {
				Entry::Occupied(e) => e.into_mut(),
				Entry::Vacant(e) => e.insert(load_blocks_at_height(store, height)?),
			};

			if let Some(i) = at_height.iter().position(|x| x == &next_child) {
				at_height.remove(i);
			}
		}
	}

	// Update all `CandidateEntry`s, deleting all those which now have empty `block_assignments`.
	let (written_candidates, deleted_candidates) = {
		let mut written = Vec::new();
		let mut deleted = Vec::new();

		for (candidate_hash, candidate) in visited_candidates {
			if candidate.block_assignments.is_empty() {
				deleted.push(candidate_entry_key(&candidate_hash));
			} else {
				written.push((candidate_entry_key(&candidate_hash), candidate.encode()));
			}
		}

		(written, deleted)
	};

	// Update all blocks-at-height keys, deleting all those which now have empty `block_assignments`.
	let written_at_height = {
		visited_heights.into_iter().filter_map(|(h, at)| {
			if at.is_empty() {
				deleted_height_keys.push(blocks_at_height_key(h));
				None
			} else {
				Some((blocks_at_height_key(h), at.encode()))
			}
		}).collect::<Vec<_>>()
	};

	// due to the fork pruning, this range actually might go too far above where our actual highest block is,
	// if a relatively short fork is canonicalized.
	let new_range = StoredBlockRange(
		canon_number + 1,
		std::cmp::max(range.1, canon_number + 2),
	).encode();

	// Because aux-store requires &&[u8], we have to collect.

	let inserted_keys: Vec<_> = std::iter::once((&STORED_BLOCKS_KEY[..], &new_range[..]))
		.chain(written_candidates.iter().map(|&(ref k, ref v)| (&k[..], &v[..])))
		.chain(written_at_height.iter().map(|&(ref k, ref v)| (&k[..], &v[..])))
		.collect();

	let deleted_keys: Vec<_> = deleted_block_keys.iter().map(|k| &k[..])
		.chain(deleted_height_keys.iter().map(|k| &k[..]))
		.chain(deleted_candidates.iter().map(|k| &k[..]))
		.collect();

	// Update the values on-disk.
	store.insert_aux(
		inserted_keys.iter(),
		deleted_keys.iter(),
	)?;

	Ok(())
}

/// Clear the aux store of everything.
pub(crate) fn clear(store: &impl AuxStore)
	-> sp_blockchain::Result<()>
{
	let range = match load_stored_blocks(store)? {
		None => return Ok(()),
		Some(range) => range,
	};

	let mut visited_height_keys = Vec::new();
	let mut visited_block_keys = Vec::new();
	let mut visited_candidate_keys = Vec::new();

	for i in range.0..range.1 {
		let at_height = load_blocks_at_height(store, i)?;

		visited_height_keys.push(blocks_at_height_key(i));

		for block_hash in at_height {
			let block_entry = match load_block_entry(store, &block_hash)? {
				None => continue,
				Some(e) => e,
			};

			visited_block_keys.push(block_entry_key(&block_hash));

			for &(_, candidate_hash) in &block_entry.candidates {
				visited_candidate_keys.push(candidate_entry_key(&candidate_hash));
			}
		}
	}

	// unfortunately demands a `collect` because aux store wants `&&[u8]` for some reason.
	let visited_keys_borrowed = visited_height_keys.iter().map(|x| &x[..])
		.chain(visited_block_keys.iter().map(|x| &x[..]))
		.chain(visited_candidate_keys.iter().map(|x| &x[..]))
		.chain(std::iter::once(&STORED_BLOCKS_KEY[..]))
		.collect::<Vec<_>>();

	store.insert_aux(&[], &visited_keys_borrowed)?;

	Ok(())
}

fn load_decode<D: Decode>(store: &impl AuxStore, key: &[u8])
	-> sp_blockchain::Result<Option<D>>
{
	match store.get_aux(key)? {
		None => Ok(None),
		Some(raw) => D::decode(&mut &raw[..])
			.map(Some)
			.map_err(|e| sp_blockchain::Error::Storage(
				format!("Failed to decode item in approvals DB: {:?}", e)
			)),
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
	store: &impl AuxStore,
	parent_hash: Hash,
	number: BlockNumber,
	entry: BlockEntry,
	n_validators: usize,
	candidate_info: impl Fn(&CandidateHash) -> Option<NewCandidateInfo>,
) -> sp_blockchain::Result<Vec<(CandidateHash, CandidateEntry)>> {
	let session = entry.session;

	let new_block_range = {
		let new_range = match load_stored_blocks(store)? {
			None => Some(StoredBlockRange(number, number + 1)),
			Some(range) => if range.1 <= number {
				Some(StoredBlockRange(range.0, number + 1))
			} else {
				None
			}
		};

		new_range.map(|n| (STORED_BLOCKS_KEY, n.encode()))
	};

	let updated_blocks_at = {
		let mut blocks_at_height = load_blocks_at_height(store, number)?;
		if blocks_at_height.contains(&entry.block_hash) {
			// seems we already have a block entry for this block. nothing to do here.
			return Ok(Vec::new())
		}

		blocks_at_height.push(entry.block_hash);
		(blocks_at_height_key(number), blocks_at_height.encode())
	};

	let mut candidate_entries = Vec::with_capacity(entry.candidates.len());

	let candidate_entry_updates = {
		let mut updated_entries = Vec::with_capacity(entry.candidates.len());
		for &(_, ref candidate_hash) in &entry.candidates {
			let NewCandidateInfo {
				candidate,
				backing_group,
				our_assignment,
			} = match candidate_info(candidate_hash) {
				None => return Ok(Vec::new()),
				Some(info) => info,
			};

			let mut candidate_entry = load_candidate_entry(store, &candidate_hash)?
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
					assignments: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
					approved: false,
				}
			);

			updated_entries.push(
				(candidate_entry_key(&candidate_hash), candidate_entry.encode())
			);

			candidate_entries.push((*candidate_hash, candidate_entry));
		}

		updated_entries
	};

	let updated_parent = {
		load_block_entry(store, &parent_hash)?.map(|mut e| {
			e.children.push(entry.block_hash);
			(block_entry_key(&parent_hash), e.encode())
		})
	};

	let write_block_entry = (block_entry_key(&entry.block_hash), entry.encode());

	// write:
	//   - new block range
	//   - updated blocks-at item
	//   - fresh and updated candidate entries
	//   - the parent block entry.
	//   - the block entry itself

	// Unfortunately have to collect because aux-store demands &(&[u8], &[u8]).
	let all_keys_and_values: Vec<_> = new_block_range.as_ref().into_iter()
		.map(|&(ref k, ref v)| (&k[..], &v[..]))
		.chain(std::iter::once((&updated_blocks_at.0[..], &updated_blocks_at.1[..])))
		.chain(candidate_entry_updates.iter().map(|&(ref k, ref v)| (&k[..], &v[..])))
		.chain(std::iter::once((&write_block_entry.0[..], &write_block_entry.1[..])))
		.chain(updated_parent.as_ref().into_iter().map(|&(ref k, ref v)| (&k[..], &v[..])))
		.collect();

	store.insert_aux(&all_keys_and_values, &[])?;

	Ok(candidate_entries)
}

// An atomic transaction of multiple candidate or block entries.
#[derive(Default)]
#[must_use = "Transactions do nothing unless written to a DB"]
pub struct Transaction {
	block_entries: HashMap<Hash, BlockEntry>,
	candidate_entries: HashMap<CandidateHash, CandidateEntry>,
}

impl Transaction {
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

	/// Write the contents of the transaction, atomically, to the DB.
	pub(crate) fn write(self, db: &impl AuxStore) -> sp_blockchain::Result<()> {
		if self.block_entries.is_empty() && self.candidate_entries.is_empty() {
			return Ok(())
		}

		let blocks: Vec<_> = self.block_entries.into_iter().map(|(hash, entry)| {
			let k = block_entry_key(&hash);
			let v = entry.encode();

			(k, v)
		}).collect();

		let candidates: Vec<_> = self.candidate_entries.into_iter().map(|(hash, entry)| {
			let k = candidate_entry_key(&hash);
			let v = entry.encode();

			(k, v)
		}).collect();

		let kv = blocks.iter().map(|(k, v)| (&k[..], &v[..]))
			.chain(candidates.iter().map(|(k, v)| (&k[..], &v[..])))
			.collect::<Vec<_>>();

		db.insert_aux(&kv, &[])
	}
}

/// Load the stored-blocks key from the state.
fn load_stored_blocks(store: &impl AuxStore)
	-> sp_blockchain::Result<Option<StoredBlockRange>>
{
	load_decode(store, STORED_BLOCKS_KEY)
}

/// Load a blocks-at-height entry for a given block number.
pub(crate) fn load_blocks_at_height(store: &impl AuxStore, block_number: BlockNumber)
	-> sp_blockchain::Result<Vec<Hash>> {
	load_decode(store, &blocks_at_height_key(block_number))
		.map(|x| x.unwrap_or_default())
}

/// Load a block entry from the aux store.
pub(crate) fn load_block_entry(store: &impl AuxStore, block_hash: &Hash)
	-> sp_blockchain::Result<Option<BlockEntry>>
{
	load_decode(store, &block_entry_key(block_hash))
}

/// Load a candidate entry from the aux store.
pub(crate) fn load_candidate_entry(store: &impl AuxStore, candidate_hash: &CandidateHash)
	-> sp_blockchain::Result<Option<CandidateEntry>>
{
	load_decode(store, &candidate_entry_key(candidate_hash))
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
