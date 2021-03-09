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

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Everything about outgoing messages sending.

use bp_message_lane::{LaneId, MessageData, MessageNonce, OutboundLaneData};

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
	///
	/// Returns `None` if confirmation is wrong/duplicate.
	/// Returns `Some` with inclusive ranges of message nonces that have been received.
	pub fn confirm_delivery(&mut self, latest_received_nonce: MessageNonce) -> Option<(MessageNonce, MessageNonce)> {
		let mut data = self.storage.data();
		if latest_received_nonce <= data.latest_received_nonce || latest_received_nonce > data.latest_generated_nonce {
			return None;
		}

		let prev_latest_received_nonce = data.latest_received_nonce;
		data.latest_received_nonce = latest_received_nonce;
		self.storage.set_data(data);

		Some((prev_latest_received_nonce + 1, latest_received_nonce))
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		mock::{message_data, run_test, TestRuntime, REGULAR_PAYLOAD, TEST_LANE_ID},
		outbound_lane,
	};

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
			assert_eq!(lane.confirm_delivery(3), Some((1, 3)));
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
			assert_eq!(lane.confirm_delivery(3), Some((1, 3)));
			assert_eq!(lane.confirm_delivery(3), None);
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 3);

			assert_eq!(lane.confirm_delivery(2), None);
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 3);
		});
	}

	#[test]
	fn confirm_delivery_rejects_nonce_larger_than_last_generated() {
		run_test(|| {
			let mut lane = outbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			lane.send_message(message_data(REGULAR_PAYLOAD));
			lane.send_message(message_data(REGULAR_PAYLOAD));
			lane.send_message(message_data(REGULAR_PAYLOAD));
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 0);
			assert_eq!(lane.confirm_delivery(10), None);
			assert_eq!(lane.storage.data().latest_generated_nonce, 3);
			assert_eq!(lane.storage.data().latest_received_nonce, 0);
		});
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
			assert_eq!(lane.confirm_delivery(2), Some((1, 2)));
			assert_eq!(lane.prune_messages(100), 2);
			assert_eq!(lane.storage.data().oldest_unpruned_nonce, 3);
			// after last message is confirmed, everything is pruned
			assert_eq!(lane.confirm_delivery(3), Some((3, 3)));
			assert_eq!(lane.prune_messages(100), 1);
			assert_eq!(lane.storage.data().oldest_unpruned_nonce, 4);
		});
	}
}
