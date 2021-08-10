// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! Basic delivery strategy. The strategy selects nonces if:
//!
//! 1) there are more nonces on the source side than on the target side;
//! 2) new nonces may be proved to target node (i.e. they have appeared at the
//!    block, which is known to the target node).

use crate::message_race_loop::{NoncesRange, RaceState, RaceStrategy, SourceClientNonces, TargetClientNonces};

use async_trait::async_trait;
use bp_messages::MessageNonce;
use relay_utils::HeaderId;
use std::{collections::VecDeque, fmt::Debug, marker::PhantomData, ops::RangeInclusive};

/// Queue of nonces known to the source node.
pub type SourceRangesQueue<SourceHeaderHash, SourceHeaderNumber, SourceNoncesRange> =
	VecDeque<(HeaderId<SourceHeaderHash, SourceHeaderNumber>, SourceNoncesRange)>;

/// Nonces delivery strategy.
#[derive(Debug)]
pub struct BasicStrategy<
	SourceHeaderNumber,
	SourceHeaderHash,
	TargetHeaderNumber,
	TargetHeaderHash,
	SourceNoncesRange,
	Proof,
> {
	/// All queued nonces.
	source_queue: SourceRangesQueue<SourceHeaderHash, SourceHeaderNumber, SourceNoncesRange>,
	/// Best nonce known to target node (at its best block). `None` if it has not been received yet.
	best_target_nonce: Option<MessageNonce>,
	/// Unused generic types dump.
	_phantom: PhantomData<(TargetHeaderNumber, TargetHeaderHash, Proof)>,
}

impl<SourceHeaderNumber, SourceHeaderHash, TargetHeaderNumber, TargetHeaderHash, SourceNoncesRange, Proof>
	BasicStrategy<SourceHeaderNumber, SourceHeaderHash, TargetHeaderNumber, TargetHeaderHash, SourceNoncesRange, Proof>
where
	SourceHeaderHash: Clone,
	SourceHeaderNumber: Clone + Ord,
	SourceNoncesRange: NoncesRange,
{
	/// Create new delivery strategy.
	pub fn new() -> Self {
		BasicStrategy {
			source_queue: VecDeque::new(),
			best_target_nonce: None,
			_phantom: Default::default(),
		}
	}

	/// Reference to source queue.
	pub(crate) fn source_queue(
		&self,
	) -> &VecDeque<(HeaderId<SourceHeaderHash, SourceHeaderNumber>, SourceNoncesRange)> {
		&self.source_queue
	}

	/// Mutable reference to source queue to use in tests.
	#[cfg(test)]
	pub(crate) fn source_queue_mut(
		&mut self,
	) -> &mut VecDeque<(HeaderId<SourceHeaderHash, SourceHeaderNumber>, SourceNoncesRange)> {
		&mut self.source_queue
	}

	/// Returns index of the latest source queue entry, that may be delivered to the target node.
	///
	/// Returns `None` if no entries may be delivered. All entries before and including the `Some(_)`
	/// index are guaranteed to be witnessed at source blocks that are known to be finalized at the
	/// target node.
	pub fn maximal_available_source_queue_index(
		&self,
		race_state: RaceState<
			HeaderId<SourceHeaderHash, SourceHeaderNumber>,
			HeaderId<TargetHeaderHash, TargetHeaderNumber>,
			Proof,
		>,
	) -> Option<usize> {
		// if we do not know best nonce at target node, we can't select anything
		let _ = self.best_target_nonce?;

		// if we have already selected nonces that we want to submit, do nothing
		if race_state.nonces_to_submit.is_some() {
			return None;
		}

		// if we already submitted some nonces, do nothing
		if race_state.nonces_submitted.is_some() {
			return None;
		}

		// 1) we want to deliver all nonces, starting from `target_nonce + 1`
		// 2) we can't deliver new nonce until header, that has emitted this nonce, is finalized
		// by target client
		// 3) selector is used for more complicated logic
		//
		// => let's first select range of entries inside deque that are already finalized at
		// the target client and pass this range to the selector
		let best_header_at_target = race_state.best_finalized_source_header_id_at_best_target?;
		self.source_queue
			.iter()
			.enumerate()
			.take_while(|(_, (queued_at, _))| queued_at.0 <= best_header_at_target.0)
			.map(|(index, _)| index)
			.last()
	}

	/// Remove all nonces that are less than or equal to given nonce from the source queue.
	pub fn remove_le_nonces_from_source_queue(&mut self, nonce: MessageNonce) {
		while let Some((queued_at, queued_range)) = self.source_queue.pop_front() {
			if let Some(range_to_requeue) = queued_range.greater_than(nonce) {
				self.source_queue.push_front((queued_at, range_to_requeue));
				break;
			}
		}
	}
}

#[async_trait]
impl<SourceHeaderNumber, SourceHeaderHash, TargetHeaderNumber, TargetHeaderHash, SourceNoncesRange, Proof>
	RaceStrategy<HeaderId<SourceHeaderHash, SourceHeaderNumber>, HeaderId<TargetHeaderHash, TargetHeaderNumber>, Proof>
	for BasicStrategy<SourceHeaderNumber, SourceHeaderHash, TargetHeaderNumber, TargetHeaderHash, SourceNoncesRange, Proof>
where
	SourceHeaderHash: Clone + Debug + Send,
	SourceHeaderNumber: Clone + Ord + Debug + Send,
	SourceNoncesRange: NoncesRange + Debug + Send,
	TargetHeaderHash: Debug + Send,
	TargetHeaderNumber: Debug + Send,
	Proof: Debug + Send,
{
	type SourceNoncesRange = SourceNoncesRange;
	type ProofParameters = ();
	type TargetNoncesData = ();

	fn is_empty(&self) -> bool {
		self.source_queue.is_empty()
	}

	fn required_source_header_at_target(
		&self,
		current_best: &HeaderId<SourceHeaderHash, SourceHeaderNumber>,
	) -> Option<HeaderId<SourceHeaderHash, SourceHeaderNumber>> {
		self.source_queue
			.back()
			.and_then(|(h, _)| if h.0 > current_best.0 { Some(h.clone()) } else { None })
	}

	fn best_at_source(&self) -> Option<MessageNonce> {
		let best_in_queue = self.source_queue.back().map(|(_, range)| range.end());
		match (best_in_queue, self.best_target_nonce) {
			(Some(best_in_queue), Some(best_target_nonce)) if best_in_queue > best_target_nonce => Some(best_in_queue),
			(_, Some(best_target_nonce)) => Some(best_target_nonce),
			(_, None) => None,
		}
	}

	fn best_at_target(&self) -> Option<MessageNonce> {
		self.best_target_nonce
	}

	fn source_nonces_updated(
		&mut self,
		at_block: HeaderId<SourceHeaderHash, SourceHeaderNumber>,
		nonces: SourceClientNonces<SourceNoncesRange>,
	) {
		let best_in_queue = self
			.source_queue
			.back()
			.map(|(_, range)| range.end())
			.or(self.best_target_nonce)
			.unwrap_or_default();
		self.source_queue.extend(
			nonces
				.new_nonces
				.greater_than(best_in_queue)
				.into_iter()
				.map(move |range| (at_block.clone(), range)),
		)
	}

	fn best_target_nonces_updated(
		&mut self,
		nonces: TargetClientNonces<()>,
		race_state: &mut RaceState<
			HeaderId<SourceHeaderHash, SourceHeaderNumber>,
			HeaderId<TargetHeaderHash, TargetHeaderNumber>,
			Proof,
		>,
	) {
		let nonce = nonces.latest_nonce;

		if let Some(best_target_nonce) = self.best_target_nonce {
			if nonce < best_target_nonce {
				return;
			}
		}

		while let Some(true) = self.source_queue.front().map(|(_, range)| range.begin() <= nonce) {
			let maybe_subrange = self
				.source_queue
				.pop_front()
				.and_then(|(at_block, range)| range.greater_than(nonce).map(|subrange| (at_block, subrange)));
			if let Some((at_block, subrange)) = maybe_subrange {
				self.source_queue.push_front((at_block, subrange));
				break;
			}
		}

		let need_to_select_new_nonces = race_state
			.nonces_to_submit
			.as_ref()
			.map(|(_, nonces, _)| *nonces.end() <= nonce)
			.unwrap_or(false);
		if need_to_select_new_nonces {
			race_state.nonces_to_submit = None;
		}

		let need_new_nonces_to_submit = race_state
			.nonces_submitted
			.as_ref()
			.map(|nonces| *nonces.end() <= nonce)
			.unwrap_or(false);
		if need_new_nonces_to_submit {
			race_state.nonces_submitted = None;
		}

		self.best_target_nonce = Some(std::cmp::max(
			self.best_target_nonce.unwrap_or(nonces.latest_nonce),
			nonce,
		));
	}

	fn finalized_target_nonces_updated(
		&mut self,
		nonces: TargetClientNonces<()>,
		_race_state: &mut RaceState<
			HeaderId<SourceHeaderHash, SourceHeaderNumber>,
			HeaderId<TargetHeaderHash, TargetHeaderNumber>,
			Proof,
		>,
	) {
		self.best_target_nonce = Some(std::cmp::max(
			self.best_target_nonce.unwrap_or(nonces.latest_nonce),
			nonces.latest_nonce,
		));
	}

	async fn select_nonces_to_deliver(
		&mut self,
		race_state: RaceState<
			HeaderId<SourceHeaderHash, SourceHeaderNumber>,
			HeaderId<TargetHeaderHash, TargetHeaderNumber>,
			Proof,
		>,
	) -> Option<(RangeInclusive<MessageNonce>, Self::ProofParameters)> {
		let maximal_source_queue_index = self.maximal_available_source_queue_index(race_state)?;
		let range_begin = self.source_queue[0].1.begin();
		let range_end = self.source_queue[maximal_source_queue_index].1.end();
		self.remove_le_nonces_from_source_queue(range_end);
		Some((range_begin..=range_end, ()))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::message_lane::MessageLane;
	use crate::message_lane_loop::tests::{
		header_id, TestMessageLane, TestMessagesProof, TestSourceHeaderHash, TestSourceHeaderNumber,
	};

	type SourceNoncesRange = RangeInclusive<MessageNonce>;

	type BasicStrategy<P> = super::BasicStrategy<
		<P as MessageLane>::SourceHeaderNumber,
		<P as MessageLane>::SourceHeaderHash,
		<P as MessageLane>::TargetHeaderNumber,
		<P as MessageLane>::TargetHeaderHash,
		SourceNoncesRange,
		<P as MessageLane>::MessagesProof,
	>;

	fn source_nonces(new_nonces: SourceNoncesRange) -> SourceClientNonces<SourceNoncesRange> {
		SourceClientNonces {
			new_nonces,
			confirmed_nonce: None,
		}
	}

	fn target_nonces(latest_nonce: MessageNonce) -> TargetClientNonces<()> {
		TargetClientNonces {
			latest_nonce,
			nonces_data: (),
		}
	}

	#[test]
	fn strategy_is_empty_works() {
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		assert!(strategy.is_empty());
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=1));
		assert!(!strategy.is_empty());
	}

	#[test]
	fn best_at_source_is_never_lower_than_target_nonce() {
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		assert_eq!(strategy.best_at_source(), None);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=5));
		assert_eq!(strategy.best_at_source(), None);
		strategy.best_target_nonces_updated(target_nonces(10), &mut Default::default());
		assert_eq!(strategy.source_queue, vec![]);
		assert_eq!(strategy.best_at_source(), Some(10));
	}

	#[test]
	fn source_nonce_is_never_lower_than_known_target_nonce() {
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.best_target_nonces_updated(target_nonces(10), &mut Default::default());
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=5));
		assert_eq!(strategy.source_queue, vec![]);
	}

	#[test]
	fn source_nonce_is_never_lower_than_latest_known_source_nonce() {
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=5));
		strategy.source_nonces_updated(header_id(2), source_nonces(1..=3));
		strategy.source_nonces_updated(header_id(2), source_nonces(1..=5));
		assert_eq!(strategy.source_queue, vec![(header_id(1), 1..=5)]);
	}

	#[test]
	fn target_nonce_is_never_lower_than_latest_known_target_nonce() {
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		assert_eq!(strategy.best_target_nonce, None);
		strategy.best_target_nonces_updated(target_nonces(10), &mut Default::default());
		assert_eq!(strategy.best_target_nonce, Some(10));
		strategy.best_target_nonces_updated(target_nonces(5), &mut Default::default());
		assert_eq!(strategy.best_target_nonce, Some(10));
	}

	#[test]
	fn updated_target_nonce_removes_queued_entries() {
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=5));
		strategy.source_nonces_updated(header_id(2), source_nonces(6..=10));
		strategy.source_nonces_updated(header_id(3), source_nonces(11..=15));
		strategy.source_nonces_updated(header_id(4), source_nonces(16..=20));
		strategy.best_target_nonces_updated(target_nonces(15), &mut Default::default());
		assert_eq!(strategy.source_queue, vec![(header_id(4), 16..=20)]);
		strategy.best_target_nonces_updated(target_nonces(17), &mut Default::default());
		assert_eq!(strategy.source_queue, vec![(header_id(4), 18..=20)]);
	}

	#[test]
	fn selected_nonces_are_dropped_on_target_nonce_update() {
		let mut state = RaceState::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		state.nonces_to_submit = Some((header_id(1), 5..=10, (5..=10, None)));
		strategy.best_target_nonces_updated(target_nonces(7), &mut state);
		assert!(state.nonces_to_submit.is_some());
		strategy.best_target_nonces_updated(target_nonces(10), &mut state);
		assert!(state.nonces_to_submit.is_none());
	}

	#[test]
	fn submitted_nonces_are_dropped_on_target_nonce_update() {
		let mut state = RaceState::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		state.nonces_submitted = Some(5..=10);
		strategy.best_target_nonces_updated(target_nonces(7), &mut state);
		assert!(state.nonces_submitted.is_some());
		strategy.best_target_nonces_updated(target_nonces(10), &mut state);
		assert!(state.nonces_submitted.is_none());
	}

	#[async_std::test]
	async fn nothing_is_selected_if_something_is_already_selected() {
		let mut state = RaceState::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		state.nonces_to_submit = Some((header_id(1), 1..=10, (1..=10, None)));
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=10));
		assert_eq!(strategy.select_nonces_to_deliver(state.clone()).await, None);
	}

	#[async_std::test]
	async fn nothing_is_selected_if_something_is_already_submitted() {
		let mut state = RaceState::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		state.nonces_submitted = Some(1..=10);
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=10));
		assert_eq!(strategy.select_nonces_to_deliver(state.clone()).await, None);
	}

	#[async_std::test]
	async fn select_nonces_to_deliver_works() {
		let mut state = RaceState::<_, _, TestMessagesProof>::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=1));
		strategy.source_nonces_updated(header_id(2), source_nonces(2..=2));
		strategy.source_nonces_updated(header_id(3), source_nonces(3..=6));
		strategy.source_nonces_updated(header_id(5), source_nonces(7..=8));

		state.best_finalized_source_header_id_at_best_target = Some(header_id(4));
		assert_eq!(
			strategy.select_nonces_to_deliver(state.clone()).await,
			Some((1..=6, ()))
		);
		strategy.best_target_nonces_updated(target_nonces(6), &mut state);
		assert_eq!(strategy.select_nonces_to_deliver(state.clone()).await, None);

		state.best_finalized_source_header_id_at_best_target = Some(header_id(5));
		assert_eq!(
			strategy.select_nonces_to_deliver(state.clone()).await,
			Some((7..=8, ()))
		);
		strategy.best_target_nonces_updated(target_nonces(8), &mut state);
		assert_eq!(strategy.select_nonces_to_deliver(state.clone()).await, None);
	}

	#[test]
	fn maximal_available_source_queue_index_works() {
		let mut state = RaceState::<_, _, TestMessagesProof>::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=3));
		strategy.source_nonces_updated(header_id(2), source_nonces(4..=6));
		strategy.source_nonces_updated(header_id(3), source_nonces(7..=9));

		state.best_finalized_source_header_id_at_best_target = Some(header_id(0));
		assert_eq!(strategy.maximal_available_source_queue_index(state.clone()), None);

		state.best_finalized_source_header_id_at_best_target = Some(header_id(1));
		assert_eq!(strategy.maximal_available_source_queue_index(state.clone()), Some(0));

		state.best_finalized_source_header_id_at_best_target = Some(header_id(2));
		assert_eq!(strategy.maximal_available_source_queue_index(state.clone()), Some(1));

		state.best_finalized_source_header_id_at_best_target = Some(header_id(3));
		assert_eq!(strategy.maximal_available_source_queue_index(state.clone()), Some(2));

		state.best_finalized_source_header_id_at_best_target = Some(header_id(4));
		assert_eq!(strategy.maximal_available_source_queue_index(state), Some(2));
	}

	#[test]
	fn remove_le_nonces_from_source_queue_works() {
		let mut state = RaceState::<_, _, TestMessagesProof>::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=3));
		strategy.source_nonces_updated(header_id(2), source_nonces(4..=6));
		strategy.source_nonces_updated(header_id(3), source_nonces(7..=9));

		fn source_queue_nonces(
			source_queue: &SourceRangesQueue<TestSourceHeaderHash, TestSourceHeaderNumber, SourceNoncesRange>,
		) -> Vec<MessageNonce> {
			source_queue.iter().flat_map(|(_, range)| range.clone()).collect()
		}

		strategy.remove_le_nonces_from_source_queue(1);
		assert_eq!(
			source_queue_nonces(&strategy.source_queue),
			vec![2, 3, 4, 5, 6, 7, 8, 9],
		);

		strategy.remove_le_nonces_from_source_queue(5);
		assert_eq!(source_queue_nonces(&strategy.source_queue), vec![6, 7, 8, 9]);

		strategy.remove_le_nonces_from_source_queue(9);
		assert_eq!(source_queue_nonces(&strategy.source_queue), Vec::<MessageNonce>::new());

		strategy.remove_le_nonces_from_source_queue(100);
		assert_eq!(source_queue_nonces(&strategy.source_queue), Vec::<MessageNonce>::new());
	}
}
