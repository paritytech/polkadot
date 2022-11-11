// Copyright 2020 Parity Technologies (UK) Ltd.
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

use crate::mock::{
	assert_last_event, assert_last_events, new_test_ext, take_processed, Configuration,
	MessageQueue, MockGenesisConfig, ParaInclusion, System, Test,
};
use frame_support::{
	assert_noop, assert_ok,
	pallet_prelude::*,
	traits::{ExecuteOverweightError, ServiceQueues},
	weights::Weight,
};
use primitives::v2::{Id as ParaId, UpwardMessage};
use sp_runtime::traits::{Bounded, Hash};
use sp_std::prelude::*;

pub(super) struct GenesisConfigBuilder {
	max_upward_message_size: u32,
	max_upward_message_num_per_candidate: u32,
	max_upward_queue_count: u32,
	max_upward_queue_size: u32,
}

impl Default for GenesisConfigBuilder {
	fn default() -> Self {
		Self {
			max_upward_message_size: 16,
			max_upward_message_num_per_candidate: 2,
			max_upward_queue_count: 4,
			max_upward_queue_size: 64,
		}
	}
}

impl GenesisConfigBuilder {
	pub(super) fn build(self) -> crate::mock::MockGenesisConfig {
		let mut genesis = default_genesis_config();
		let config = &mut genesis.configuration.config;

		config.max_upward_message_size = self.max_upward_message_size;
		config.max_upward_message_num_per_candidate = self.max_upward_message_num_per_candidate;
		config.max_upward_queue_count = self.max_upward_queue_count;
		config.max_upward_queue_size = self.max_upward_queue_size;
		genesis
	}
}

fn default_genesis_config() -> MockGenesisConfig {
	MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: crate::configuration::HostConfiguration {
				max_downward_message_size: 1024,
				..Default::default()
			},
		},
		..Default::default()
	}
}

fn queue_upward_msg(para: ParaId, msg: UpwardMessage) {
	let msgs = vec![msg];
	assert!(ParaInclusion::check_upward_messages(&Configuration::config(), para, &msgs).is_ok());
	let _ = ParaInclusion::receive_upward_messages(&Configuration::config(), para, msgs);
}

#[test]
fn dispatch_empty() {
	new_test_ext(default_genesis_config()).execute_with(|| {
		// make sure that the case with empty queues is handled properly
		MessageQueue::service_queues(Weight::max_value());
	});
}

#[test]
fn dispatch_single_message() {
	let a = ParaId::from(228);
	let msg = 1000u32.encode();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		queue_upward_msg(a, msg.clone());
		MessageQueue::service_queues(Weight::max_value());
		assert_eq!(take_processed(), vec![(a, msg)]);
	});
}

#[test]
fn dispatch_resume_after_exceeding_dispatch_stage_weight() {
	let a = ParaId::from(128);
	let c = ParaId::from(228);
	let q = ParaId::from(911);

	let a_msg_1 = (200u32, "a_msg_1").encode();
	let a_msg_2 = (100u32, "a_msg_2").encode();
	let c_msg_1 = (300u32, "c_msg_1").encode();
	let c_msg_2 = (100u32, "c_msg_2").encode();
	let q_msg = (500u32, "q_msg").encode();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		queue_upward_msg(q, q_msg.clone());
		queue_upward_msg(c, c_msg_1.clone());
		queue_upward_msg(a, a_msg_1.clone());
		queue_upward_msg(a, a_msg_2.clone());

		// we expect only two first messages to fit in the first iteration.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(take_processed(), vec![(q, q_msg)]);
		queue_upward_msg(c, c_msg_2.clone());
		// second iteration should process the second message.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(take_processed(), vec![(c, c_msg_1), (c, c_msg_2)]);
		// 3rd iteration.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(take_processed(), vec![(a, a_msg_1), (a, a_msg_2)]);
		// finally, make sure that the queue is empty.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(take_processed(), vec![]);
	});
}

#[test]
fn dispatch_keeps_message_after_weight_exhausted() {
	let a = ParaId::from(128);

	let a_msg_1 = (300u32, "a_msg_1").encode();
	let a_msg_2 = (300u32, "a_msg_2").encode();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		queue_upward_msg(a, a_msg_1.clone());
		queue_upward_msg(a, a_msg_2.clone());

		// we expect only one message to fit in the first iteration.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(take_processed(), vec![(a, a_msg_1)]);
		// second iteration should process the remaining message.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(take_processed(), vec![(a, a_msg_2)]);
		// finally, make sure that the queue is empty.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(take_processed(), vec![]);
	});
}

#[test]
fn dispatch_correctly_handle_remove_of_latest() {
	let a = ParaId::from(1991);
	let b = ParaId::from(1999);

	let a_msg_1 = (300u32, "a_msg_1").encode();
	let a_msg_2 = (300u32, "a_msg_2").encode();
	let b_msg_1 = (300u32, "b_msg_1").encode();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		// We want to test here an edge case, where we remove the queue with the highest
		// para id (i.e. last in the `needs_dispatch` order).
		//
		// If the last entry was removed we should proceed execution, assuming we still have
		// weight available.

		queue_upward_msg(a, a_msg_1.clone());
		queue_upward_msg(a, a_msg_2.clone());
		queue_upward_msg(b, b_msg_1.clone());
		MessageQueue::service_queues(Weight::from_parts(900, 900));
		assert_eq!(take_processed(), vec![(a, a_msg_1), (a, a_msg_2), (b, b_msg_1)]);
	});
}

#[test]
fn verify_relay_dispatch_queue_size_is_externally_accessible() {
	// Make sure that the relay dispatch queue size storage entry is accessible via well known
	// keys and is decodable into a (u32, u32).

	use parity_scale_codec::Decode as _;
	use primitives::v2::well_known_keys;

	let a = ParaId::from(228);
	let msg = vec![1, 2, 3];

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		queue_upward_msg(a, msg);

		#[allow(deprecated)]
		let raw_queue_size = sp_io::storage::get(&well_known_keys::relay_dispatch_queue_size(a))
			.expect(
				"enqueing a message should create the dispatch queue\
				and it should be accessible via the well known keys",
			);
		let (cnt, size) = <(u32, u32)>::decode(&mut &raw_queue_size[..])
			.expect("the dispatch queue size should be decodable into (u32, u32)");

		assert_eq!(cnt, 1);
		assert_eq!(size, 3);
	});
}

#[test]
fn service_overweight_unknown() {
	// This test just makes sure that 0 is not a valid index and we can use it not worrying in
	// the next test.
	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		assert_noop!(
			<MessageQueue as ServiceQueues>::execute_overweight(
				Weight::from_ref_time(1000),
				(0u32.into(), 0, 0)
			),
			ExecuteOverweightError::NotFound,
		);
	});
}

#[test]
fn overweight_queue_works() {
	let para_a = ParaId::from(2021);

	let a_msg_1 = (301u32, "a_msg_1").encode();
	let a_msg_2 = (501u32, "a_msg_2").encode();
	let a_msg_3 = (501u32, "a_msg_3").encode();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		// HACK: Start with the block number 1. This is needed because should an event be
		// emitted during the genesis block they will be implicitly wiped.
		System::set_block_number(1);

		// This one is overweight. However, the weight is plenty and we can afford to execute
		// this message, thus expect it.
		queue_upward_msg(para_a, a_msg_1.clone());
		queue_upward_msg(para_a, a_msg_2.clone());
		queue_upward_msg(para_a, a_msg_3.clone());

		MessageQueue::service_queues(Weight::from_parts(500, 500));
		let hash_1 = <<Test as frame_system::Config>::Hashing as Hash>::hash(&a_msg_1[..]);
		let hash_2 = <<Test as frame_system::Config>::Hashing as Hash>::hash(&a_msg_2[..]);
		let hash_3 = <<Test as frame_system::Config>::Hashing as Hash>::hash(&a_msg_3[..]);
		assert_last_events(
			[
				pallet_message_queue::Event::<Test>::Processed {
					hash: hash_1.clone(),
					origin: para_a,
					weight_used: Weight::from_parts(301, 301),
					success: true,
				}
				.into(),
				pallet_message_queue::Event::<Test>::OverweightEnqueued {
					hash: hash_2.clone(),
					origin: para_a,
					page_index: 0,
					message_index: 1,
				}
				.into(),
				pallet_message_queue::Event::<Test>::OverweightEnqueued {
					hash: hash_3.clone(),
					origin: para_a,
					page_index: 0,
					message_index: 2,
				}
				.into(),
			]
			.into_iter(),
		);
		assert_eq!(take_processed(), vec![(para_a, a_msg_1)]);

		// Now verify that if we wanted to service this overweight message with less than enough
		// weight it will fail.
		assert_noop!(
			<MessageQueue as ServiceQueues>::execute_overweight(
				Weight::from_parts(500, 500),
				(para_a, 0, 2)
			),
			ExecuteOverweightError::InsufficientWeight,
		);

		// ... and if we try to service it with just enough weight it will succeed as well.
		assert_ok!(<MessageQueue as ServiceQueues>::execute_overweight(
			Weight::from_parts(501, 501),
			(para_a, 0, 2)
		));
		assert_last_event(
			pallet_message_queue::Event::<Test>::Processed {
				hash: hash_3,
				origin: para_a,
				weight_used: Weight::from_parts(501, 501),
				success: true,
			}
			.into(),
		);

		// ... and if we try to service a message with index that doesn't exist it will error
		// out.
		assert_noop!(
			<MessageQueue as ServiceQueues>::execute_overweight(
				Weight::from_parts(501, 501),
				(para_a, 0, 2)
			),
			ExecuteOverweightError::NotFound,
		);
	});
}
