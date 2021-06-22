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

//! A [`SelectChain`] implementation designed for relay chains.
//!
//! This uses information about parachains to inform GRANDPA and BABE
//! about blocks which are safe to build on and blocks which are safe to
//! finalize.
//!
//! To learn more about chain-selection rules for Relay Chains, please see the
//! documentation on [chain-selection][chain-selection-guide]
//! in the implementers' guide.
//!
//! This is mostly a wrapper around a subsystem which implements the
//! chain-selection rule, which leaves the code to be very simple.
//!
//! However, this does apply the further finality constraints to the best
//! leaf returned from the chain selection subsystem by calling into other
//! subsystems which yield information about approvals and disputes.
//!
//! [chain-selection-guide]: https://w3f.github.io/parachain-implementers-guide/protocol-chain-selection.html

#![cfg(feature = "full-node")]

use {
	polkadot_primitives::v1::{
		Hash, BlockNumber, Block as PolkadotBlock, Header as PolkadotHeader,
	},
	polkadot_subsystem::messages::{ApprovalVotingMessage, ChainSelectionMessage},
	prometheus_endpoint::{self, Registry},
	polkadot_overseer::OverseerHandler,
	futures::channel::oneshot,
	consensus_common::{Error as ConsensusError, SelectChain},
	sp_blockchain::HeaderBackend,
	sp_runtime::generic::BlockId,
	std::sync::Arc,
};

/// The maximum amount of unfinalized blocks we are willing to allow due to approval checking
/// or disputes.
///
/// This is a safety net that should be removed at some point in the future.
const MAX_FINALITY_LAG: polkadot_primitives::v1::BlockNumber = 50;

// TODO [now]: metrics.

/// A chain-selection implementation which provides safety for relay chains.
pub struct SelectRelayChain<B> {
	backend: Arc<B>,
	overseer: OverseerHandler,
	// A fallback to use in case the overseer is disconnected.
	//
	// This is used on relay chains which have not yet enabled
	// parachains as well as situations where the node is offline.
	fallback: sc_consensus::LongestChain<B, PolkadotBlock>,
}

impl<B> SelectRelayChain<B>
	where B: sc_client_api::backend::Backend<PolkadotBlock> + 'static
{
	/// Create a new [`SelectRelayChain`] wrapping the given chain backend
	/// and a handle to the overseer.
	pub fn new(backend: Arc<B>, overseer: OverseerHandler) -> Self {
		SelectRelayChain {
			fallback: sc_consensus::LongestChain::new(backend.clone()),
			backend,
			overseer,
		}
	}
}

impl<B> SelectRelayChain<B> {
	/// Given an overseer handler, this connects the [`SelectRelayChain`]'s
	/// internal handler to the same overseer.
	pub fn connect_overseer_handler(
		&mut self,
		other_handler: &OverseerHandler,
	) {
		other_handler.connect_other(&mut self.overseer);
	}
}

impl<B> Clone for SelectRelayChain<B>
	where B: sc_client_api::backend::Backend<PolkadotBlock> + 'static
{
	fn clone(&self) -> SelectRelayChain<B> {
		SelectRelayChain {
			backend: self.backend.clone(),
			overseer: self.overseer.clone(),
			fallback: self.fallback.clone(),
		}
	}
}

#[derive(thiserror::Error, Debug)]
enum Error {
	// A request to the subsystem was canceled.
	#[error("Overseer is disconnected from Chain Selection")]
	OverseerDisconnected(oneshot::Canceled),
	/// Chain selection returned empty leaves.
	#[error("ChainSelection returned no leaves")]
	EmptyLeaves,
}

#[async_trait::async_trait]
impl<B> SelectChain<PolkadotBlock> for SelectRelayChain<B>
	where B: sc_client_api::backend::Backend<PolkadotBlock> + 'static
{
	/// Get all leaves of the chain, i.e. block hashes that are suitable to
	/// build upon and have no suitable children.
	async fn leaves(&self) -> Result<Vec<Hash>, ConsensusError> {
		if self.overseer.is_disconnected() {
			return self.fallback.leaves().await
		}

		let (tx, rx) = oneshot::channel();

		self.overseer
			.clone()
			.send_msg(ChainSelectionMessage::Leaves(tx)).await;

		rx.await
			.map_err(Error::OverseerDisconnected)
			.map_err(|e| ConsensusError::Other(Box::new(e)))
	}

	/// Among all leaves, pick the one which is the best chain to build upon.
	async fn best_chain(&self) -> Result<PolkadotHeader, ConsensusError> {
		if self.overseer.is_disconnected() {
			return self.fallback.best_chain().await
		}

		// The Chain Selection subsystem is supposed to treat the finalized
		// block as the best leaf in the case that there are no viable
		// leaves, so this should not happen in practice.
		let best_leaf = self.leaves()
			.await?
			.first()
			.ok_or_else(|| ConsensusError::Other(Box::new(Error::EmptyLeaves)))?
			.clone();

		match self.backend.blockchain().header(BlockId::Hash(best_leaf)) {
			Ok(Some(header)) => Ok(header),
			Ok(None) => Err(ConsensusError::ChainLookup(format!(
				"Missing leaf with hash {:?}",
				best_leaf,
			))),
			Err(e) => Err(ConsensusError::ChainLookup(format!(
				"Lookup failed for leaf with hash {:?}: {:?}",
				best_leaf,
				e,
			))),
		}
	}

	/// Get the best descendent of `target_hash` that we should attempt to
	/// finalize next, if any. It is valid to return the `target_hash` if
	/// no better block exists.
	///
	/// This will search all leaves to find the best one containing the
	/// given target hash, and then constrain to the given block number.
	///
	/// It will also constrain the chain to only chains which are fully
	/// approved, and chains which contain no disputes.
	async fn finality_target(
		&self,
		target_hash: Hash,
		maybe_max_number: Option<BlockNumber>,
	) -> Result<Option<Hash>, ConsensusError> {
		if self.overseer.is_disconnected() {
			return self.fallback.finality_target(target_hash, maybe_max_number).await
		}

		let mut overseer = self.overseer.clone();

		let subchain_head = {
			let (tx, rx) = oneshot::channel();
			overseer.send_msg(ChainSelectionMessage::BestLeafContaining(target_hash, tx)).await;

			let best = rx.await
				.map_err(Error::OverseerDisconnected)
				.map_err(|e| ConsensusError::Other(Box::new(e)))?;

			match best {
				// No viable leaves containing the block.
				//
				// REVIEW: should we return `Some(target_hash)` here
				// or `None`? Docs are unclear. it seems from GRANDPA
				// usage that `None` is treated as an error variant?
				// Should probably be removed from the API.
				//
				// I think `target_hash` is fine as long as it's a
				// round-estimate in practice.
				None => return Ok(Some(target_hash)),
				Some(best) => best,
			}
		};

		// 1. Constrain the leaf according to `maybe_max_number`.
		let subchain_head = match maybe_max_number {
			None => subchain_head,
			Some(max) => {
				// find the current number.
				let res = self.backend.blockchain().header(BlockId::Hash(subchain_head));
				let subchain_header = match res {
					Ok(Some(header)) => header,
					Ok(None) => return Err(ConsensusError::ChainLookup(format!(
						"No header for leaf hash {:?}",
						subchain_head,
					))),
					Err(e) => return Err(ConsensusError::ChainLookup(format!(
						"Header lookup failed for leaf hash {:?}: {:?}",
						subchain_head,
						e,
					))),
				};

				if subchain_header.number <= max {
					subchain_head
				} else {
					let (ancestor_hash, _) = crate::grandpa_support::walk_backwards_to_target_block(
						self.backend.blockchain(),
						max,
						&subchain_header,
					).map_err(|e| ConsensusError::ChainLookup(format!("{:?}", e)))?;

					ancestor_hash
				}
			}
		};

		// 2. Constrain according to `ApprovedAncestor`.
		let subchain_head = {
			let target_number = match self.backend.blockchain().number(target_hash) {
				Ok(Some(number)) => number,
				Ok(None) => return Err(ConsensusError::ChainLookup(format!(
					"No number for target hash {:?}",
					target_hash,
				))),
				Err(e) => return Err(ConsensusError::ChainLookup(format!(
					"Number lookup failed for target hash {:?}: {:?}",
					target_hash,
					e,
				))),
			};

			let (tx, rx) = oneshot::channel();
			overseer.send_msg(ApprovalVotingMessage::ApprovedAncestor(
				subchain_head,
				target_number,
				tx,
			)).await;

			match rx.await
				.map_err(Error::OverseerDisconnected)
				.map_err(|e| ConsensusError::Other(Box::new(e)))?
			{
				// No approved ancestors means target hash is maximal vote.
				None => return Ok(Some(target_hash)),
				Some((subchain_head, _)) => subchain_head,
			}
		};

		// 3. Constrain according to disputes:
		// TODO: https://github.com/paritytech/polkadot/issues/3164

		Ok(Some(subchain_head))
	}
}
