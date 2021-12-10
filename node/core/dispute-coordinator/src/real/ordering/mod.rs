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
	cmp::{Ord, Ordering, PartialOrd},
	collections::{BTreeMap, HashSet},
};

use futures::channel::oneshot;

use polkadot_node_subsystem::{
	messages::ChainApiMessage, ActivatedLeaf, ActiveLeavesUpdate, ChainApiError, SubsystemSender,
};
use polkadot_node_subsystem_util::runtime::get_candidate_events;
use polkadot_primitives::v1::{BlockNumber, CandidateEvent, CandidateHash, CandidateReceipt, Hash};

use super::{
	error::{Fatal, FatalResult, NonFatalResult, Result},
	LOG_TARGET,
};

#[cfg(test)]
mod tests;

/// Provider of `CandidateComparator` for candidates.
pub struct OrderingProvider {
	/// All candidates we have seen included, which not yet have been finalized.
	included_candidates: HashSet<CandidateHash>,
	/// including block -> `CandidateHash`
	///
	/// We need this to clean up `included_candidates` on `ActiveLeavesUpdate`.
	candidates_by_block_number: BTreeMap<BlockNumber, HashSet<CandidateHash>>,
	/// Hash of the latest relay block observed by the provider. It's assumed that
	/// all ancestors of this block are already processed, i.e. we have saved corresponding
	/// included candidates.
	last_observed_block: Hash,
}

/// `Comparator` for ordering of disputes for candidates.
///
/// This `comparator` makes it possible to order disputes based on age and to ensure some fairness
/// between chains in case of equally old disputes.
///
/// Objective ordering between nodes is important in case of lots disputes, so nodes will pull in
/// the same direction and work on resolving the same disputes first. This ensures that we will
/// conclude some disputes, even if there are lots of them. While any objective ordering would
/// suffice for this goal, ordering by age ensures we are not only resolving disputes, but also
/// resolve the oldest one first, which are also the most urgent and important ones to resolve.
///
/// Note: That by `oldest` we mean oldest in terms of relay chain block number, for any block
/// number that has not yet been finalized. If a block has been finalized already it should be
/// treated as low priority when it comes to disputes, as even in the case of a negative outcome,
/// we are already too late. The ordering mechanism here serves to prevent this from happening in
/// the first place.
#[derive(Copy, Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct CandidateComparator {
	/// Block number of the relay parent.
	///
	/// Important, so we will be participating in oldest disputes first.
	///
	/// Note: In theory it would make more sense to use the `BlockNumber` of the including
	/// block, as inclusion time is the actual relevant event when it comes to ordering. The
	/// problem is, that a candidate can get included multiple times on forks, so the `BlockNumber`
	/// of the including block is not unique. We could theoretically work around that problem, by
	/// just using the lowest `BlockNumber` of all available including blocks - the problem is,
	/// that is not stable. If a new fork appears after the fact, we would start ordering the same
	/// candidate differently, which would result in the same candidate getting queued twice.
	relay_parent_block_number: BlockNumber,
	/// By adding the `CandidateHash`, we can guarantee a unique ordering across candidates.
	candidate_hash: CandidateHash,
}

impl PartialEq for CandidateComparator {
	fn eq(&self, other: &CandidateComparator) -> bool {
		Ordering::Equal == self.cmp(other)
	}
}

impl Eq for CandidateComparator {}

impl PartialOrd for CandidateComparator {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for CandidateComparator {
	fn cmp(&self, other: &Self) -> Ordering {
		match self.relay_parent_block_number.cmp(&other.relay_parent_block_number) {
			Ordering::Equal => (),
			o => return o,
		}
		self.candidate_hash.cmp(&other.candidate_hash)
	}
}

impl CandidateComparator {
	/// Create a candidate comparator based on given (fake) values.
	///
	/// Useful for testing.
	#[cfg(test)]
	pub fn new_dummy(block_number: BlockNumber, candidate_hash: CandidateHash) -> Self {
		Self { relay_parent_block_number: block_number, candidate_hash }
	}
	/// Check whether the given candidate hash belongs to this comparator.
	pub fn matches_candidate(&self, candidate_hash: &CandidateHash) -> bool {
		&self.candidate_hash == candidate_hash
	}
}

impl OrderingProvider {
	/// Create a properly initialized `OrderingProvider`.
	pub async fn new<Sender: SubsystemSender>(
		sender: &mut Sender,
		initial_head: ActivatedLeaf,
	) -> Result<Self> {
		// Try to fetch the latest finalized block. Storing it in `last_observed_block`
		// will enforce traversing non-finalized ancestors of `initial_head` in
		// `process_active_leaves_update`.
		let leaf = initial_head.hash;
		let last_observed_block = get_finalized_block_hash(sender).await?.unwrap_or_else(|| {
			tracing::warn!(
				target: LOG_TARGET,
				"No finalized block returned by the Chain API during start up: initial head ancestors will be ignored"
			);
			leaf
		});

		let mut s = Self {
			included_candidates: HashSet::new(),
			candidates_by_block_number: BTreeMap::new(),
			last_observed_block,
		};
		let update =
			ActiveLeavesUpdate { activated: Some(initial_head), deactivated: Default::default() };
		s.process_active_leaves_update(sender, &update).await?;
		Ok(s)
	}

	/// Retrieve a candidate `comparator` if available.
	///
	/// If not available, we can treat disputes concerning this candidate with low priority and
	/// should use spam slots for such disputes.
	pub async fn candidate_comparator<'a>(
		&mut self,
		sender: &mut impl SubsystemSender,
		candidate: &CandidateReceipt,
	) -> FatalResult<Option<CandidateComparator>> {
		let candidate_hash = candidate.hash();
		if !self.included_candidates.contains(&candidate_hash) {
			return Ok(None)
		}
		let n = match get_block_number(sender, candidate.descriptor().relay_parent).await? {
			None => {
				tracing::warn!(
					target: LOG_TARGET,
					candidate_hash = ?candidate_hash,
					"Candidate's relay_parent could not be found via chain API, but we saw candidate included?!"
				);
				return Ok(None)
			},
			Some(n) => n,
		};

		Ok(Some(CandidateComparator { relay_parent_block_number: n, candidate_hash }))
	}

	/// Query active leaves for any candidate `CandidateEvent::CandidateIncluded` events.
	///
	/// and updates current heads, so we can query candidates for all non finalized blocks.
	pub async fn process_active_leaves_update<Sender: SubsystemSender>(
		&mut self,
		sender: &mut Sender,
		update: &ActiveLeavesUpdate,
	) -> Result<()> {
		if let Some(activated) = update.activated.as_ref() {
			// Fetch ancestors of the activated leaf up until the cached one.
			let ancestors =
				get_block_ancestors_hashes(sender, activated.hash, self.last_observed_block)
					.await
					.unwrap_or_default();
			let blocks_to_process = std::iter::once(activated.hash).chain(ancestors);
			for block_hash in blocks_to_process {
				// Get included events:
				let included = get_candidate_events(sender, block_hash)
					.await?
					.into_iter()
					.filter_map(|ev| match ev {
						CandidateEvent::CandidateIncluded(receipt, _, _, _) => Some(receipt),
						_ => None,
					});
				for receipt in included {
					let candidate_hash = receipt.hash();
					self.included_candidates.insert(candidate_hash);
					self.candidates_by_block_number
						.entry(activated.number)
						.or_default()
						.insert(candidate_hash);
				}
			}

			self.last_observed_block = activated.hash;
		}

		Ok(())
	}

	/// Prune finalized candidates.
	///
	/// Once a candidate lives in a relay chain block that's behind the finalized chain/got
	/// finalized, we can treat it as low priority.
	pub fn process_finalized_block(&mut self, finalized: &BlockNumber) {
		let not_finalized = self.candidates_by_block_number.split_off(finalized);
		let finalized = std::mem::take(&mut self.candidates_by_block_number);
		self.candidates_by_block_number = not_finalized;
		// Clean up finalized:
		for finalized_candidate in finalized.into_values().flatten() {
			self.included_candidates.remove(&finalized_candidate);
		}
	}
}

async fn send_message_fatal<Sender, Response>(
	sender: &mut Sender,
	message: ChainApiMessage,
	receiver: oneshot::Receiver<std::result::Result<Response, ChainApiError>>,
) -> FatalResult<Response>
where
	Sender: SubsystemSender,
{
	sender.send_message(message.into()).await;

	receiver
		.await
		.map_err(|_| Fatal::ChainApiSenderDropped)?
		.map_err(Fatal::ChainApi)
}

async fn get_block_number(
	sender: &mut impl SubsystemSender,
	relay_parent: Hash,
) -> FatalResult<Option<BlockNumber>> {
	let (tx, rx) = oneshot::channel();
	send_message_fatal(sender, ChainApiMessage::BlockNumber(relay_parent, tx), rx).await
}

async fn get_finalized_block_hash(sender: &mut impl SubsystemSender) -> FatalResult<Option<Hash>> {
	let (number_tx, number_rx) = oneshot::channel();
	let number =
		send_message_fatal(sender, ChainApiMessage::FinalizedBlockNumber(number_tx), number_rx)
			.await?;

	let (hash_tx, hash_rx) = oneshot::channel();
	send_message_fatal(sender, ChainApiMessage::FinalizedBlockHash(number, hash_tx), hash_rx).await
}

/// Returns ancestor hashes of `head` in the descending order, stopping
/// either at `last_observed_hash` or the genesis block.
///
/// Suited specifically for querying non-finalized chains, thus doesn't
/// rely on block numbers.
///
/// The caller must ensure that `last_observed_hash` is indeed an ancestor
/// of `head` in order to avoid traversing the whole chain.
/// Both `head` and `last_observed_hash` are **not** included in the result.
async fn get_block_ancestors_hashes<Sender>(
	sender: &mut Sender,
	mut head: Hash,
	last_observed_hash: Hash,
) -> NonFatalResult<Vec<Hash>>
where
	Sender: SubsystemSender,
{
	const ANCESTRY_STEP: usize = 4;

	let mut ancestors = Vec::new();

	if head == last_observed_hash {
		return Ok(ancestors)
	}

	loop {
		let (tx, rx) = oneshot::channel();
		let hashes = {
			sender
				.send_message(
					ChainApiMessage::Ancestors {
						hash: head,
						k: ANCESTRY_STEP,
						response_channel: tx,
					}
					.into(),
				)
				.await;

			rx.await??
		};

		for hash in &hashes {
			if *hash == last_observed_hash {
				return Ok(ancestors)
			}

			ancestors.push(*hash);
		}

		match hashes.last() {
			Some(last_hash) => head = *last_hash,
			None => break,
		}
	}
	return Ok(ancestors)
}
