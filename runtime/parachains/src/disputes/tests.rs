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
use crate::{
	configuration::HostConfiguration,
	disputes::DisputesHandler,
	mock::{
		new_test_ext, AccountId, AllPalletsWithSystem, Initializer, MockGenesisConfig, System,
		Test, PUNISH_VALIDATORS_AGAINST, PUNISH_VALIDATORS_FOR, PUNISH_VALIDATORS_INCONCLUSIVE,
		REWARD_VALIDATORS,
	},
};
use frame_support::{
	assert_err, assert_noop, assert_ok,
	traits::{OnFinalize, OnInitialize},
};
use primitives::v2::BlockNumber;
use sp_core::{crypto::CryptoType, Pair};

/// Filtering updates the spam slots, as such update them.
fn update_spam_slots(stmts: MultiDisputeStatementSet) -> CheckedMultiDisputeStatementSet {
	let config = <configuration::Pallet<Test>>::config();
	let max_spam_slots = config.dispute_max_spam_slots;
	let post_conclusion_acceptance_period = config.dispute_post_conclusion_acceptance_period;

	stmts
		.into_iter()
		.filter_map(|set| {
			// updates spam slots implicitly
			let filter = Pallet::<Test>::filter_dispute_data(
				&set,
				post_conclusion_acceptance_period,
				max_spam_slots,
				VerifyDisputeSignatures::Skip,
			);
			filter.filter_statement_set(set)
		})
		.collect::<Vec<_>>()
}

// All arguments for `initializer::on_new_session`
type NewSession<'a> = (
	bool,
	SessionIndex,
	Vec<(&'a AccountId, ValidatorId)>,
	Option<Vec<(&'a AccountId, ValidatorId)>>,
);

// Run to specific block, while calling disputes pallet hooks manually, because disputes is not
// integrated in initializer yet.
pub(crate) fn run_to_block<'a>(
	to: BlockNumber,
	new_session: impl Fn(BlockNumber) -> Option<NewSession<'a>>,
) {
	while System::block_number() < to {
		let b = System::block_number();
		if b != 0 {
			// circumvent requirement to have bitfields and headers in block for testing purposes
			crate::paras_inherent::Included::<Test>::set(Some(()));

			AllPalletsWithSystem::on_finalize(b);
			System::finalize();
		}

		System::reset_events();
		System::initialize(&(b + 1), &Default::default(), &Default::default());
		AllPalletsWithSystem::on_initialize(b + 1);

		if let Some(new_session) = new_session(b + 1) {
			Initializer::test_trigger_on_new_session(
				new_session.0,
				new_session.1,
				new_session.2.into_iter(),
				new_session.3.map(|q| q.into_iter()),
			);
		}
	}
}

#[test]
fn test_contains_duplicates_in_sorted_iter() {
	// We here use the implicit ascending sorting and builtin equality of integers
	let v = vec![1, 2, 3, 5, 5, 8];
	assert_eq!(true, contains_duplicates_in_sorted_iter(&v, |a, b| a == b));

	let v = vec![1, 2, 3, 4];
	assert_eq!(false, contains_duplicates_in_sorted_iter(&v, |a, b| a == b));
}

#[test]
fn test_dispute_state_flag_from_state() {
	assert_eq!(
		DisputeStateFlags::from_state(&DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		}),
		DisputeStateFlags::default(),
	);

	assert_eq!(
		DisputeStateFlags::from_state(&DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 1, 1, 1, 1, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		}),
		DisputeStateFlags::FOR_SUPERMAJORITY | DisputeStateFlags::CONFIRMED,
	);

	assert_eq!(
		DisputeStateFlags::from_state(&DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 1, 1, 1, 1, 1, 0, 0],
			start: 0,
			concluded_at: None,
		}),
		DisputeStateFlags::AGAINST_SUPERMAJORITY | DisputeStateFlags::CONFIRMED,
	);
}

#[test]
fn test_import_new_participant_spam_inc() {
	let mut importer = DisputeStateImporter::new(
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
		0,
	);

	assert_err!(
		importer.import(ValidatorIndex(9), true),
		VoteImportError::ValidatorIndexOutOfBounds,
	);

	assert_err!(importer.import(ValidatorIndex(0), true), VoteImportError::DuplicateStatement);
	assert_ok!(importer.import(ValidatorIndex(0), false));

	assert_ok!(importer.import(ValidatorIndex(2), true));
	assert_err!(importer.import(ValidatorIndex(2), true), VoteImportError::DuplicateStatement);

	assert_ok!(importer.import(ValidatorIndex(2), false));
	assert_err!(importer.import(ValidatorIndex(2), false), VoteImportError::DuplicateStatement);

	let summary = importer.finish();
	assert_eq!(summary.new_flags, DisputeStateFlags::default());
	assert_eq!(
		summary.state,
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
	);
	assert_eq!(summary.spam_slot_changes, vec![(ValidatorIndex(2), SpamSlotChange::Inc)]);
	assert!(summary.slash_for.is_empty());
	assert!(summary.slash_against.is_empty());
	assert_eq!(summary.new_participants, bitvec![u8, BitOrderLsb0; 0, 0, 1, 0, 0, 0, 0, 0]);
}

#[test]
fn test_import_prev_participant_spam_dec_confirmed() {
	let mut importer = DisputeStateImporter::new(
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
		0,
	);

	assert_ok!(importer.import(ValidatorIndex(2), true));

	let summary = importer.finish();
	assert_eq!(
		summary.state,
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
	);
	assert_eq!(
		summary.spam_slot_changes,
		vec![(ValidatorIndex(0), SpamSlotChange::Dec), (ValidatorIndex(1), SpamSlotChange::Dec),],
	);
	assert!(summary.slash_for.is_empty());
	assert!(summary.slash_against.is_empty());
	assert_eq!(summary.new_participants, bitvec![u8, BitOrderLsb0; 0, 0, 1, 0, 0, 0, 0, 0]);
	assert_eq!(summary.new_flags, DisputeStateFlags::CONFIRMED);
}

#[test]
fn test_import_prev_participant_spam_dec_confirmed_slash_for() {
	let mut importer = DisputeStateImporter::new(
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
		0,
	);

	assert_ok!(importer.import(ValidatorIndex(2), true));
	assert_ok!(importer.import(ValidatorIndex(2), false));
	assert_ok!(importer.import(ValidatorIndex(3), false));
	assert_ok!(importer.import(ValidatorIndex(4), false));
	assert_ok!(importer.import(ValidatorIndex(5), false));
	assert_ok!(importer.import(ValidatorIndex(6), false));

	let summary = importer.finish();
	assert_eq!(
		summary.state,
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 1, 1, 1, 1, 1, 0],
			start: 0,
			concluded_at: Some(0),
		},
	);
	assert_eq!(
		summary.spam_slot_changes,
		vec![(ValidatorIndex(0), SpamSlotChange::Dec), (ValidatorIndex(1), SpamSlotChange::Dec),],
	);
	assert_eq!(summary.slash_for, vec![ValidatorIndex(0), ValidatorIndex(2)]);
	assert!(summary.slash_against.is_empty());
	assert_eq!(summary.new_participants, bitvec![u8, BitOrderLsb0; 0, 0, 1, 1, 1, 1, 1, 0]);
	assert_eq!(
		summary.new_flags,
		DisputeStateFlags::CONFIRMED | DisputeStateFlags::AGAINST_SUPERMAJORITY,
	);
}

#[test]
fn test_import_slash_against() {
	let mut importer = DisputeStateImporter::new(
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
		0,
	);

	assert_ok!(importer.import(ValidatorIndex(3), true));
	assert_ok!(importer.import(ValidatorIndex(4), true));
	assert_ok!(importer.import(ValidatorIndex(5), false));
	assert_ok!(importer.import(ValidatorIndex(6), true));
	assert_ok!(importer.import(ValidatorIndex(7), true));

	let summary = importer.finish();
	assert_eq!(
		summary.state,
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 1, 1, 1, 0, 1, 1],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 0, 0, 1, 0, 0],
			start: 0,
			concluded_at: Some(0),
		},
	);
	assert!(summary.spam_slot_changes.is_empty());
	assert!(summary.slash_for.is_empty());
	assert_eq!(summary.slash_against, vec![ValidatorIndex(1), ValidatorIndex(5)]);
	assert_eq!(summary.new_participants, bitvec![u8, BitOrderLsb0; 0, 0, 0, 1, 1, 1, 1, 1]);
	assert_eq!(summary.new_flags, DisputeStateFlags::FOR_SUPERMAJORITY);
}

// Test that punish_inconclusive is correctly called.
#[test]
fn test_initializer_initialize() {
	let dispute_conclusion_by_time_out_period = 3;
	let start = 10;

	let mock_genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				dispute_conclusion_by_time_out_period,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(mock_genesis_config).execute_with(|| {
		// We need 6 validators for the byzantine threshold to be 2
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v6 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(start, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![
					(&0, v0.public()),
					(&1, v1.public()),
					(&2, v2.public()),
					(&3, v3.public()),
					(&4, v4.public()),
					(&5, v5.public()),
					(&6, v6.public()),
				],
				Some(vec![
					(&0, v0.public()),
					(&1, v1.public()),
					(&2, v2.public()),
					(&3, v3.public()),
					(&4, v4.public()),
					(&5, v5.public()),
					(&6, v6.public()),
				]),
			))
		});

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

		// v0 votes for 3, v6 against.
		let stmts = vec![DisputeStatementSet {
			candidate_hash: candidate_hash.clone(),
			session: start - 1,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement {
							valid: true,
							candidate_hash: candidate_hash.clone(),
							session: start - 1,
						}
						.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(6),
					v2.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: start - 1,
						}
						.signing_payload(),
					),
				),
			],
		}];

		let stmts = update_spam_slots(stmts);
		assert_eq!(SpamSlots::<Test>::get(start - 1), Some(vec![1, 0, 0, 0, 0, 0, 1]));

		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(stmts),
			vec![(9, candidate_hash.clone())],
		);

		// Run to timeout period
		run_to_block(start + dispute_conclusion_by_time_out_period, |_| None);
		assert_eq!(SpamSlots::<Test>::get(start - 1), Some(vec![1, 0, 0, 0, 0, 0, 1]));

		// Run to timeout + 1 in order to executive on_finalize(timeout)
		run_to_block(start + dispute_conclusion_by_time_out_period + 1, |_| None);
		assert_eq!(SpamSlots::<Test>::get(start - 1), Some(vec![0, 0, 0, 0, 0, 0, 0]));
		assert_eq!(
			PUNISH_VALIDATORS_INCONCLUSIVE.with(|r| r.borrow()[0].clone()),
			(9, vec![ValidatorIndex(0), ValidatorIndex(6)]),
		);
	});
}

// Test pruning works
#[test]
fn test_initializer_on_new_session() {
	let dispute_period = 3;

	let mock_genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { dispute_period, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(mock_genesis_config).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
		Pallet::<Test>::note_included(0, candidate_hash.clone(), 0);
		Pallet::<Test>::note_included(1, candidate_hash.clone(), 1);
		Pallet::<Test>::note_included(2, candidate_hash.clone(), 2);
		Pallet::<Test>::note_included(3, candidate_hash.clone(), 3);
		Pallet::<Test>::note_included(4, candidate_hash.clone(), 4);
		Pallet::<Test>::note_included(5, candidate_hash.clone(), 5);
		Pallet::<Test>::note_included(6, candidate_hash.clone(), 5);

		run_to_block(7, |b| {
			// a new session at each block
			Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
		});

		// current session is 7,
		// we keep for dispute_period + 1 session and we remove in on_finalize
		// thus we keep info for session 3, 4, 5, 6, 7.
		assert_eq!(Included::<Test>::iter_prefix(0).count(), 0);
		assert_eq!(Included::<Test>::iter_prefix(1).count(), 0);
		assert_eq!(Included::<Test>::iter_prefix(2).count(), 0);
		assert_eq!(Included::<Test>::iter_prefix(3).count(), 1);
		assert_eq!(Included::<Test>::iter_prefix(4).count(), 1);
		assert_eq!(Included::<Test>::iter_prefix(5).count(), 1);
		assert_eq!(Included::<Test>::iter_prefix(6).count(), 1);
	});
}

#[test]
fn test_provide_data_duplicate_error() {
	new_test_ext(Default::default()).execute_with(|| {
		let candidate_hash_1 = CandidateHash(sp_core::H256::repeat_byte(1));
		let candidate_hash_2 = CandidateHash(sp_core::H256::repeat_byte(2));

		let mut stmts = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash_2,
				session: 2,
				statements: vec![],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_1,
				session: 1,
				statements: vec![],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_2,
				session: 2,
				statements: vec![],
			},
		];

		assert!(Pallet::<Test>::deduplicate_and_sort_dispute_data(&mut stmts).is_err());
		assert_eq!(stmts.len(), 2);
	})
}

#[test]
fn test_provide_multi_dispute_is_providing() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			if b == 1 {
				Some((
					true,
					b,
					vec![(&0, v0.public()), (&1, v1.public())],
					Some(vec![(&0, v0.public()), (&1, v1.public())]),
				))
			} else {
				Some((true, b, vec![(&1, v1.public())], Some(vec![(&1, v1.public())])))
			}
		});

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
		let stmts = vec![DisputeStatementSet {
			candidate_hash: candidate_hash.clone(),
			session: 1,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement {
							valid: true,
							candidate_hash: candidate_hash.clone(),
							session: 1,
						}
						.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: 1,
						}
						.signing_payload(),
					),
				),
			],
		}];

		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(
				stmts
					.into_iter()
					.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
					.collect()
			),
			vec![(1, candidate_hash.clone())],
		);
	})
}

#[test]
fn test_freeze_on_note_included() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(6, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![(&0, v0.public()), (&1, v1.public())],
				Some(vec![(&0, v0.public()), (&1, v1.public())]),
			))
		});

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

		// v0 votes for 3
		let stmts = vec![DisputeStatementSet {
			candidate_hash: candidate_hash.clone(),
			session: 3,
			statements: vec![
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement {
							valid: true,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				),
			],
		}];
		assert!(Pallet::<Test>::process_checked_multi_dispute_data(
			stmts
				.into_iter()
				.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
				.collect()
		)
		.is_ok());

		Pallet::<Test>::note_included(3, candidate_hash.clone(), 3);
		assert_eq!(Frozen::<Test>::get(), Some(2));
	});
}

#[test]
fn test_freeze_provided_against_supermajority_for_included() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(6, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![(&0, v0.public()), (&1, v1.public())],
				Some(vec![(&0, v0.public()), (&1, v1.public())]),
			))
		});

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

		// v0 votes for 3
		let stmts = vec![DisputeStatementSet {
			candidate_hash: candidate_hash.clone(),
			session: 3,
			statements: vec![
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement {
							valid: true,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				),
			],
		}];

		Pallet::<Test>::note_included(3, candidate_hash.clone(), 3);
		assert!(Pallet::<Test>::process_checked_multi_dispute_data(
			stmts
				.into_iter()
				.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
				.collect()
		)
		.is_ok());
		assert_eq!(Frozen::<Test>::get(), Some(2));
	});
}

// tests for:
// * provide_multi_dispute: with success scenario
// * disputes: correctness of datas
// * could_be_invalid: correctness of datas
// * note_included: decrement spam correctly
// * spam slots: correctly incremented and decremented
// * ensure rewards and punishment are correctly called.
#[test]
fn test_provide_multi_dispute_success_and_other() {
	new_test_ext(Default::default()).execute_with(|| {
		// 7 validators needed for byzantine threshold of 2.
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v6 = <ValidatorId as CryptoType>::Pair::generate().0;

		// v0 -> 0
		// v1 -> 3
		// v2 -> 6
		// v3 -> 5
		// v4 -> 1
		// v5 -> 4
		// v6 -> 2

		run_to_block(6, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![
					(&0, v0.public()),
					(&1, v1.public()),
					(&2, v2.public()),
					(&3, v3.public()),
					(&4, v4.public()),
					(&5, v5.public()),
					(&6, v6.public()),
				],
				Some(vec![
					(&0, v0.public()),
					(&1, v1.public()),
					(&2, v2.public()),
					(&3, v3.public()),
					(&4, v4.public()),
					(&5, v5.public()),
					(&6, v6.public()),
				]),
			))
		});

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

		// v0 votes for 3, v6 votes against
		let stmts = vec![DisputeStatementSet {
			candidate_hash: candidate_hash.clone(),
			session: 3,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement {
							valid: true,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(2),
					v6.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				),
			],
		}];

		let stmts = update_spam_slots(stmts);
		assert_eq!(SpamSlots::<Test>::get(3), Some(vec![1, 0, 1, 0, 0, 0, 0]));

		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(stmts),
			vec![(3, candidate_hash.clone())],
		);

		// v1 votes for 4 and for 3, v6 votes against 4.
		let stmts = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 4,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(3),
						v1.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 4,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(2),
						v6.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 4,
							}
							.signing_payload(),
						),
					),
				],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 3,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(3),
					v1.sign(
						&ExplicitDisputeStatement {
							valid: true,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				)],
			},
		];

		let stmts = update_spam_slots(stmts);

		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(stmts),
			vec![(4, candidate_hash.clone())],
		);
		assert_eq!(SpamSlots::<Test>::get(3), Some(vec![0, 0, 0, 0, 0, 0, 0])); // Confirmed as no longer spam
		assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 1, 1, 0, 0, 0]));

		// v3 votes against 3 and for 5, v6 votes against 5.
		let stmts = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 3,
				statements: vec![(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(5),
					v3.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				)],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 5,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(5),
						v3.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 5,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(2),
						v6.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 5,
							}
							.signing_payload(),
						),
					),
				],
			},
		];

		let stmts = update_spam_slots(stmts);
		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(stmts),
			vec![(5, candidate_hash.clone())],
		);
		assert_eq!(SpamSlots::<Test>::get(3), Some(vec![0, 0, 0, 0, 0, 0, 0]));
		assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 1, 1, 0, 0, 0]));
		assert_eq!(SpamSlots::<Test>::get(5), Some(vec![0, 0, 1, 0, 0, 1, 0]));

		// v2 votes for 3 and against 5
		let stmts = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 3,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(6),
					v2.sign(
						&ExplicitDisputeStatement {
							valid: true,
							candidate_hash: candidate_hash.clone(),
							session: 3,
						}
						.signing_payload(),
					),
				)],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 5,
				statements: vec![(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(6),
					v2.sign(
						&ExplicitDisputeStatement {
							valid: false,
							candidate_hash: candidate_hash.clone(),
							session: 5,
						}
						.signing_payload(),
					),
				)],
			},
		];
		let stmts = update_spam_slots(stmts);
		assert_ok!(Pallet::<Test>::process_checked_multi_dispute_data(stmts), vec![]);
		assert_eq!(SpamSlots::<Test>::get(3), Some(vec![0, 0, 0, 0, 0, 0, 0]));
		assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 1, 1, 0, 0, 0]));
		assert_eq!(SpamSlots::<Test>::get(5), Some(vec![0, 0, 0, 0, 0, 0, 0]));

		let stmts = vec![
			// 0, 4, and 5 vote against 5
			DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 5,
				statements: vec![
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						v0.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 5,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v4.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 5,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(4),
						v5.sign(
							&ExplicitDisputeStatement {
								valid: false,
								candidate_hash: candidate_hash.clone(),
								session: 5,
							}
							.signing_payload(),
						),
					),
				],
			},
			// 4 and 5 vote for 3
			DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 3,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v4.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(4),
						v5.sign(
							&ExplicitDisputeStatement {
								valid: true,
								candidate_hash: candidate_hash.clone(),
								session: 3,
							}
							.signing_payload(),
						),
					),
				],
			},
		];
		let stmts = update_spam_slots(stmts);
		assert_ok!(Pallet::<Test>::process_checked_multi_dispute_data(stmts), vec![]);

		assert_eq!(
			Pallet::<Test>::disputes(),
			vec![
				(
					5,
					candidate_hash.clone(),
					DisputeState {
						validators_for: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 1, 0],
						validators_against: bitvec![u8, BitOrderLsb0; 1, 1, 1, 0, 1, 0, 1],
						start: 6,
						concluded_at: Some(6), // 5 vote against
					}
				),
				(
					3,
					candidate_hash.clone(),
					DisputeState {
						validators_for: bitvec![u8, BitOrderLsb0; 1, 1, 0, 1, 1, 0, 1],
						validators_against: bitvec![u8, BitOrderLsb0; 0, 0, 1, 0, 0, 1, 0],
						start: 6,
						concluded_at: Some(6), // 5 vote for
					}
				),
				(
					4,
					candidate_hash.clone(),
					DisputeState {
						validators_for: bitvec![u8, BitOrderLsb0; 0, 0, 0, 1, 0, 0, 0],
						validators_against: bitvec![u8, BitOrderLsb0; 0, 0, 1, 0, 0, 0, 0],
						start: 6,
						concluded_at: None,
					}
				),
			]
		);

		assert!(!Pallet::<Test>::concluded_invalid(3, candidate_hash.clone()));
		assert!(!Pallet::<Test>::concluded_invalid(4, candidate_hash.clone()));
		assert!(Pallet::<Test>::concluded_invalid(5, candidate_hash.clone()));

		// Ensure inclusion removes spam slots
		assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 1, 1, 0, 0, 0]));
		Pallet::<Test>::note_included(4, candidate_hash.clone(), 4);
		assert_eq!(SpamSlots::<Test>::get(4), Some(vec![0, 0, 0, 0, 0, 0, 0]));

		// Ensure the `reward_validator` function was correctly called
		assert_eq!(
			REWARD_VALIDATORS.with(|r| r.borrow().clone()),
			vec![
				(3, vec![ValidatorIndex(0), ValidatorIndex(2)]),
				(4, vec![ValidatorIndex(2), ValidatorIndex(3)]),
				(3, vec![ValidatorIndex(3)]),
				(3, vec![ValidatorIndex(5)]),
				(5, vec![ValidatorIndex(2), ValidatorIndex(5)]),
				(3, vec![ValidatorIndex(6)]),
				(5, vec![ValidatorIndex(6)]),
				(5, vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(4)]),
				(3, vec![ValidatorIndex(1), ValidatorIndex(4)]),
			],
		);

		// Ensure punishment against is called
		assert_eq!(
			PUNISH_VALIDATORS_AGAINST.with(|r| r.borrow().clone()),
			vec![
				(3, vec![]),
				(4, vec![]),
				(3, vec![]),
				(3, vec![]),
				(5, vec![]),
				(3, vec![]),
				(5, vec![]),
				(5, vec![]),
				(3, vec![ValidatorIndex(2), ValidatorIndex(5)]),
			],
		);

		// Ensure punishment for is called
		assert_eq!(
			PUNISH_VALIDATORS_FOR.with(|r| r.borrow().clone()),
			vec![
				(3, vec![]),
				(4, vec![]),
				(3, vec![]),
				(3, vec![]),
				(5, vec![]),
				(3, vec![]),
				(5, vec![]),
				(5, vec![ValidatorIndex(5)]),
				(3, vec![]),
			],
		);
	})
}

#[test]
fn test_revert_and_freeze() {
	new_test_ext(Default::default()).execute_with(|| {
		// events are ignored for genesis block
		System::set_block_number(1);

		Frozen::<Test>::put(Some(0));
		assert_noop!(
			{
				Pallet::<Test>::revert_and_freeze(0);
				Result::<(), ()>::Err(()) // Just a small trick in order to use `assert_noop`.
			},
			(),
		);

		Frozen::<Test>::kill();
		Pallet::<Test>::revert_and_freeze(0);

		assert_eq!(Frozen::<Test>::get(), Some(0));
		assert_eq!(System::digest().logs[0], ConsensusLog::Revert(1).into());
		System::assert_has_event(Event::Revert(1).into());
	})
}

#[test]
fn test_revert_and_freeze_merges() {
	new_test_ext(Default::default()).execute_with(|| {
		Frozen::<Test>::put(Some(10));
		assert_noop!(
			{
				Pallet::<Test>::revert_and_freeze(10);
				Result::<(), ()>::Err(()) // Just a small trick in order to use `assert_noop`.
			},
			(),
		);

		Pallet::<Test>::revert_and_freeze(8);
		assert_eq!(Frozen::<Test>::get(), Some(8));
	})
}

#[test]
fn test_has_supermajority_against() {
	assert_eq!(
		has_supermajority_against(&DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 1, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 1, 1, 1, 1, 1, 0, 0, 0],
			start: 0,
			concluded_at: None,
		}),
		false,
	);

	assert_eq!(
		has_supermajority_against(&DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 1, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 1, 1, 1, 1, 1, 1, 0, 0],
			start: 0,
			concluded_at: None,
		}),
		true,
	);
}

#[test]
fn test_decrement_spam() {
	let original_spam_slots = vec![0, 1, 2, 3, 4, 5, 6, 7];

	// Test confirm is no-op
	let mut spam_slots = original_spam_slots.clone();
	let dispute_state_confirm = DisputeState {
		validators_for: bitvec![u8, BitOrderLsb0; 1, 1, 0, 0, 0, 0, 0, 0],
		validators_against: bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
		start: 0,
		concluded_at: None,
	};
	assert_eq!(DisputeStateFlags::from_state(&dispute_state_confirm), DisputeStateFlags::CONFIRMED);
	assert_eq!(
		decrement_spam(spam_slots.as_mut(), &dispute_state_confirm),
		bitvec![u8, BitOrderLsb0; 1, 1, 1, 0, 0, 0, 0, 0],
	);
	assert_eq!(spam_slots, original_spam_slots);

	// Test not confirm is decreasing spam
	let mut spam_slots = original_spam_slots.clone();
	let dispute_state_no_confirm = DisputeState {
		validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 0, 0, 0, 0, 0, 0],
		validators_against: bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
		start: 0,
		concluded_at: None,
	};
	assert_eq!(
		DisputeStateFlags::from_state(&dispute_state_no_confirm),
		DisputeStateFlags::default()
	);
	assert_eq!(
		decrement_spam(spam_slots.as_mut(), &dispute_state_no_confirm),
		bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
	);
	assert_eq!(spam_slots, vec![0, 1, 1, 3, 4, 5, 6, 7]);
}

#[test]
fn test_check_signature() {
	let validator_id = <ValidatorId as CryptoType>::Pair::generate().0;
	let wrong_validator_id = <ValidatorId as CryptoType>::Pair::generate().0;

	let session = 0;
	let wrong_session = 1;
	let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
	let wrong_candidate_hash = CandidateHash(sp_core::H256::repeat_byte(2));
	let inclusion_parent = sp_core::H256::repeat_byte(3);
	let wrong_inclusion_parent = sp_core::H256::repeat_byte(4);

	let statement_1 = DisputeStatement::Valid(ValidDisputeStatementKind::Explicit);
	let statement_2 = DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(
		inclusion_parent.clone(),
	));
	let wrong_statement_2 = DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(
		wrong_inclusion_parent.clone(),
	));
	let statement_3 =
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(inclusion_parent.clone()));
	let wrong_statement_3 = DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
		wrong_inclusion_parent.clone(),
	));
	let statement_4 = DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking);
	let statement_5 = DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit);

	let signed_1 = validator_id.sign(
		&ExplicitDisputeStatement { valid: true, candidate_hash: candidate_hash.clone(), session }
			.signing_payload(),
	);
	let signed_2 =
		validator_id.sign(&CompactStatement::Seconded(candidate_hash.clone()).signing_payload(
			&SigningContext { session_index: session, parent_hash: inclusion_parent.clone() },
		));
	let signed_3 =
		validator_id.sign(&CompactStatement::Valid(candidate_hash.clone()).signing_payload(
			&SigningContext { session_index: session, parent_hash: inclusion_parent.clone() },
		));
	let signed_4 =
		validator_id.sign(&ApprovalVote(candidate_hash.clone()).signing_payload(session));
	let signed_5 = validator_id.sign(
		&ExplicitDisputeStatement { valid: false, candidate_hash: candidate_hash.clone(), session }
			.signing_payload(),
	);

	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_1,
		&signed_1
	)
	.is_ok());
	assert!(check_signature(
		&wrong_validator_id.public(),
		candidate_hash,
		session,
		&statement_1,
		&signed_1
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		wrong_candidate_hash,
		session,
		&statement_1,
		&signed_1
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		wrong_session,
		&statement_1,
		&signed_1
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_2,
		&signed_1
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_3,
		&signed_1
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_4,
		&signed_1
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_5,
		&signed_1
	)
	.is_err());

	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_2,
		&signed_2
	)
	.is_ok());
	assert!(check_signature(
		&wrong_validator_id.public(),
		candidate_hash,
		session,
		&statement_2,
		&signed_2
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		wrong_candidate_hash,
		session,
		&statement_2,
		&signed_2
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		wrong_session,
		&statement_2,
		&signed_2
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&wrong_statement_2,
		&signed_2
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_1,
		&signed_2
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_3,
		&signed_2
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_4,
		&signed_2
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_5,
		&signed_2
	)
	.is_err());

	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_3,
		&signed_3
	)
	.is_ok());
	assert!(check_signature(
		&wrong_validator_id.public(),
		candidate_hash,
		session,
		&statement_3,
		&signed_3
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		wrong_candidate_hash,
		session,
		&statement_3,
		&signed_3
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		wrong_session,
		&statement_3,
		&signed_3
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&wrong_statement_3,
		&signed_3
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_1,
		&signed_3
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_2,
		&signed_3
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_4,
		&signed_3
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_5,
		&signed_3
	)
	.is_err());

	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_4,
		&signed_4
	)
	.is_ok());
	assert!(check_signature(
		&wrong_validator_id.public(),
		candidate_hash,
		session,
		&statement_4,
		&signed_4
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		wrong_candidate_hash,
		session,
		&statement_4,
		&signed_4
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		wrong_session,
		&statement_4,
		&signed_4
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_1,
		&signed_4
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_2,
		&signed_4
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_3,
		&signed_4
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_5,
		&signed_4
	)
	.is_err());

	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_5,
		&signed_5
	)
	.is_ok());
	assert!(check_signature(
		&wrong_validator_id.public(),
		candidate_hash,
		session,
		&statement_5,
		&signed_5
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		wrong_candidate_hash,
		session,
		&statement_5,
		&signed_5
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		wrong_session,
		&statement_5,
		&signed_5
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_1,
		&signed_5
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_2,
		&signed_5
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_3,
		&signed_5
	)
	.is_err());
	assert!(check_signature(
		&validator_id.public(),
		candidate_hash,
		session,
		&statement_4,
		&signed_5
	)
	.is_err());
}

#[test]
fn deduplication_and_sorting_works() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v3 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![(&0, v0.public()), (&1, v1.public()), (&2, v2.public()), (&3, v3.public())],
				Some(vec![
					(&0, v0.public()),
					(&1, v1.public()),
					(&2, v2.public()),
					(&3, v3.public()),
				]),
			))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));
		let candidate_hash_b = CandidateHash(sp_core::H256::repeat_byte(2));
		let candidate_hash_c = CandidateHash(sp_core::H256::repeat_byte(3));

		let create_explicit_statement = |vidx: ValidatorIndex,
		                                 validator: &<ValidatorId as CryptoType>::Pair,
		                                 c_hash: &CandidateHash,
		                                 valid,
		                                 session| {
			let payload =
				ExplicitDisputeStatement { valid, candidate_hash: c_hash.clone(), session }
					.signing_payload();
			let sig = validator.sign(&payload);
			(DisputeStatement::Valid(ValidDisputeStatementKind::Explicit), vidx, sig.clone())
		};

		let explicit_triple_a =
			create_explicit_statement(ValidatorIndex(0), &v0, &candidate_hash_a, true, 1);
		let explicit_triple_a_bad =
			create_explicit_statement(ValidatorIndex(1), &v1, &candidate_hash_a, false, 1);

		let explicit_triple_b =
			create_explicit_statement(ValidatorIndex(0), &v0, &candidate_hash_b, true, 2);
		let explicit_triple_b_bad =
			create_explicit_statement(ValidatorIndex(1), &v1, &candidate_hash_b, false, 2);

		let explicit_triple_c =
			create_explicit_statement(ValidatorIndex(0), &v0, &candidate_hash_c, true, 2);
		let explicit_triple_c_bad =
			create_explicit_statement(ValidatorIndex(1), &v1, &candidate_hash_c, false, 2);

		let mut disputes = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash_b.clone(),
				session: 2,
				statements: vec![explicit_triple_b.clone(), explicit_triple_b_bad.clone()],
			},
			// same session as above
			DisputeStatementSet {
				candidate_hash: candidate_hash_c.clone(),
				session: 2,
				statements: vec![explicit_triple_c, explicit_triple_c_bad],
			},
			// the duplicate set
			DisputeStatementSet {
				candidate_hash: candidate_hash_b.clone(),
				session: 2,
				statements: vec![explicit_triple_b.clone(), explicit_triple_b_bad.clone()],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: vec![explicit_triple_a, explicit_triple_a_bad],
			},
		];

		let disputes_orig = disputes.clone();

		<Pallet<Test> as DisputesHandler<
				<Test as frame_system::Config>::BlockNumber,
			>>::deduplicate_and_sort_dispute_data(&mut disputes).unwrap_err();

		// assert ordering of local only disputes, and at the same time, and being free of duplicates
		assert_eq!(disputes_orig.len(), disputes.len() + 1);

		let are_these_equal = |a: &DisputeStatementSet, b: &DisputeStatementSet| {
			use core::cmp::Ordering;
			// we only have local disputes here, so sorting of those adheres to the
			// simplified sorting logic
			let cmp =
				a.session.cmp(&b.session).then_with(|| a.candidate_hash.cmp(&b.candidate_hash));
			assert_ne!(cmp, Ordering::Greater);
			cmp == Ordering::Equal
		};

		assert_eq!(false, contains_duplicates_in_sorted_iter(&disputes, are_these_equal));
	})
}

fn apply_filter_all<T: Config, I: IntoIterator<Item = DisputeStatementSet>>(
	sets: I,
) -> Vec<CheckedDisputeStatementSet> {
	let config = <configuration::Pallet<T>>::config();
	let max_spam_slots = config.dispute_max_spam_slots;
	let post_conclusion_acceptance_period = config.dispute_post_conclusion_acceptance_period;

	let mut acc = Vec::<CheckedDisputeStatementSet>::new();
	for dispute_statement in sets {
		if let Some(checked) = <Pallet<T> as DisputesHandler<<T>::BlockNumber>>::filter_dispute_data(
			dispute_statement,
			max_spam_slots,
			post_conclusion_acceptance_period,
			VerifyDisputeSignatures::Yes,
		) {
			acc.push(checked);
		}
	}
	acc
}

#[test]
fn filter_removes_duplicates_within_set() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![(&0, v0.public()), (&1, v1.public())],
				Some(vec![(&0, v0.public()), (&1, v1.public())]),
			))
		});

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash.clone(),
			session: 1,
		}
		.signing_payload();

		let payload_against = ExplicitDisputeStatement {
			valid: false,
			candidate_hash: candidate_hash.clone(),
			session: 1,
		}
		.signing_payload();

		let sig_a = v0.sign(&payload);
		let sig_b = v0.sign(&payload);
		let sig_c = v0.sign(&payload);
		let sig_d = v1.sign(&payload_against);

		let statements = DisputeStatementSet {
			candidate_hash: candidate_hash.clone(),
			session: 1,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a.clone(),
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_b,
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_c,
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					sig_d.clone(),
				),
			],
		};

		let max_spam_slots = 10;
		let post_conclusion_acceptance_period = 10;
		let statements = <Pallet<Test> as DisputesHandler<
			<Test as frame_system::Config>::BlockNumber,
		>>::filter_dispute_data(
			statements,
			max_spam_slots,
			post_conclusion_acceptance_period,
			VerifyDisputeSignatures::Yes,
		);

		assert_eq!(
			statements,
			Some(CheckedDisputeStatementSet::unchecked_from_unchecked(DisputeStatementSet {
				candidate_hash: candidate_hash.clone(),
				session: 1,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_a,
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						sig_d,
					),
				]
			}))
		);
	})
}

#[test]
fn filter_bad_signatures_correctly_detects_single_sided() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v3 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![(&0, v0.public()), (&1, v1.public()), (&2, v2.public()), (&3, v3.public())],
				Some(vec![
					(&0, v0.public()),
					(&1, v1.public()),
					(&2, v2.public()),
					(&3, v3.public()),
				]),
			))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload = |c_hash: &CandidateHash, valid| {
			ExplicitDisputeStatement { valid, candidate_hash: c_hash.clone(), session: 1 }
				.signing_payload()
		};

		let payload_a = payload(&candidate_hash_a, true);
		let payload_a_bad = payload(&candidate_hash_a, false);

		let sig_0 = v0.sign(&payload_a);
		let sig_1 = v1.sign(&payload_a_bad);

		let statements = vec![DisputeStatementSet {
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_0.clone(),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(2),
					sig_1.clone(),
				),
			],
		}];

		let statements = apply_filter_all::<Test, _>(statements);

		assert!(statements.is_empty());
	})
}

#[test]
fn filter_correctly_accounts_spam_slots() {
	let dispute_max_spam_slots = 2;

	let mock_genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { dispute_max_spam_slots, ..Default::default() },
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(mock_genesis_config).execute_with(|| {
		// We need 7 validators for the byzantine threshold to be 2
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v6 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![
					(&0, v0.public()),
					(&1, v1.public()),
					(&2, v2.public()),
					(&3, v3.public()),
					(&4, v4.public()),
					(&5, v5.public()),
					(&6, v6.public()),
				],
				Some(vec![
					(&0, v0.public()),
					(&1, v1.public()),
					(&2, v2.public()),
					(&3, v3.public()),
					(&4, v4.public()),
					(&5, v5.public()),
					(&6, v6.public()),
				]),
			))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));
		let candidate_hash_b = CandidateHash(sp_core::H256::repeat_byte(2));
		let candidate_hash_c = CandidateHash(sp_core::H256::repeat_byte(3));

		let payload = |c_hash: &CandidateHash, valid| {
			ExplicitDisputeStatement { valid, candidate_hash: c_hash.clone(), session: 1 }
				.signing_payload()
		};

		let payload_a = payload(&candidate_hash_a, true);
		let payload_b = payload(&candidate_hash_b, true);
		let payload_c = payload(&candidate_hash_c, true);

		let payload_a_bad = payload(&candidate_hash_a, false);
		let payload_b_bad = payload(&candidate_hash_b, false);
		let payload_c_bad = payload(&candidate_hash_c, false);

		let sig_0a = v0.sign(&payload_a);
		let sig_0b = v0.sign(&payload_b);
		let sig_0c = v0.sign(&payload_c);

		let sig_1b = v1.sign(&payload_b);

		let sig_2a = v2.sign(&payload_a_bad);
		let sig_2b = v2.sign(&payload_b_bad);
		let sig_2c = v2.sign(&payload_c_bad);

		let statements = vec![
			// validators 0 and 2 get 1 spam slot from this.
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_0a.clone(),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(6),
						sig_2a.clone(),
					),
				],
			},
			// Validators 0, 2, and 3 get no spam slots for this
			DisputeStatementSet {
				candidate_hash: candidate_hash_b.clone(),
				session: 1,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_0b.clone(),
					),
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(3),
						sig_1b.clone(),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(6),
						sig_2b.clone(),
					),
				],
			},
			// Validators 0 and 2 get an extra spam slot for this.
			DisputeStatementSet {
				candidate_hash: candidate_hash_c.clone(),
				session: 1,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						sig_0c.clone(),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(6),
						sig_2c.clone(),
					),
				],
			},
		];

		let old_statements = statements
			.clone()
			.into_iter()
			.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
			.collect::<Vec<_>>();
		let statements = apply_filter_all::<Test, _>(statements);

		assert_eq!(statements, old_statements);
	})
}

#[test]
fn filter_removes_session_out_of_bounds() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
		});

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash.clone(),
			session: 1,
		}
		.signing_payload();

		let sig_a = v0.sign(&payload);

		let statements = vec![DisputeStatementSet {
			candidate_hash: candidate_hash.clone(),
			session: 100,
			statements: vec![(
				DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
				ValidatorIndex(0),
				sig_a,
			)],
		}];

		let statements = apply_filter_all::<Test, _>(statements);

		assert!(statements.is_empty());
	})
}

#[test]
fn filter_removes_concluded_ancient() {
	let dispute_post_conclusion_acceptance_period = 2;

	let mock_genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration {
				dispute_post_conclusion_acceptance_period,
				..Default::default()
			},
			..Default::default()
		},
		..Default::default()
	};

	new_test_ext(mock_genesis_config).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));
		let candidate_hash_b = CandidateHash(sp_core::H256::repeat_byte(2));

		<Disputes<Test>>::insert(
			&1,
			&candidate_hash_a,
			DisputeState {
				validators_for: bitvec![u8, BitOrderLsb0; 0; 4],
				validators_against: bitvec![u8, BitOrderLsb0; 1; 4],
				start: 0,
				concluded_at: Some(0),
			},
		);

		<Disputes<Test>>::insert(
			&1,
			&candidate_hash_b,
			DisputeState {
				validators_for: bitvec![u8, BitOrderLsb0; 0; 4],
				validators_against: bitvec![u8, BitOrderLsb0; 1; 4],
				start: 0,
				concluded_at: Some(1),
			},
		);

		let payload_a = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let payload_b = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash_b.clone(),
			session: 1,
		}
		.signing_payload();

		let sig_a = v0.sign(&payload_a);
		let sig_b = v0.sign(&payload_b);

		let statements = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a,
				)],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_b.clone(),
				session: 1,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_b.clone(),
				)],
			},
		];

		let statements = apply_filter_all::<Test, _>(statements);

		assert_eq!(
			statements,
			vec![CheckedDisputeStatementSet::unchecked_from_unchecked(DisputeStatementSet {
				candidate_hash: candidate_hash_b.clone(),
				session: 1,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_b,
				),]
			})]
		);
	})
}

#[test]
fn filter_removes_duplicate_statements_sets() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![(&0, v0.public()), (&1, v1.public())],
				Some(vec![(&0, v0.public()), (&1, v1.public())]),
			))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let payload_against = ExplicitDisputeStatement {
			valid: false,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let sig_a = v0.sign(&payload);
		let sig_a_against = v1.sign(&payload_against);

		let statements = vec![
			(
				DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
				ValidatorIndex(0),
				sig_a.clone(),
			),
			(
				DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
				ValidatorIndex(1),
				sig_a_against.clone(),
			),
		];

		let mut sets = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: statements.clone(),
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: statements.clone(),
			},
		];

		// `Err(())` indicates presence of duplicates
		assert!(<Pallet::<Test> as DisputesHandler<
			<Test as frame_system::Config>::BlockNumber,
		>>::deduplicate_and_sort_dispute_data(&mut sets)
		.is_err());

		assert_eq!(
			sets,
			vec![DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements,
			}]
		);
	})
}

#[test]
fn assure_no_duplicate_statements_sets_are_fine() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![(&0, v0.public()), (&1, v1.public())],
				Some(vec![(&0, v0.public()), (&1, v1.public())]),
			))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let payload_against = ExplicitDisputeStatement {
			valid: false,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let sig_a = v0.sign(&payload);
		let sig_a_against = v1.sign(&payload_against);

		let statements = vec![
			(
				DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
				ValidatorIndex(0),
				sig_a.clone(),
			),
			(
				DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
				ValidatorIndex(1),
				sig_a_against.clone(),
			),
		];

		let sets = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: statements.clone(),
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 2,
				statements: statements.clone(),
			},
		];

		// `Err(())` indicates presence of duplicates
		assert!(<Pallet::<Test> as DisputesHandler<
			<Test as frame_system::Config>::BlockNumber,
		>>::assure_deduplicated_and_sorted(&sets)
		.is_ok());
	})
}

#[test]
fn assure_detects_duplicate_statements_sets() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((
				true,
				b,
				vec![(&0, v0.public()), (&1, v1.public())],
				Some(vec![(&0, v0.public()), (&1, v1.public())]),
			))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let payload_against = ExplicitDisputeStatement {
			valid: false,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let sig_a = v0.sign(&payload);
		let sig_a_against = v1.sign(&payload_against);

		let statements = vec![
			(
				DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
				ValidatorIndex(0),
				sig_a.clone(),
			),
			(
				DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
				ValidatorIndex(1),
				sig_a_against.clone(),
			),
		];

		let sets = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: statements.clone(),
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_a.clone(),
				session: 1,
				statements: statements.clone(),
			},
		];

		// `Err(())` indicates presence of duplicates
		assert!(<Pallet::<Test> as DisputesHandler<
			<Test as frame_system::Config>::BlockNumber,
		>>::assure_deduplicated_and_sorted(&sets)
		.is_err());
	})
}

#[test]
fn filter_ignores_single_sided() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let sig_a = v0.sign(&payload);

		let statements = vec![DisputeStatementSet {
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
			statements: vec![(
				DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
				ValidatorIndex(0),
				sig_a.clone(),
			)],
		}];

		let statements = apply_filter_all::<Test, _>(statements);

		assert!(statements.is_empty());
	})
}

#[test]
fn import_ignores_single_sided() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
		});

		let candidate_hash_a = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload = ExplicitDisputeStatement {
			valid: true,
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
		}
		.signing_payload();

		let sig_a = v0.sign(&payload);

		let statements = vec![DisputeStatementSet {
			candidate_hash: candidate_hash_a.clone(),
			session: 1,
			statements: vec![(
				DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
				ValidatorIndex(0),
				sig_a.clone(),
			)],
		}];

		let statements = apply_filter_all::<Test, _>(statements);
		assert!(statements.is_empty());
	})
}
