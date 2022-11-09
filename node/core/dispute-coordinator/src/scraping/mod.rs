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

use std::{
	collections::{BTreeMap, HashSet},
	num::NonZeroUsize,
};

use futures::channel::oneshot;
use lru::LruCache;

use polkadot_node_primitives::MAX_FINALITY_LAG;
use polkadot_node_subsystem::{
	messages::ChainApiMessage, overseer, ActivatedLeaf, ActiveLeavesUpdate, ChainApiError,
	SubsystemSender,
};
use polkadot_node_subsystem_util::runtime::{get_candidate_events, get_on_chain_votes};
use polkadot_primitives::v2::{
	BlockNumber, CandidateEvent, CandidateHash, Hash, ScrapedOnChainVotes,
};

use crate::{
	error::{FatalError, FatalResult, Result},
	LOG_TARGET,
};

#[cfg(test)]
mod tests;

mod candidates;

/// Number of hashes to keep in the LRU.
///
///
/// When traversing the ancestry of a block we will stop once we hit a hash that we find in the
/// `last_observed_blocks` LRU. This means, this value should the very least be as large as the
/// number of expected forks for keeping chain scraping efficient. Making the LRU much larger than
/// that has very limited use.
const LRU_OBSERVED_BLOCKS_CAPACITY: NonZeroUsize = match NonZeroUsize::new(20) {
	Some(cap) => cap,
	None => panic!("Observed blocks cache size must be non-zero"),
};

/// Chain scraper
///
/// Scrapes unfinalized chain in order to collect information from blocks.
///
/// Concretely:
///
/// - Monitors for `CandidateIncluded` events to keep track of candidates that have been included on chains.
/// - Monitors for `CandidateBacked` events to keep track of all backed candidates.
/// - Calls `FetchOnChainVotes` for each block to gather potentially missed votes from chain.
///
/// With this information it provides a `CandidateComparator` and as a return value of
/// `process_active_leaves_update` any scraped votes.
pub struct ChainScraper {
	/// All candidates we have seen included, which not yet have been finalized.
	included_candidates: candidates::RefCountedCandidates,
	/// All candidates we have seen backed
	backed_candidates: candidates::RefCountedCandidates,
	/// including block -> `CandidateHash`
	///
	/// We need this to clean up `included_candidates` on finalization.
	included_candidates_by_block_number: BTreeMap<BlockNumber, HashSet<CandidateHash>>,
	/// Keeps track at which block a `CandidateHash` was backed. If a candidate was backed
	/// and included - it will be removed once the block at its height is finalized. However
	/// a candidate can be backed and never included. To avoid leaking memory in such cases
	/// we keep track at which block height the candidate was backed. We remove backed (and
	/// not included) candidates when `BACKED_CANDIDATE_MAX_LIFETIME` more blocks are
	/// finalized after the block they were initially backed in.
	/// E.g. if `BACKED_CANDIDATE_MAX_LIFETIME = 2` when block 4 is finalized we will remove
	/// all candidates backed in block 2 which were not already cleaned up on finalization.
	///
	/// Please note that if a candidate is backed AND included it will be cleaned up on
	/// finalization thanks to the tracking in `included_candidates_by_block_number`.
	///
	/// Also please note that this `BTreeMap` keeps information when a candidate was backed
	/// only to guarantee that it will be removed at some point instead of staying in the
	/// `BTreeMap` forever. If a candidate was included it will be removed from
	/// `backed_candidates` but will remain here until `BACKED_CANDIDATE_MAX_LIFETIME`
	/// blocks are finalized from its inclusion.
	/// An alternative to fix this is to keep track of yet another mapping -
	/// `BTreeMap<CandidateHash, HashSet<BlockNumber>>` so that we can lookup when a candidate
	/// was backed and clean it up. It's better to have a few stale entries in one BTreeMap
	/// instead of keeping three copies of each candidate.
	backed_candidates_by_block_number: BTreeMap<BlockNumber, HashSet<CandidateHash>>,
	/// Latest relay blocks observed by the provider.
	///
	/// We assume that ancestors of cached blocks are already processed, i.e. we have saved
	/// corresponding included candidates.
	last_observed_blocks: LruCache<Hash, ()>,
}

impl ChainScraper {
	/// Limits the number of ancestors received for a single request.
	pub(crate) const ANCESTRY_CHUNK_SIZE: u32 = 10;
	/// Limits the overall number of ancestors walked through for a given head.
	///
	/// As long as we have `MAX_FINALITY_LAG` this makes sense as a value.
	pub(crate) const ANCESTRY_SIZE_LIMIT: u32 = MAX_FINALITY_LAG;

	/// How many blocks after finalization a backed and not included candidate should
	/// be kept.
	pub(crate) const BACKED_CANDIDATE_MAX_LIFETIME: BlockNumber = 2;

	/// Create a properly initialized `OrderingProvider`.
	///
	/// Returns: `Self` and any scraped votes.
	pub async fn new<Sender>(
		sender: &mut Sender,
		initial_head: ActivatedLeaf,
	) -> Result<(Self, Vec<ScrapedOnChainVotes>)>
	where
		Sender: overseer::DisputeCoordinatorSenderTrait,
	{
		let mut s = Self {
			included_candidates: candidates::RefCountedCandidates::new(),
			backed_candidates: candidates::RefCountedCandidates::new(),
			included_candidates_by_block_number: BTreeMap::new(),
			backed_candidates_by_block_number: BTreeMap::new(),
			last_observed_blocks: LruCache::new(LRU_OBSERVED_BLOCKS_CAPACITY),
		};
		let update =
			ActiveLeavesUpdate { activated: Some(initial_head), deactivated: Default::default() };
		let votes = s.process_active_leaves_update(sender, &update).await?;
		Ok((s, votes))
	}

	/// Check whether we have seen a candidate included on any chain.
	pub fn is_candidate_included(&self, candidate_hash: &CandidateHash) -> bool {
		self.included_candidates.contains(candidate_hash)
	}

	/// Check whether the candidate is backed
	pub fn is_candidate_backed(&self, candidate_hash: &CandidateHash) -> bool {
		self.backed_candidates.contains(candidate_hash)
	}

	/// Query active leaves for any candidate `CandidateEvent::CandidateIncluded` events.
	///
	/// and updates current heads, so we can query candidates for all non finalized blocks.
	///
	/// Returns: On chain vote for the leaf and any ancestors we might not yet have seen.
	pub async fn process_active_leaves_update<Sender>(
		&mut self,
		sender: &mut Sender,
		update: &ActiveLeavesUpdate,
	) -> Result<Vec<ScrapedOnChainVotes>>
	where
		Sender: overseer::DisputeCoordinatorSenderTrait,
	{
		let activated = match update.activated.as_ref() {
			Some(activated) => activated,
			None => return Ok(Vec::new()),
		};

		// Fetch ancestry up to last finalized block.
		let ancestors = self
			.get_unfinalized_block_ancestors(sender, activated.hash, activated.number)
			.await?;

		// Ancestors block numbers are consecutive in the descending order.
		let earliest_block_number = activated.number - ancestors.len() as u32;
		let block_numbers = (earliest_block_number..=activated.number).rev();

		let block_hashes = std::iter::once(activated.hash).chain(ancestors);

		let mut on_chain_votes = Vec::new();
		for (block_number, block_hash) in block_numbers.zip(block_hashes) {
			gum::trace!(?block_number, ?block_hash, "In ancestor processing.");

			self.process_candidate_events(sender, block_number, block_hash).await?;

			if let Some(votes) = get_on_chain_votes(sender, block_hash).await? {
				on_chain_votes.push(votes);
			}
		}

		self.last_observed_blocks.put(activated.hash, ());

		Ok(on_chain_votes)
	}

	/// Prune finalized candidates.
	///
	/// Once a candidate lives in a relay chain block that's behind the finalized chain/got
	/// finalized, we can treat it as low priority.
	pub fn process_finalized_block(&mut self, finalized_block_number: &BlockNumber) {
		// First remove any stale backed and not included candidates
		self.remove_stale_backed_candidates(&finalized_block_number);

		// Then handle included candidates
		let not_finalized =
			self.included_candidates_by_block_number.split_off(finalized_block_number);
		let finalized = std::mem::take(&mut self.included_candidates_by_block_number);
		self.included_candidates_by_block_number = not_finalized;
		// Clean up finalized
		for finalized_candidate in finalized.into_values().flatten() {
			self.included_candidates.remove(&finalized_candidate);
			// Remove the candidate from backed. Note that the candidate will remain in
			// `backed_candidates_by_block_number`. This is not desirable but acceptable trade off.
			// The `BTreeMap` is used to track stale candidates only. Decision if a candidate is backed
			// or not is made by looking it up in `backed_candidates`.
			// Entries in `backed_candidates_by_block_number` will be removed by `remove_stale_backed_candidates`
			self.backed_candidates.remove(&finalized_candidate);
		}
	}

	/// Remove any backed but not included candidates after `BACKED_CANDIDATE_MAX_LIFETIME` has passed
	/// since the (finalized) block containing the corresponding `CandidateBacked` event.
	/// Used in `process_finalized_block`.
	fn remove_stale_backed_candidates(&mut self, finalized_block_number: &BlockNumber) {
		// Cleanup stale backed and unincluded blocks. We want to keep the candidates backed in the last
		// `BACKED_CANDIDATE_MAX_LIFETIME` finalzied blocks.
		// We subtract `BACKED_CANDIDATE_MAX_LIFETIME - 1` from the finalzied block number to get the correct
		// boundry for `split_off`.
		//
		// Example:
		// finalized_block_number = 4; BACKED_CANDIDATE_MAX_LIFETIME = 2;
		// boundry = 4 - (2 - 1) = 3
		// After `split_off` at 3:
		// 0, 1, 2 will be in stale
		// 3, 4 will be in not stale
		// => We keep backed candidates in the last two finalized blocks
		finalized_block_number.checked_sub(Self::BACKED_CANDIDATE_MAX_LIFETIME - 1).map(
			|key_to_clean| {
				// let key_to_clean = key_to_clean + 1;
				let not_stale = self.backed_candidates_by_block_number.split_off(&(key_to_clean));
				let stale = std::mem::take(&mut self.backed_candidates_by_block_number);
				self.backed_candidates_by_block_number = not_stale;
				for candidates in stale.values() {
					for c in candidates {
						self.backed_candidates.remove(c);
					}
				}
			},
		);
	}

	/// Process candidate events of a block.
	///
	/// Keep track of all included and backed candidates.
	async fn process_candidate_events<Sender>(
		&mut self,
		sender: &mut Sender,
		block_number: BlockNumber,
		block_hash: Hash,
	) -> Result<()>
	where
		Sender: overseer::DisputeCoordinatorSenderTrait,
	{
		// Get included and backed events:
		for ev in get_candidate_events(sender, block_hash).await? {
			match ev {
				CandidateEvent::CandidateIncluded(receipt, _, _, _) => {
					let candidate_hash = receipt.hash();
					gum::trace!(
						target: LOG_TARGET,
						?candidate_hash,
						?block_number,
						"Processing included event"
					);
					self.included_candidates.insert(candidate_hash);
					self.included_candidates_by_block_number
						.entry(block_number)
						.or_default()
						.insert(candidate_hash);
				},
				CandidateEvent::CandidateBacked(receipt, _, _, _) => {
					let candidate_hash = receipt.hash();
					gum::trace!(
						target: LOG_TARGET,
						?candidate_hash,
						?block_number,
						"Processing backed event"
					);
					self.backed_candidates.insert(candidate_hash);
					self.backed_candidates_by_block_number
						.entry(block_number)
						.or_default()
						.insert(candidate_hash);
				},
				_ => {
					// skip the rest
				},
			}
		}
		Ok(())
	}

	/// Returns ancestors of `head` in the descending order, stopping
	/// either at the block present in cache or at the last finalized block.
	///
	/// Both `head` and the latest finalized block are **not** included in the result.
	async fn get_unfinalized_block_ancestors<Sender>(
		&mut self,
		sender: &mut Sender,
		mut head: Hash,
		mut head_number: BlockNumber,
	) -> Result<Vec<Hash>>
	where
		Sender: overseer::DisputeCoordinatorSenderTrait,
	{
		let target_ancestor = get_finalized_block_number(sender).await?;

		let mut ancestors = Vec::new();

		// If head_number <= target_ancestor + 1 the ancestry will be empty.
		if self.last_observed_blocks.get(&head).is_some() || head_number <= target_ancestor + 1 {
			return Ok(ancestors)
		}

		loop {
			let hashes = get_block_ancestors(sender, head, Self::ANCESTRY_CHUNK_SIZE).await?;

			let earliest_block_number = match head_number.checked_sub(hashes.len() as u32) {
				Some(number) => number,
				None => {
					// It's assumed that it's impossible to retrieve
					// more than N ancestors for block number N.
					gum::error!(
						target: LOG_TARGET,
						"Received {} ancestors for block number {} from Chain API",
						hashes.len(),
						head_number,
					);
					return Ok(ancestors)
				},
			};
			// The reversed order is parent, grandparent, etc. excluding the head.
			let block_numbers = (earliest_block_number..head_number).rev();

			for (block_number, hash) in block_numbers.zip(&hashes) {
				// Return if we either met target/cached block or
				// hit the size limit for the returned ancestry of head.
				if self.last_observed_blocks.get(hash).is_some() ||
					block_number <= target_ancestor ||
					ancestors.len() >= Self::ANCESTRY_SIZE_LIMIT as usize
				{
					return Ok(ancestors)
				}

				ancestors.push(*hash);
			}

			match hashes.last() {
				Some(last_hash) => {
					head = *last_hash;
					head_number = earliest_block_number;
				},
				None => break,
			}
		}
		return Ok(ancestors)
	}

	// Used only for tests to verify the pruning doesn't leak data.
	#[cfg(test)]
	fn backed_candidates_by_block_number_is_empty(&self) -> bool {
		println!("{:?}", self.backed_candidates_by_block_number);
		self.backed_candidates_by_block_number.is_empty()
	}

	#[cfg(test)]
	fn backed_candidates_by_block_number_has_key(&self, key: &BlockNumber) -> bool {
		self.backed_candidates_by_block_number.contains_key(key)
	}
}

async fn get_finalized_block_number<Sender>(sender: &mut Sender) -> FatalResult<BlockNumber>
where
	Sender: overseer::DisputeCoordinatorSenderTrait,
{
	let (number_tx, number_rx) = oneshot::channel();
	send_message_fatal(sender, ChainApiMessage::FinalizedBlockNumber(number_tx), number_rx).await
}

async fn get_block_ancestors<Sender>(
	sender: &mut Sender,
	head: Hash,
	num_ancestors: BlockNumber,
) -> FatalResult<Vec<Hash>>
where
	Sender: overseer::DisputeCoordinatorSenderTrait,
{
	let (tx, rx) = oneshot::channel();
	sender
		.send_message(ChainApiMessage::Ancestors {
			hash: head,
			k: num_ancestors as usize,
			response_channel: tx,
		})
		.await;

	rx.await
		.or(Err(FatalError::ChainApiSenderDropped))?
		.map_err(FatalError::ChainApiAncestors)
}

async fn send_message_fatal<Sender, Response>(
	sender: &mut Sender,
	message: ChainApiMessage,
	receiver: oneshot::Receiver<std::result::Result<Response, ChainApiError>>,
) -> FatalResult<Response>
where
	Sender: SubsystemSender<ChainApiMessage>,
{
	sender.send_message(message).await;

	receiver
		.await
		.map_err(|_| FatalError::ChainApiSenderDropped)?
		.map_err(FatalError::ChainApiAncestors)
}
