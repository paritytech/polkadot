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

//! Everything about incoming messages receival.

use bp_message_lane::{
	target_chain::{DispatchMessage, DispatchMessageData, MessageDispatch},
	InboundLaneData, LaneId, MessageKey, MessageNonce, OutboundLaneData,
};
use sp_std::prelude::PartialEq;

/// Inbound lane storage.
pub trait InboundLaneStorage {
	/// Delivery and dispatch fee type on source chain.
	type MessageFee;
	/// Id of relayer on source chain.
	type Relayer: PartialEq;

	/// Lane id.
	fn id(&self) -> LaneId;
	/// Return maximal number of unrewarded relayer entries in inbound lane.
	fn max_unrewarded_relayer_entries(&self) -> MessageNonce;
	/// Return maximal number of unconfirmed messages in inbound lane.
	fn max_unconfirmed_messages(&self) -> MessageNonce;
	/// Get lane data from the storage.
	fn data(&self) -> InboundLaneData<Self::Relayer>;
	/// Update lane data in the storage.
	fn set_data(&mut self, data: InboundLaneData<Self::Relayer>);
}

/// Inbound messages lane.
pub struct InboundLane<S> {
	storage: S,
}

impl<S: InboundLaneStorage> InboundLane<S> {
	/// Create new inbound lane backed by given storage.
	pub fn new(storage: S) -> Self {
		InboundLane { storage }
	}

	/// Receive state of the corresponding outbound lane.
	pub fn receive_state_update(&mut self, outbound_lane_data: OutboundLaneData) -> Option<MessageNonce> {
		let mut data = self.storage.data();
		let last_delivered_nonce = data.last_delivered_nonce();

		if outbound_lane_data.latest_received_nonce > last_delivered_nonce {
			// this is something that should never happen if proofs are correct
			return None;
		}
		if outbound_lane_data.latest_received_nonce <= data.last_confirmed_nonce {
			return None;
		}

		let new_confirmed_nonce = outbound_lane_data.latest_received_nonce;
		data.last_confirmed_nonce = new_confirmed_nonce;
		// Firstly, remove all of the records where higher nonce <= new confirmed nonce
		while data
			.relayers
			.front()
			.map(|(_, nonce_high, _)| *nonce_high <= new_confirmed_nonce)
			.unwrap_or(false)
		{
			data.relayers.pop_front();
		}
		// Secondly, update the next record with lower nonce equal to new confirmed nonce if needed.
		// Note: There will be max. 1 record to update as we don't allow messages from relayers to overlap.
		match data.relayers.front_mut() {
			Some((nonce_low, _, _)) if *nonce_low < new_confirmed_nonce => {
				*nonce_low = new_confirmed_nonce + 1;
			}
			_ => {}
		}

		self.storage.set_data(data);
		Some(outbound_lane_data.latest_received_nonce)
	}

	/// Receive new message.
	pub fn receive_message<P: MessageDispatch<S::MessageFee>>(
		&mut self,
		relayer: S::Relayer,
		nonce: MessageNonce,
		message_data: DispatchMessageData<P::DispatchPayload, S::MessageFee>,
	) -> bool {
		let mut data = self.storage.data();
		let is_correct_message = nonce == data.last_delivered_nonce() + 1;
		if !is_correct_message {
			return false;
		}

		// if there are more unrewarded relayer entries than we may accept, reject this message
		if data.relayers.len() as MessageNonce >= self.storage.max_unrewarded_relayer_entries() {
			return false;
		}

		// if there are more unconfirmed messages than we may accept, reject this message
		let unconfirmed_messages_count = nonce.saturating_sub(data.last_confirmed_nonce);
		if unconfirmed_messages_count > self.storage.max_unconfirmed_messages() {
			return false;
		}

		let push_new = match data.relayers.back_mut() {
			Some((_, nonce_high, last_relayer)) if last_relayer == &relayer => {
				*nonce_high = nonce;
				false
			}
			_ => true,
		};
		if push_new {
			data.relayers.push_back((nonce, nonce, relayer));
		}

		self.storage.set_data(data);

		P::dispatch(DispatchMessage {
			key: MessageKey {
				lane_id: self.storage.id(),
				nonce,
			},
			data: message_data,
		});

		true
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		inbound_lane,
		mock::{
			message_data, run_test, TestMessageDispatch, TestRuntime, REGULAR_PAYLOAD, TEST_LANE_ID, TEST_RELAYER_A,
			TEST_RELAYER_B, TEST_RELAYER_C,
		},
		DefaultInstance, RuntimeInboundLaneStorage,
	};

	fn receive_regular_message(
		lane: &mut InboundLane<RuntimeInboundLaneStorage<TestRuntime, DefaultInstance>>,
		nonce: MessageNonce,
	) {
		assert!(lane.receive_message::<TestMessageDispatch>(
			TEST_RELAYER_A,
			nonce,
			message_data(REGULAR_PAYLOAD).into()
		));
	}

	#[test]
	fn receive_status_update_ignores_status_from_the_future() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			receive_regular_message(&mut lane, 1);
			assert_eq!(
				lane.receive_state_update(OutboundLaneData {
					latest_received_nonce: 10,
					..Default::default()
				}),
				None,
			);

			assert_eq!(lane.storage.data().last_confirmed_nonce, 0);
		});
	}

	#[test]
	fn receive_status_update_ignores_obsolete_status() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			receive_regular_message(&mut lane, 1);
			receive_regular_message(&mut lane, 2);
			receive_regular_message(&mut lane, 3);
			assert_eq!(
				lane.receive_state_update(OutboundLaneData {
					latest_received_nonce: 3,
					..Default::default()
				}),
				Some(3),
			);
			assert_eq!(lane.storage.data().last_confirmed_nonce, 3);

			assert_eq!(
				lane.receive_state_update(OutboundLaneData {
					latest_received_nonce: 3,
					..Default::default()
				}),
				None,
			);
			assert_eq!(lane.storage.data().last_confirmed_nonce, 3);
		});
	}

	#[test]
	fn receive_status_update_works() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			receive_regular_message(&mut lane, 1);
			receive_regular_message(&mut lane, 2);
			receive_regular_message(&mut lane, 3);
			assert_eq!(lane.storage.data().last_confirmed_nonce, 0);
			assert_eq!(lane.storage.data().relayers, vec![(1, 3, TEST_RELAYER_A)]);

			assert_eq!(
				lane.receive_state_update(OutboundLaneData {
					latest_received_nonce: 2,
					..Default::default()
				}),
				Some(2),
			);
			assert_eq!(lane.storage.data().last_confirmed_nonce, 2);
			assert_eq!(lane.storage.data().relayers, vec![(3, 3, TEST_RELAYER_A)]);

			assert_eq!(
				lane.receive_state_update(OutboundLaneData {
					latest_received_nonce: 3,
					..Default::default()
				}),
				Some(3),
			);
			assert_eq!(lane.storage.data().last_confirmed_nonce, 3);
			assert_eq!(lane.storage.data().relayers, vec![]);
		});
	}

	#[test]
	fn receive_status_update_works_with_batches_from_relayers() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			let mut seed_storage_data = lane.storage.data();
			// Prepare data
			seed_storage_data.last_confirmed_nonce = 0;
			seed_storage_data.relayers.push_back((1, 1, TEST_RELAYER_A));
			// Simulate messages batch (2, 3, 4) from relayer #2
			seed_storage_data.relayers.push_back((2, 4, TEST_RELAYER_B));
			seed_storage_data.relayers.push_back((5, 5, TEST_RELAYER_C));
			lane.storage.set_data(seed_storage_data);
			// Check
			assert_eq!(
				lane.receive_state_update(OutboundLaneData {
					latest_received_nonce: 3,
					..Default::default()
				}),
				Some(3),
			);
			assert_eq!(lane.storage.data().last_confirmed_nonce, 3);
			assert_eq!(
				lane.storage.data().relayers,
				vec![(4, 4, TEST_RELAYER_B), (5, 5, TEST_RELAYER_C)]
			);
		});
	}

	#[test]
	fn fails_to_receive_message_with_incorrect_nonce() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			assert!(!lane.receive_message::<TestMessageDispatch>(
				TEST_RELAYER_A,
				10,
				message_data(REGULAR_PAYLOAD).into()
			));
			assert_eq!(lane.storage.data().last_delivered_nonce(), 0);
		});
	}

	#[test]
	fn fails_to_receive_messages_above_unrewarded_relayer_entries_limit_per_lane() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			let max_nonce = <TestRuntime as crate::Config>::MaxUnrewardedRelayerEntriesAtInboundLane::get();
			for current_nonce in 1..max_nonce + 1 {
				assert!(lane.receive_message::<TestMessageDispatch>(
					TEST_RELAYER_A + current_nonce,
					current_nonce,
					message_data(REGULAR_PAYLOAD).into()
				));
			}
			// Fails to dispatch new message from different than latest relayer.
			assert_eq!(
				false,
				lane.receive_message::<TestMessageDispatch>(
					TEST_RELAYER_A + max_nonce + 1,
					max_nonce + 1,
					message_data(REGULAR_PAYLOAD).into()
				)
			);
			// Fails to dispatch new messages from latest relayer. Prevents griefing attacks.
			assert_eq!(
				false,
				lane.receive_message::<TestMessageDispatch>(
					TEST_RELAYER_A + max_nonce,
					max_nonce + 1,
					message_data(REGULAR_PAYLOAD).into()
				)
			);
		});
	}

	#[test]
	fn fails_to_receive_messages_above_unconfirmed_messages_limit_per_lane() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			let max_nonce = <TestRuntime as crate::Config>::MaxUnconfirmedMessagesAtInboundLane::get();
			for current_nonce in 1..=max_nonce {
				assert!(lane.receive_message::<TestMessageDispatch>(
					TEST_RELAYER_A,
					current_nonce,
					message_data(REGULAR_PAYLOAD).into()
				));
			}
			// Fails to dispatch new message from different than latest relayer.
			assert_eq!(
				false,
				lane.receive_message::<TestMessageDispatch>(
					TEST_RELAYER_B,
					max_nonce + 1,
					message_data(REGULAR_PAYLOAD).into()
				)
			);
			// Fails to dispatch new messages from latest relayer.
			assert_eq!(
				false,
				lane.receive_message::<TestMessageDispatch>(
					TEST_RELAYER_A,
					max_nonce + 1,
					message_data(REGULAR_PAYLOAD).into()
				)
			);
		});
	}

	#[test]
	fn correctly_receives_following_messages_from_two_relayers_alternately() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			assert!(lane.receive_message::<TestMessageDispatch>(
				TEST_RELAYER_A,
				1,
				message_data(REGULAR_PAYLOAD).into()
			));
			assert!(lane.receive_message::<TestMessageDispatch>(
				TEST_RELAYER_B,
				2,
				message_data(REGULAR_PAYLOAD).into()
			));
			assert!(lane.receive_message::<TestMessageDispatch>(
				TEST_RELAYER_A,
				3,
				message_data(REGULAR_PAYLOAD).into()
			));
			assert_eq!(
				lane.storage.data().relayers,
				vec![(1, 1, TEST_RELAYER_A), (2, 2, TEST_RELAYER_B), (3, 3, TEST_RELAYER_A)]
			);
		});
	}

	#[test]
	fn rejects_same_message_from_two_different_relayers() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			assert!(lane.receive_message::<TestMessageDispatch>(
				TEST_RELAYER_A,
				1,
				message_data(REGULAR_PAYLOAD).into()
			));
			assert_eq!(
				false,
				lane.receive_message::<TestMessageDispatch>(TEST_RELAYER_B, 1, message_data(REGULAR_PAYLOAD).into())
			);
		});
	}

	#[test]
	fn correct_message_is_processed_instantly() {
		run_test(|| {
			let mut lane = inbound_lane::<TestRuntime, _>(TEST_LANE_ID);
			receive_regular_message(&mut lane, 1);
			assert_eq!(lane.storage.data().last_delivered_nonce(), 1);
		});
	}
}
