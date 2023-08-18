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
use crate::{
	configuration::HostConfiguration,
	disputes::DisputesHandler,
	mock::{
		new_test_ext, AccountId, AllPalletsWithSystem, Initializer, MockGenesisConfig, System,
		Test, PUNISH_BACKERS_FOR, PUNISH_VALIDATORS_AGAINST, PUNISH_VALIDATORS_FOR,
		REWARD_VALIDATORS,
	},
};
use frame_support::{
	assert_err, assert_noop, assert_ok,
	traits::{OnFinalize, OnInitialize},
};
use frame_system::pallet_prelude::BlockNumberFor;
use primitives::BlockNumber;
use sp_core::{crypto::CryptoType, Pair};

const VOTE_FOR: VoteKind = VoteKind::ExplicitValid;
const VOTE_AGAINST: VoteKind = VoteKind::Invalid;
const VOTE_BACKING: VoteKind = VoteKind::Backing;

fn filter_dispute_set(stmts: MultiDisputeStatementSet) -> CheckedMultiDisputeStatementSet {
	let config = <configuration::Pallet<Test>>::config();
	let post_conclusion_acceptance_period = config.dispute_post_conclusion_acceptance_period;

	stmts
		.into_iter()
		.filter_map(|set| {
			let filter =
				Pallet::<Test>::filter_dispute_data(&set, post_conclusion_acceptance_period);
			filter.filter_statement_set(set)
		})
		.collect::<Vec<_>>()
}

/// Returns `true` if duplicate items were found, otherwise `false`.
///
/// `check_equal(a: &T, b: &T)` _must_ return `true`, iff `a` and `b` are equal, otherwise `false.
/// The definition of _equal_ is to be defined by the user.
///
/// Attention: Requires the input `iter` to be sorted, such that _equals_
/// would be adjacent in respect whatever `check_equal` defines as equality!
fn contains_duplicates_in_sorted_iter<
	'a,
	T: 'a,
	I: 'a + IntoIterator<Item = &'a T>,
	C: 'static + FnMut(&T, &T) -> bool,
>(
	iter: I,
	mut check_equal: C,
) -> bool {
	let mut iter = iter.into_iter();
	if let Some(mut previous) = iter.next() {
		while let Some(current) = iter.next() {
			if check_equal(previous, current) {
				return true
			}
			previous = current;
		}
	}
	return false
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
			validators_against: bitvec![u8, BitOrderLsb0; 1, 1, 1, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		}),
		DisputeStateFlags::CONFIRMED | DisputeStateFlags::AGAINST_BYZANTINE,
	);

	assert_eq!(
		DisputeStateFlags::from_state(&DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 1, 1, 1, 1, 1, 0, 0],
			start: 0,
			concluded_at: None,
		}),
		DisputeStateFlags::AGAINST_SUPERMAJORITY |
			DisputeStateFlags::CONFIRMED |
			DisputeStateFlags::AGAINST_BYZANTINE,
	);
}

#[test]
fn test_import_new_participant() {
	let mut importer = DisputeStateImporter::new(
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
		BTreeSet::new(),
		0,
	);

	assert_err!(
		importer.import(ValidatorIndex(9), VOTE_FOR),
		VoteImportError::ValidatorIndexOutOfBounds,
	);

	assert_err!(importer.import(ValidatorIndex(0), VOTE_FOR), VoteImportError::DuplicateStatement);
	assert_ok!(importer.import(ValidatorIndex(0), VOTE_AGAINST));

	assert_ok!(importer.import(ValidatorIndex(2), VOTE_FOR));
	assert_err!(importer.import(ValidatorIndex(2), VOTE_FOR), VoteImportError::DuplicateStatement);

	assert_ok!(importer.import(ValidatorIndex(2), VOTE_AGAINST));
	assert_err!(
		importer.import(ValidatorIndex(2), VOTE_AGAINST),
		VoteImportError::DuplicateStatement
	);

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
	assert!(summary.slash_for.is_empty());
	assert!(summary.slash_against.is_empty());
	assert_eq!(summary.new_participants, bitvec![u8, BitOrderLsb0; 0, 0, 1, 0, 0, 0, 0, 0]);
}

#[test]
fn test_import_prev_participant_confirmed() {
	let mut importer = DisputeStateImporter::new(
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
		BTreeSet::new(),
		0,
	);

	assert_ok!(importer.import(ValidatorIndex(2), VOTE_FOR));

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

	assert!(summary.slash_for.is_empty());
	assert!(summary.slash_against.is_empty());
	assert_eq!(summary.new_participants, bitvec![u8, BitOrderLsb0; 0, 0, 1, 0, 0, 0, 0, 0]);
	assert_eq!(summary.new_flags, DisputeStateFlags::CONFIRMED);
}

#[test]
fn test_import_prev_participant_confirmed_slash_for() {
	let mut importer = DisputeStateImporter::new(
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 0, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
		BTreeSet::new(),
		0,
	);

	assert_ok!(importer.import(ValidatorIndex(2), VOTE_FOR));
	assert_ok!(importer.import(ValidatorIndex(2), VOTE_AGAINST));
	assert_ok!(importer.import(ValidatorIndex(3), VOTE_AGAINST));
	assert_ok!(importer.import(ValidatorIndex(4), VOTE_AGAINST));
	assert_ok!(importer.import(ValidatorIndex(5), VOTE_AGAINST));
	assert_ok!(importer.import(ValidatorIndex(6), VOTE_AGAINST));

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

	assert_eq!(summary.slash_for, vec![ValidatorIndex(0), ValidatorIndex(2)]);
	assert!(summary.slash_against.is_empty());
	assert_eq!(summary.new_participants, bitvec![u8, BitOrderLsb0; 0, 0, 1, 1, 1, 1, 1, 0]);
	assert_eq!(
		summary.new_flags,
		DisputeStateFlags::CONFIRMED |
			DisputeStateFlags::AGAINST_SUPERMAJORITY |
			DisputeStateFlags::AGAINST_BYZANTINE,
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
		BTreeSet::new(),
		0,
	);

	assert_ok!(importer.import(ValidatorIndex(3), VOTE_FOR));
	assert_ok!(importer.import(ValidatorIndex(4), VOTE_FOR));
	assert_ok!(importer.import(ValidatorIndex(5), VOTE_AGAINST));
	assert_ok!(importer.import(ValidatorIndex(6), VOTE_FOR));
	assert_ok!(importer.import(ValidatorIndex(7), VOTE_FOR));

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
	assert!(summary.slash_for.is_empty());
	assert_eq!(summary.slash_against, vec![ValidatorIndex(1), ValidatorIndex(5)]);
	assert_eq!(summary.new_participants, bitvec![u8, BitOrderLsb0; 0, 0, 0, 1, 1, 1, 1, 1]);
	assert_eq!(summary.new_flags, DisputeStateFlags::FOR_SUPERMAJORITY);
}

#[test]
fn test_import_backing_votes() {
	let mut importer = DisputeStateImporter::new(
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 1, 0, 0, 0, 0, 0],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 0, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
		BTreeSet::from_iter([ValidatorIndex(0)]),
		0,
	);

	assert_ok!(importer.import(ValidatorIndex(3), VOTE_FOR));
	assert_ok!(importer.import(ValidatorIndex(3), VOTE_BACKING));
	assert_ok!(importer.import(ValidatorIndex(3), VOTE_AGAINST));
	assert_ok!(importer.import(ValidatorIndex(6), VOTE_FOR));
	assert_ok!(importer.import(ValidatorIndex(7), VOTE_BACKING));
	// Don't import backing vote twice
	assert_err!(
		importer.import(ValidatorIndex(0), VOTE_BACKING),
		VoteImportError::DuplicateStatement,
	);
	// Don't import explicit votes after backing
	assert_err!(importer.import(ValidatorIndex(7), VOTE_FOR), VoteImportError::MaliciousBacker,);

	let summary = importer.finish();
	assert_eq!(
		summary.state,
		DisputeState {
			validators_for: bitvec![u8, BitOrderLsb0; 1, 0, 1, 1, 0, 0, 1, 1],
			validators_against: bitvec![u8, BitOrderLsb0; 0, 1, 0, 1, 0, 0, 0, 0],
			start: 0,
			concluded_at: None,
		},
	);
	assert_eq!(
		summary.backers,
		BTreeSet::from_iter([ValidatorIndex(0), ValidatorIndex(3), ValidatorIndex(7),]),
	);
}

// Test pruning works
#[test]
fn test_initializer_on_new_session() {
	let dispute_period = 3;

	let mock_genesis_config = MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: HostConfiguration { dispute_period, ..Default::default() },
		},
		..Default::default()
	};

	new_test_ext(mock_genesis_config).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
		Pallet::<Test>::note_included(0, candidate_hash, 0);
		Pallet::<Test>::note_included(1, candidate_hash, 1);
		Pallet::<Test>::note_included(2, candidate_hash, 2);
		Pallet::<Test>::note_included(3, candidate_hash, 3);
		Pallet::<Test>::note_included(4, candidate_hash, 4);
		Pallet::<Test>::note_included(5, candidate_hash, 5);
		Pallet::<Test>::note_included(6, candidate_hash, 5);

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
		let inclusion_parent = sp_core::H256::repeat_byte(0xff);
		let session = 1;
		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
						inclusion_parent,
					)),
					ValidatorIndex(0),
					v0.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
						&SigningContext { session_index: session, parent_hash: inclusion_parent },
					)),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
			],
		}];

		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(
				&stmts
					.into_iter()
					.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
					.collect()
			),
			vec![(1, candidate_hash)],
		);
	})
}

#[test]
fn test_disputes_with_missing_backing_votes_are_rejected() {
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
		let session = 1;

		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement { valid: true, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
			],
		}];

		assert!(Pallet::<Test>::process_checked_multi_dispute_data(
			&stmts
				.into_iter()
				.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
				.collect()
		)
		.is_err(),);
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
		let inclusion_parent = sp_core::H256::repeat_byte(0xff);
		let session = 3;

		// v0 votes for 3
		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session: 3,
			statements: vec![
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session: 3 }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session: 3 }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
						inclusion_parent,
					)),
					ValidatorIndex(1),
					v0.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
						&SigningContext { session_index: session, parent_hash: inclusion_parent },
					)),
				),
			],
		}];
		assert!(Pallet::<Test>::process_checked_multi_dispute_data(
			&stmts
				.into_iter()
				.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
				.collect()
		)
		.is_ok());

		Pallet::<Test>::note_included(3, candidate_hash, 3);
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
		let inclusion_parent = sp_core::H256::repeat_byte(0xff);
		let session = 3;

		// v0 votes for 3
		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session,
			statements: vec![
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
						inclusion_parent,
					)),
					ValidatorIndex(1),
					v0.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
						&SigningContext { session_index: session, parent_hash: inclusion_parent },
					)),
				),
			],
		}];

		Pallet::<Test>::note_included(3, candidate_hash, 3);
		assert!(Pallet::<Test>::process_checked_multi_dispute_data(
			&stmts
				.into_iter()
				.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
				.collect()
		)
		.is_ok());
		assert_eq!(Frozen::<Test>::get(), Some(2));
	});
}

#[test]
fn test_freeze_provided_against_byzantine_threshold_for_included() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v6 = <ValidatorId as CryptoType>::Pair::generate().0;

		let active_set = vec![
			(&0, v0.public()),
			(&1, v1.public()),
			(&2, v2.public()),
			(&3, v3.public()),
			(&4, v4.public()),
			(&5, v5.public()),
			(&6, v6.public()),
		];

		run_to_block(6, |b| Some((true, b, active_set.clone(), Some(active_set.clone()))));

		// A candidate which will be disputed
		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));
		let inclusion_parent = sp_core::H256::repeat_byte(0xff);
		let session = 3;

		// A byzantine threshold of INVALID
		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session,
			statements: vec![
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v1.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(2),
					v2.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
						inclusion_parent,
					)),
					ValidatorIndex(1),
					v0.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
						&SigningContext { session_index: session, parent_hash: inclusion_parent },
					)),
				),
			],
		}];

		// Include the candidate and import the votes
		Pallet::<Test>::note_included(3, candidate_hash, 3);
		assert!(Pallet::<Test>::process_checked_multi_dispute_data(
			&stmts
				.into_iter()
				.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
				.collect()
		)
		.is_ok());
		// Successful import should freeze the chain
		assert_eq!(Frozen::<Test>::get(), Some(2));

		// Now include one more block
		run_to_block(7, |b| Some((true, b, active_set.clone(), Some(active_set.clone()))));
		Pallet::<Test>::note_included(3, CandidateHash(sp_core::H256::repeat_byte(2)), 3);

		// And generate enough votes to reach supermajority of invalid votes
		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session,
			statements: vec![
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(3),
					v3.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(4),
					v4.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(5),
					v5.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
			],
		}];
		assert!(Pallet::<Test>::process_checked_multi_dispute_data(
			&stmts
				.into_iter()
				.map(CheckedDisputeStatementSet::unchecked_from_unchecked)
				.collect()
		)
		.is_ok());
		// Chain should still be frozen
		assert_eq!(Frozen::<Test>::get(), Some(2));
	});
}

mod unconfirmed_disputes {
	use super::*;
	use assert_matches::assert_matches;
	use sp_runtime::ModuleError;

	// Shared initialization code between `test_unconfirmed_are_ignored` and
	// `test_unconfirmed_disputes_cause_block_import_error`
	fn generate_dispute_statement_set_and_run_to_block() -> DisputeStatementSet {
		// 7 validators needed for byzantine threshold of 2.
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v6 = <ValidatorId as CryptoType>::Pair::generate().0;

		// Mapping between key pair and `ValidatorIndex`
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

		// v0 votes for 4, v1 votes against 4.
		DisputeStatementSet {
			candidate_hash,
			session: 4,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					v0.sign(
						&ExplicitDisputeStatement { valid: true, candidate_hash, session: 4 }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(3),
					v1.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session: 4 }
							.signing_payload(),
					),
				),
			],
		}
	}
	#[test]
	fn test_unconfirmed_are_ignored() {
		new_test_ext(Default::default()).execute_with(|| {
			let stmts = vec![generate_dispute_statement_set_and_run_to_block()];
			let stmts = filter_dispute_set(stmts);

			// Not confirmed => should be filtered out
			assert_ok!(Pallet::<Test>::process_checked_multi_dispute_data(&stmts), vec![],);
		});
	}

	#[test]
	fn test_unconfirmed_disputes_cause_block_import_error() {
		new_test_ext(Default::default()).execute_with(|| {

		let stmts = generate_dispute_statement_set_and_run_to_block();
		let stmts = vec![CheckedDisputeStatementSet::unchecked_from_unchecked(stmts)];

		assert_matches!(
			Pallet::<Test>::process_checked_multi_dispute_data(&stmts),
			Err(DispatchError::Module(ModuleError{index: _, error: _, message})) => assert_eq!(message, Some("UnconfirmedDispute"))
		);

	});
	}
}

// tests for:
// * provide_multi_dispute: with success scenario
// * disputes: correctness of datas
// * could_be_invalid: correctness of datas
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

		// Mapping between key pair and `ValidatorIndex`
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
		let inclusion_parent = sp_core::H256::repeat_byte(0xff);
		let session = 3;

		// v0 and v1 vote for 3, v6 votes against
		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
						inclusion_parent,
					)),
					ValidatorIndex(0),
					v0.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
						&SigningContext { session_index: session, parent_hash: inclusion_parent },
					)),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(2),
					v6.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(3),
					v1.sign(
						&ExplicitDisputeStatement { valid: true, candidate_hash, session: 3 }
							.signing_payload(),
					),
				),
			],
		}];

		let stmts = filter_dispute_set(stmts);

		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(&stmts),
			vec![(3, candidate_hash)],
		);

		// v3 votes against 3 and for 5, v2 and v6 vote against 5.
		let stmts = vec![
			DisputeStatementSet {
				candidate_hash,
				session: 3,
				statements: vec![(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(5),
					v3.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session: 3 }
							.signing_payload(),
					),
				)],
			},
			DisputeStatementSet {
				candidate_hash,
				session: 5,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
							inclusion_parent,
						)),
						ValidatorIndex(5),
						v3.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
							&SigningContext { session_index: 5, parent_hash: inclusion_parent },
						)),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(6),
						v2.sign(
							&ExplicitDisputeStatement { valid: false, candidate_hash, session: 5 }
								.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(2),
						v6.sign(
							&ExplicitDisputeStatement { valid: false, candidate_hash, session: 5 }
								.signing_payload(),
						),
					),
				],
			},
		];

		let stmts = filter_dispute_set(stmts);
		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(&stmts),
			vec![(5, candidate_hash)],
		);

		// v2 votes for 3
		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session: 3,
			statements: vec![(
				DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
				ValidatorIndex(6),
				v2.sign(
					&ExplicitDisputeStatement { valid: true, candidate_hash, session: 3 }
						.signing_payload(),
				),
			)],
		}];
		let stmts = filter_dispute_set(stmts);
		assert_ok!(Pallet::<Test>::process_checked_multi_dispute_data(&stmts), vec![]);

		let stmts = vec![
			// 0, 4, and 5 vote against 5
			DisputeStatementSet {
				candidate_hash,
				session: 5,
				statements: vec![
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(0),
						v0.sign(
							&ExplicitDisputeStatement { valid: false, candidate_hash, session: 5 }
								.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v4.sign(
							&ExplicitDisputeStatement { valid: false, candidate_hash, session: 5 }
								.signing_payload(),
						),
					),
					(
						DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
						ValidatorIndex(4),
						v5.sign(
							&ExplicitDisputeStatement { valid: false, candidate_hash, session: 5 }
								.signing_payload(),
						),
					),
				],
			},
			// 4 and 5 vote for 3
			DisputeStatementSet {
				candidate_hash,
				session: 3,
				statements: vec![
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(1),
						v4.sign(
							&ExplicitDisputeStatement { valid: true, candidate_hash, session: 3 }
								.signing_payload(),
						),
					),
					(
						DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
						ValidatorIndex(4),
						v5.sign(
							&ExplicitDisputeStatement { valid: true, candidate_hash, session: 3 }
								.signing_payload(),
						),
					),
				],
			},
		];
		let stmts = filter_dispute_set(stmts);
		assert_ok!(Pallet::<Test>::process_checked_multi_dispute_data(&stmts), vec![]);

		assert_eq!(
			Pallet::<Test>::disputes(),
			vec![
				(
					5,
					candidate_hash,
					DisputeState {
						validators_for: bitvec![u8, BitOrderLsb0; 0, 0, 0, 0, 0, 1, 0],
						validators_against: bitvec![u8, BitOrderLsb0; 1, 1, 1, 0, 1, 0, 1],
						start: 6,
						concluded_at: Some(6), // 5 vote against
					}
				),
				(
					3,
					candidate_hash,
					DisputeState {
						validators_for: bitvec![u8, BitOrderLsb0; 1, 1, 0, 1, 1, 0, 1],
						validators_against: bitvec![u8, BitOrderLsb0; 0, 0, 1, 0, 0, 1, 0],
						start: 6,
						concluded_at: Some(6), // 5 vote for
					}
				),
			]
		);

		assert!(!Pallet::<Test>::concluded_invalid(3, candidate_hash));
		assert!(!Pallet::<Test>::concluded_invalid(4, candidate_hash));
		assert!(Pallet::<Test>::concluded_invalid(5, candidate_hash));

		// Ensure the `reward_validator` function was correctly called
		assert_eq!(
			REWARD_VALIDATORS.with(|r| r.borrow().clone()),
			vec![
				(3, vec![ValidatorIndex(0), ValidatorIndex(2), ValidatorIndex(3)]),
				(3, vec![ValidatorIndex(5)]),
				(5, vec![ValidatorIndex(2), ValidatorIndex(5), ValidatorIndex(6)]),
				(3, vec![ValidatorIndex(6)]),
				(5, vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(4)]),
				(3, vec![ValidatorIndex(1), ValidatorIndex(4)]),
			],
		);

		// Ensure punishment against is called
		assert_eq!(
			PUNISH_VALIDATORS_AGAINST.with(|r| r.borrow().clone()),
			vec![
				(3, vec![]),
				(3, vec![]),
				(5, vec![]),
				(3, vec![]),
				(5, vec![]),
				(3, vec![ValidatorIndex(2), ValidatorIndex(5)]),
			],
		);

		// Ensure punishment for is called
		assert_eq!(
			PUNISH_VALIDATORS_FOR.with(|r| r.borrow().clone()),
			vec![
				(3, vec![]),
				(3, vec![]),
				(5, vec![]),
				(3, vec![]),
				(5, vec![ValidatorIndex(5)]),
				(3, vec![]),
			],
		);
	})
}

/// In this setup we have only one dispute concluding AGAINST.
/// There are some votes imported post dispute conclusion.
/// We make sure these votes are accounted for in punishment.
#[test]
fn test_punish_post_conclusion() {
	new_test_ext(Default::default()).execute_with(|| {
		// supermajority threshold is 5
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v1 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v2 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v3 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v4 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v5 = <ValidatorId as CryptoType>::Pair::generate().0;
		let v6 = <ValidatorId as CryptoType>::Pair::generate().0;
		// Mapping between key pair and `ValidatorIndex`
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
		let inclusion_parent = sp_core::H256::repeat_byte(0xff);
		let session = 3;

		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
						inclusion_parent,
					)),
					ValidatorIndex(0),
					v0.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
						&SigningContext { session_index: session, parent_hash: inclusion_parent },
					)),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(1),
					v4.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(2),
					v6.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(6),
					v2.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(4),
					v5.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(5),
					v3.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking),
					ValidatorIndex(3),
					v1.sign(&ApprovalVote(candidate_hash).signing_payload(session)),
				),
			],
		}];

		let stmts = filter_dispute_set(stmts);
		assert_ok!(
			Pallet::<Test>::process_checked_multi_dispute_data(&stmts),
			vec![(session, candidate_hash)],
		);

		assert_eq!(
			PUNISH_VALIDATORS_FOR.with(|r| r.borrow().clone()),
			vec![(session, vec![ValidatorIndex(0), ValidatorIndex(3)]),],
		);
		assert_eq!(
			PUNISH_BACKERS_FOR.with(|r| r.borrow().clone()),
			vec![(session, vec![ValidatorIndex(0)]),],
		);

		// someone reveals 3 backing vote, 6 votes against
		let stmts = vec![DisputeStatementSet {
			candidate_hash,
			session,
			statements: vec![
				(
					DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(
						inclusion_parent,
					)),
					ValidatorIndex(3),
					v1.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
						&SigningContext { session_index: session, parent_hash: inclusion_parent },
					)),
				),
				(
					DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit),
					ValidatorIndex(6),
					v2.sign(
						&ExplicitDisputeStatement { valid: false, candidate_hash, session }
							.signing_payload(),
					),
				),
			],
		}];

		let stmts = filter_dispute_set(stmts);
		assert_ok!(Pallet::<Test>::process_checked_multi_dispute_data(&stmts), vec![],);

		// Ensure punishment for is called
		assert_eq!(
			PUNISH_VALIDATORS_FOR.with(|r| r.borrow().clone()),
			vec![
				(session, vec![ValidatorIndex(0), ValidatorIndex(3)]),
				(session, vec![ValidatorIndex(3)]),
			],
		);

		assert_eq!(
			PUNISH_BACKERS_FOR.with(|r| r.borrow().clone()),
			vec![
				(session, vec![ValidatorIndex(0)]),
				(session, vec![ValidatorIndex(0), ValidatorIndex(3)])
			],
		);

		assert_eq!(
			PUNISH_VALIDATORS_AGAINST.with(|r| r.borrow().clone()),
			vec![(session, vec![]), (session, vec![]),],
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
	let statement_2 =
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(inclusion_parent));
	let wrong_statement_2 =
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingSeconded(wrong_inclusion_parent));
	let statement_3 =
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(inclusion_parent));
	let wrong_statement_3 =
		DisputeStatement::Valid(ValidDisputeStatementKind::BackingValid(wrong_inclusion_parent));
	let statement_4 = DisputeStatement::Valid(ValidDisputeStatementKind::ApprovalChecking);
	let statement_5 = DisputeStatement::Invalid(InvalidDisputeStatementKind::Explicit);

	let signed_1 = validator_id
		.sign(&ExplicitDisputeStatement { valid: true, candidate_hash, session }.signing_payload());
	let signed_2 = validator_id.sign(&CompactStatement::Seconded(candidate_hash).signing_payload(
		&SigningContext { session_index: session, parent_hash: inclusion_parent },
	));
	let signed_3 = validator_id.sign(&CompactStatement::Valid(candidate_hash).signing_payload(
		&SigningContext { session_index: session, parent_hash: inclusion_parent },
	));
	let signed_4 = validator_id.sign(&ApprovalVote(candidate_hash).signing_payload(session));
	let signed_5 = validator_id.sign(
		&ExplicitDisputeStatement { valid: false, candidate_hash, session }.signing_payload(),
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
			let payload = ExplicitDisputeStatement { valid, candidate_hash: *c_hash, session }
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
				candidate_hash: candidate_hash_b,
				session: 2,
				statements: vec![explicit_triple_b.clone(), explicit_triple_b_bad.clone()],
			},
			// same session as above
			DisputeStatementSet {
				candidate_hash: candidate_hash_c,
				session: 2,
				statements: vec![explicit_triple_c, explicit_triple_c_bad],
			},
			// the duplicate set
			DisputeStatementSet {
				candidate_hash: candidate_hash_b,
				session: 2,
				statements: vec![explicit_triple_b.clone(), explicit_triple_b_bad.clone()],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_a,
				session: 1,
				statements: vec![explicit_triple_a, explicit_triple_a_bad],
			},
		];

		let disputes_orig = disputes.clone();

		<Pallet<Test> as DisputesHandler<BlockNumberFor<Test>>>::deduplicate_and_sort_dispute_data(
			&mut disputes,
		)
		.unwrap_err();

		// assert ordering of local only disputes, and at the same time, and being free of
		// duplicates
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
	let post_conclusion_acceptance_period = config.dispute_post_conclusion_acceptance_period;

	let mut acc = Vec::<CheckedDisputeStatementSet>::new();
	for dispute_statement in sets {
		if let Some(checked) =
			<Pallet<T> as DisputesHandler<BlockNumberFor<T>>>::filter_dispute_data(
				dispute_statement,
				post_conclusion_acceptance_period,
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

		let payload =
			ExplicitDisputeStatement { valid: true, candidate_hash, session: 1 }.signing_payload();

		let payload_against =
			ExplicitDisputeStatement { valid: false, candidate_hash, session: 1 }.signing_payload();

		let sig_a = v0.sign(&payload);
		let sig_b = v0.sign(&payload);
		let sig_c = v0.sign(&payload);
		let sig_d = v1.sign(&payload_against);

		let statements = DisputeStatementSet {
			candidate_hash,
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

		let post_conclusion_acceptance_period = 10;
		let statements =
			<Pallet<Test> as DisputesHandler<BlockNumberFor<Test>>>::filter_dispute_data(
				statements,
				post_conclusion_acceptance_period,
			);

		assert_eq!(
			statements,
			Some(CheckedDisputeStatementSet::unchecked_from_unchecked(DisputeStatementSet {
				candidate_hash,
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
			ExplicitDisputeStatement { valid, candidate_hash: *c_hash, session: 1 }
				.signing_payload()
		};

		let payload_a = payload(&candidate_hash_a, true);
		let payload_a_bad = payload(&candidate_hash_a, false);

		let sig_0 = v0.sign(&payload_a);
		let sig_1 = v1.sign(&payload_a_bad);

		let statements = vec![DisputeStatementSet {
			candidate_hash: candidate_hash_a,
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
fn filter_removes_session_out_of_bounds() {
	new_test_ext(Default::default()).execute_with(|| {
		let v0 = <ValidatorId as CryptoType>::Pair::generate().0;

		run_to_block(3, |b| {
			// a new session at each block
			Some((true, b, vec![(&0, v0.public())], Some(vec![(&0, v0.public())])))
		});

		let candidate_hash = CandidateHash(sp_core::H256::repeat_byte(1));

		let payload =
			ExplicitDisputeStatement { valid: true, candidate_hash, session: 1 }.signing_payload();

		let sig_a = v0.sign(&payload);

		let statements = vec![DisputeStatementSet {
			candidate_hash,
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

		let payload_a =
			ExplicitDisputeStatement { valid: true, candidate_hash: candidate_hash_a, session: 1 }
				.signing_payload();

		let payload_b =
			ExplicitDisputeStatement { valid: true, candidate_hash: candidate_hash_b, session: 1 }
				.signing_payload();

		let sig_a = v0.sign(&payload_a);
		let sig_b = v0.sign(&payload_b);

		let statements = vec![
			DisputeStatementSet {
				candidate_hash: candidate_hash_a,
				session: 1,
				statements: vec![(
					DisputeStatement::Valid(ValidDisputeStatementKind::Explicit),
					ValidatorIndex(0),
					sig_a,
				)],
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_b,
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
				candidate_hash: candidate_hash_b,
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

		let payload =
			ExplicitDisputeStatement { valid: true, candidate_hash: candidate_hash_a, session: 1 }
				.signing_payload();

		let payload_against =
			ExplicitDisputeStatement { valid: false, candidate_hash: candidate_hash_a, session: 1 }
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
				candidate_hash: candidate_hash_a,
				session: 1,
				statements: statements.clone(),
			},
			DisputeStatementSet {
				candidate_hash: candidate_hash_a,
				session: 1,
				statements: statements.clone(),
			},
		];

		// `Err(())` indicates presence of duplicates
		assert!(<Pallet::<Test> as DisputesHandler<
			BlockNumberFor<Test>,
		>>::deduplicate_and_sort_dispute_data(&mut sets)
		.is_err());

		assert_eq!(
			sets,
			vec![DisputeStatementSet { candidate_hash: candidate_hash_a, session: 1, statements }]
		);
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

		let payload =
			ExplicitDisputeStatement { valid: true, candidate_hash: candidate_hash_a, session: 1 }
				.signing_payload();

		let sig_a = v0.sign(&payload);

		let statements = vec![DisputeStatementSet {
			candidate_hash: candidate_hash_a,
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

		let payload =
			ExplicitDisputeStatement { valid: true, candidate_hash: candidate_hash_a, session: 1 }
				.signing_payload();

		let sig_a = v0.sign(&payload);

		let statements = vec![DisputeStatementSet {
			candidate_hash: candidate_hash_a,
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
