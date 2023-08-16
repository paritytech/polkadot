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

use crate::{
	inclusion::{
		tests::run_to_block_default_notifications as run_to_block, AggregateMessageOrigin,
		AggregateMessageOrigin::Ump, UmpAcceptanceCheckErr, UmpQueueId,
	},
	mock::{
		assert_last_event, assert_last_events, new_test_ext, Configuration, MessageQueue,
		MessageQueueSize, MockGenesisConfig, ParaInclusion, Processed, System, Test, *,
	},
};
use frame_support::{
	assert_noop, assert_ok,
	pallet_prelude::*,
	traits::{EnqueueMessage, ExecuteOverweightError, ServiceQueues},
	weights::Weight,
};
use primitives::{well_known_keys, Id as ParaId, UpwardMessage};
use sp_core::twox_64;
use sp_io::hashing::blake2_256;
use sp_runtime::traits::Bounded;
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
	pub(super) fn large_queue_count() -> Self {
		Self { max_upward_queue_count: 128, ..Default::default() }
	}

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
	try_queue_upward_msg(para, msg).unwrap();
}

fn try_queue_upward_msg(para: ParaId, msg: UpwardMessage) -> Result<(), UmpAcceptanceCheckErr> {
	let msgs = vec![msg];
	ParaInclusion::check_upward_messages(&Configuration::config(), para, &msgs)?;
	ParaInclusion::receive_upward_messages(para, msgs.as_slice());
	Ok(())
}

mod check_upward_messages {
	use super::*;

	const P_0: ParaId = ParaId::new(0u32);
	const P_1: ParaId = ParaId::new(1u32);

	// Currently its trivial since unbounded, but this function will be handy when we bound it.
	fn msg(data: &str) -> UpwardMessage {
		data.as_bytes().to_vec()
	}

	/// Check that these messages *could* be queued.
	fn check(para: ParaId, msgs: Vec<UpwardMessage>, err: Option<UmpAcceptanceCheckErr>) {
		assert_eq!(
			ParaInclusion::check_upward_messages(&Configuration::config(), para, &msgs[..]).err(),
			err
		);
	}

	/// Enqueue these upward messages.
	fn queue(para: ParaId, msgs: Vec<UpwardMessage>) {
		msgs.into_iter().for_each(|msg| super::queue_upward_msg(para, msg));
	}

	#[test]
	fn basic_works() {
		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			let _g = frame_support::StorageNoopGuard::default();
			check(P_0, vec![msg("p0m0")], None);
			check(P_1, vec![msg("p1m0")], None);
			check(P_0, vec![msg("p0m1")], None);
			check(P_1, vec![msg("p1m1")], None);
		});
	}

	#[test]
	fn num_per_candidate_exceeded_error() {
		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			let _g = frame_support::StorageNoopGuard::default();
			let permitted = Configuration::config().max_upward_message_num_per_candidate;

			for sent in 0..permitted + 1 {
				check(P_0, vec![msg(""); sent as usize], None);
			}
			for sent in permitted + 1..permitted + 10 {
				check(
					P_0,
					vec![msg(""); sent as usize],
					Some(UmpAcceptanceCheckErr::MoreMessagesThanPermitted { sent, permitted }),
				);
			}
		});
	}

	#[test]
	fn size_per_message_exceeded_error() {
		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			let _g = frame_support::StorageNoopGuard::default();
			let max_size = Configuration::config().max_upward_message_size;
			let max_per_candidate = Configuration::config().max_upward_message_num_per_candidate;

			for msg_size in 0..=max_size {
				check(P_0, vec![vec![0; msg_size as usize]], None);
			}
			for msg_size in max_size + 1..max_size + 10 {
				for goods in 0..max_per_candidate {
					let mut msgs = vec![vec![0; max_size as usize]; goods as usize];
					msgs.push(vec![0; msg_size as usize]);

					check(
						P_0,
						msgs,
						Some(UmpAcceptanceCheckErr::MessageSize { idx: goods, msg_size, max_size }),
					);
				}
			}
		});
	}

	#[test]
	fn queue_count_exceeded_error() {
		new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
			let limit = Configuration::config().max_upward_queue_count as u64;

			for _ in 0..limit {
				check(P_0, vec![msg("")], None);
				queue(P_0, vec![msg("")]);
			}

			check(
				P_0,
				vec![msg("")],
				Some(UmpAcceptanceCheckErr::CapacityExceeded { count: limit + 1, limit }),
			);
			check(
				P_0,
				vec![msg(""); 2],
				Some(UmpAcceptanceCheckErr::CapacityExceeded { count: limit + 2, limit }),
			);
		});
	}

	#[test]
	fn queue_size_exceeded_error() {
		new_test_ext(GenesisConfigBuilder::large_queue_count().build()).execute_with(|| {
			let limit = Configuration::config().max_upward_queue_size as u64;
			assert_eq!(pallet_message_queue::ItemHeader::<MessageQueueSize>::max_encoded_len(), 5);
			assert!(
				Configuration::config().max_upward_queue_size <
					crate::inclusion::MaxUmpMessageLenOf::<Test>::get(),
				"Test will not work"
			);

			for _ in 0..limit {
				check(P_0, vec![msg("1")], None);
				queue(P_0, vec![msg("1")]);
			}

			check(
				P_0,
				vec![msg("1")],
				Some(UmpAcceptanceCheckErr::TotalSizeExceeded { total_size: limit + 1, limit }),
			);
			check(
				P_0,
				vec![msg("123456")],
				Some(UmpAcceptanceCheckErr::TotalSizeExceeded { total_size: limit + 6, limit }),
			);
		});
	}
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
		assert_eq!(Processed::take(), vec![(a, msg)]);
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
		assert_eq!(Processed::take(), vec![(q, q_msg)]);
		queue_upward_msg(c, c_msg_2.clone());
		// second iteration should process the second message.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(Processed::take(), vec![(c, c_msg_1), (c, c_msg_2)]);
		// 3rd iteration.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(Processed::take(), vec![(a, a_msg_1), (a, a_msg_2)]);
		// finally, make sure that the queue is empty.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(Processed::take(), vec![]);
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
		assert_eq!(Processed::take(), vec![(a, a_msg_1)]);
		// second iteration should process the remaining message.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(Processed::take(), vec![(a, a_msg_2)]);
		// finally, make sure that the queue is empty.
		MessageQueue::service_queues(Weight::from_parts(500, 500));
		assert_eq!(Processed::take(), vec![]);
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
		assert_eq!(Processed::take(), vec![(a, a_msg_1), (a, a_msg_2), (b, b_msg_1)]);
	});
}

#[test]
#[cfg_attr(debug_assertions, should_panic = "Defensive failure has been triggered")]
fn queue_enact_too_long_ignored() {
	const P_0: ParaId = ParaId::new(0u32);

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		let max_enact = crate::inclusion::MaxUmpMessageLenOf::<Test>::get() as usize;
		let m1 = (300u32, "a_msg_1").encode();
		let m2 = vec![0u8; max_enact + 1];
		let m3 = (300u32, "a_msg_3").encode();

		// .. but the enact defensively ignores.
		ParaInclusion::receive_upward_messages(P_0, &[m1.clone(), m2.clone(), m3.clone()]);
		// There is one message in the queue now:
		MessageQueue::service_queues(Weight::from_parts(900, 900));
		assert_eq!(Processed::take(), vec![(P_0, m1), (P_0, m3)]);
	});
}

/// Check that the Inclusion pallet correctly updates the well known keys in the MQ handler.
///
/// Also checks that it works in the presence of overweight messages.
#[test]
fn relay_dispatch_queue_size_is_updated() {
	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		let cfg = Configuration::config();

		for p in 0..100 {
			let para = p.into();
			// Do some tricks with the weight such that the MQ pallet will process in order:
			// Q0:0, Q1:0 … Q0:1, Q1:1 …
			let m1 = (300u32 * (100 - p), "m1").encode();
			let m2 = (300u32 * (100 - p), "m11").encode();

			queue_upward_msg(para, m1);
			queue_upward_msg(para, m2);

			assert_queue_size(para, 2, 15);
			assert_queue_remaining(
				para,
				cfg.max_upward_queue_count - 2,
				cfg.max_upward_queue_size - 15,
			);

			// Now processing one message should also update the queue size.
			MessageQueue::service_queues(Weight::from_all(300u64 * (100 - p) as u64));
			assert_queue_remaining(
				para,
				cfg.max_upward_queue_count - 1,
				cfg.max_upward_queue_size - 8,
			);
		}

		// The messages of Q0…Q98 are overweight, so `service_queues` wont help.
		for p in 0..98 {
			let para = UmpQueueId::Para(p.into());
			MessageQueue::service_queues(Weight::from_all(u64::MAX));

			let fp = MessageQueue::footprint(AggregateMessageOrigin::Ump(para));
			let (para_queue_count, para_queue_size) = (fp.count, fp.size);
			assert_eq!(para_queue_count, 1, "count wrong for para: {}", p);
			assert_eq!(para_queue_size, 8, "size wrong for para: {}", p);
		}
		// All queues are empty after processing overweight messages.
		for p in 0..100 {
			let para = UmpQueueId::Para(p.into());
			let _ = <MessageQueue as ServiceQueues>::execute_overweight(
				Weight::from_all(u64::MAX),
				(AggregateMessageOrigin::Ump(para.clone()), 0, 1),
			);

			assert_queue_remaining(p.into(), cfg.max_upward_queue_count, cfg.max_upward_queue_size);
			let fp = MessageQueue::footprint(AggregateMessageOrigin::Ump(para));
			let (para_queue_count, para_queue_size) = (fp.count, fp.size);
			assert_eq!(para_queue_count, 0, "count wrong for para: {}", p);
			assert_eq!(para_queue_size, 0, "size wrong for para: {}", p);
		}
	});
}

/// Assert that the old and the new way of accessing `relay_dispatch_queue_size` is the same.
#[test]
fn relay_dispatch_queue_size_key_is_correct() {
	#![allow(deprecated)]
	// Storage alias to the old way of accessing the queue size.
	#[frame_support::storage_alias]
	type RelayDispatchQueueSize = StorageMap<Ump, Twox64Concat, ParaId, (u32, u32), ValueQuery>;

	for i in 0..1024 {
		// A "random" para id.
		let para: ParaId = u32::from_ne_bytes(twox_64(&i.encode())[..4].try_into().unwrap()).into();

		let well_known = primitives::well_known_keys::relay_dispatch_queue_size(para);
		let aliased = RelayDispatchQueueSize::hashed_key_for(para);

		assert_eq!(well_known, aliased, "Old and new key must match");
	}
}

#[test]
fn verify_relay_dispatch_queue_size_is_externally_accessible() {
	// Make sure that the relay dispatch queue size storage entry is accessible via well known
	// keys and is decodable into a (u32, u32).
	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		let cfg = Configuration::config();

		for para in 0..10 {
			let para = para.into();
			queue_upward_msg(para, vec![0u8; 3]);
			assert_queue_size(para, 1, 3);
			assert_queue_remaining(
				para,
				cfg.max_upward_queue_count - 1,
				cfg.max_upward_queue_size - 3,
			);

			queue_upward_msg(para, vec![0u8; 3]);
			assert_queue_size(para, 2, 6);
			assert_queue_remaining(
				para,
				cfg.max_upward_queue_count - 2,
				cfg.max_upward_queue_size - 6,
			);
		}
	});
}

fn assert_queue_size(para: ParaId, count: u32, size: u32) {
	#[allow(deprecated)]
	let raw_queue_size = sp_io::storage::get(&well_known_keys::relay_dispatch_queue_size(para)).expect(
		"enqueing a message should create the dispatch queue\
				and it should be accessible via the well known keys",
	);
	let (c, s) = <(u32, u32)>::decode(&mut &raw_queue_size[..])
		.expect("the dispatch queue size should be decodable into (u32, u32)");
	assert_eq!((c, s), (count, size));

	// Test the deprecated but at least type-safe `relay_dispatch_queue_size_typed`:
	#[allow(deprecated)]
	let (c, s) = well_known_keys::relay_dispatch_queue_size_typed(para).get().expect(
		"enqueing a message should create the dispatch queue\
				and it should be accessible via the well known keys",
	);
	assert_eq!((c, s), (count, size));
}

fn assert_queue_remaining(para: ParaId, count: u32, size: u32) {
	let (remaining_cnt, remaining_size) =
		well_known_keys::relay_dispatch_queue_remaining_capacity(para)
			.get()
			.expect("No storage value");
	assert_eq!(remaining_cnt, count, "Wrong number of remaining messages in Q{}", para);
	assert_eq!(remaining_size, size, "Wrong remaining size in Q{}", para);
}

#[test]
fn service_overweight_unknown() {
	// This test just makes sure that 0 is not a valid index and we can use it not worrying in
	// the next test.
	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		assert_noop!(
			<MessageQueue as ServiceQueues>::execute_overweight(
				Weight::MAX,
				(Ump(UmpQueueId::Para(0u32.into())), 0, 0)
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
		let hash_1 = blake2_256(&a_msg_1[..]);
		let hash_2 = blake2_256(&a_msg_2[..]);
		let hash_3 = blake2_256(&a_msg_3[..]);
		assert_last_events(
			[
				pallet_message_queue::Event::<Test>::Processed {
					id: hash_1,
					origin: Ump(UmpQueueId::Para(para_a)),
					weight_used: Weight::from_parts(301, 301),
					success: true,
				}
				.into(),
				pallet_message_queue::Event::<Test>::OverweightEnqueued {
					id: hash_2,
					origin: Ump(UmpQueueId::Para(para_a)),
					page_index: 0,
					message_index: 1,
				}
				.into(),
				pallet_message_queue::Event::<Test>::OverweightEnqueued {
					id: hash_3,
					origin: Ump(UmpQueueId::Para(para_a)),
					page_index: 0,
					message_index: 2,
				}
				.into(),
			]
			.into_iter(),
		);
		assert_eq!(Processed::take(), vec![(para_a, a_msg_1)]);

		// Now verify that if we wanted to service this overweight message with less than enough
		// weight it will fail.
		assert_noop!(
			<MessageQueue as ServiceQueues>::execute_overweight(
				Weight::from_parts(500, 500),
				(Ump(UmpQueueId::Para(para_a)), 0, 2)
			),
			ExecuteOverweightError::InsufficientWeight,
		);

		// ... and if we try to service it with just enough weight it will succeed as well.
		assert_ok!(<MessageQueue as ServiceQueues>::execute_overweight(
			Weight::from_parts(501, 501),
			(Ump(UmpQueueId::Para(para_a)), 0, 2)
		));
		assert_last_event(
			pallet_message_queue::Event::<Test>::Processed {
				id: hash_3,
				origin: Ump(UmpQueueId::Para(para_a)),
				weight_used: Weight::from_parts(501, 501),
				success: true,
			}
			.into(),
		);

		// But servicing again will not work.
		assert_noop!(
			<MessageQueue as ServiceQueues>::execute_overweight(
				Weight::from_parts(501, 501),
				(Ump(UmpQueueId::Para(para_a)), 0, 2)
			),
			ExecuteOverweightError::AlreadyProcessed,
		);

		// Using an invalid index does not work.
		assert_noop!(
			<MessageQueue as ServiceQueues>::execute_overweight(
				Weight::from_parts(501, 501),
				(Ump(UmpQueueId::Para(para_a)), 0, 3)
			),
			ExecuteOverweightError::NotFound,
		);
	});
}

/// Tests that UMP messages in the dispatch queue of the relay prevents the parachain from being
/// scheduled for offboarding.
#[test]
fn cannot_offboard_while_ump_dispatch_queued() {
	let para = 32.into();
	let msg = (300u32, "something").encode();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		register_parachain(para);
		run_to_block(5, vec![4, 5]);

		queue_upward_msg(para, msg.clone());
		queue_upward_msg(para, msg.clone());
		// Cannot offboard since there are two UMP messages in the queue.
		for i in 6..10 {
			assert!(try_deregister_parachain(para).is_err());
			run_to_block(i, vec![i]);
			assert!(Paras::is_valid_para(para));
		}

		// Now let's process the first message.
		MessageQueue::on_initialize(System::block_number());
		assert_eq!(Processed::take().len(), 1);
		// Cannot offboard since there is another one in the queue.
		assert!(try_deregister_parachain(para).is_err());
		// Now also process the second message ...
		MessageQueue::on_initialize(System::block_number());
		assert_eq!(Processed::take().len(), 1);

		// ... and offboard.
		run_to_block(10, vec![10]);
		assert!(Paras::is_valid_para(para));
		assert_ok!(try_deregister_parachain(para));
		assert!(Paras::is_offboarding(para));

		// Offboarding completed.
		run_to_block(11, vec![11]);
		assert!(!Paras::is_valid_para(para));
	});
}

/// A para-chain cannot send an UMP to the relay chain while it is offboarding.
#[test]
fn cannot_enqueue_ump_while_offboarding() {
	let para = 32.into();
	let msg = (300u32, "something").encode();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		register_parachain(para);
		run_to_block(5, vec![4, 5]);

		// Start with an offboarding para.
		assert_ok!(try_deregister_parachain(para));
		assert!(Paras::is_offboarding(para));

		// Cannot enqueue a message.
		assert!(try_queue_upward_msg(para, msg.clone()).is_err());
		run_to_block(6, vec![6]);
		// Para is still there and still cannot enqueue a message.
		assert!(Paras::is_offboarding(para));
		assert!(try_queue_upward_msg(para, msg.clone()).is_err());
		// Now offboarding is completed.
		run_to_block(7, vec![7]);
		assert!(!Paras::is_valid_para(para));
	});
}
