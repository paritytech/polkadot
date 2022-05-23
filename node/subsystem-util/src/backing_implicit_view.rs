// Copyright 2022 Parity Technologies (UK) Ltd.
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

use futures::channel::oneshot;
use polkadot_node_subsystem::{
	errors::ChainApiError,
	messages::{ChainApiMessage, ProspectiveParachainsMessage},
	SubsystemSender,
};
use polkadot_primitives::vstaging::{BlockNumber, Hash, Id as ParaId};

use std::collections::HashMap;

// Always aim to retain 1 block before the active leaves.
const MINIMUM_RETAIN_LENGTH: BlockNumber = 2;

/// Handles the implicit view of the relay chain derived from the immediate view, which
/// is composed of active leaves, and the minimum relay-parents allowed for
/// candidates of various parachains at those leaves.
#[derive(Default, Clone)]
pub struct View {
	leaves: HashMap<Hash, ActiveLeafPruningInfo>,
	block_info_storage: HashMap<Hash, BlockInfo>,
}

// Minimum relay parents implicitly relative to a particular block.
#[derive(Clone)]
struct AllowedRelayParents {
	// minimum relay parents can only be fetched for active leaves,
	// so this will be empty for all blocks that haven't ever been
	// witnessed as active leaves.
	minimum_relay_parents: HashMap<ParaId, BlockNumber>,
	// Ancestry, in descending order, starting from the block hash itself down
	// to and including the minimum of `minimum_relay_parentes`.
	allowed_relay_parents_contiguous: Vec<Hash>,
}

impl AllowedRelayParents {
	fn allowed_relay_parents_for(&self, para_id: ParaId, base_number: BlockNumber) -> &[Hash] {
		let para_min = match self.minimum_relay_parents.get(&para_id) {
			Some(p) => *p,
			None => return &[],
		};

		if base_number < para_min {
			return &[]
		}

		let diff = base_number - para_min;

		// difference of 0 should lead to slice len of 1
		let slice_len = ((diff + 1) as usize).min(self.allowed_relay_parents_contiguous.len());
		&self.allowed_relay_parents_contiguous[..slice_len]
	}
}

#[derive(Clone)]
struct ActiveLeafPruningInfo {
	// The mimimum block in the same branch of the relay-chain that should be
	// preserved.
	retain_minimum: BlockNumber,
}

#[derive(Clone)]
struct BlockInfo {
	block_number: BlockNumber,
	// If this was previously an active leaf, this will be `Some`
	// and is useful for understanding the views of peers in the network
	// which may not be in perfect synchrony with our own view.
	//
	// If they are ahead of us in getting a new leaf, there's nothing we
	// can do as it's an unrecognized block hash. But if they're behind us,
	// it's useful for us to retain some information about previous leaves'
	// implicit views so we can continue to send relevant messages to them
	// until they catch up.
	maybe_allowed_relay_parents: Option<AllowedRelayParents>,
	parent_hash: Hash,
}

impl View {
	/// Update the view to a new view, preserving any previous still-relevant information
	/// about blocks. This will request the minimum relay parents from the
	/// Prospective Parachains subsystem for each leaf and will load headers in the ancestry of each
	/// leaf in the view as needed.
	pub async fn update<Sender>(
		&mut self,
		sender: &mut Sender,
		new_view: Vec<Hash>,
		observe_err: impl Fn(Hash, FetchError),
	) where
		Sender: SubsystemSender<ChainApiMessage>,
		Sender: SubsystemSender<ProspectiveParachainsMessage>,
	{
		// Remove all leaves not present in the new view.
		self.leaves.retain(|prev, _| new_view.contains(prev));

		let fresh: Vec<Hash> = {
			new_view
				.iter()
				.filter(|head| !self.leaves.contains_key(head))
				.cloned()
				.collect()
		};

		for leaf_hash in fresh {
			let res = fetch_fresh_leaf_and_insert_ancestry(
				leaf_hash,
				&mut self.block_info_storage,
				&mut *sender,
			)
			.await;

			match res {
				Ok(fetched) => {
					let retain_minimum = std::cmp::max(
						fetched.minimum_ancestor_number,
						fetched.leaf_number.saturating_sub(MINIMUM_RETAIN_LENGTH),
					);

					self.leaves.insert(leaf_hash, ActiveLeafPruningInfo { retain_minimum });
				},
				Err(e) => observe_err(leaf_hash, e),
			}
		}

		// Prune everything before the minimum out of all leaves,
		// pruning absolutely everything if there are no leaves (empty view)
		//
		// Pruning by block number does leave behind orphaned forks slightly longer
		// but the memory overhead is negligible.
		{
			let minimum = self.leaves.values().map(|l| l.retain_minimum).min();

			self.block_info_storage
				.retain(|_, i| minimum.map_or(false, |m| i.block_number < m));
		}
	}

	/// Get the known, allowed relay-parents that are valid for parachain candidates
	/// which could be backed in a child of a given block for a given para ID.
	///
	/// This is expressed as a contiguous slice of relay-chain block hashes which may
	/// include the provided block hash itself.
	///
	/// `None` indicates that the block hash isn't part of the implicit view or that
	/// there are no known allowed relay parents.
	///
	/// This always returns `Some` for active leaves or for blocks that previously
	/// were active leaves.
	///
	/// This can return the empty slice, which indicates that no relay-parents are allowed
	/// for the para, e.g. if the para is not scheduled at the given block hash.
	pub fn known_allowed_relay_parents_under(
		&self,
		block_hash: &Hash,
		para_id: ParaId,
	) -> Option<&[Hash]> {
		let block_info = self.block_info_storage.get(block_hash)?;
		block_info
			.maybe_allowed_relay_parents
			.as_ref()
			.map(|mins| mins.allowed_relay_parents_for(para_id, block_info.block_number))
	}
}

/// Errors when fetching a leaf and associated ancestry.
#[derive(Debug)]
pub enum FetchError {
	/// The prospective parachains subsystem was uavailable.
	ProspectiveParachainsUnavailable,
	/// A block header was unavailable.
	BlockHeaderUnavailable(Hash, BlockHeaderUnavailableReason),
	/// A block header was unavailable due to a chain API error.
	ChainApiError(Hash, ChainApiError),
	/// The chain API subsystem was unavailable.
	ChainApiUnavailable,
}

/// Reasons a block header might have been unavailable.
#[derive(Debug)]
pub enum BlockHeaderUnavailableReason {
	/// Block header simply unknown.
	Unknown,
	/// Internal Chain API error.
	Internal(ChainApiError),
	/// The subsystem was unavailable.
	SubsystemUnavailable,
}

struct FetchSummary {
	minimum_ancestor_number: BlockNumber,
	leaf_number: BlockNumber,
}

async fn fetch_fresh_leaf_and_insert_ancestry<Sender>(
	leaf_hash: Hash,
	block_info_storage: &mut HashMap<Hash, BlockInfo>,
	sender: &mut Sender,
) -> Result<FetchSummary, FetchError>
where
	Sender: SubsystemSender<ChainApiMessage>,
	Sender: SubsystemSender<ProspectiveParachainsMessage>,
{
	let min_relay_parents_raw = {
		let (tx, rx) = oneshot::channel();
		sender
			.send_message(ProspectiveParachainsMessage::GetMinimumRelayParents(leaf_hash, tx))
			.await;

		match rx.await {
			Ok(m) => m,
			Err(_) => return Err(FetchError::ProspectiveParachainsUnavailable),
		}
	};

	let leaf_header = {
		let (tx, rx) = oneshot::channel();
		sender.send_message(ChainApiMessage::BlockHeader(leaf_hash, tx)).await;

		match rx.await {
			Ok(Ok(Some(header))) => header,
			Ok(Ok(None)) =>
				return Err(FetchError::BlockHeaderUnavailable(
					leaf_hash,
					BlockHeaderUnavailableReason::Unknown,
				)),
			Ok(Err(e)) =>
				return Err(FetchError::BlockHeaderUnavailable(
					leaf_hash,
					BlockHeaderUnavailableReason::Internal(e),
				)),
			Err(_) =>
				return Err(FetchError::BlockHeaderUnavailable(
					leaf_hash,
					BlockHeaderUnavailableReason::SubsystemUnavailable,
				)),
		}
	};

	let min_min = min_relay_parents_raw.iter().map(|x| x.1).min().unwrap_or(leaf_header.number);
	let expected_ancestry_len = (leaf_header.number.saturating_sub(min_min) as usize) + 1;

	let ancestry = if leaf_header.number > 0 {
		let mut next_ancestor_number = leaf_header.number - 1;
		let mut next_ancestor_hash = leaf_header.parent_hash;

		let mut ancestry = Vec::with_capacity(expected_ancestry_len);
		ancestry.push(leaf_hash);

		// Ensure all ancestors up to and including `min_min` are in the
		// block storage. When views advance incrementally, everything
		// should already be present.
		while next_ancestor_number >= min_min {
			let parent_hash = if let Some(info) = block_info_storage.get(&next_ancestor_hash) {
				info.parent_hash
			} else {
				// load the header and insert into block storage.
				let (tx, rx) = oneshot::channel();
				sender.send_message(ChainApiMessage::BlockHeader(next_ancestor_hash, tx)).await;

				let header = match rx.await {
					Ok(Ok(Some(header))) => header,
					Ok(Ok(None)) =>
						return Err(FetchError::BlockHeaderUnavailable(
							next_ancestor_hash,
							BlockHeaderUnavailableReason::Unknown,
						)),
					Ok(Err(e)) =>
						return Err(FetchError::BlockHeaderUnavailable(
							next_ancestor_hash,
							BlockHeaderUnavailableReason::Internal(e),
						)),
					Err(_) =>
						return Err(FetchError::BlockHeaderUnavailable(
							next_ancestor_hash,
							BlockHeaderUnavailableReason::SubsystemUnavailable,
						)),
				};

				block_info_storage.insert(
					next_ancestor_hash,
					BlockInfo {
						block_number: next_ancestor_number,
						parent_hash: header.parent_hash,
						maybe_allowed_relay_parents: None,
					},
				);

				header.parent_hash
			};

			ancestry.push(next_ancestor_hash);
			if next_ancestor_number == 0 {
				break
			}

			next_ancestor_number -= 1;
			next_ancestor_hash = parent_hash;
		}

		ancestry
	} else {
		Vec::new()
	};

	let fetched_ancestry =
		FetchSummary { minimum_ancestor_number: min_min, leaf_number: leaf_header.number };

	let allowed_relay_parents = AllowedRelayParents {
		minimum_relay_parents: min_relay_parents_raw.iter().cloned().collect(),
		allowed_relay_parents_contiguous: ancestry,
	};

	let leaf_block_info = BlockInfo {
		parent_hash: leaf_header.parent_hash,
		block_number: leaf_header.number,
		maybe_allowed_relay_parents: Some(allowed_relay_parents),
	};

	block_info_storage.insert(leaf_hash, leaf_block_info);

	Ok(fetched_ancestry)
}
