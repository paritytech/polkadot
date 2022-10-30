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

use frame_support::assert_ok;
use keyring::Sr25519Keyring;
use primitives::v2::{BlockNumber, CollatorId, SessionIndex, ValidatorId};

use crate::{
	configuration::HostConfiguration,
	initializer::SessionChangeNotification,
	mock::{
		new_test_ext, Configuration, MockGenesisConfig, Paras, ParasShared, Scheduler, System, Test,
	},
	paras::{ParaGenesisArgs, ParaKind},
};

fn schedule_blank_para(id: ParaId, parakind: ParaKind) {
	assert_ok!(Paras::schedule_para_initialize(
		id,
		ParaGenesisArgs {
			genesis_head: Vec::new().into(),
			validation_code: vec![1, 2, 3].into(),
			para_kind: parakind,
		}
	));
}

fn run_to_block(
	to: BlockNumber,
	new_session: impl Fn(BlockNumber) -> Option<SessionChangeNotification<BlockNumber>>,
) {
	while System::block_number() < to {
		let b = System::block_number();

		Scheduler::initializer_finalize();
		Paras::initializer_finalize(b);

		if let Some(notification) = new_session(b + 1) {
			let mut notification_with_session_index = notification;
			// We will make every session change trigger an action queue. Normally this may require 2 or more session changes.
			if notification_with_session_index.session_index == SessionIndex::default() {
				notification_with_session_index.session_index = ParasShared::scheduled_session();
			}
			Paras::initializer_on_new_session(&notification_with_session_index);
			Scheduler::initializer_on_new_session(&notification_with_session_index);
		}

		System::on_finalize(b);

		System::on_initialize(b + 1);
		System::set_block_number(b + 1);

		Paras::initializer_initialize(b + 1);
		Scheduler::initializer_initialize(b + 1);

		// In the real runtime this is expected to be called by the `InclusionInherent` pallet.
		Scheduler::clear();
		Scheduler::schedule(Vec::new(), b + 1);
	}
}

fn run_to_end_of_block(
	to: BlockNumber,
	new_session: impl Fn(BlockNumber) -> Option<SessionChangeNotification<BlockNumber>>,
) {
	run_to_block(to, &new_session);

	Scheduler::initializer_finalize();
	Paras::initializer_finalize(to);

	if let Some(notification) = new_session(to + 1) {
		Paras::initializer_on_new_session(&notification);
		Scheduler::initializer_on_new_session(&notification);
	}

	System::on_finalize(to);
}

fn default_config() -> HostConfiguration<BlockNumber> {
	HostConfiguration {
		parathread_cores: 3,
		group_rotation_frequency: 10,
		chain_availability_period: 3,
		thread_availability_period: 5,
		scheduling_lookahead: 2,
		parathread_retries: 1,
		pvf_checking_enabled: false,
		// This field does not affect anything that scheduler does. However, `HostConfiguration`
		// is still a subject to consistency test. It requires that `minimum_validation_upgrade_delay`
		// is greater than `chain_availability_period` and `thread_availability_period`.
		minimum_validation_upgrade_delay: 6,
		..Default::default()
	}
}

#[test]
fn add_parathread_claim_works() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let thread_id = ParaId::from(10);
	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(thread_id, ParaKind::Parathread);

		assert!(!Paras::is_parathread(thread_id));

		run_to_block(10, |n| if n == 10 { Some(Default::default()) } else { None });

		assert!(Paras::is_parathread(thread_id));

		{
			Scheduler::add_parathread_claim(ParathreadClaim(thread_id, collator.clone()));
			let queue = ParathreadQueue::<Test>::get();
			assert_eq!(queue.next_core_offset, 1);
			assert_eq!(queue.queue.len(), 1);
			assert_eq!(
				queue.queue[0],
				QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_id, collator.clone()),
						retries: 0,
					},
					core_offset: 0,
				}
			);
		}

		// due to the index, completing claims are not allowed.
		{
			let collator2 = CollatorId::from(Sr25519Keyring::Bob.public());
			Scheduler::add_parathread_claim(ParathreadClaim(thread_id, collator2.clone()));
			let queue = ParathreadQueue::<Test>::get();
			assert_eq!(queue.next_core_offset, 1);
			assert_eq!(queue.queue.len(), 1);
			assert_eq!(
				queue.queue[0],
				QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_id, collator.clone()),
						retries: 0,
					},
					core_offset: 0,
				}
			);
		}

		// claims on non-live parathreads have no effect.
		{
			let thread_id2 = ParaId::from(11);
			Scheduler::add_parathread_claim(ParathreadClaim(thread_id2, collator.clone()));
			let queue = ParathreadQueue::<Test>::get();
			assert_eq!(queue.next_core_offset, 1);
			assert_eq!(queue.queue.len(), 1);
			assert_eq!(
				queue.queue[0],
				QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_id, collator.clone()),
						retries: 0,
					},
					core_offset: 0,
				}
			);
		}
	})
}

#[test]
fn cannot_add_claim_when_no_parathread_cores() {
	let config = {
		let mut config = default_config();
		config.parathread_cores = 0;
		config
	};
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig { config, ..Default::default() },
		..Default::default()
	};

	let thread_id = ParaId::from(10);
	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(thread_id, ParaKind::Parathread);

		assert!(!Paras::is_parathread(thread_id));

		run_to_block(10, |n| if n == 10 { Some(Default::default()) } else { None });

		assert!(Paras::is_parathread(thread_id));

		Scheduler::add_parathread_claim(ParathreadClaim(thread_id, collator.clone()));
		assert_eq!(ParathreadQueue::<Test>::get(), Default::default());
	});
}

#[test]
fn session_change_prunes_cores_beyond_retries_and_those_from_non_live_parathreads() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};
	let max_parathread_retries = default_config().parathread_retries;

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);
	let thread_c = ParaId::from(3_u32);
	let thread_d = ParaId::from(4_u32);

	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(Configuration::config(), default_config());

		// threads a, b, and c will be live in next session, but not d.
		{
			schedule_blank_para(thread_a, ParaKind::Parathread);
			schedule_blank_para(thread_b, ParaKind::Parathread);
			schedule_blank_para(thread_c, ParaKind::Parathread);
		}

		// set up a queue as if `n_cores` was 4 and with some with many retries.
		ParathreadQueue::<Test>::put({
			let mut queue = ParathreadClaimQueue::default();

			// Will be pruned: too many retries.
			queue.enqueue_entry(
				ParathreadEntry {
					claim: ParathreadClaim(thread_a, collator.clone()),
					retries: max_parathread_retries + 1,
				},
				4,
			);

			// Will not be pruned.
			queue.enqueue_entry(
				ParathreadEntry {
					claim: ParathreadClaim(thread_b, collator.clone()),
					retries: max_parathread_retries,
				},
				4,
			);

			// Will not be pruned.
			queue.enqueue_entry(
				ParathreadEntry { claim: ParathreadClaim(thread_c, collator.clone()), retries: 0 },
				4,
			);

			// Will be pruned: not a live parathread.
			queue.enqueue_entry(
				ParathreadEntry { claim: ParathreadClaim(thread_d, collator.clone()), retries: 0 },
				4,
			);

			queue
		});

		ParathreadClaimIndex::<Test>::put(vec![thread_a, thread_b, thread_c, thread_d]);

		run_to_block(10, |b| match b {
			10 => Some(SessionChangeNotification {
				new_config: Configuration::config(),
				..Default::default()
			}),
			_ => None,
		});
		assert_eq!(Configuration::config(), default_config());

		let queue = ParathreadQueue::<Test>::get();
		assert_eq!(
			queue.queue,
			vec![
				QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_b, collator.clone()),
						retries: max_parathread_retries,
					},
					core_offset: 0,
				},
				QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_c, collator.clone()),
						retries: 0,
					},
					core_offset: 1,
				},
			]
		);
		assert_eq!(queue.next_core_offset, 2);

		assert_eq!(ParathreadClaimIndex::<Test>::get(), vec![thread_b, thread_c]);
	})
}

#[test]
fn session_change_shuffles_validators() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	assert_eq!(default_config().parathread_cores, 3);
	new_test_ext(genesis_config).execute_with(|| {
		let chain_a = ParaId::from(1_u32);
		let chain_b = ParaId::from(2_u32);

		// ensure that we have 5 groups by registering 2 parachains.
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(chain_b, ParaKind::Parachain);

		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Bob.public()),
					ValidatorId::from(Sr25519Keyring::Charlie.public()),
					ValidatorId::from(Sr25519Keyring::Dave.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
					ValidatorId::from(Sr25519Keyring::Ferdie.public()),
					ValidatorId::from(Sr25519Keyring::One.public()),
				],
				random_seed: [99; 32],
				..Default::default()
			}),
			_ => None,
		});

		let groups = ValidatorGroups::<Test>::get();
		assert_eq!(groups.len(), 5);

		// first two groups have the overflow.
		for i in 0..2 {
			assert_eq!(groups[i].len(), 2);
		}

		for i in 2..5 {
			assert_eq!(groups[i].len(), 1);
		}
	});
}

#[test]
fn session_change_takes_only_max_per_core() {
	let config = {
		let mut config = default_config();
		config.parathread_cores = 0;
		config.max_validators_per_core = Some(1);
		config
	};

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(genesis_config).execute_with(|| {
		let chain_a = ParaId::from(1_u32);
		let chain_b = ParaId::from(2_u32);
		let chain_c = ParaId::from(3_u32);

		// ensure that we have 5 groups by registering 2 parachains.
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(chain_b, ParaKind::Parachain);
		schedule_blank_para(chain_c, ParaKind::Parathread);

		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: config.clone(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Bob.public()),
					ValidatorId::from(Sr25519Keyring::Charlie.public()),
					ValidatorId::from(Sr25519Keyring::Dave.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
					ValidatorId::from(Sr25519Keyring::Ferdie.public()),
					ValidatorId::from(Sr25519Keyring::One.public()),
				],
				random_seed: [99; 32],
				..Default::default()
			}),
			_ => None,
		});

		let groups = ValidatorGroups::<Test>::get();
		assert_eq!(groups.len(), 7);

		// Every validator gets its own group, even though there are 2 paras.
		for i in 0..7 {
			assert_eq!(groups[i].len(), 1);
		}
	});
}

#[test]
fn schedule_schedules() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);

	let thread_a = ParaId::from(3_u32);
	let thread_b = ParaId::from(4_u32);
	let thread_c = ParaId::from(5_u32);

	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().parathread_cores, 3);

		// register 2 parachains
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(chain_b, ParaKind::Parachain);

		// and 3 parathreads
		schedule_blank_para(thread_a, ParaKind::Parathread);
		schedule_blank_para(thread_b, ParaKind::Parathread);
		schedule_blank_para(thread_c, ParaKind::Parathread);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Bob.public()),
					ValidatorId::from(Sr25519Keyring::Charlie.public()),
					ValidatorId::from(Sr25519Keyring::Dave.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		{
			let scheduled = Scheduler::scheduled();
			assert_eq!(scheduled.len(), 2);

			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(0),
					para_id: chain_a,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(0),
				}
			);

			assert_eq!(
				scheduled[1],
				CoreAssignment {
					core: CoreIndex(1),
					para_id: chain_b,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(1),
				}
			);
		}

		// add a couple of parathread claims.
		Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
		Scheduler::add_parathread_claim(ParathreadClaim(thread_c, collator.clone()));

		run_to_block(2, |_| None);

		{
			let scheduled = Scheduler::scheduled();
			assert_eq!(scheduled.len(), 4);

			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(0),
					para_id: chain_a,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(0),
				}
			);

			assert_eq!(
				scheduled[1],
				CoreAssignment {
					core: CoreIndex(1),
					para_id: chain_b,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(1),
				}
			);

			assert_eq!(
				scheduled[2],
				CoreAssignment {
					core: CoreIndex(2),
					para_id: thread_a,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(2),
				}
			);

			assert_eq!(
				scheduled[3],
				CoreAssignment {
					core: CoreIndex(3),
					para_id: thread_c,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(3),
				}
			);
		}
	});
}

#[test]
fn schedule_schedules_including_just_freed() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);

	let thread_a = ParaId::from(3_u32);
	let thread_b = ParaId::from(4_u32);
	let thread_c = ParaId::from(5_u32);
	let thread_d = ParaId::from(6_u32);
	let thread_e = ParaId::from(7_u32);

	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().parathread_cores, 3);

		// register 2 parachains
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(chain_b, ParaKind::Parachain);

		// and 5 parathreads
		schedule_blank_para(thread_a, ParaKind::Parathread);
		schedule_blank_para(thread_b, ParaKind::Parathread);
		schedule_blank_para(thread_c, ParaKind::Parathread);
		schedule_blank_para(thread_d, ParaKind::Parathread);
		schedule_blank_para(thread_e, ParaKind::Parathread);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Bob.public()),
					ValidatorId::from(Sr25519Keyring::Charlie.public()),
					ValidatorId::from(Sr25519Keyring::Dave.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		// add a couple of parathread claims now that the parathreads are live.
		Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
		Scheduler::add_parathread_claim(ParathreadClaim(thread_c, collator.clone()));

		run_to_block(2, |_| None);

		assert_eq!(Scheduler::scheduled().len(), 4);

		// cores 0, 1, 2, and 3 should be occupied. mark them as such.
		Scheduler::occupied(&[CoreIndex(0), CoreIndex(1), CoreIndex(2), CoreIndex(3)]);

		{
			let cores = AvailabilityCores::<Test>::get();

			assert!(cores[0].is_some());
			assert!(cores[1].is_some());
			assert!(cores[2].is_some());
			assert!(cores[3].is_some());
			assert!(cores[4].is_none());

			assert!(Scheduler::scheduled().is_empty());
		}

		// add a couple more parathread claims - the claim on `b` will go to the 3rd parathread core (4)
		// and the claim on `d` will go back to the 1st parathread core (2). The claim on `e` then
		// will go for core `3`.
		Scheduler::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));
		Scheduler::add_parathread_claim(ParathreadClaim(thread_d, collator.clone()));
		Scheduler::add_parathread_claim(ParathreadClaim(thread_e, collator.clone()));

		run_to_block(3, |_| None);

		{
			let scheduled = Scheduler::scheduled();

			// cores 0 and 1 are occupied by parachains. cores 2 and 3 are occupied by parathread
			// claims. core 4 was free.
			assert_eq!(scheduled.len(), 1);
			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(4),
					para_id: thread_b,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(4),
				}
			);
		}

		// now note that cores 0, 2, and 3 were freed.
		Scheduler::schedule(
			vec![
				(CoreIndex(0), FreedReason::Concluded),
				(CoreIndex(2), FreedReason::Concluded),
				(CoreIndex(3), FreedReason::TimedOut), // should go back on queue.
			],
			3,
		);

		{
			let scheduled = Scheduler::scheduled();

			// 1 thing scheduled before, + 3 cores freed.
			assert_eq!(scheduled.len(), 4);
			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(0),
					para_id: chain_a,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(0),
				}
			);
			assert_eq!(
				scheduled[1],
				CoreAssignment {
					core: CoreIndex(2),
					para_id: thread_d,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(2),
				}
			);
			assert_eq!(
				scheduled[2],
				CoreAssignment {
					core: CoreIndex(3),
					para_id: thread_e,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(3),
				}
			);
			assert_eq!(
				scheduled[3],
				CoreAssignment {
					core: CoreIndex(4),
					para_id: thread_b,
					kind: AssignmentKind::Parathread(collator.clone(), 0),
					group_idx: GroupIndex(4),
				}
			);

			// the prior claim on thread A concluded, but the claim on thread C was marked as
			// timed out.
			let index = ParathreadClaimIndex::<Test>::get();
			let parathread_queue = ParathreadQueue::<Test>::get();

			// thread A claim should have been wiped, but thread C claim should remain.
			assert_eq!(index, vec![thread_b, thread_c, thread_d, thread_e]);

			// Although C was descheduled, the core `4`  was occupied so C goes back on the queue.
			assert_eq!(parathread_queue.queue.len(), 1);
			assert_eq!(
				parathread_queue.queue[0],
				QueuedParathread {
					claim: ParathreadEntry {
						claim: ParathreadClaim(thread_c, collator.clone()),
						retries: 0, // retries not incremented by timeout - validators' fault.
					},
					core_offset: 2, // reassigned to next core. thread_e claim was on offset 1.
				}
			);
		}
	});
}

#[test]
fn schedule_clears_availability_cores() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);
	let chain_c = ParaId::from(3_u32);

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().parathread_cores, 3);

		// register 3 parachains
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(chain_b, ParaKind::Parachain);
		schedule_blank_para(chain_c, ParaKind::Parachain);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Bob.public()),
					ValidatorId::from(Sr25519Keyring::Charlie.public()),
					ValidatorId::from(Sr25519Keyring::Dave.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		run_to_block(2, |_| None);

		assert_eq!(Scheduler::scheduled().len(), 3);

		// cores 0, 1, and 2 should be occupied. mark them as such.
		Scheduler::occupied(&[CoreIndex(0), CoreIndex(1), CoreIndex(2)]);

		{
			let cores = AvailabilityCores::<Test>::get();

			assert!(cores[0].is_some());
			assert!(cores[1].is_some());
			assert!(cores[2].is_some());

			assert!(Scheduler::scheduled().is_empty());
		}

		run_to_block(3, |_| None);

		// now note that cores 0 and 2 were freed.
		Scheduler::schedule(
			vec![(CoreIndex(0), FreedReason::Concluded), (CoreIndex(2), FreedReason::Concluded)],
			3,
		);

		{
			let scheduled = Scheduler::scheduled();

			assert_eq!(scheduled.len(), 2);
			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(0),
					para_id: chain_a,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(0),
				}
			);
			assert_eq!(
				scheduled[1],
				CoreAssignment {
					core: CoreIndex(2),
					para_id: chain_c,
					kind: AssignmentKind::Parachain,
					group_idx: GroupIndex(2),
				}
			);

			// The freed cores should be `None` in `AvailabilityCores`.
			let cores = AvailabilityCores::<Test>::get();
			assert!(cores[0].is_none());
			assert!(cores[2].is_none());
		}
	});
}

#[test]
fn schedule_rotates_groups() {
	let config = {
		let mut config = default_config();

		// make sure parathread requests don't retry-out
		config.parathread_retries = config.group_rotation_frequency * 3;
		config.parathread_cores = 2;
		config
	};

	let rotation_frequency = config.group_rotation_frequency;
	let parathread_cores = config.parathread_cores;

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	};

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);

	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().parathread_cores, 3);

		schedule_blank_para(thread_a, ParaKind::Parathread);
		schedule_blank_para(thread_b, ParaKind::Parathread);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: config.clone(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		let session_start_block = <Scheduler as Store>::SessionStartBlock::get();
		assert_eq!(session_start_block, 1);

		Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
		Scheduler::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));

		run_to_block(2, |_| None);

		let assert_groups_rotated = |rotations: u32| {
			let scheduled = Scheduler::scheduled();
			assert_eq!(scheduled.len(), 2);
			assert_eq!(scheduled[0].group_idx, GroupIndex((0u32 + rotations) % parathread_cores));
			assert_eq!(scheduled[1].group_idx, GroupIndex((1u32 + rotations) % parathread_cores));
		};

		assert_groups_rotated(0);

		// one block before first rotation.
		run_to_block(rotation_frequency, |_| None);

		assert_groups_rotated(0);

		// first rotation.
		run_to_block(rotation_frequency + 1, |_| None);
		assert_groups_rotated(1);

		// one block before second rotation.
		run_to_block(rotation_frequency * 2, |_| None);
		assert_groups_rotated(1);

		// second rotation.
		run_to_block(rotation_frequency * 2 + 1, |_| None);
		assert_groups_rotated(2);
	});
}

#[test]
fn parathread_claims_are_pruned_after_retries() {
	let max_retries = default_config().parathread_retries;

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);

	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().parathread_cores, 3);

		schedule_blank_para(thread_a, ParaKind::Parathread);
		schedule_blank_para(thread_b, ParaKind::Parathread);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
		Scheduler::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));

		run_to_block(2, |_| None);
		assert_eq!(Scheduler::scheduled().len(), 2);

		run_to_block(2 + max_retries, |_| None);
		assert_eq!(Scheduler::scheduled().len(), 2);

		run_to_block(2 + max_retries + 1, |_| None);
		assert_eq!(Scheduler::scheduled().len(), 0);
	});
}

#[test]
fn availability_predicate_works() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let HostConfiguration {
		group_rotation_frequency,
		chain_availability_period,
		thread_availability_period,
		..
	} = default_config();
	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	assert!(
		chain_availability_period < thread_availability_period &&
			thread_availability_period < group_rotation_frequency
	);

	let chain_a = ParaId::from(1_u32);
	let thread_a = ParaId::from(2_u32);

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(thread_a, ParaKind::Parathread);

		// start a new session with our chain & thread registered.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Bob.public()),
					ValidatorId::from(Sr25519Keyring::Charlie.public()),
					ValidatorId::from(Sr25519Keyring::Dave.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		// assign some availability cores.
		{
			AvailabilityCores::<Test>::mutate(|cores| {
				cores[0] = Some(CoreOccupied::Parachain);
				cores[1] = Some(CoreOccupied::Parathread(ParathreadEntry {
					claim: ParathreadClaim(thread_a, collator),
					retries: 0,
				}))
			});
		}

		run_to_block(1 + thread_availability_period, |_| None);
		assert!(Scheduler::availability_timeout_predicate().is_none());

		run_to_block(1 + group_rotation_frequency, |_| None);

		{
			let pred = Scheduler::availability_timeout_predicate()
				.expect("predicate exists recently after rotation");

			let now = System::block_number();
			let would_be_timed_out = now - thread_availability_period;
			for i in 0..AvailabilityCores::<Test>::get().len() {
				// returns true for unoccupied cores.
				// And can time out both threads and chains at this stage.
				assert!(pred(CoreIndex(i as u32), would_be_timed_out));
			}

			assert!(!pred(CoreIndex(0), now)); // assigned: chain
			assert!(!pred(CoreIndex(1), now)); // assigned: thread
			assert!(pred(CoreIndex(2), now));

			// check the tighter bound on chains vs threads.
			assert!(pred(CoreIndex(0), now - chain_availability_period));
			assert!(!pred(CoreIndex(1), now - chain_availability_period));

			// check the threshold is exact.
			assert!(!pred(CoreIndex(0), now - chain_availability_period + 1));
			assert!(!pred(CoreIndex(1), now - thread_availability_period + 1));
		}

		run_to_block(1 + group_rotation_frequency + chain_availability_period, |_| None);

		{
			let pred = Scheduler::availability_timeout_predicate()
				.expect("predicate exists recently after rotation");

			let would_be_timed_out = System::block_number() - thread_availability_period;

			assert!(!pred(CoreIndex(0), would_be_timed_out)); // chains can't be timed out now.
			assert!(pred(CoreIndex(1), would_be_timed_out)); // but threads can.
		}

		run_to_block(1 + group_rotation_frequency + thread_availability_period, |_| None);

		assert!(Scheduler::availability_timeout_predicate().is_none());
	});
}

#[test]
fn next_up_on_available_uses_next_scheduled_or_none_for_thread() {
	let mut config = default_config();
	config.parathread_cores = 1;

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	};

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);

	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(thread_a, ParaKind::Parathread);
		schedule_blank_para(thread_b, ParaKind::Parathread);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: config.clone(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		let thread_claim_a = ParathreadClaim(thread_a, collator.clone());
		let thread_claim_b = ParathreadClaim(thread_b, collator.clone());

		Scheduler::add_parathread_claim(thread_claim_a.clone());

		run_to_block(2, |_| None);

		{
			assert_eq!(Scheduler::scheduled().len(), 1);
			assert_eq!(Scheduler::availability_cores().len(), 1);

			Scheduler::occupied(&[CoreIndex(0)]);

			let cores = Scheduler::availability_cores();
			match cores[0].as_ref().unwrap() {
				CoreOccupied::Parathread(entry) => assert_eq!(entry.claim, thread_claim_a),
				_ => panic!("with no chains, only core should be a thread core"),
			}

			assert!(Scheduler::next_up_on_available(CoreIndex(0)).is_none());

			Scheduler::add_parathread_claim(thread_claim_b);

			let queue = ParathreadQueue::<Test>::get();
			assert_eq!(
				queue.get_next_on_core(0).unwrap().claim,
				ParathreadClaim(thread_b, collator.clone()),
			);

			assert_eq!(
				Scheduler::next_up_on_available(CoreIndex(0)).unwrap(),
				ScheduledCore { para_id: thread_b, collator: Some(collator.clone()) }
			);
		}
	});
}

#[test]
fn next_up_on_time_out_reuses_claim_if_nothing_queued() {
	let mut config = default_config();
	config.parathread_cores = 1;

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	};

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);

	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(thread_a, ParaKind::Parathread);
		schedule_blank_para(thread_b, ParaKind::Parathread);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: config.clone(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		let thread_claim_a = ParathreadClaim(thread_a, collator.clone());
		let thread_claim_b = ParathreadClaim(thread_b, collator.clone());

		Scheduler::add_parathread_claim(thread_claim_a.clone());

		run_to_block(2, |_| None);

		{
			assert_eq!(Scheduler::scheduled().len(), 1);
			assert_eq!(Scheduler::availability_cores().len(), 1);

			Scheduler::occupied(&[CoreIndex(0)]);

			let cores = Scheduler::availability_cores();
			match cores[0].as_ref().unwrap() {
				CoreOccupied::Parathread(entry) => assert_eq!(entry.claim, thread_claim_a),
				_ => panic!("with no chains, only core should be a thread core"),
			}

			let queue = ParathreadQueue::<Test>::get();
			assert!(queue.get_next_on_core(0).is_none());
			assert_eq!(
				Scheduler::next_up_on_time_out(CoreIndex(0)).unwrap(),
				ScheduledCore { para_id: thread_a, collator: Some(collator.clone()) }
			);

			Scheduler::add_parathread_claim(thread_claim_b);

			let queue = ParathreadQueue::<Test>::get();
			assert_eq!(
				queue.get_next_on_core(0).unwrap().claim,
				ParathreadClaim(thread_b, collator.clone()),
			);

			// Now that there is an earlier next-up, we use that.
			assert_eq!(
				Scheduler::next_up_on_available(CoreIndex(0)).unwrap(),
				ScheduledCore { para_id: thread_b, collator: Some(collator.clone()) }
			);
		}
	});
}

#[test]
fn next_up_on_available_is_parachain_always() {
	let mut config = default_config();
	config.parathread_cores = 0;

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	};

	let chain_a = ParaId::from(1_u32);

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(chain_a, ParaKind::Parachain);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: config.clone(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		run_to_block(2, |_| None);

		{
			assert_eq!(Scheduler::scheduled().len(), 1);
			assert_eq!(Scheduler::availability_cores().len(), 1);

			Scheduler::occupied(&[CoreIndex(0)]);

			let cores = Scheduler::availability_cores();
			match cores[0].as_ref().unwrap() {
				CoreOccupied::Parachain => {},
				_ => panic!("with no threads, only core should be a chain core"),
			}

			// Now that there is an earlier next-up, we use that.
			assert_eq!(
				Scheduler::next_up_on_available(CoreIndex(0)).unwrap(),
				ScheduledCore { para_id: chain_a, collator: None }
			);
		}
	});
}

#[test]
fn next_up_on_time_out_is_parachain_always() {
	let mut config = default_config();
	config.parathread_cores = 0;

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	};

	let chain_a = ParaId::from(1_u32);

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(chain_a, ParaKind::Parachain);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: config.clone(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		run_to_block(2, |_| None);

		{
			assert_eq!(Scheduler::scheduled().len(), 1);
			assert_eq!(Scheduler::availability_cores().len(), 1);

			Scheduler::occupied(&[CoreIndex(0)]);

			let cores = Scheduler::availability_cores();
			match cores[0].as_ref().unwrap() {
				CoreOccupied::Parachain => {},
				_ => panic!("with no threads, only core should be a chain core"),
			}

			// Now that there is an earlier next-up, we use that.
			assert_eq!(
				Scheduler::next_up_on_available(CoreIndex(0)).unwrap(),
				ScheduledCore { para_id: chain_a, collator: None }
			);
		}
	});
}

#[test]
fn session_change_requires_reschedule_dropping_removed_paras() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	assert_eq!(default_config().parathread_cores, 3);
	new_test_ext(genesis_config).execute_with(|| {
		let chain_a = ParaId::from(1_u32);
		let chain_b = ParaId::from(2_u32);

		// ensure that we have 5 groups by registering 2 parachains.
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(chain_b, ParaKind::Parachain);

		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Bob.public()),
					ValidatorId::from(Sr25519Keyring::Charlie.public()),
					ValidatorId::from(Sr25519Keyring::Dave.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
					ValidatorId::from(Sr25519Keyring::Ferdie.public()),
					ValidatorId::from(Sr25519Keyring::One.public()),
				],
				random_seed: [99; 32],
				..Default::default()
			}),
			_ => None,
		});

		assert_eq!(Scheduler::scheduled().len(), 2);

		let groups = ValidatorGroups::<Test>::get();
		assert_eq!(groups.len(), 5);

		assert_ok!(Paras::schedule_para_cleanup(chain_b));

		run_to_end_of_block(2, |number| match number {
			2 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Bob.public()),
					ValidatorId::from(Sr25519Keyring::Charlie.public()),
					ValidatorId::from(Sr25519Keyring::Dave.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
					ValidatorId::from(Sr25519Keyring::Ferdie.public()),
					ValidatorId::from(Sr25519Keyring::One.public()),
				],
				random_seed: [99; 32],
				..Default::default()
			}),
			_ => None,
		});

		Scheduler::clear();
		Scheduler::schedule(Vec::new(), 3);

		assert_eq!(
			Scheduler::scheduled(),
			vec![CoreAssignment {
				core: CoreIndex(0),
				para_id: chain_a,
				kind: AssignmentKind::Parachain,
				group_idx: GroupIndex(0),
			}],
		);
	});
}

#[test]
fn parathread_claims_are_pruned_after_deregistration() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);

	let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().parathread_cores, 3);

		schedule_blank_para(thread_a, ParaKind::Parathread);
		schedule_blank_para(thread_b, ParaKind::Parathread);

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(1, |number| match number {
			1 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		Scheduler::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
		Scheduler::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));

		run_to_block(2, |_| None);
		assert_eq!(Scheduler::scheduled().len(), 2);

		assert_ok!(Paras::schedule_para_cleanup(thread_a));

		// start a new session to activate, 5 validators for 5 cores.
		run_to_block(3, |number| match number {
			3 => Some(SessionChangeNotification {
				new_config: default_config(),
				validators: vec![
					ValidatorId::from(Sr25519Keyring::Alice.public()),
					ValidatorId::from(Sr25519Keyring::Eve.public()),
				],
				..Default::default()
			}),
			_ => None,
		});

		assert_eq!(Scheduler::scheduled().len(), 1);
	});
}
