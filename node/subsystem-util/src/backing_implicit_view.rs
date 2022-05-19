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
	messages::{ChainApiMessage, ProspectiveParachainsMessage},
	SubsystemSender,
};
use polkadot_primitives::vstaging::{BlockNumber, Hash, Id as ParaId};

use std::collections::HashMap;

/// Handles the implicit view of the relay chain derived from the immediate view, which
/// is composed of active leaves, and the minimum relay-parents allowed for
/// candidates of various parachains at those leaves.
#[derive(Default, Clone)]
pub struct View {
	leaves: HashMap<Hash, ImplicitActiveLeafData>,
	block_info_storage: HashMap<Hash, BlockInfo>,
}

#[derive(Clone)]
struct ImplicitActiveLeafData {
	// minimum relay parents can only be fetched for active leaves,
	// so this will be empty for all blocks that haven't ever been
	// witnessed as active leaves.
	minimum_relay_parents: HashMap<ParaId, BlockNumber>,
	// The minimum of all minimum relay parents for all paras
	// in `minimum_relay_parents`
	minimum_relay_ancestor: BlockNumber,
	number: BlockNumber,
	// Ancestry, in descending order, starting from the parent block down
	// to the `minimum_relay_ancestor`.
	ancestry: Vec<Hash>,
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
	maybe_minimum_relay_parents: Option<HashMap<ParaId, BlockNumber>>,
	parent_hash: Hash,
}

impl View {
	/// Update the view to a new view, preserving previous
	pub async fn update<Sender>(&mut self, sender: &mut Sender, new_view: Vec<Hash>)
	where
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

			if let Some(x) = res {
				self.leaves.insert(leaf_hash, x);
			}
		}

		// Prune everything before the minimum out of all leaves,
		// pruning absolutely everything if there are no leaves (empty view)
		//
		// Pruning by block number does leave behind orphaned forks slightly longer
		// but the memory overhead is negligible.
		{
			let minimum = self.leaves.values().map(|l| l.minimum_relay_ancestor).min();

			self.block_info_storage
				.retain(|_, i| minimum.map_or(false, |m| i.block_number < m));
		}
	}
}

async fn fetch_fresh_leaf_and_insert_ancestry<Sender>(
	leaf_hash: Hash,
	block_info_storage: &mut HashMap<Hash, BlockInfo>,
	sender: &mut Sender,
) -> Option<ImplicitActiveLeafData>
where
	Sender: SubsystemSender<ChainApiMessage>,
	Sender: SubsystemSender<ProspectiveParachainsMessage>,
{
	let min_relay_parents = {
		let (tx, rx) = oneshot::channel();
		sender
			.send_message(ProspectiveParachainsMessage::GetMinimumRelayParents(leaf_hash, tx))
			.await;

		match rx.await {
			Ok(m) => m,
			Err(_) => return None,
		}
	};

	let leaf_header = {
		let (tx, rx) = oneshot::channel();
		sender.send_message(ChainApiMessage::BlockHeader(leaf_hash, tx)).await;

		match rx.await {
			Ok(Ok(Some(header))) => header,
			Ok(Ok(None)) => return None,
			Ok(Err(_)) => return None,
			Err(_) => return None,
		}
	};

	let min_min = min_relay_parents.iter().map(|x| x.1).min().unwrap_or(leaf_header.number);

	let leaf_block_info = BlockInfo {
		parent_hash: leaf_header.parent_hash,
		block_number: leaf_header.number,
		maybe_minimum_relay_parents: Some(min_relay_parents.iter().cloned().collect()),
	};

	block_info_storage.insert(leaf_hash, leaf_block_info);

	let ancestry = if leaf_header.number > 0 {
		let mut next_ancestor_number = leaf_header.number - 1;
		let mut next_ancestor_hash = leaf_header.parent_hash;
		let mut ancestry = Vec::with_capacity(leaf_header.number.saturating_sub(min_min) as _);

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
					Ok(Ok(None)) => break,
					Ok(Err(_)) => break,
					Err(_) => break,
				};

				block_info_storage.insert(
					next_ancestor_hash,
					BlockInfo {
						block_number: next_ancestor_number,
						parent_hash: header.parent_hash,
						maybe_minimum_relay_parents: None,
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

	Some(ImplicitActiveLeafData {
		minimum_relay_parents: min_relay_parents.iter().cloned().collect(),
		minimum_relay_ancestor: min_min,
		ancestry,
		number: leaf_header.number,
	})
}
