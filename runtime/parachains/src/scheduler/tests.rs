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
			// We will make every session change trigger an action queue. Normally this may require 2 or more session changes.
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
		// is still a subject to consistency test. It requires that `minimum_validation_upgrade_delay`
		// is greater than `chain_availability_period` and `thread_availability_period`.
		minimum_validation_upgrade_delay: 6,
		..Default::default()
	}
}

#[cfg(test)]
pub(crate) fn claimqueue_contains_only_none() -> bool {
	let mut cq = Scheduler::claimqueue();
	//let mut cq = ClaimQueue::<T>::get();
	for (_, v) in cq.iter_mut() {
		v.retain(|e| e.is_some());
	}

	cq.iter().map(|(_, v)| v.len()).sum::<usize>() == 0
}

#[cfg(test)]
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

#[test]
fn claimqueue_ttl_drop_fn_works() {
	let mut config = default_config();
	config.scheduling_lookahead = 3;
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig { config, ..Default::default() },
		..Default::default()
	};

	let para_id = ParaId::from(100);
	let core_idx = CoreIndex::from(0);
	let mut now = 10;

	new_test_ext(genesis_config).execute_with(|| {
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
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

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

//#[test]
//fn session_change_shuffles_validators() {
//	let genesis_config = MockGenesisConfig {
//		configuration: crate::configuration::GenesisConfig {
//			config: default_config(),
//			..Default::default()
//		},
//		..Default::default()
//	};
//
//	assert_eq!(default_config().parathread_cores, 3);
//	new_test_ext(genesis_config).execute_with(|| {
//		let chain_a = ParaId::from(1_u32);
//		let chain_b = ParaId::from(2_u32);
//
//		// ensure that we have 5 groups by registering 2 parachains.
//		schedule_blank_para(chain_a, ParaKind::Parachain);
//		schedule_blank_para(chain_b, ParaKind::Parachain);
//
//		run_to_block(1, |number| match number {
//			1 => Some(SessionChangeNotification {
//				new_config: default_config(),
//				validators: vec![
//					ValidatorId::from(Sr25519Keyring::Alice.public()),
//					ValidatorId::from(Sr25519Keyring::Bob.public()),
//					ValidatorId::from(Sr25519Keyring::Charlie.public()),
//					ValidatorId::from(Sr25519Keyring::Dave.public()),
//					ValidatorId::from(Sr25519Keyring::Eve.public()),
//					ValidatorId::from(Sr25519Keyring::Ferdie.public()),
//					ValidatorId::from(Sr25519Keyring::One.public()),
//				],
//				random_seed: [99; 32],
//				..Default::default()
//			}),
//			_ => None,
//		});
//
//		let groups = ValidatorGroups::<Test>::get();
//		assert_eq!(groups.len(), 5);
//
//		// first two groups have the overflow.
//		for i in 0..2 {
//			assert_eq!(groups[i].len(), 2);
//		}
//
//		for i in 2..5 {
//			assert_eq!(groups[i].len(), 1);
//		}
//	});
//}

#[test]
fn session_change_takes_only_max_per_core() {
	let config = {
		let mut config = default_config();
		config.on_demand_cores = 0;
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
fn fill_claimqueue_fills() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let lookahead = genesis_config.configuration.config.scheduling_lookahead as usize;
	let chain_a = ParaId::from(1_u32);
	let chain_b = ParaId::from(2_u32);

	let thread_a = ParaId::from(3_u32);
	let thread_b = ParaId::from(4_u32);
	let thread_c = ParaId::from(5_u32);

	//let collator = CollatorId::from(Sr25519Keyring::Alice.public());

	new_test_ext(genesis_config).execute_with(|| {
		assert_eq!(default_config().on_demand_cores, 3);

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
			assert_eq!(Scheduler::claimqueue_len(), 2 * lookahead);
			//let cq = Scheduler::claimqueue();

			// Cannot assert on indices anymore as they depend on the assignment providers
			assert!(claimqueue_contains_para_ids::<Test>(vec![chain_a, chain_b]));
			// TODO: checks for group indices?
			//assert_eq!(
			//	scheduled[0],
			//	CoreAssignment {
			//		core: CoreIndex(0),
			//		kind: Assignment::Parachain(chain_a),
			//		group_idx: GroupIndex(0),
			//	}
			//);

			//assert_eq!(
			//	scheduled[1],
			//	CoreAssignment {
			//		core: CoreIndex(1),
			//		kind: Assignment::Parachain(chain_b),
			//		group_idx: GroupIndex(1),
			//	}
			//);
		}

		// add a couple of parathread claims.
		//SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
		//SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_c, collator.clone()));

		//run_to_block(2, |_| None);

		//{

		//	assert_eq!(Scheduler::claimqueue_len(), 4);
		//	let cq = Scheduler::claimqueue();

		//	assert_eq!(
		//		scheduled[0],
		//		CoreAssignment {
		//			core: CoreIndex(0),
		//			kind: Assignment::Parachain(chain_a),
		//			group_idx: GroupIndex(0),
		//		}
		//	);

		//	assert_eq!(
		//		scheduled[1],
		//		CoreAssignment {
		//			core: CoreIndex(1),
		//			kind: Assignment::Parachain(chain_b),
		//			group_idx: GroupIndex(1),
		//		}
		//	);

		//	assert_eq!(
		//		scheduled[2],
		//		CoreAssignment {
		//			core: CoreIndex(2),
		//			kind: Assignment::Parathread(ParathreadEntry {claim: ParathreadClaim(thread_a, Some(collator.clone())), retries: 0}),
		//			group_idx: GroupIndex(2),
		//		}
		//	);

		//	assert_eq!(
		//		scheduled[3],
		//		CoreAssignment {
		//			core: CoreIndex(3),
		//			kind: Assignment::Parathread(ParathreadEntry {claim: ParathreadClaim(thread_c, Some(collator.clone())), retries: 0}),
		//			group_idx: GroupIndex(3),
		//		}
		//	);
		//}
	});
}

//#[test]
//fn schedule_schedules_including_just_freed() {
//	let genesis_config = MockGenesisConfig {
//		configuration: crate::configuration::GenesisConfig {
//			config: default_config(),
//			..Default::default()
//		},
//		..Default::default()
//	};
//
//	let chain_a = ParaId::from(1_u32);
//	let chain_b = ParaId::from(2_u32);
//
//	let thread_a = ParaId::from(3_u32);
//	let thread_b = ParaId::from(4_u32);
//	let thread_c = ParaId::from(5_u32);
//	let thread_d = ParaId::from(6_u32);
//	let thread_e = ParaId::from(7_u32);
//
//	let collator = CollatorId::from(Sr25519Keyring::Alice.public());
//
//	new_test_ext(genesis_config).execute_with(|| {
//		assert_eq!(default_config().parathread_cores, 3);
//
//		// register 2 parachains
//		schedule_blank_para(chain_a, ParaKind::Parachain);
//		schedule_blank_para(chain_b, ParaKind::Parachain);
//
//		// and 5 parathreads
//		schedule_blank_para(thread_a, ParaKind::Parathread);
//		schedule_blank_para(thread_b, ParaKind::Parathread);
//		schedule_blank_para(thread_c, ParaKind::Parathread);
//		schedule_blank_para(thread_d, ParaKind::Parathread);
//		schedule_blank_para(thread_e, ParaKind::Parathread);
//
//		// start a new session to activate, 5 validators for 5 cores.
//		run_to_block(1, |number| match number {
//			1 => Some(SessionChangeNotification {
//				new_config: default_config(),
//				validators: vec![
//					ValidatorId::from(Sr25519Keyring::Alice.public()),
//					ValidatorId::from(Sr25519Keyring::Bob.public()),
//					ValidatorId::from(Sr25519Keyring::Charlie.public()),
//					ValidatorId::from(Sr25519Keyring::Dave.public()),
//					ValidatorId::from(Sr25519Keyring::Eve.public()),
//				],
//				..Default::default()
//			}),
//			_ => None,
//		});
//
//		// add a couple of parathread claims now that the parathreads are live.
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_c, collator.clone()));
//
//		run_to_block(2, |_| None);
//
//		assert_eq!(Scheduler::scheduled().len(), 4);
//
//		// cores 0, 1, 2, and 3 should be occupied. mark them as such.
//		Scheduler::occupied(&[CoreIndex(0), CoreIndex(1), CoreIndex(2), CoreIndex(3)]);
//
//		{
//			let cores = AvailabilityCores::<Test>::get();
//
//			assert!(cores[0].is_some());
//			assert!(cores[1].is_some());
//			assert!(cores[2].is_some());
//			assert!(cores[3].is_some());
//			assert!(cores[4].is_none());
//
//			assert!(Scheduler::scheduled().is_empty());
//		}
//
//		// add a couple more parathread claims - the claim on `b` will go to the 3rd parathread core (4)
//		// and the claim on `d` will go back to the 1st parathread core (2). The claim on `e` then
//		// will go for core `3`.
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_d, collator.clone()));
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_e, collator.clone()));
//
//		run_to_block(3, |_| None);
//
//		{
//			let scheduled = Scheduler::scheduled();
//
//			// cores 0 and 1 are occupied by parachains. cores 2 and 3 are occupied by parathread
//			// claims. core 4 was free.
//			assert_eq!(scheduled.len(), 1);
//			assert_eq!(
//				scheduled[0],
//				CoreAssignment {
//					core: CoreIndex(4),
//					para_id: thread_b,
//					kind: Assignment::Parathread(collator.clone(), 0),
//					group_idx: GroupIndex(4),
//				}
//			);
//		}
//
//		// now note that cores 0, 2, and 3 were freed.
//		Scheduler::schedule(
//			vec![
//				(CoreIndex(0), FreedReason::Concluded),
//				(CoreIndex(2), FreedReason::Concluded),
//				(CoreIndex(3), FreedReason::TimedOut), // should go back on queue.
//			]
//			.into_iter()
//			.collect(),
//			3,
//		);
//
//		{
//			let scheduled = Scheduler::scheduled();
//
//			// 1 thing scheduled before, + 3 cores freed.
//			assert_eq!(scheduled.len(), 4);
//			assert_eq!(
//				scheduled[0],
//				CoreAssignment {
//					core: CoreIndex(0),
//					para_id: chain_a,
//					kind: Assignment::Parachain,
//					group_idx: GroupIndex(0),
//				}
//			);
//			assert_eq!(
//				scheduled[1],
//				CoreAssignment {
//					core: CoreIndex(2),
//					para_id: thread_d,
//					kind: Assignment::Parathread(collator.clone(), 0),
//					group_idx: GroupIndex(2),
//				}
//			);
//			assert_eq!(
//				scheduled[2],
//				CoreAssignment {
//					core: CoreIndex(3),
//					para_id: thread_e,
//					kind: Assignment::Parathread(collator.clone(), 0),
//					group_idx: GroupIndex(3),
//				}
//			);
//			assert_eq!(
//				scheduled[3],
//				CoreAssignment {
//					core: CoreIndex(4),
//					para_id: thread_b,
//					kind: Assignment::Parathread(collator.clone(), 0),
//					group_idx: GroupIndex(4),
//				}
//			);
//
//			// the prior claim on thread A concluded, but the claim on thread C was marked as
//			// timed out.
//			let index = ParathreadClaimIndex::<Test>::get();
//			let parathread_queue = ParathreadQueue::<Test>::get();
//
//			// thread A claim should have been wiped, but thread C claim should remain.
//			assert_eq!(index, vec![thread_b, thread_c, thread_d, thread_e]);
//
//			// Although C was descheduled, the core `4`  was occupied so C goes back on the queue.
//			assert_eq!(parathread_queue.queue.len(), 1);
//			assert_eq!(
//				parathread_queue.queue[0],
//				QueuedParathread {
//					claim: ParathreadEntry {
//						claim: ParathreadClaim(thread_c, collator.clone()),
//						retries: 0, // retries not incremented by timeout - validators' fault.
//					},
//					core_offset: 2, // reassigned to next core. thread_e claim was on offset 1.
//				}
//			);
//		}
//	});
//}

#[test]
fn schedule_clears_availability_cores() {
	let mut config = default_config();
	config.scheduling_lookahead = 1;
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig { config, ..Default::default() },
		..Default::default()
	};

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

//#[test]
//fn schedule_rotates_groups() {
//	let config = {
//		let mut config = default_config();
//
//		// make sure parathread requests don't retry-out
//		config.parathread_retries = config.group_rotation_frequency * 3;
//		config.parathread_cores = 2;
//		config
//	};
//
//	let rotation_frequency = config.group_rotation_frequency;
//	let parathread_cores = config.parathread_cores;
//
//	let genesis_config = MockGenesisConfig {
//		configuration: crate::configuration::GenesisConfig {
//			config: config.clone(),
//			..Default::default()
//		},
//		..Default::default()
//	};
//
//	let thread_a = ParaId::from(1_u32);
//	let thread_b = ParaId::from(2_u32);
//
//	let collator = CollatorId::from(Sr25519Keyring::Alice.public());
//
//	new_test_ext(genesis_config).execute_with(|| {
//		assert_eq!(default_config().parathread_cores, 3);
//
//		schedule_blank_para(thread_a, ParaKind::Parathread);
//		schedule_blank_para(thread_b, ParaKind::Parathread);
//
//		// start a new session to activate, 5 validators for 5 cores.
//		run_to_block(1, |number| match number {
//			1 => Some(SessionChangeNotification {
//				new_config: config.clone(),
//				validators: vec![
//					ValidatorId::from(Sr25519Keyring::Alice.public()),
//					ValidatorId::from(Sr25519Keyring::Eve.public()),
//				],
//				..Default::default()
//			}),
//			_ => None,
//		});
//
//		let session_start_block = <Scheduler as Store>::SessionStartBlock::get();
//		assert_eq!(session_start_block, 1);
//
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));
//
//		run_to_block(2, |_| None);
//
//		let assert_groups_rotated = |rotations: u32| {
//			let scheduled = Scheduler::scheduled();
//			assert_eq!(scheduled.len(), 2);
//			assert_eq!(scheduled[0].group_idx, GroupIndex((0u32 + rotations) % parathread_cores));
//			assert_eq!(scheduled[1].group_idx, GroupIndex((1u32 + rotations) % parathread_cores));
//		};
//
//		assert_groups_rotated(0);
//
//		// one block before first rotation.
//		run_to_block(rotation_frequency, |_| None);
//
//		assert_groups_rotated(0);
//
//		// first rotation.
//		run_to_block(rotation_frequency + 1, |_| None);
//		assert_groups_rotated(1);
//
//		// one block before second rotation.
//		run_to_block(rotation_frequency * 2, |_| None);
//		assert_groups_rotated(1);
//
//		// second rotation.
//		run_to_block(rotation_frequency * 2 + 1, |_| None);
//		assert_groups_rotated(2);
//	});
//}

//#[test]
//fn parathread_claims_are_pruned_after_retries() {
//	let max_retries = default_config().parathread_retries;
//
//	let genesis_config = MockGenesisConfig {
//		configuration: crate::configuration::GenesisConfig {
//			config: default_config(),
//			..Default::default()
//		},
//		..Default::default()
//	};
//
//	let thread_a = ParaId::from(1_u32);
//	let thread_b = ParaId::from(2_u32);
//
//	let collator = CollatorId::from(Sr25519Keyring::Alice.public());
//
//	new_test_ext(genesis_config).execute_with(|| {
//		assert_eq!(default_config().parathread_cores, 3);
//
//		schedule_blank_para(thread_a, ParaKind::Parathread);
//		schedule_blank_para(thread_b, ParaKind::Parathread);
//
//		// start a new session to activate, 5 validators for 5 cores.
//		run_to_block(1, |number| match number {
//			1 => Some(SessionChangeNotification {
//				new_config: default_config(),
//				validators: vec![
//					ValidatorId::from(Sr25519Keyring::Alice.public()),
//					ValidatorId::from(Sr25519Keyring::Eve.public()),
//				],
//				..Default::default()
//			}),
//			_ => None,
//		});
//
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_a, collator.clone()));
//		SchedulerParathreads::add_parathread_claim(ParathreadClaim(thread_b, collator.clone()));
//
//		run_to_block(2, |_| None);
//		assert_eq!(Scheduler::scheduled().len(), 2);
//
//		run_to_block(2 + max_retries, |_| None);
//		assert_eq!(Scheduler::scheduled().len(), 2);
//
//		run_to_block(2 + max_retries + 1, |_| None);
//		assert_eq!(Scheduler::scheduled().len(), 0);
//	});
//}

#[test]
fn availability_predicate_works() {
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: default_config(),
			..Default::default()
		},
		..Default::default()
	};

	let HostConfiguration { group_rotation_frequency, paras_availability_period, .. } =
		default_config();

	assert!(paras_availability_period < group_rotation_frequency);

	let chain_a = ParaId::from(1_u32);

	new_test_ext(genesis_config).execute_with(|| {
		schedule_blank_para(chain_a, ParaKind::Parachain);

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

		// assign an availability core.
		{
			let entry_ttl = 10_000;
			AvailabilityCores::<Test>::mutate(|cores| {
				cores[0] =
					CoreOccupied::Paras(ParasEntry::new(Assignment::new(chain_a), entry_ttl));
			});
		}

		run_to_block(1 + paras_availability_period, |_| None);

		run_to_block(1 + group_rotation_frequency, |_| None);

		{
			let pred = Scheduler::availability_timeout_predicate();
			let now = System::block_number();
			let would_be_timed_out = now - paras_availability_period;
			for i in 0..AvailabilityCores::<Test>::get().len() {
				// returns true for unoccupied cores.
				// And can time out paras at this stage.
				assert!(pred(CoreIndex(i as u32), would_be_timed_out));
			}

			assert!(!pred(CoreIndex(0), now));
			assert!(pred(CoreIndex(1), now));

			// check the tight bound.
			assert!(pred(CoreIndex(0), now - paras_availability_period));

			// check the threshold is exact.
			assert!(!pred(CoreIndex(0), now - paras_availability_period + 1));
		}

		run_to_block(1 + group_rotation_frequency + paras_availability_period, |_| None);
	});
}

#[test]
fn next_up_on_available_uses_next_scheduled_or_none_for_thread() {
	let mut config = default_config();
	config.on_demand_cores = 1;

	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	};

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

		let thread_entry_a =
			ParasEntry { assignment: Assignment { para_id: thread_a }, retries: 0, ttl: 5 };
		let thread_entry_b =
			ParasEntry { assignment: Assignment { para_id: thread_b }, retries: 0, ttl: 5 };

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
				ScheduledCore { para_id: thread_b }
			);
		}
	});
}

//#[test]
//fn next_up_on_time_out_reuses_claim_if_nothing_queued() {
//	let mut config = default_config();
//	config.parathread_cores = 1;
//
//	let genesis_config = MockGenesisConfig {
//		configuration: crate::configuration::GenesisConfig {
//			config: config.clone(),
//			..Default::default()
//		},
//		..Default::default()
//	};
//
//	let thread_a = ParaId::from(1_u32);
//	let thread_b = ParaId::from(2_u32);
//
//	let collator = CollatorId::from(Sr25519Keyring::Alice.public());
//
//	new_test_ext(genesis_config).execute_with(|| {
//		schedule_blank_para(thread_a, ParaKind::Parathread);
//		schedule_blank_para(thread_b, ParaKind::Parathread);
//
//		// start a new session to activate, 5 validators for 5 cores.
//		run_to_block(1, |number| match number {
//			1 => Some(SessionChangeNotification {
//				new_config: config.clone(),
//				validators: vec![
//					ValidatorId::from(Sr25519Keyring::Alice.public()),
//					ValidatorId::from(Sr25519Keyring::Eve.public()),
//				],
//				..Default::default()
//			}),
//			_ => None,
//		});
//
//		let thread_claim_a = ParathreadClaim(thread_a, collator.clone());
//		let thread_claim_b = ParathreadClaim(thread_b, collator.clone());
//
//		SchedulerParathreads::add_parathread_claim(thread_claim_a.clone());
//
//		run_to_block(2, |_| None);
//
//		{
//			assert_eq!(Scheduler::scheduled().len(), 1);
//			assert_eq!(Scheduler::availability_cores().len(), 1);
//
//			Scheduler::occupied(&[CoreIndex(0)]);
//
//			let cores = Scheduler::availability_cores();
//			match cores[0].as_ref().unwrap() {
//				CoreOccupied::Parathread(entry) => assert_eq!(entry.claim, thread_claim_a),
//				_ => panic!("with no chains, only core should be a thread core"),
//			}
//
//			let queue = ParathreadQueue::<Test>::get();
//			assert!(queue.get_next_on_core(0).is_none());
//			assert_eq!(
//				Scheduler::next_up_on_time_out(CoreIndex(0)).unwrap(),
//				ScheduledCore { para_id: thread_a, collator: Some(collator.clone()) }
//			);
//
//			SchedulerParathreads::add_parathread_claim(thread_claim_b);
//
//			let queue = ParathreadQueue::<Test>::get();
//			assert_eq!(
//				queue.get_next_on_core(0).unwrap().claim,
//				ParathreadClaim(thread_b, collator.clone()),
//			);
//
//			// Now that there is an earlier next-up, we use that.
//			assert_eq!(
//				Scheduler::next_up_on_available(CoreIndex(0)).unwrap(),
//				ScheduledCore { para_id: thread_b, collator: Some(collator.clone()) }
//			);
//		}
//	});
//}

fn genesis_config(config: &HostConfiguration<BlockNumber>) -> MockGenesisConfig {
	MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	}
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
				ScheduledCore { para_id: chain_a }
			);
		}
	});
}

#[test]
fn next_up_on_time_out_is_parachain_always() {
	let mut config = default_config();
	config.on_demand_cores = 0;

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
				ScheduledCore { para_id: chain_a }
			);
		}
	});
}

#[test]
fn session_change_requires_reschedule_dropping_removed_paras() {
	let mut config = default_config();
	config.scheduling_lookahead = 1;
	let genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: config.clone(),
			..Default::default()
		},
		..Default::default()
	};

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
