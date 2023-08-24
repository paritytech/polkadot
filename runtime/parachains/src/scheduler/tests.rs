// Copyright (C) Parity Technologies (UK) Ltd.
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
use primitives::{v5::Assignment, BlockNumber, SessionIndex, ValidationCode, ValidatorId};
use sp_std::collections::{btree_map::BTreeMap, btree_set::BTreeSet};

use crate::{
	assigner_on_demand::QueuePushDirection,
	configuration::HostConfiguration,
	initializer::SessionChangeNotification,
	mock::{
		new_test_ext, MockGenesisConfig, OnDemandAssigner, Paras, ParasShared, RuntimeOrigin,
		Scheduler, System, Test,
	},
	paras::{ParaGenesisArgs, ParaKind},
};

fn schedule_blank_para(id: ParaId, parakind: ParaKind) {
	let validation_code: ValidationCode = vec![1, 2, 3].into();
	assert_ok!(Paras::schedule_para_initialize(
		id,
		ParaGenesisArgs {
			genesis_head: Vec::new().into(),
			validation_code: validation_code.clone(),
			para_kind: parakind,
		}
	));

	assert_ok!(Paras::add_trusted_validation_code(RuntimeOrigin::root(), validation_code));
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
			// We will make every session change trigger an action queue. Normally this may require
			// 2 or more session changes.
			if notification_with_session_index.session_index == SessionIndex::default() {
				notification_with_session_index.session_index = ParasShared::scheduled_session();
			}
			Scheduler::pre_new_session();

			Paras::initializer_on_new_session(&notification_with_session_index);
			Scheduler::initializer_on_new_session(&notification_with_session_index);
		}

		System::on_finalize(b);

		System::on_initialize(b + 1);
		System::set_block_number(b + 1);

		Paras::initializer_initialize(b + 1);
		Scheduler::initializer_initialize(b + 1);

		// In the real runtime this is expected to be called by the `InclusionInherent` pallet.
		Scheduler::update_claimqueue(BTreeMap::new(), b + 1);
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
		Scheduler::pre_new_session();

		Paras::initializer_on_new_session(&notification);
		Scheduler::initializer_on_new_session(&notification);
	}

	System::on_finalize(to);
}

fn default_config() -> HostConfiguration<BlockNumber> {
	HostConfiguration {
		on_demand_cores: 3,
		group_rotation_frequency: 10,
		paras_availability_period: 3,
		scheduling_lookahead: 2,
		on_demand_retries: 1,
		// This field does not affect anything that scheduler does. However, `HostConfiguration`
		// is still a subject to consistency test. It requires that
		// `minimum_validation_upgrade_delay` is greater than `chain_availability_period` and
		// `thread_availability_period`.
		minimum_validation_upgrade_delay: 6,
		..Default::default()
	}
}

fn genesis_config(config: &HostConfiguration<BlockNumber>) -> MockGenesisConfig {
	MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig { config: config.clone() },
		..Default::default()
	}
}

pub(crate) fn claimqueue_contains_only_none() -> bool {
	let mut cq = Scheduler::claimqueue();
	for (_, v) in cq.iter_mut() {
		v.retain(|e| e.is_some());
	}

	cq.values().map(|v| v.len()).sum::<usize>() == 0
}

pub(crate) fn claimqueue_contains_para_ids<T: Config>(pids: Vec<ParaId>) -> bool {
	let set: BTreeSet<ParaId> = ClaimQueue::<T>::get()
		.into_iter()
		.flat_map(|(_, assignments)| {
			assignments
				.into_iter()
				.filter_map(|assignment| assignment.and_then(|pe| Some(pe.para_id())))
		})
		.collect();

	pids.into_iter().all(|pid| set.contains(&pid))
}

pub(crate) fn availability_cores_contains_para_ids<T: Config>(pids: Vec<ParaId>) -> bool {
	let set: BTreeSet<ParaId> = AvailabilityCores::<T>::get()
		.into_iter()
		.filter_map(|core| match core {
			CoreOccupied::Free => None,
			CoreOccupied::Paras(entry) => Some(entry.para_id()),
		})
		.collect();

	pids.into_iter().all(|pid| set.contains(&pid))
}

#[test]
fn claimqueue_ttl_drop_fn_works() {
	let mut config = default_config();
	config.scheduling_lookahead = 3;
	let genesis_config = genesis_config(&config);

	let para_id = ParaId::from(100);
	let core_idx = CoreIndex::from(0);
	let mut now = 10;

	new_test_ext(genesis_config).execute_with(|| {
		assert!(default_config().on_demand_ttl == 5);
		// Register and run to a blockheight where the para is in a valid state.
		schedule_blank_para(para_id, ParaKind::Parathread);
		run_to_block(10, |n| if n == 10 { Some(Default::default()) } else { None });

		// Add a claim on core 0 with a ttl in the past.
		let paras_entry = ParasEntry::new(Assignment::new(para_id), now - 5);
		Scheduler::add_to_claimqueue(core_idx, paras_entry.clone());

		// Claim is in queue prior to call.
		assert!(claimqueue_contains_para_ids::<Test>(vec![para_id]));

		// Claim is dropped post call.
		Scheduler::drop_expired_claims_from_claimqueue();
		assert!(!claimqueue_contains_para_ids::<Test>(vec![para_id]));

		// Add a claim on core 0 with a ttl in the future (15).
		let paras_entry = ParasEntry::new(Assignment::new(para_id), now + 5);
		Scheduler::add_to_claimqueue(core_idx, paras_entry.clone());

		// Claim is in queue post call.
		Scheduler::drop_expired_claims_from_claimqueue();
		assert!(claimqueue_contains_para_ids::<Test>(vec![para_id]));

		now = now + 6;
		run_to_block(now, |_| None);

		// Claim is dropped
		Scheduler::drop_expired_claims_from_claimqueue();
		assert!(!claimqueue_contains_para_ids::<Test>(vec![para_id]));

		// Add a claim on core 0 with a ttl == now (16)
		let paras_entry = ParasEntry::new(Assignment::new(para_id), now);
		Scheduler::add_to_claimqueue(core_idx, paras_entry.clone());

		// Claim is in queue post call.
		Scheduler::drop_expired_claims_from_claimqueue();
		assert!(claimqueue_contains_para_ids::<Test>(vec![para_id]));

		now = now + 1;
		run_to_block(now, |_| None);

		// Drop expired claim.
		Scheduler::drop_expired_claims_from_claimqueue();

		// Add a claim on core 0 with a ttl == now (17)
		let paras_entry_non_expired = ParasEntry::new(Assignment::new(para_id), now);
		let paras_entry_expired = ParasEntry::new(Assignment::new(para_id), now - 2);
		// ttls = [17, 15, 17]
		Scheduler::add_to_claimqueue(core_idx, paras_entry_non_expired.clone());
		Scheduler::add_to_claimqueue(core_idx, paras_entry_expired.clone());
		Scheduler::add_to_claimqueue(core_idx, paras_entry_non_expired.clone());
		let cq = Scheduler::claimqueue();
		assert!(cq.get(&core_idx).unwrap().len() == 3);

		// Add claims to on demand assignment provider.
		let assignment = Assignment::new(para_id);

		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment.clone(),
			QueuePushDirection::Back
		));

		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment,
			QueuePushDirection::Back
		));

		// Drop expired claim.
		Scheduler::drop_expired_claims_from_claimqueue();

		let cq = Scheduler::claimqueue();
		let cqc = cq.get(&core_idx).unwrap();
		// Same number of claims
		assert!(cqc.len() == 3);

		// The first 2 claims in the queue should have a ttl of 17,
		// being the ones set up prior in this test as claims 1 and 3.
		// The third claim is popped from the assignment provider and
		// has a new ttl set by the scheduler of now + config.on_demand_ttl.
		// ttls = [17, 17, 22]
		assert!(cqc.iter().enumerate().all(|(index, entry)| {
			match index {
				0 | 1 => return entry.clone().unwrap().ttl == 17,
				2 => return entry.clone().unwrap().ttl == 22,
				_ => return false,
			}
		}))
	});
}

// Pretty useless here. Should be on parathread assigner... if at all
#[test]
fn add_parathread_claim_works() {
	let genesis_config = genesis_config(&default_config());

	let thread_id = ParaId::from(10);
	let core_index = CoreIndex::from(0);
	let entry_ttl = 10_000;

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(thread_id, ParaKind::Parathread);

		assert!(!Paras::is_parathread(thread_id));

		run_to_block(10, |n| if n == 10 { Some(Default::default()) } else { None });

		assert!(Paras::is_parathread(thread_id));

		let pe = ParasEntry::new(Assignment::new(thread_id), entry_ttl);
		Scheduler::add_to_claimqueue(core_index, pe.clone());

		let cq = Scheduler::claimqueue();
		assert_eq!(Scheduler::claimqueue_len(), 1);
		assert_eq!(*(cq.get(&core_index).unwrap().front().unwrap()), Some(pe));
	})
}

#[test]
fn session_change_shuffles_validators() {
	let genesis_config = genesis_config(&default_config());

	assert_eq!(default_config().on_demand_cores, 3);
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
		config.on_demand_cores = 0;
		config.max_validators_per_core = Some(1);
		config
	};

	let genesis_config = genesis_config(&config);

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
fn fill_claimqueue_fills() {
	let genesis_config = genesis_config(&default_config());

	let lookahead = genesis_config.configuration.config.scheduling_lookahead as usize;
	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);

	let thread_a = ParaId::from(3_u32);
	let thread_b = ParaId::from(4_u32);
	let thread_c = ParaId::from(5_u32);

	let assignment_a = Assignment { para_id: thread_a };
	let assignment_b = Assignment { para_id: thread_b };
	let assignment_c = Assignment { para_id: thread_c };

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().on_demand_cores, 3);

		// register 2 lease holding parachains
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(chain_b, ParaKind::Parachain);

		// and 3 parathreads (on-demand parachains)
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
			assert_eq!(Scheduler::claimqueue_len(), 2 * lookahead);
			let scheduled = Scheduler::scheduled_claimqueue();

			// Cannot assert on indices anymore as they depend on the assignment providers
			assert!(claimqueue_contains_para_ids::<Test>(vec![chain_a, chain_b]));

			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(0),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: chain_a },
						availability_timeouts: 0,
						ttl: 6
					},
				}
			);

			assert_eq!(
				scheduled[1],
				CoreAssignment {
					core: CoreIndex(1),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: chain_b },
						availability_timeouts: 0,
						ttl: 6
					},
				}
			);
		}

		// add a couple of parathread assignments.
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_a,
			QueuePushDirection::Back
		));
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_b,
			QueuePushDirection::Back
		));
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_c,
			QueuePushDirection::Back
		));

		run_to_block(2, |_| None);
		// cores 0 and 1 should be occupied. mark them as such.
		Scheduler::occupied(
			vec![(CoreIndex(0), chain_a), (CoreIndex(1), chain_b)].into_iter().collect(),
		);

		run_to_block(3, |_| None);

		{
			assert_eq!(Scheduler::claimqueue_len(), 5);
			let scheduled = Scheduler::scheduled_claimqueue();

			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(0),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: chain_a },
						availability_timeouts: 0,
						ttl: 6
					},
				}
			);
			assert_eq!(
				scheduled[1],
				CoreAssignment {
					core: CoreIndex(1),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: chain_b },
						availability_timeouts: 0,
						ttl: 6
					},
				}
			);

			// Was added a block later, note the TTL.
			assert_eq!(
				scheduled[2],
				CoreAssignment {
					core: CoreIndex(2),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: thread_a },
						availability_timeouts: 0,
						ttl: 7
					},
				}
			);
			// Sits on the same core as `thread_a`
			assert_eq!(
				Scheduler::claimqueue().get(&CoreIndex(2)).unwrap()[1],
				Some(ParasEntry {
					assignment: Assignment { para_id: thread_b },
					availability_timeouts: 0,
					ttl: 7
				})
			);
			assert_eq!(
				scheduled[3],
				CoreAssignment {
					core: CoreIndex(3),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: thread_c },
						availability_timeouts: 0,
						ttl: 7
					},
				}
			);
		}
	});
}

#[test]
fn schedule_schedules_including_just_freed() {
	let mut config = default_config();
	// NOTE: This test expects on demand cores to each get slotted on to a different core
	// and not fill up the claimqueue of each core first.
	config.scheduling_lookahead = 1;
	let genesis_config = genesis_config(&config);

	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);

	let thread_a = ParaId::from(3_u32);
	let thread_b = ParaId::from(4_u32);
	let thread_c = ParaId::from(5_u32);
	let thread_d = ParaId::from(6_u32);
	let thread_e = ParaId::from(7_u32);

	let assignment_a = Assignment { para_id: thread_a };
	let assignment_b = Assignment { para_id: thread_b };
	let assignment_c = Assignment { para_id: thread_c };
	let assignment_d = Assignment { para_id: thread_d };
	let assignment_e = Assignment { para_id: thread_e };

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().on_demand_cores, 3);

		// register 2 lease holding parachains
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(chain_b, ParaKind::Parachain);

		// and 5 parathreads (on-demand parachains)
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
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_a,
			QueuePushDirection::Back
		));
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_c,
			QueuePushDirection::Back
		));

		let mut now = 2;
		run_to_block(now, |_| None);

		assert_eq!(Scheduler::scheduled_claimqueue().len(), 4);

		// cores 0, 1, 2, and 3 should be occupied. mark them as such.
		let mut occupied_map: BTreeMap<CoreIndex, ParaId> = BTreeMap::new();
		occupied_map.insert(CoreIndex(0), chain_a);
		occupied_map.insert(CoreIndex(1), chain_b);
		occupied_map.insert(CoreIndex(2), thread_a);
		occupied_map.insert(CoreIndex(3), thread_c);
		Scheduler::occupied(occupied_map);

		{
			let cores = AvailabilityCores::<Test>::get();

			// cores 0, 1, 2, and 3 are all `CoreOccupied::Paras(ParasEntry...)`
			assert!(cores[0] != CoreOccupied::Free);
			assert!(cores[1] != CoreOccupied::Free);
			assert!(cores[2] != CoreOccupied::Free);
			assert!(cores[3] != CoreOccupied::Free);

			// core 4 is free
			assert!(cores[4] == CoreOccupied::Free);

			assert!(Scheduler::scheduled_claimqueue().is_empty());

			// All core index entries in the claimqueue should have `None` in them.
			Scheduler::claimqueue().iter().for_each(|(_core_idx, core_queue)| {
				assert!(core_queue.iter().all(|claim| claim.is_none()))
			})
		}

		// add a couple more parathread claims - the claim on `b` will go to the 3rd parathread core
		// (4) and the claim on `d` will go back to the 1st parathread core (2). The claim on `e`
		// then will go for core `3`.
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_b,
			QueuePushDirection::Back
		));
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_d,
			QueuePushDirection::Back
		));
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_e.clone(),
			QueuePushDirection::Back
		));
		now = 3;
		run_to_block(now, |_| None);

		{
			let scheduled = Scheduler::scheduled_claimqueue();

			// cores 0 and 1 are occupied by lease holding parachains. cores 2 and 3 are occupied by
			// on-demand parachain claims. core 4 was free.
			assert_eq!(scheduled.len(), 1);
			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(4),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: thread_b },
						availability_timeouts: 0,
						ttl: 8
					},
				}
			);
		}

		// now note that cores 0, 2, and 3 were freed.
		let just_updated: BTreeMap<CoreIndex, FreedReason> = vec![
			(CoreIndex(0), FreedReason::Concluded),
			(CoreIndex(2), FreedReason::Concluded),
			(CoreIndex(3), FreedReason::TimedOut), // should go back on queue.
		]
		.into_iter()
		.collect();
		Scheduler::update_claimqueue(just_updated, now);

		{
			let scheduled = Scheduler::scheduled_claimqueue();

			// 1 thing scheduled before, + 3 cores freed.
			assert_eq!(scheduled.len(), 4);
			assert_eq!(
				scheduled[0],
				CoreAssignment {
					core: CoreIndex(0),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: chain_a },
						availability_timeouts: 0,
						ttl: 8
					},
				}
			);
			assert_eq!(
				scheduled[1],
				CoreAssignment {
					core: CoreIndex(2),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: thread_d },
						availability_timeouts: 0,
						ttl: 8
					},
				}
			);
			// Although C was descheduled, the core `4` was occupied so C goes back to the queue.
			assert_eq!(
				scheduled[2],
				CoreAssignment {
					core: CoreIndex(3),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: thread_c },
						availability_timeouts: 1,
						ttl: 8
					},
				}
			);
			assert_eq!(
				scheduled[3],
				CoreAssignment {
					core: CoreIndex(4),
					paras_entry: ParasEntry {
						assignment: Assignment { para_id: thread_b },
						availability_timeouts: 0,
						ttl: 8
					},
				}
			);

			// The only assignment yet to be popped on to the claim queue is `thread_e`.
			// This is due to `thread_c` timing out.
			let order_queue = OnDemandAssigner::get_queue();
			assert!(order_queue.len() == 1);
			assert!(order_queue[0] == assignment_e);

			// Chain B's core was not marked concluded or timed out, it should be on an
			// availability core
			assert!(availability_cores_contains_para_ids::<Test>(vec![chain_b]));
			// Thread A claim should have been wiped, but thread C claim should remain.
			assert!(!claimqueue_contains_para_ids::<Test>(vec![thread_a]));
			assert!(claimqueue_contains_para_ids::<Test>(vec![thread_c]));
			assert!(!availability_cores_contains_para_ids::<Test>(vec![thread_a, thread_c]));
		}
	});
}

#[test]
fn schedule_clears_availability_cores() {
	let mut config = default_config();
	config.scheduling_lookahead = 1;
	let genesis_config = genesis_config(&config);

	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);
	let chain_c = ParaId::from(3_u32);

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().on_demand_cores, 3);

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

		assert_eq!(Scheduler::claimqueue().len(), 3);

		// cores 0, 1, and 2 should be occupied. mark them as such.
		Scheduler::occupied(
			vec![(CoreIndex(0), chain_a), (CoreIndex(1), chain_b), (CoreIndex(2), chain_c)]
				.into_iter()
				.collect(),
		);

		{
			let cores = AvailabilityCores::<Test>::get();

			assert_eq!(cores[0].is_free(), false);
			assert_eq!(cores[1].is_free(), false);
			assert_eq!(cores[2].is_free(), false);

			assert!(claimqueue_contains_only_none());
		}

		run_to_block(3, |_| None);

		// now note that cores 0 and 2 were freed.
		Scheduler::free_cores_and_fill_claimqueue(
			vec![(CoreIndex(0), FreedReason::Concluded), (CoreIndex(2), FreedReason::Concluded)]
				.into_iter()
				.collect::<Vec<_>>(),
			3,
		);

		{
			let claimqueue = Scheduler::claimqueue();
			let claimqueue_0 = claimqueue.get(&CoreIndex(0)).unwrap().clone();
			let claimqueue_2 = claimqueue.get(&CoreIndex(2)).unwrap().clone();
			let entry_ttl = 8;
			assert_eq!(claimqueue_0.len(), 1);
			assert_eq!(claimqueue_2.len(), 1);
			assert_eq!(
				claimqueue_0,
				vec![Some(ParasEntry::new(Assignment::new(chain_a), entry_ttl))],
			);
			assert_eq!(
				claimqueue_2,
				vec![Some(ParasEntry::new(Assignment::new(chain_c), entry_ttl))],
			);

			// The freed cores should be `Free` in `AvailabilityCores`.
			let cores = AvailabilityCores::<Test>::get();
			assert!(cores[0].is_free());
			assert!(cores[2].is_free());
		}
	});
}

#[test]
fn schedule_rotates_groups() {
	let config = {
		let mut config = default_config();

		// make sure on demand requests don't retry-out
		config.on_demand_retries = config.group_rotation_frequency * 3;
		config.on_demand_cores = 2;
		config.scheduling_lookahead = 1;
		config
	};

	let rotation_frequency = config.group_rotation_frequency;
	let on_demand_cores = config.on_demand_cores;

	let genesis_config = genesis_config(&config);

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);

	let assignment_a = Assignment { para_id: thread_a };
	let assignment_b = Assignment { para_id: thread_b };

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().on_demand_cores, 3);

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

		let session_start_block = Scheduler::session_start_block();
		assert_eq!(session_start_block, 1);

		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_a,
			QueuePushDirection::Back
		));
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_b,
			QueuePushDirection::Back
		));

		let mut now = 2;
		run_to_block(now, |_| None);

		let assert_groups_rotated = |rotations: u32, now: &BlockNumberFor<Test>| {
			let scheduled = Scheduler::scheduled_claimqueue();
			assert_eq!(scheduled.len(), 2);
			assert_eq!(
				Scheduler::group_assigned_to_core(scheduled[0].core, *now).unwrap(),
				GroupIndex((0u32 + rotations) % on_demand_cores)
			);
			assert_eq!(
				Scheduler::group_assigned_to_core(scheduled[1].core, *now).unwrap(),
				GroupIndex((1u32 + rotations) % on_demand_cores)
			);
		};

		assert_groups_rotated(0, &now);

		// one block before first rotation.
		now = rotation_frequency;
		run_to_block(rotation_frequency, |_| None);

		assert_groups_rotated(0, &now);

		// first rotation.
		now = now + 1;
		run_to_block(now, |_| None);
		assert_groups_rotated(1, &now);

		// one block before second rotation.
		now = rotation_frequency * 2;
		run_to_block(now, |_| None);
		assert_groups_rotated(1, &now);

		// second rotation.
		now = now + 1;
		run_to_block(now, |_| None);
		assert_groups_rotated(2, &now);
	});
}

#[test]
fn on_demand_claims_are_pruned_after_timing_out() {
	let max_retries = 20;
	let mut config = default_config();
	config.scheduling_lookahead = 1;
	config.on_demand_cores = 2;
	config.on_demand_retries = max_retries;
	let genesis_config = genesis_config(&config);

	let thread_a = ParaId::from(1_u32);

	let assignment_a = Assignment { para_id: thread_a };

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(thread_a, ParaKind::Parathread);

		// #1
		let mut now = 1;
		run_to_block(now, |number| match number {
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

		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_a.clone(),
			QueuePushDirection::Back
		));

		// #2
		now += 1;
		run_to_block(now, |_| None);
		assert_eq!(Scheduler::claimqueue().len(), 1);
		// ParaId a is in the claimqueue.
		assert!(claimqueue_contains_para_ids::<Test>(vec![thread_a]));

		Scheduler::occupied(vec![(CoreIndex(0), thread_a)].into_iter().collect());
		// ParaId a is no longer in the claimqueue.
		assert!(!claimqueue_contains_para_ids::<Test>(vec![thread_a]));
		// It is in availability cores.
		assert!(availability_cores_contains_para_ids::<Test>(vec![thread_a]));

		// #3
		now += 1;
		// Run to block #n over the max_retries value.
		// In this case, both validator groups with time out on availability and
		// the assignment will be dropped.
		for n in now..=(now + max_retries + 1) {
			// #n
			run_to_block(n, |_| None);
			// Time out on core 0.
			let just_updated: BTreeMap<CoreIndex, FreedReason> = vec![
				(CoreIndex(0), FreedReason::TimedOut), // should go back on queue.
			]
			.into_iter()
			.collect();
			let core_assignments = Scheduler::update_claimqueue(just_updated, now);

			// ParaId a exists in the claim queue until max_retries is reached.
			if n < max_retries + now {
				assert!(claimqueue_contains_para_ids::<Test>(vec![thread_a]));
			} else {
				assert!(!claimqueue_contains_para_ids::<Test>(vec![thread_a]));
			}

			// Occupy the cores based on the output of update_claimqueue.
			Scheduler::occupied(
				core_assignments
					.iter()
					.map(|core_assignment| (core_assignment.core, core_assignment.para_id()))
					.collect(),
			);
		}

		// ParaId a does not exist in the claimqueue/availability_cores after
		// threshold has been reached.
		assert!(!claimqueue_contains_para_ids::<Test>(vec![thread_a]));
		assert!(!availability_cores_contains_para_ids::<Test>(vec![thread_a]));

		// #25
		now += max_retries + 2;

		// Add assignment back to the mix.
		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_a.clone(),
			QueuePushDirection::Back
		));
		run_to_block(now, |_| None);

		assert!(claimqueue_contains_para_ids::<Test>(vec![thread_a]));

		// #26
		now += 1;
		// Run to block #n but this time have group 1 conclude the availabilty.
		for n in now..=(now + max_retries + 1) {
			// #n
			run_to_block(n, |_| None);
			// Time out core 0 if group 0 is assigned to it, if group 1 is assigned, conclude.
			let mut just_updated: BTreeMap<CoreIndex, FreedReason> = BTreeMap::new();
			if let Some(group) = Scheduler::group_assigned_to_core(CoreIndex(0), n) {
				match group {
					GroupIndex(0) => {
						just_updated.insert(CoreIndex(0), FreedReason::TimedOut); // should go back on queue.
					},
					GroupIndex(1) => {
						just_updated.insert(CoreIndex(0), FreedReason::Concluded);
					},
					_ => panic!("Should only have 2 groups here"),
				}
			}

			let core_assignments = Scheduler::update_claimqueue(just_updated, now);

			// ParaId a exists in the claim queue until groups are rotated.
			if n < 31 {
				assert!(claimqueue_contains_para_ids::<Test>(vec![thread_a]));
			} else {
				assert!(!claimqueue_contains_para_ids::<Test>(vec![thread_a]));
			}

			// Occupy the cores based on the output of update_claimqueue.
			Scheduler::occupied(
				core_assignments
					.iter()
					.map(|core_assignment| (core_assignment.core, core_assignment.para_id()))
					.collect(),
			);
		}

		// ParaId a does not exist in the claimqueue/availability_cores after
		// being concluded
		assert!(!claimqueue_contains_para_ids::<Test>(vec![thread_a]));
		assert!(!availability_cores_contains_para_ids::<Test>(vec![thread_a]));
	});
}

#[test]
fn availability_predicate_works() {
	let genesis_config = genesis_config(&default_config());

	let HostConfiguration { group_rotation_frequency, paras_availability_period, .. } =
		default_config();

	assert!(paras_availability_period < group_rotation_frequency);

	let chain_a = ParaId::from(1_u32);
	let thread_a = ParaId::from(2_u32);

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(chain_a, ParaKind::Parachain);
		schedule_blank_para(thread_a, ParaKind::Parathread);

		// start a new session with our chain registered.
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
			let entry_ttl = 10_000;
			AvailabilityCores::<Test>::mutate(|cores| {
				cores[0] =
					CoreOccupied::Paras(ParasEntry::new(Assignment::new(chain_a), entry_ttl));
				cores[1] =
					CoreOccupied::Paras(ParasEntry::new(Assignment::new(thread_a), entry_ttl));
			});
		}

		run_to_block(1 + paras_availability_period, |_| None);

		assert!(Scheduler::availability_timeout_predicate().is_none());

		run_to_block(1 + group_rotation_frequency, |_| None);

		{
			let pred = Scheduler::availability_timeout_predicate()
				.expect("predicate exists recently after rotation");

			let now = System::block_number();
			let would_be_timed_out = now - paras_availability_period;
			for i in 0..AvailabilityCores::<Test>::get().len() {
				// returns true for unoccupied cores.
				// And can time out paras at this stage.
				assert!(pred(CoreIndex(i as u32), would_be_timed_out));
			}

			assert!(!pred(CoreIndex(0), now));
			assert!(!pred(CoreIndex(1), now));
			assert!(pred(CoreIndex(2), now));

			// check the tight bound.
			assert!(pred(CoreIndex(0), now - paras_availability_period));
			assert!(pred(CoreIndex(1), now - paras_availability_period));

			// check the threshold is exact.
			assert!(!pred(CoreIndex(0), now - paras_availability_period + 1));
			assert!(!pred(CoreIndex(1), now - paras_availability_period + 1));
		}

		run_to_block(1 + group_rotation_frequency + paras_availability_period, |_| None);
	});
}

#[test]
fn next_up_on_available_uses_next_scheduled_or_none_for_thread() {
	let mut config = default_config();
	config.on_demand_cores = 1;

	let genesis_config = genesis_config(&config);

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);

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

		let thread_entry_a = ParasEntry {
			assignment: Assignment { para_id: thread_a },
			availability_timeouts: 0,
			ttl: 5,
		};
		let thread_entry_b = ParasEntry {
			assignment: Assignment { para_id: thread_b },
			availability_timeouts: 0,
			ttl: 5,
		};

		Scheduler::add_to_claimqueue(CoreIndex(0), thread_entry_a.clone());

		run_to_block(2, |_| None);

		{
			assert_eq!(Scheduler::claimqueue_len(), 1);
			assert_eq!(Scheduler::availability_cores().len(), 1);

			let mut map = BTreeMap::new();
			map.insert(CoreIndex(0), thread_a);
			Scheduler::occupied(map);

			let cores = Scheduler::availability_cores();
			match &cores[0] {
				CoreOccupied::Paras(entry) => assert_eq!(entry, &thread_entry_a),
				_ => panic!("with no chains, only core should be a thread core"),
			}

			assert!(Scheduler::next_up_on_available(CoreIndex(0)).is_none());

			Scheduler::add_to_claimqueue(CoreIndex(0), thread_entry_b);

			assert_eq!(
				Scheduler::next_up_on_available(CoreIndex(0)).unwrap(),
				ScheduledCore { para_id: thread_b, collator: None }
			);
		}
	});
}

#[test]
fn next_up_on_time_out_reuses_claim_if_nothing_queued() {
	let mut config = default_config();
	config.on_demand_cores = 1;

	let genesis_config = genesis_config(&config);

	let thread_a = ParaId::from(1_u32);
	let thread_b = ParaId::from(2_u32);

	let assignment_a = Assignment { para_id: thread_a };
	let assignment_b = Assignment { para_id: thread_b };

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

		assert_ok!(OnDemandAssigner::add_on_demand_assignment(
			assignment_a.clone(),
			QueuePushDirection::Back
		));

		run_to_block(2, |_| None);

		{
			assert_eq!(Scheduler::claimqueue().len(), 1);
			assert_eq!(Scheduler::availability_cores().len(), 1);

			let mut map = BTreeMap::new();
			map.insert(CoreIndex(0), thread_a);
			Scheduler::occupied(map);

			let cores = Scheduler::availability_cores();
			match cores.get(0).unwrap() {
				CoreOccupied::Paras(entry) => assert_eq!(entry.assignment, assignment_a.clone()),
				_ => panic!("with no chains, only core should be a thread core"),
			}

			// There's nothing more to pop for core 0 from the assignment provider.
			assert!(
				OnDemandAssigner::pop_assignment_for_core(CoreIndex(0), Some(thread_a)).is_none()
			);

			assert_eq!(
				Scheduler::next_up_on_time_out(CoreIndex(0)).unwrap(),
				ScheduledCore { para_id: thread_a, collator: None }
			);

			assert_ok!(OnDemandAssigner::add_on_demand_assignment(
				assignment_b.clone(),
				QueuePushDirection::Back
			));

			// Pop assignment_b into the claimqueue
			Scheduler::update_claimqueue(BTreeMap::new(), 2);

			//// Now that there is an earlier next-up, we use that.
			assert_eq!(
				Scheduler::next_up_on_available(CoreIndex(0)).unwrap(),
				ScheduledCore { para_id: thread_b, collator: None }
			);
		}
	});
}

#[test]
fn next_up_on_available_is_parachain_always() {
	let mut config = default_config();
	config.on_demand_cores = 0;
	let genesis_config = genesis_config(&config);
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
			assert_eq!(Scheduler::claimqueue().len(), 1);
			assert_eq!(Scheduler::availability_cores().len(), 1);

			Scheduler::occupied(vec![(CoreIndex(0), chain_a)].into_iter().collect());

			let cores = Scheduler::availability_cores();
			match &cores[0] {
				CoreOccupied::Paras(pe) if pe.para_id() == chain_a => {},
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
	config.on_demand_cores = 0;

	let genesis_config = genesis_config(&config);

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
			assert_eq!(Scheduler::claimqueue().len(), 1);
			assert_eq!(Scheduler::availability_cores().len(), 1);

			Scheduler::occupied(vec![(CoreIndex(0), chain_a)].into_iter().collect());

			let cores = Scheduler::availability_cores();
			match &cores[0] {
				CoreOccupied::Paras(pe) if pe.para_id() == chain_a => {},
				_ => panic!("Core should be occupied by chain_a ParaId"),
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
	let mut config = default_config();
	config.scheduling_lookahead = 1;
	let genesis_config = genesis_config(&config);

	assert_eq!(default_config().on_demand_cores, 3);
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

		assert_eq!(Scheduler::claimqueue().len(), 2);

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

		Scheduler::update_claimqueue(BTreeMap::new(), 3);

		assert_eq!(
			Scheduler::claimqueue(),
			vec![(
				CoreIndex(0),
				vec![Some(ParasEntry::new(
					Assignment::new(chain_a),
					// At end of block 2
					config.on_demand_ttl + 2
				))]
				.into_iter()
				.collect()
			)]
			.into_iter()
			.collect()
		);

		// Add parachain back
		schedule_blank_para(chain_b, ParaKind::Parachain);

		run_to_block(3, |number| match number {
			3 => Some(SessionChangeNotification {
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

		assert_eq!(Scheduler::claimqueue().len(), 2);

		let groups = ValidatorGroups::<Test>::get();
		assert_eq!(groups.len(), 5);

		Scheduler::update_claimqueue(BTreeMap::new(), 4);

		assert_eq!(
			Scheduler::claimqueue(),
			vec![
				(
					CoreIndex(0),
					vec![Some(ParasEntry::new(
						Assignment::new(chain_a),
						// At block 3
						config.on_demand_ttl + 3
					))]
					.into_iter()
					.collect()
				),
				(
					CoreIndex(1),
					vec![Some(ParasEntry::new(
						Assignment::new(chain_b),
						// At block 3
						config.on_demand_ttl + 3
					))]
					.into_iter()
					.collect()
				),
			]
			.into_iter()
			.collect()
		);
	});
}
