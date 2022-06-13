// Copyright 2020-2021 Parity Technologies (UK) Ltd.
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

//! Tests inside this module are made to ensure that our custom justification verification
//! implementation works exactly as `fn finality_grandpa::validate_commit`.
//!
//! Some of tests in this module may partially duplicate tests from `justification.rs`,
//! but their purpose is different.

use bp_header_chain::justification::{verify_justification, Error, GrandpaJustification};
use bp_test_utils::{
	header_id, make_justification_for_header, signed_precommit, test_header, Account,
	JustificationGeneratorParams, ALICE, BOB, CHARLIE, DAVE, EVE, TEST_GRANDPA_SET_ID,
};
use finality_grandpa::voter_set::VoterSet;
use sp_finality_grandpa::{AuthorityId, AuthorityWeight};
use sp_runtime::traits::Header as HeaderT;

type TestHeader = sp_runtime::testing::Header;
type TestHash = <TestHeader as HeaderT>::Hash;
type TestNumber = <TestHeader as HeaderT>::Number;

/// Implementation of `finality_grandpa::Chain` that is used in tests.
struct AncestryChain(bp_header_chain::justification::AncestryChain<TestHeader>);

impl AncestryChain {
	fn new(ancestry: &[TestHeader]) -> Self {
		Self(bp_header_chain::justification::AncestryChain::new(ancestry))
	}
}

impl finality_grandpa::Chain<TestHash, TestNumber> for AncestryChain {
	fn ancestry(
		&self,
		base: TestHash,
		block: TestHash,
	) -> Result<Vec<TestHash>, finality_grandpa::Error> {
		let mut route = Vec::new();
		let mut current_hash = block;
		loop {
			if current_hash == base {
				break
			}
			match self.0.parents.get(&current_hash).cloned() {
				Some(parent_hash) => {
					current_hash = parent_hash;
					route.push(current_hash);
				},
				_ => return Err(finality_grandpa::Error::NotDescendent),
			}
		}
		route.pop(); // remove the base

		Ok(route)
	}
}

/// Get a full set of accounts.
fn full_accounts_set() -> Vec<(Account, AuthorityWeight)> {
	vec![(ALICE, 1), (BOB, 1), (CHARLIE, 1), (DAVE, 1), (EVE, 1)]
}

/// Get a full set of GRANDPA authorities.
fn full_voter_set() -> VoterSet<AuthorityId> {
	VoterSet::new(full_accounts_set().iter().map(|(id, w)| (AuthorityId::from(*id), *w))).unwrap()
}

/// Get a minimal set of accounts.
fn minimal_accounts_set() -> Vec<(Account, AuthorityWeight)> {
	// there are 5 accounts in the full set => we need 2/3 + 1 accounts, which results in 4 accounts
	vec![(ALICE, 1), (BOB, 1), (CHARLIE, 1), (DAVE, 1)]
}

/// Get a minimal subset of GRANDPA authorities that have enough cumulative vote weight to justify a
/// header finality.
pub fn minimal_voter_set() -> VoterSet<AuthorityId> {
	VoterSet::new(minimal_accounts_set().iter().map(|(id, w)| (AuthorityId::from(*id), *w)))
		.unwrap()
}

/// Make a valid GRANDPA justification with sensible defaults.
pub fn make_default_justification(header: &TestHeader) -> GrandpaJustification<TestHeader> {
	make_justification_for_header(JustificationGeneratorParams {
		header: header.clone(),
		authorities: minimal_accounts_set(),
		..Default::default()
	})
}

// the `finality_grandpa::validate_commit` function has two ways to report an unsuccessful
// commit validation:
//
// 1) to return `Err()` (which only may happen if `finality_grandpa::Chain` implementation
//    returns an error);
// 2) to return `Ok(validation_result)` if `validation_result.is_valid()` is false.
//
// Our implementation would just return error in both cases.

#[test]
fn same_result_when_precommit_target_has_lower_number_than_commit_target() {
	let mut justification = make_default_justification(&test_header(1));
	// the number of header in precommit (0) is lower than number of header in commit (1)
	justification.commit.precommits[0].precommit.target_number = 0;

	// our implementation returns an error
	assert_eq!(
		verify_justification::<TestHeader>(
			header_id::<TestHeader>(1),
			TEST_GRANDPA_SET_ID,
			&full_voter_set(),
			&justification,
		),
		Err(Error::PrecommitIsNotCommitDescendant),
	);

	// original implementation returns `Ok(validation_result)`
	// with `validation_result.is_valid() == false`.
	let result = finality_grandpa::validate_commit(
		&justification.commit,
		&full_voter_set(),
		&AncestryChain::new(&justification.votes_ancestries),
	)
	.unwrap();

	assert!(!result.is_valid());
}

#[test]
fn same_result_when_precommit_target_is_not_descendant_of_commit_target() {
	let not_descendant = test_header::<TestHeader>(10);
	let mut justification = make_default_justification(&test_header(1));
	// the route from header of commit (1) to header of precommit (10) is missing from
	// the votes ancestries
	justification.commit.precommits[0].precommit.target_number = *not_descendant.number();
	justification.commit.precommits[0].precommit.target_hash = not_descendant.hash();
	justification.votes_ancestries.push(not_descendant);

	// our implementation returns an error
	assert_eq!(
		verify_justification::<TestHeader>(
			header_id::<TestHeader>(1),
			TEST_GRANDPA_SET_ID,
			&full_voter_set(),
			&justification,
		),
		Err(Error::PrecommitIsNotCommitDescendant),
	);

	// original implementation returns `Ok(validation_result)`
	// with `validation_result.is_valid() == false`.
	let result = finality_grandpa::validate_commit(
		&justification.commit,
		&full_voter_set(),
		&AncestryChain::new(&justification.votes_ancestries),
	)
	.unwrap();

	assert!(!result.is_valid());
}

#[test]
fn same_result_when_justification_contains_duplicate_vote() {
	let mut justification = make_justification_for_header(JustificationGeneratorParams {
		header: test_header(1),
		authorities: minimal_accounts_set(),
		ancestors: 0,
		..Default::default()
	});

	// the justification may contain exactly the same vote (i.e. same precommit and same signature)
	// multiple times && it isn't treated as an error by original implementation
	justification.commit.precommits.push(justification.commit.precommits[0].clone());
	justification.commit.precommits.push(justification.commit.precommits[0].clone());

	// our implementation succeeds
	assert_eq!(
		verify_justification::<TestHeader>(
			header_id::<TestHeader>(1),
			TEST_GRANDPA_SET_ID,
			&full_voter_set(),
			&justification,
		),
		Ok(()),
	);

	// original implementation returns `Ok(validation_result)`
	// with `validation_result.is_valid() == true`.
	let result = finality_grandpa::validate_commit(
		&justification.commit,
		&full_voter_set(),
		&AncestryChain::new(&justification.votes_ancestries),
	)
	.unwrap();

	assert!(result.is_valid());
}

#[test]
fn same_result_when_authority_equivocates_once_in_a_round() {
	let mut justification = make_justification_for_header(JustificationGeneratorParams {
		header: test_header(1),
		authorities: minimal_accounts_set(),
		ancestors: 0,
		..Default::default()
	});

	// the justification original implementation allows authority to submit two different
	// votes in a single round, of which only first is 'accepted'
	justification.commit.precommits.push(signed_precommit::<TestHeader>(
		&ALICE,
		header_id::<TestHeader>(1),
		justification.round,
		TEST_GRANDPA_SET_ID,
	));

	// our implementation succeeds
	assert_eq!(
		verify_justification::<TestHeader>(
			header_id::<TestHeader>(1),
			TEST_GRANDPA_SET_ID,
			&full_voter_set(),
			&justification,
		),
		Ok(()),
	);

	// original implementation returns `Ok(validation_result)`
	// with `validation_result.is_valid() == true`.
	let result = finality_grandpa::validate_commit(
		&justification.commit,
		&full_voter_set(),
		&AncestryChain::new(&justification.votes_ancestries),
	)
	.unwrap();

	assert!(result.is_valid());
}

#[test]
fn same_result_when_authority_equivocates_twice_in_a_round() {
	let mut justification = make_justification_for_header(JustificationGeneratorParams {
		header: test_header(1),
		authorities: minimal_accounts_set(),
		ancestors: 0,
		..Default::default()
	});

	// there's some code in the original implementation that should return an error when
	// same authority submits more than two different votes in a single round:
	// https://github.com/paritytech/finality-grandpa/blob/6aeea2d1159d0f418f0b86e70739f2130629ca09/src/lib.rs#L473
	// but there's also a code that prevents this from happening:
	// https://github.com/paritytech/finality-grandpa/blob/6aeea2d1159d0f418f0b86e70739f2130629ca09/src/round.rs#L287
	// => so now we are also just ignoring all votes from the same authority, except the first one
	justification.commit.precommits.push(signed_precommit::<TestHeader>(
		&ALICE,
		header_id::<TestHeader>(1),
		justification.round,
		TEST_GRANDPA_SET_ID,
	));
	justification.commit.precommits.push(signed_precommit::<TestHeader>(
		&ALICE,
		header_id::<TestHeader>(1),
		justification.round,
		TEST_GRANDPA_SET_ID,
	));

	// our implementation succeeds
	assert_eq!(
		verify_justification::<TestHeader>(
			header_id::<TestHeader>(1),
			TEST_GRANDPA_SET_ID,
			&full_voter_set(),
			&justification,
		),
		Ok(()),
	);

	// original implementation returns `Ok(validation_result)`
	// with `validation_result.is_valid() == true`.
	let result = finality_grandpa::validate_commit(
		&justification.commit,
		&full_voter_set(),
		&AncestryChain::new(&justification.votes_ancestries),
	)
	.unwrap();

	assert!(result.is_valid());
}

#[test]
fn same_result_when_there_are_not_enough_cumulative_weight_to_finalize_commit_target() {
	// just remove one authority from the minimal set and we shall not reach the threshold
	let mut authorities_set = minimal_accounts_set();
	authorities_set.pop();
	let justification = make_justification_for_header(JustificationGeneratorParams {
		header: test_header(1),
		authorities: authorities_set,
		..Default::default()
	});

	// our implementation returns an error
	assert_eq!(
		verify_justification::<TestHeader>(
			header_id::<TestHeader>(1),
			TEST_GRANDPA_SET_ID,
			&full_voter_set(),
			&justification,
		),
		Err(Error::TooLowCumulativeWeight),
	);

	// original implementation returns `Ok(validation_result)`
	// with `validation_result.is_valid() == false`.
	let result = finality_grandpa::validate_commit(
		&justification.commit,
		&full_voter_set(),
		&AncestryChain::new(&justification.votes_ancestries),
	)
	.unwrap();

	assert!(!result.is_valid());
}
