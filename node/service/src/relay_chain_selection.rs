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

use super::{HeaderProvider, HeaderProviderProvider};
use consensus_common::{Error as ConsensusError, SelectChain};
use futures::channel::oneshot;
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_overseer::{AllMessages, Handle};
use polkadot_primitives::v1::{
	Block as PolkadotBlock, BlockNumber, Hash, Header as PolkadotHeader,
};
use polkadot_subsystem::messages::{
	ApprovalVotingMessage, ChainSelectionMessage, DisputeCoordinatorMessage,
	HighestApprovedAncestorBlock,
};
use std::sync::Arc;

/// The maximum amount of unfinalized blocks we are willing to allow due to approval checking
/// or disputes.
///
/// This is a safety net that should be removed at some point in the future.
const MAX_FINALITY_LAG: polkadot_primitives::v1::BlockNumber = 500;

const LOG_TARGET: &str = "parachain::chain-selection";

/// Prometheus metrics for chain-selection.
#[derive(Debug, Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

#[derive(Debug, Clone)]
struct MetricsInner {
	approval_checking_finality_lag: prometheus::Gauge<prometheus::U64>,
	disputes_finality_lag: prometheus::Gauge<prometheus::U64>,
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			approval_checking_finality_lag: prometheus::register(
				prometheus::Gauge::with_opts(
					prometheus::Opts::new(
						"parachain_approval_checking_finality_lag",
						"How far behind the head of the chain the Approval Checking protocol wants to vote",
					)
				)?,
				registry,
			)?,
			disputes_finality_lag: prometheus::register(
				prometheus::Gauge::with_opts(
					prometheus::Opts::new(
						"parachain_disputes_finality_lag",
						"How far behind the head of the chain the Disputes protocol wants to vote",
					)
				)?,
				registry,
			)?,
		};

		Ok(Metrics(Some(metrics)))
	}
}

impl Metrics {
	fn note_approval_checking_finality_lag(&self, lag: BlockNumber) {
		if let Some(ref metrics) = self.0 {
			metrics.approval_checking_finality_lag.set(lag as _);
		}
	}

	fn note_disputes_finality_lag(&self, lag: BlockNumber) {
		if let Some(ref metrics) = self.0 {
			metrics.disputes_finality_lag.set(lag as _);
		}
	}
}

/// A chain-selection implementation which provides safety for relay chains.
pub struct SelectRelayChain<B: sc_client_api::Backend<PolkadotBlock>> {
	is_relay_chain: bool,
	longest_chain: sc_consensus::LongestChain<B, PolkadotBlock>,
	selection: SelectRelayChainInner<B, Handle>,
}

impl<B> Clone for SelectRelayChain<B>
where
	B: sc_client_api::Backend<PolkadotBlock>,
	SelectRelayChainInner<B, Handle>: Clone,
{
	fn clone(&self) -> Self {
		Self {
			longest_chain: self.longest_chain.clone(),
			is_relay_chain: self.is_relay_chain,
			selection: self.selection.clone(),
		}
	}
}

impl<B> SelectRelayChain<B>
where
	B: sc_client_api::Backend<PolkadotBlock> + 'static,
{
	/// Create a new [`SelectRelayChain`] wrapping the given chain backend
	/// and a handle to the overseer.
	pub fn new(backend: Arc<B>, overseer: Handle, is_relay_chain: bool, metrics: Metrics) -> Self {
		tracing::debug!(
			target: LOG_TARGET,
			"Using {} as chain selection algorithm",
			if is_relay_chain { "dispute aware relay" } else { "longest" }
		);
		SelectRelayChain {
			longest_chain: sc_consensus::LongestChain::new(backend.clone()),
			selection: SelectRelayChainInner::new(backend, overseer, metrics),
			is_relay_chain,
		}
	}

	/// Allow access to the inner chain, for usage during the node setup.
	pub fn as_longest_chain(&self) -> &sc_consensus::LongestChain<B, PolkadotBlock> {
		&self.longest_chain
	}
}

#[async_trait::async_trait]
impl<B> SelectChain<PolkadotBlock> for SelectRelayChain<B>
where
	B: sc_client_api::Backend<PolkadotBlock> + 'static,
{
	async fn leaves(&self) -> Result<Vec<Hash>, ConsensusError> {
		if !self.is_relay_chain {
			return self.longest_chain.leaves().await
		}

		self.selection.leaves().await
	}

	async fn best_chain(&self) -> Result<PolkadotHeader, ConsensusError> {
		if !self.is_relay_chain {
			return self.longest_chain.best_chain().await
		}
		self.selection.best_chain().await
	}

	async fn finality_target(
		&self,
		target_hash: Hash,
		maybe_max_number: Option<BlockNumber>,
	) -> Result<Hash, ConsensusError> {
		let longest_chain_best =
			self.longest_chain.finality_target(target_hash, maybe_max_number).await?;

		if !self.is_relay_chain {
			return Ok(longest_chain_best)
		}
		self.selection
			.finality_target_with_longest_chain(target_hash, longest_chain_best, maybe_max_number)
			.await
	}
}

/// A chain-selection implementation which provides safety for relay chains
/// but does not handle situations where the overseer is not yet connected.
pub struct SelectRelayChainInner<B, OH> {
	backend: Arc<B>,
	overseer: OH,
	metrics: Metrics,
}

impl<B, OH> SelectRelayChainInner<B, OH>
where
	B: HeaderProviderProvider<PolkadotBlock>,
	OH: OverseerHandleT,
{
	/// Create a new [`SelectRelayChainInner`] wrapping the given chain backend
	/// and a handle to the overseer.
	pub fn new(backend: Arc<B>, overseer: OH, metrics: Metrics) -> Self {
		SelectRelayChainInner { backend, overseer, metrics }
	}

	fn block_header(&self, hash: Hash) -> Result<PolkadotHeader, ConsensusError> {
		match HeaderProvider::header(self.backend.header_provider(), hash) {
			Ok(Some(header)) => Ok(header),
			Ok(None) =>
				Err(ConsensusError::ChainLookup(format!("Missing header with hash {:?}", hash,))),
			Err(e) => Err(ConsensusError::ChainLookup(format!(
				"Lookup failed for header with hash {:?}: {:?}",
				hash, e,
			))),
		}
	}

	fn block_number(&self, hash: Hash) -> Result<BlockNumber, ConsensusError> {
		match HeaderProvider::number(self.backend.header_provider(), hash) {
			Ok(Some(number)) => Ok(number),
			Ok(None) =>
				Err(ConsensusError::ChainLookup(format!("Missing number with hash {:?}", hash,))),
			Err(e) => Err(ConsensusError::ChainLookup(format!(
				"Lookup failed for number with hash {:?}: {:?}",
				hash, e,
			))),
		}
	}
}

impl<B, OH> Clone for SelectRelayChainInner<B, OH>
where
	B: HeaderProviderProvider<PolkadotBlock> + Send + Sync,
	OH: OverseerHandleT,
{
	fn clone(&self) -> Self {
		SelectRelayChainInner {
			backend: self.backend.clone(),
			overseer: self.overseer.clone(),
			metrics: self.metrics.clone(),
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

/// Decoupling trait for the overseer handle.
///
/// Required for testing purposes.
#[async_trait::async_trait]
pub trait OverseerHandleT: Clone + Send + Sync {
	async fn send_msg<M: Send + Into<AllMessages>>(&mut self, msg: M, origin: &'static str);
}

#[async_trait::async_trait]
impl OverseerHandleT for Handle {
	async fn send_msg<M: Send + Into<AllMessages>>(&mut self, msg: M, origin: &'static str) {
		Handle::send_msg(self, msg, origin).await
	}
}

impl<B, OH> SelectRelayChainInner<B, OH>
where
	B: HeaderProviderProvider<PolkadotBlock>,
	OH: OverseerHandleT,
{
	/// Get all leaves of the chain, i.e. block hashes that are suitable to
	/// build upon and have no suitable children.
	async fn leaves(&self) -> Result<Vec<Hash>, ConsensusError> {
		let (tx, rx) = oneshot::channel();

		self.overseer
			.clone()
			.send_msg(ChainSelectionMessage::Leaves(tx), std::any::type_name::<Self>())
			.await;

		let leaves = rx
			.await
			.map_err(Error::OverseerDisconnected)
			.map_err(|e| ConsensusError::Other(Box::new(e)))?;

		tracing::trace!(target: LOG_TARGET, ?leaves, "Chain selection leaves");

		Ok(leaves)
	}

	/// Among all leaves, pick the one which is the best chain to build upon.
	async fn best_chain(&self) -> Result<PolkadotHeader, ConsensusError> {
		// The Chain Selection subsystem is supposed to treat the finalized
		// block as the best leaf in the case that there are no viable
		// leaves, so this should not happen in practice.
		let best_leaf = self
			.leaves()
			.await?
			.first()
			.ok_or_else(|| ConsensusError::Other(Box::new(Error::EmptyLeaves)))?
			.clone();

		tracing::trace!(target: LOG_TARGET, ?best_leaf, "Best chain");

		self.block_header(best_leaf)
	}

	/// Get the best descendant of `target_hash` that we should attempt to
	/// finalize next, if any. It is valid to return the `target_hash` if
	/// no better block exists.
	///
	/// This will search all leaves to find the best one containing the
	/// given target hash, and then constrain to the given block number.
	///
	/// It will also constrain the chain to only chains which are fully
	/// approved, and chains which contain no disputes.
	pub(crate) async fn finality_target_with_longest_chain(
		&self,
		target_hash: Hash,
		best_leaf: Hash,
		maybe_max_number: Option<BlockNumber>,
	) -> Result<Hash, ConsensusError> {
		let mut overseer = self.overseer.clone();
		tracing::trace!(target: LOG_TARGET, ?best_leaf, "Longest chain");

		let subchain_head = if cfg!(feature = "disputes") {
			let (tx, rx) = oneshot::channel();
			overseer
				.send_msg(
					ChainSelectionMessage::BestLeafContaining(target_hash, tx),
					std::any::type_name::<Self>(),
				)
				.await;

			let best = rx
				.await
				.map_err(Error::OverseerDisconnected)
				.map_err(|e| ConsensusError::Other(Box::new(e)))?;

			tracing::trace!(target: LOG_TARGET, ?best, "Best leaf containing");

			match best {
				// No viable leaves containing the block.
				None => return Ok(target_hash),
				Some(best) => best,
			}
		} else {
			tracing::trace!(target: LOG_TARGET, ?best_leaf, "Dummy disputes active");
			if best_leaf == target_hash {
				return Ok(target_hash)
			} else {
				best_leaf
			}
		};

		let target_number = self.block_number(target_hash)?;

		// 1. Constrain the leaf according to `maybe_max_number`.
		let subchain_head = match maybe_max_number {
			None => subchain_head,
			Some(max) => {
				if max <= target_number {
					if max < target_number {
						tracing::warn!(
							LOG_TARGET,
							max_number = max,
							target_number,
							"`finality_target` max number is less than target number",
						);
					}
					return Ok(target_hash)
				}
				// find the current number.
				let subchain_header = self.block_header(subchain_head)?;

				if subchain_header.number <= max {
					tracing::trace!(target: LOG_TARGET, ?best_leaf, "Constrained sub-chain head",);
					subchain_head
				} else {
					let (ancestor_hash, _) =
						crate::grandpa_support::walk_backwards_to_target_block(
							self.backend.header_provider(),
							max,
							&subchain_header,
						)
						.map_err(|e| ConsensusError::ChainLookup(format!("{:?}", e)))?;
					tracing::trace!(
						target: LOG_TARGET,
						?ancestor_hash,
						"Grandpa walk backwards sub-chain head"
					);
					ancestor_hash
				}
			},
		};

		let initial_leaf = subchain_head;
		let initial_leaf_number = self.block_number(initial_leaf)?;

		// 2. Constrain according to `ApprovedAncestor`.
		let (subchain_head, subchain_number, subchain_block_descriptions) = {
			let (tx, rx) = oneshot::channel();
			overseer
				.send_msg(
					ApprovalVotingMessage::ApprovedAncestor(subchain_head, target_number, tx),
					std::any::type_name::<Self>(),
				)
				.await;

			match rx
				.await
				.map_err(Error::OverseerDisconnected)
				.map_err(|e| ConsensusError::Other(Box::new(e)))?
			{
				// No approved ancestors means target hash is maximal vote.
				None => (target_hash, target_number, Vec::new()),
				Some(HighestApprovedAncestorBlock { number, hash, descriptions }) =>
					(hash, number, descriptions),
			}
		};

		tracing::trace!(
			target: LOG_TARGET,
			?subchain_head,
			"Ancestor approval restriction applied",
		);

		let lag = initial_leaf_number.saturating_sub(subchain_number);
		self.metrics.note_approval_checking_finality_lag(lag);

		let (lag, subchain_head) = if cfg!(feature = "disputes") {
			// Prevent sending flawed data to the dispute-coordinator.
			if Some(subchain_block_descriptions.len() as _) !=
				subchain_number.checked_sub(target_number)
			{
				tracing::error!(
					LOG_TARGET,
					present_block_descriptions = subchain_block_descriptions.len(),
					target_number,
					subchain_number,
					"Mismatch of anticipated block descriptions and block number difference.",
				);
				return Ok(target_hash)
			}
			// 3. Constrain according to disputes:
			let (tx, rx) = oneshot::channel();
			overseer
				.send_msg(
					DisputeCoordinatorMessage::DetermineUndisputedChain {
						base: (target_number, target_hash),
						block_descriptions: subchain_block_descriptions,
						tx,
					},
					std::any::type_name::<Self>(),
				)
				.await;
			let (subchain_number, subchain_head) = rx
				.await
				.map_err(Error::OverseerDisconnected)
				.map_err(|e| ConsensusError::Other(Box::new(e)))?;

			// The the total lag accounting for disputes.
			let lag_disputes = initial_leaf_number.saturating_sub(subchain_number);
			self.metrics.note_disputes_finality_lag(lag_disputes);
			(lag_disputes, subchain_head)
		} else {
			(lag, subchain_head)
		};

		tracing::trace!(
			target: LOG_TARGET,
			?subchain_head,
			"Disputed blocks in ancestry restriction applied",
		);

		// 4. Apply the maximum safeguard to the finality lag.
		if lag > MAX_FINALITY_LAG {
			// We need to constrain our vote as a safety net to
			// ensure the network continues to finalize.
			let safe_target = initial_leaf_number - MAX_FINALITY_LAG;

			if safe_target <= target_number {
				tracing::warn!(target: LOG_TARGET, ?target_hash, "Safeguard enforced finalization");
				// Minimal vote needs to be on the target number.
				Ok(target_hash)
			} else {
				// Otherwise we're looking for a descendant.
				let initial_leaf_header = self.block_header(initial_leaf)?;
				let (forced_target, _) = crate::grandpa_support::walk_backwards_to_target_block(
					self.backend.header_provider(),
					safe_target,
					&initial_leaf_header,
				)
				.map_err(|e| ConsensusError::ChainLookup(format!("{:?}", e)))?;

				tracing::warn!(
					target: LOG_TARGET,
					?forced_target,
					"Safeguard enforced finalization of child"
				);

				Ok(forced_target)
			}
		} else {
			Ok(subchain_head)
		}
	}
}
