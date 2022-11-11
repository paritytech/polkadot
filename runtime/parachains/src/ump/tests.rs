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

use super::*;
use crate::mock::{
	assert_last_event, new_test_ext, take_processed, Configuration, MockGenesisConfig,
	RuntimeOrigin, System, Test, Ump,
};
use frame_support::{assert_noop, assert_ok, weights::Weight};
use std::collections::HashSet;

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
	assert!(Ump::check_upward_messages(&Configuration::config(), para, &msgs).is_ok());
	let _ = Ump::receive_upward_messages(para, msgs);
}
/*
fn assert_storage_consistency_exhaustive() {
	// check that empty queues don't clutter the storage.
	for (_para, queue) in <Ump as Store>::RelayDispatchQueues::iter() {
		assert!(!queue.is_empty());
	}

	// actually count the counts and sizes in queues and compare them to the bookkept version.
	for (para, queue) in <Ump as Store>::RelayDispatchQueues::iter() {
		let (expected_count, expected_size) = <Ump as Store>::RelayDispatchQueueSize::get(para);
		let (actual_count, actual_size) = queue
			.into_iter()
			.fold((0, 0), |(acc_count, acc_size), x| (acc_count + 1, acc_size + x.len() as u32));

		assert_eq!(expected_count, actual_count);
		assert_eq!(expected_size, actual_size);
	}

	// since we wipe the empty queues the sets of paras in queue contents, queue sizes and
	// need dispatch set should all be equal.
	let queue_contents_set = <Ump as Store>::RelayDispatchQueues::iter()
		.map(|(k, _)| k)
		.collect::<HashSet<ParaId>>();
	let queue_sizes_set = <Ump as Store>::RelayDispatchQueueSize::iter()
		.map(|(k, _)| k)
		.collect::<HashSet<ParaId>>();
	let needs_dispatch_set =
		<Ump as Store>::NeedsDispatch::get().into_iter().collect::<HashSet<ParaId>>();
	assert_eq!(queue_contents_set, queue_sizes_set);
	assert_eq!(queue_contents_set, needs_dispatch_set);

	// `NextDispatchRoundStartWith` should point into a para that is tracked.
	if let Some(para) = <Ump as Store>::NextDispatchRoundStartWith::get() {
		assert!(queue_contents_set.contains(&para));
	}

	// `NeedsDispatch` is always sorted.
	assert!(<Ump as Store>::NeedsDispatch::get().windows(2).all(|xs| xs[0] <= xs[1]));
}

#[test]
fn dispatch_empty() {
	new_test_ext(default_genesis_config()).execute_with(|| {
		assert_storage_consistency_exhaustive();

		// make sure that the case with empty queues is handled properly
		Ump::process_pending_upward_messages();

		assert_storage_consistency_exhaustive();
	});
}

#[test]
fn dispatch_single_message() {
	let a = ParaId::from(228);
	let msg = 1000u32.encode();

	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		queue_upward_msg(a, msg.clone());
		Ump::process_pending_upward_messages();
		assert_eq!(take_processed(), vec![(a, msg)]);

		assert_storage_consistency_exhaustive();
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

		//ump_service_total_weight: Weight::from_ref_time(500).set_proof_size(500),
		assert_storage_consistency_exhaustive();

		// we expect only two first messages to fit in the first iteration.
		Ump::process_pending_upward_messages();
		assert_eq!(take_processed(), vec![(a, a_msg_1), (c, c_msg_1)]);
		assert_storage_consistency_exhaustive();

		queue_upward_msg(c, c_msg_2.clone());
		assert_storage_consistency_exhaustive();

		// second iteration should process the second message.
		Ump::process_pending_upward_messages();
		assert_eq!(take_processed(), vec![(q, q_msg)]);
		assert_storage_consistency_exhaustive();

		// 3rd iteration.
		Ump::process_pending_upward_messages();
		assert_eq!(take_processed(), vec![(a, a_msg_2), (c, c_msg_2)]);
		assert_storage_consistency_exhaustive();

		// finally, make sure that the queue is empty.
		Ump::process_pending_upward_messages();
		assert_eq!(take_processed(), vec![]);
		assert_storage_consistency_exhaustive();
	});
}

*/