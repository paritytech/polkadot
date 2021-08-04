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

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Everything about outgoing messages sending.

use bitvec::prelude::*;
use bp_messages::{
	DeliveredMessages, DispatchResultsBitVec, LaneId, MessageData, MessageNonce, OutboundLaneData, UnrewardedRelayer,
};
use frame_support::RuntimeDebug;
use sp_std::collections::vec_deque::VecDeque;

/// Outbound lane storage.
pub trait OutboundLaneStorage {
	/// Delivery and dispatch fee type on source chain.
	type MessageFee;

	/// Lane id.
	fn id(&self) -> LaneId;
	/// Get lane data from the storage.
	fn data(&self) -> OutboundLaneData;
	/// Update lane data in the storage.
	fn set_data(&mut self, data: OutboundLaneData);
	/// Returns saved outbound message payload.
	#[cfg(test)]
	fn message(&self, nonce: &MessageNonce) -> Option<MessageData<Self::MessageFee>>;
	/// Save outbound message in the storage.
	fn save_message(&mut self, nonce: MessageNonce, message_data: MessageData<Self::MessageFee>);
	/// Remove outbound message from the storage.
	fn remove_message(&mut self, nonce: &MessageNonce);
}

/// Result of messages receival confirmation.
#[derive(RuntimeDebug, PartialEq, Eq)]
pub enum ReceivalConfirmationResult {
	/// New messages have been confirmed by the confirmation transaction.
	ConfirmedMessages(DeliveredMessages),
	/// Confirmation transaction brings no new confirmation. This may be a result of relayer
	/// error or several relayers runnng.
	NoNewConfirmations,
	/// Bridged chain is trying to confirm more messages than we have generated. May be a result
	/// of invalid bridged chain storage.
	FailedToConfirmFutureMessages,
	/// The unrewarded relayers vec contains an empty entry. May be a result of invalid bridged
	/// chain storage.
	EmptyUnrewardedRelayerEntry,
	/// The unrewarded relayers vec contains non-consecutive entries. May be a result of invalid bridged
	/// chain storage.
	NonConsecutiveUnrewardedRelayerEntries,
	/// The unrewarded relayers vec contains entry with mismatched number of dispatch results. May be
	/// a result of invalid bridged chain storage.
	InvalidNumberOfDispatchResults,
}

/// Outbound messages lane.
pub struct OutboundLane<S> {
	storage: S,
}

impl<S: OutboundLaneStorage> OutboundLane<S> {
	/// Create new inbound lane backed by given storage.
	pub fn new(storage: S) -> Self {
		OutboundLane { storage }
	}

	/// Get this lane data.
	pub fn data(&self) -> OutboundLaneData {
		self.storage.data()
	}

	/// Send message over lane.
	///
	/// Returns new message nonce.
	pub fn send_message(&mut self, message_data: MessageData<S::MessageFee>) -> MessageNonce {
		let mut data = self.storage.data();
		let nonce = data.latest_generated_nonce + 1;
		data.latest_generated_nonce = nonce;

		self.storage.save_message(nonce, message_data);
		self.storage.set_data(data);

		nonce
	}

	/// Confirm messages delivery.
	pub fn confirm_delivery<RelayerId>(
		&mut self,
		latest_received_nonce: MessageNonce,
		relayers: &VecDeque<UnrewardedRelayer<RelayerId>>,
	) -> ReceivalConfirmationResult {
		let mut data = self.storage.data();
		if latest_received_nonce <= data.latest_received_nonce {
			return ReceivalConfirmationResult::NoNewConfirmations;
		}
		if latest_received_nonce > data.latest_generated_nonce {
			return ReceivalConfirmationResult::FailedToConfirmFutureMessages;
		}

		let dispatch_results =
			match extract_dispatch_results(data.latest_received_nonce, latest_received_nonce, relayers) {
				Ok(dispatch_results) => dispatch_results,
				Err(extract_error) => return extract_error,
			};

		let prev_latest_received_nonce = data.latest_received_nonce;
		data.latest_received_nonce = latest_received_nonce;
		self.storage.set_data(data);

		ReceivalConfirmationResult::ConfirmedMessages(DeliveredMessages {
			begin: prev_latest_received_nonce + 1,
			end: latest_received_nonce,
			dispatch_results,
		})
	}

	/// Prune at most `max_messages_to_prune` already received messages.
	///
	/// Returns number of pruned messages.
	pub fn prune_messages(&mut self, max_messages_to_prune: MessageNonce) -> MessageNonce {
		let mut pruned_messages = 0;
		let mut anything_changed = false;
		let mut data = self.storage.data();
		while pruned_messages < max_messages_to_prune && data.oldest_unpruned_nonce <= data.latest_received_nonce {
			self.storage.remove_message(&data.oldest_unpruned_nonce);

			anything_changed = true;
			pruned_messages += 1;
			data.oldest_unpruned_nonce += 1;
		}

		if anything_changed {
			self.storage.set_data(data);
		}

		pruned_messages
	}
}

/// Extract new dispatch results from the unrewarded relayers vec.
///
/// Returns `Err(_)` if unrewarded relayers vec contains invalid data, meaning that the bridged
/// chain has invalid runtime storage.
fn extract_dispatch_results<RelayerId>(
	prev_latest_received_nonce: MessageNonce,
	latest_received_nonce: MessageNonce,
	relayers: &VecDeque<UnrewardedRelayer<RelayerId>>,
) -> Result<DispatchResultsBitVec, ReceivalConfirmationResult> {
	// the only caller of this functions checks that the prev_latest_received_nonce..=latest_received_nonce
	// is valid, so we're ready to accept messages in this range
	// => with_capacity call must succeed here or we'll be unable to receive confirmations at all
	let mut received_dispatch_result =
		BitVec::with_capacity((latest_received_nonce - prev_latest_received_nonce + 1) as _);
	let mut last_entry_end: Option<MessageNonce> = None;
	for entry in relayers {
		// unrewarded relayer entry must have at least 1 unconfirmed message
		// (guaranteed by the `InboundLane::receive_message()`)
		if entry.messages.end < entry.messages.begin {
			return Err(ReceivalConfirmationResult::EmptyUnrewardedRelayerEntry);
		}
		// every entry must confirm range of messages that follows previous entry range
		// (guaranteed by the `InboundLane::receive_message()`)
		if let Some(last_entry_end) = last_entry_end {
			let expected_entry_begin = last_entry_end.checked_add(1);
			if expected_entry_begin != Some(entry.messages.begin) {
				return Err(ReceivalConfirmationResult::NonConsecutiveUnrewardedRelayerEntries);
			}
		}
		last_entry_end = Some(entry.messages.end);
		// entry can't confirm messages larger than `inbound_lane_data.latest_received_nonce()`
		// (guaranteed by the `InboundLane::receive_message()`)
		if entry.messages.end > latest_received_nonce {
			// technically this will be detected in the next loop iteration as `InvalidNumberOfDispatchResults`
			// but to guarantee safety of loop operations below this is detected now
			return Err(ReceivalConfirmationResult::FailedToConfirmFutureMessages);
		}
		// entry must have single dispatch result for every message
		// (guaranteed by the `InboundLane::receive_message()`)
		if entry.messages.dispatch_results.len() as MessageNonce != entry.messages.end - entry.messages.begin + 1 {
			return Err(ReceivalConfirmationResult::InvalidNumberOfDispatchResults);
		}

		// now we know that the entry is valid
		// => let's check if it brings new confirmations
		let new_messages_begin = sp_std::cmp::max(entry.messages.begin, prev_latest_received_nonce + 1);
		let new_messages_end = sp_std::cmp::min(entry.messages.end, latest_received_nonce);
		let new_messages_range = new_messages_begin..=new_messages_end;
		if new_messages_range.is_empty() {
			continue;
		}

		// now we know that entry brings new confirmations
		// => let's extract dispatch results
		received_dispatch_result.extend_from_bitslice(
			&entry.messages.dispatch_results[(new_messages_begin - entry.messages.begin) as usize..],
		);
	}

	Ok(received_dispatch_result)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		mock::{message_data, run_test, unrewarded_relayer, TestRelayer, TestRuntime, REGULAR_PAYLOAD, TEST_LANE_ID},
		outbound_lane,
	};
	use sp_std::ops::RangeInclusive;

	fn unrewarded_relayers(nonces: RangeInclusive<MessageNonce>) -> VecDeque<UnrewardedRelayer<TestRelayer>> {
		vec![unrewarded_relayer(*nonces.start(), *nonces.end(), 0)]
			.into_iter()
			.collect()
	}

	fn delivered_messages(nonces: RangeInclusive<MessageNonce>) -> DeliveredMessages {
		DeliveredMessages {
			begin: *nonces.start(),
			end: *nonces.end(),
			dispatch_results: bitvec![Msb0, u8; 1; (nonces.end() - nonces.start() + 1) as _],
		}
	}

	fn assert_3_messages_confirmation_fails(
		latest_received_nonce: MessageNonce,
		relayers: &VecDeque<UnrewardedRelayer<TestRelayer>>,
	) -> ReceivalConfirmationResult {
		run_test(|| {
			let mut lane = outbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			lane.send_message(message_data(REGULAR_PAYLOAD));
			lane.send_message(message_data(REGULAR_PAYLOAD));
			lane.send_message(message_data(REGULAR_PAYLOAD));
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 0);
			let result = lane.confirm_delivery(latest_received_nonce, relayers);
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 0);
			result
		})
	}

	#[test]
	fn send_message_works() {
		run_test(|| {
			let mut lane = outbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			assert_eq!(lane.storage.data().latest_generated_nonce, 0);
			assert_eq!(lane.send_message(message_data(REGULAR_PAYLOAD)), 1);
			assert!(lane.storage.message(&1).is_some());
			assert_eq!(lane.storage.data().latest_generated_nonce, 1);
		});
	}

	#[test]
	fn confirm_delivery_works() {
		run_test(|| {
			let mut lane = outbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			assert_eq!(lane.send_message(message_data(REGULAR_PAYLOAD)), 1);
			assert_eq!(lane.send_message(message_data(REGULAR_PAYLOAD)), 2);
			assert_eq!(lane.send_message(message_data(REGULAR_PAYLOAD)), 3);
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 0);
			assert_eq!(
				lane.confirm_delivery(3, &unrewarded_relayers(1..=3)),
				ReceivalConfirmationResult::ConfirmedMessages(delivered_messages(1..=3)),
			);
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 3);
		});
	}

	#[test]
	fn confirm_delivery_rejects_nonce_lesser_than_latest_received() {
		run_test(|| {
			let mut lane = outbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			lane.send_message(message_data(REGULAR_PAYLOAD));
			lane.send_message(message_data(REGULAR_PAYLOAD));
			lane.send_message(message_data(REGULAR_PAYLOAD));
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 0);
			assert_eq!(
				lane.confirm_delivery(3, &unrewarded_relayers(1..=3)),
				ReceivalConfirmationResult::ConfirmedMessages(delivered_messages(1..=3)),
			);
			assert_eq!(
				lane.confirm_delivery(3, &unrewarded_relayers(1..=3)),
				ReceivalConfirmationResult::NoNewConfirmations,
			);
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 3);

			assert_eq!(
				lane.confirm_delivery(2, &unrewarded_relayers(1..=1)),
				ReceivalConfirmationResult::NoNewConfirmations,
			);
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 3);
		});
	}

	#[test]
	fn confirm_delivery_rejects_nonce_larger_than_last_generated() {
		assert_eq!(
			assert_3_messages_confirmation_fails(10, &unrewarded_relayers(1..=10),),
			ReceivalConfirmationResult::FailedToConfirmFutureMessages,
		);
	}

	#[test]
	fn confirm_delivery_fails_if_entry_confirms_future_messages() {
		assert_eq!(
			assert_3_messages_confirmation_fails(
				3,
				&unrewarded_relayers(1..=1)
					.into_iter()
					.chain(unrewarded_relayers(2..=30).into_iter())
					.chain(unrewarded_relayers(3..=3).into_iter())
					.collect(),
			),
			ReceivalConfirmationResult::FailedToConfirmFutureMessages,
		);
	}

	#[test]
	#[allow(clippy::reversed_empty_ranges)]
	fn confirm_delivery_fails_if_entry_is_empty() {
		assert_eq!(
			assert_3_messages_confirmation_fails(
				3,
				&unrewarded_relayers(1..=1)
					.into_iter()
					.chain(unrewarded_relayers(2..=1).into_iter())
					.chain(unrewarded_relayers(2..=3).into_iter())
					.collect(),
			),
			ReceivalConfirmationResult::EmptyUnrewardedRelayerEntry,
		);
	}

	#[test]
	fn confirm_delivery_fails_if_entries_are_non_consecutive() {
		assert_eq!(
			assert_3_messages_confirmation_fails(
				3,
				&unrewarded_relayers(1..=1)
					.into_iter()
					.chain(unrewarded_relayers(3..=3).into_iter())
					.chain(unrewarded_relayers(2..=2).into_iter())
					.collect(),
			),
			ReceivalConfirmationResult::NonConsecutiveUnrewardedRelayerEntries,
		);
	}

	#[test]
	fn confirm_delivery_fails_if_number_of_dispatch_results_in_entry_is_invalid() {
		let mut relayers: VecDeque<_> = unrewarded_relayers(1..=1)
			.into_iter()
			.chain(unrewarded_relayers(2..=2).into_iter())
			.chain(unrewarded_relayers(3..=3).into_iter())
			.collect();
		relayers[0].messages.dispatch_results.clear();
		assert_eq!(
			assert_3_messages_confirmation_fails(3, &relayers),
			ReceivalConfirmationResult::InvalidNumberOfDispatchResults,
		);
	}

	#[test]
	fn prune_messages_works() {
		run_test(|| {
			let mut lane = outbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			// when lane is empty, nothing is pruned
			assert_eq!(lane.prune_messages(100), 0);
			assert_eq!(lane.storage.data().oldest_unpruned_nonce, 1);
			// when nothing is confirmed, nothing is pruned
			lane.send_message(message_data(REGULAR_PAYLOAD));
			lane.send_message(message_data(REGULAR_PAYLOAD));
			lane.send_message(message_data(REGULAR_PAYLOAD));
			assert_eq!(lane.prune_messages(100), 0);
			assert_eq!(lane.storage.data().oldest_unpruned_nonce, 1);
			// after confirmation, some messages are received
			assert_eq!(
				lane.confirm_delivery(2, &unrewarded_relayers(1..=2)),
				ReceivalConfirmationResult::ConfirmedMessages(delivered_messages(1..=2)),
			);
			assert_eq!(lane.prune_messages(100), 2);
			assert_eq!(lane.storage.data().oldest_unpruned_nonce, 3);
			// after last message is confirmed, everything is pruned
			assert_eq!(
				lane.confirm_delivery(3, &unrewarded_relayers(3..=3)),
				ReceivalConfirmationResult::ConfirmedMessages(delivered_messages(3..=3)),
			);
			assert_eq!(lane.prune_messages(100), 1);
			assert_eq!(lane.storage.data().oldest_unpruned_nonce, 4);
		});
	}
}
