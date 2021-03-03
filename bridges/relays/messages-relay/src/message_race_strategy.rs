// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

use bp_message_lane::MessageNonce;
use relay_utils::HeaderId;
use std::{collections::VecDeque, fmt::Debug, marker::PhantomData, ops::RangeInclusive};

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
	source_queue: VecDeque<(HeaderId<SourceHeaderHash, SourceHeaderNumber>, SourceNoncesRange)>,
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

	/// Mutable reference to source queue to use in tests.
	#[cfg(test)]
	pub(crate) fn source_queue_mut(
		&mut self,
	) -> &mut VecDeque<(HeaderId<SourceHeaderHash, SourceHeaderNumber>, SourceNoncesRange)> {
		&mut self.source_queue
	}

	/// Should return `Some(nonces)` if we need to deliver proof of `nonces` (and associated
	/// data) from source to target node.
	///
	/// The `selector` function receives range of nonces and should return `None` if the whole
	/// range needs to be delivered. If there are some nonces in the range that can't be delivered
	/// right now, it should return `Some` with 'undeliverable' nonces. Please keep in mind that
	/// this should be the sub-range that the passed range ends with, because nonces are always
	/// delivered in-order. Otherwise the function will panic.
	pub fn select_nonces_to_deliver_with_selector(
		&mut self,
		race_state: &RaceState<
			HeaderId<SourceHeaderHash, SourceHeaderNumber>,
			HeaderId<TargetHeaderHash, TargetHeaderNumber>,
			Proof,
		>,
		mut selector: impl FnMut(SourceNoncesRange) -> Option<SourceNoncesRange>,
	) -> Option<RangeInclusive<MessageNonce>> {
		// if we do not know best nonce at target node, we can't select anything
		let target_nonce = self.best_target_nonce?;

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
		let best_header_at_target = &race_state.best_finalized_source_header_id_at_best_target.as_ref()?;
		let mut nonces_end = None;
		while let Some((queued_at, queued_range)) = self.source_queue.pop_front() {
			// select (sub) range to deliver
			let queued_range_begin = queued_range.begin();
			let queued_range_end = queued_range.end();
			let range_to_requeue = if queued_at.0 > best_header_at_target.0 {
				// if header that has queued the range is not yet finalized at bridged chain,
				// we can't prove anything
				Some(queued_range)
			} else {
				// selector returns `Some(range)` if this `range` needs to be requeued
				selector(queued_range)
			};

			// requeue (sub) range and update range to deliver
			match range_to_requeue {
				Some(range_to_requeue) => {
					assert!(
						range_to_requeue.begin() <= range_to_requeue.end()
							&& range_to_requeue.begin() >= queued_range_begin
							&& range_to_requeue.end() == queued_range_end,
						"Incorrect implementation of internal `selector` function. Expected original\
						range {:?} to end with returned range {:?}",
						queued_range_begin..=queued_range_end,
						range_to_requeue,
					);

					if range_to_requeue.begin() != queued_range_begin {
						nonces_end = Some(range_to_requeue.begin() - 1);
					}
					self.source_queue.push_front((queued_at, range_to_requeue));
					break;
				}
				None => {
					nonces_end = Some(queued_range_end);
				}
			}
		}

		nonces_end.map(|nonces_end| RangeInclusive::new(target_nonce + 1, nonces_end))
	}
}

impl<SourceHeaderNumber, SourceHeaderHash, TargetHeaderNumber, TargetHeaderHash, SourceNoncesRange, Proof>
	RaceStrategy<HeaderId<SourceHeaderHash, SourceHeaderNumber>, HeaderId<TargetHeaderHash, TargetHeaderNumber>, Proof>
	for BasicStrategy<SourceHeaderNumber, SourceHeaderHash, TargetHeaderNumber, TargetHeaderHash, SourceNoncesRange, Proof>
where
	SourceHeaderHash: Clone + Debug,
	SourceHeaderNumber: Clone + Ord + Debug,
	SourceNoncesRange: NoncesRange + Debug,
	TargetHeaderHash: Debug,
	TargetHeaderNumber: Debug,
	Proof: Debug,
{
	type SourceNoncesRange = SourceNoncesRange;
	type ProofParameters = ();
	type TargetNoncesData = ();

	fn is_empty(&self) -> bool {
		self.source_queue.is_empty()
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

	fn select_nonces_to_deliver(
		&mut self,
		race_state: &RaceState<
			HeaderId<SourceHeaderHash, SourceHeaderNumber>,
			HeaderId<TargetHeaderHash, TargetHeaderNumber>,
			Proof,
		>,
	) -> Option<(RangeInclusive<MessageNonce>, Self::ProofParameters)> {
		self.select_nonces_to_deliver_with_selector(race_state, |_| None)
			.map(|range| (range, ()))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::message_lane::MessageLane;
	use crate::message_lane_loop::tests::{header_id, TestMessageLane, TestMessagesProof};

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
		assert_eq!(strategy.is_empty(), true);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=1));
		assert_eq!(strategy.is_empty(), false);
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

	#[test]
	fn nothing_is_selected_if_something_is_already_selected() {
		let mut state = RaceState::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		state.nonces_to_submit = Some((header_id(1), 1..=10, (1..=10, None)));
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=10));
		assert_eq!(strategy.select_nonces_to_deliver(&state), None);
	}

	#[test]
	fn nothing_is_selected_if_something_is_already_submitted() {
		let mut state = RaceState::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		state.nonces_submitted = Some(1..=10);
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=10));
		assert_eq!(strategy.select_nonces_to_deliver(&state), None);
	}

	#[test]
	fn select_nonces_to_deliver_works() {
		let mut state = RaceState::<_, _, TestMessagesProof>::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=1));
		strategy.source_nonces_updated(header_id(2), source_nonces(2..=2));
		strategy.source_nonces_updated(header_id(3), source_nonces(3..=6));
		strategy.source_nonces_updated(header_id(5), source_nonces(7..=8));

		state.best_finalized_source_header_id_at_best_target = Some(header_id(4));
		assert_eq!(strategy.select_nonces_to_deliver(&state), Some((1..=6, ())));
		strategy.best_target_nonces_updated(target_nonces(6), &mut state);
		assert_eq!(strategy.select_nonces_to_deliver(&state), None);

		state.best_finalized_source_header_id_at_best_target = Some(header_id(5));
		assert_eq!(strategy.select_nonces_to_deliver(&state), Some((7..=8, ())));
		strategy.best_target_nonces_updated(target_nonces(8), &mut state);
		assert_eq!(strategy.select_nonces_to_deliver(&state), None);
	}

	#[test]
	fn select_nonces_to_deliver_able_to_split_ranges_with_selector() {
		let mut state = RaceState::<_, _, TestMessagesProof>::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.best_target_nonces_updated(target_nonces(0), &mut state);
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=100));

		state.best_finalized_source_header_id_at_source = Some(header_id(1));
		state.best_finalized_source_header_id_at_best_target = Some(header_id(1));
		state.best_target_header_id = Some(header_id(1));

		assert_eq!(
			strategy.select_nonces_to_deliver_with_selector(&state, |_| Some(50..=100)),
			Some(1..=49),
		);
	}

	fn run_panic_test_for_incorrect_selector(
		invalid_selector: impl Fn(SourceNoncesRange) -> Option<SourceNoncesRange>,
	) {
		let mut state = RaceState::<_, _, TestMessagesProof>::default();
		let mut strategy = BasicStrategy::<TestMessageLane>::new();
		strategy.source_nonces_updated(header_id(1), source_nonces(1..=100));
		strategy.best_target_nonces_updated(target_nonces(50), &mut state);
		state.best_finalized_source_header_id_at_source = Some(header_id(1));
		state.best_finalized_source_header_id_at_best_target = Some(header_id(1));
		state.best_target_header_id = Some(header_id(1));
		strategy.select_nonces_to_deliver_with_selector(&state, invalid_selector);
	}

	#[test]
	#[should_panic]
	fn select_nonces_to_deliver_panics_if_selector_returns_empty_range() {
		#[allow(clippy::reversed_empty_ranges)]
		run_panic_test_for_incorrect_selector(|_| Some(2..=1))
	}

	#[test]
	#[should_panic]
	fn select_nonces_to_deliver_panics_if_selector_returns_range_that_starts_before_passed_range() {
		run_panic_test_for_incorrect_selector(|range| Some(range.begin() - 1..=*range.end()))
	}

	#[test]
	#[should_panic]
	fn select_nonces_to_deliver_panics_if_selector_returns_range_with_mismatched_end() {
		run_panic_test_for_incorrect_selector(|range| Some(range.begin()..=*range.end() + 1))
	}
}
