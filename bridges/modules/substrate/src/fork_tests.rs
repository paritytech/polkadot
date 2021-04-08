// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Tests for checking that behaviour of importing headers and finality proofs works correctly.
//!
//! The tests are built around the idea that we will be importing headers on different forks and we
//! should be able to check that we're correctly importing headers, scheduling changes, and
//! finalizing headers across different forks.
//!
//! Each test is depicted using beautiful ASCII art. The symbols used in the tests are the
//! following:
//!
//! - S|N: Schedules change in N blocks
//! - E: Enacts change
//! - F: Finalized
//! - FN: Finality proof imported for header N
//!
//! Each diagram also comes with an import order. This is important since we expect things to fail
//! when headers or proofs are imported in a certain order.
//!
//! Tests can be read as follows:
//!
//! ## Example Import 1
//!
//! (Type::Header(2, 1, None, None), Ok(()))
//!
//! Import header 2 on fork 1. This does not create a fork, or schedule an authority set change. We
//! expect this header import to be succesful.
//!
//! ## Example Import 2
//!
//! (Type::Header(4, 2, Some((3, 1)), Some(0)), Ok(()))
//!
//! Import header 4 on fork 2. This header starts a new fork from header 3 on fork 1. It also
//! schedules a change with a delay of 0 blocks. It should be succesfully imported.
//!
//! ## Example Import 3
//!
//! (Type::Finality(2, 1), Err(FinalizationError::OldHeader.into()))
//!
//! Import a finality proof for header 2 on fork 1. This finalty proof should fail to be imported
//! because the header is an old header.

use crate::mock::*;
use crate::storage::ImportedHeader;
use crate::verifier::*;
use crate::{BestFinalized, BestHeight, BridgeStorage, NextScheduledChange, PalletStorage};
use bp_header_chain::AuthoritySet;
use bp_test_utils::{alice, authority_list, bob, make_justification_for_header};
use codec::Encode;
use frame_support::{IterableStorageMap, StorageValue};
use sp_finality_grandpa::{ConsensusLog, GRANDPA_ENGINE_ID};
use sp_runtime::{Digest, DigestItem};
use std::collections::BTreeMap;

type ForkId = u64;
type Delay = u64;

// Indicates when to start a new fork. The first item in the tuple
// will be the parent header of the header starting this fork.
type ForksAt = Option<(TestNumber, ForkId)>;
type ScheduledChangeAt = Option<Delay>;

#[derive(Debug)]
enum Type {
	Header(TestNumber, ForkId, ForksAt, ScheduledChangeAt),
	Finality(TestNumber, ForkId),
}

// Order: 1, 2, 2', 3, 3''
//
//          / [3'']
//    / [2']
// [1] <- [2] <- [3]
#[test]
fn fork_can_import_headers_on_different_forks() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, None), Ok(())),
			(Type::Header(2, 2, Some((1, 1)), None), Ok(())),
			(Type::Header(3, 1, None, None), Ok(())),
			(Type::Header(3, 3, Some((2, 2)), None), Ok(())),
		];

		create_chain(&mut storage, &mut chain);

		let best_headers = storage.best_headers();
		assert_eq!(best_headers.len(), 2);
		assert_eq!(<BestHeight<TestRuntime>>::get(), 3);
	})
}

// Order: 1, 2, 2', F2, F2'
//
// [1] <- [2: F]
//   \ [2']
//
// Not allowed to finalize 2'
#[test]
fn fork_does_not_allow_competing_finality_proofs() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, None), Ok(())),
			(Type::Header(2, 2, Some((1, 1)), None), Ok(())),
			(Type::Finality(2, 1), Ok(())),
			(Type::Finality(2, 2), Err(FinalizationError::OldHeader.into())),
		];

		create_chain(&mut storage, &mut chain);
	})
}

// Order: 1, 2, 3, F2, 3
//
// [1] <- [2: S|0] <- [3]
//
// Not allowed to import 3 until we get F2
//
// Note: GRANDPA would technically allow 3 to be imported as long as it didn't try and enact an
// authority set change. However, since we expect finality proofs to be imported quickly we've
// decided to simplify our import process and disallow header imports until we get a finality proof.
#[test]
fn fork_waits_for_finality_proof_before_importing_header_past_one_which_enacts_a_change() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, Some(0)), Ok(())),
			(
				Type::Header(3, 1, None, None),
				Err(ImportError::AwaitingFinalityProof.into()),
			),
			(Type::Finality(2, 1), Ok(())),
			(Type::Header(3, 1, None, None), Ok(())),
		];

		create_chain(&mut storage, &mut chain);
	})
}

// Order: 1, 2, F2, 3
//
// [1] <- [2: S|1] <- [3: S|0]
//
// GRANDPA can have multiple authority set changes pending on the same fork. However, we've decided
// to introduce a limit of _one_ pending authority set change per fork in order to simplify pallet
// logic and to prevent DoS attacks if GRANDPA finality were to temporarily stall for a long time
// (we'd have to perform a lot of expensive ancestry checks to catch back up).
#[test]
fn fork_does_not_allow_multiple_scheduled_changes_on_the_same_fork() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, Some(1)), Ok(())),
			(
				Type::Header(3, 1, None, Some(0)),
				Err(ImportError::PendingAuthoritySetChange.into()),
			),
			(Type::Finality(2, 1), Ok(())),
			(Type::Header(3, 1, None, Some(0)), Ok(())),
		];

		create_chain(&mut storage, &mut chain);
	})
}

// Order: 1, 2, 2'
//
//   / [2': S|0]
// [1] <- [2: S|0]
//
// Both 2 and 2' should be marked as needing justifications since they enact changes.
#[test]
fn fork_correctly_tracks_which_headers_require_finality_proofs() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, Some(0)), Ok(())),
			(Type::Header(2, 2, Some((1, 1)), Some(0)), Ok(())),
		];

		create_chain(&mut storage, &mut chain);

		let header_ids = storage.missing_justifications();
		assert_eq!(header_ids.len(), 2);
		assert!(header_ids[0].hash != header_ids[1].hash);
		assert_eq!(header_ids[0].number, 2);
		assert_eq!(header_ids[1].number, 2);
	})
}

// Order: 1, 2, 2', 3', F2, 3, 4'
//
//   / [2': S|1] <- [3'] <- [4']
// [1] <- [2: S|0] <- [3]
//
//
// Not allowed to import 3 or 4'
// Can only import 3 after we get the finality proof for 2
#[test]
fn fork_does_not_allow_importing_past_header_that_enacts_changes_on_forks() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, Some(0)), Ok(())),
			(Type::Header(2, 2, Some((1, 1)), Some(1)), Ok(())),
			(
				Type::Header(3, 1, None, None),
				Err(ImportError::AwaitingFinalityProof.into()),
			),
			(Type::Header(3, 2, None, None), Ok(())),
			(Type::Finality(2, 1), Ok(())),
			(Type::Header(3, 1, None, None), Ok(())),
			(
				Type::Header(4, 2, None, None),
				Err(ImportError::AwaitingFinalityProof.into()),
			),
		];

		create_chain(&mut storage, &mut chain);

		// Since we can't query the map directly to check if we applied the right authority set
		// change (we don't know the header hash of 2) we need to get a little clever.
		let mut next_change = <NextScheduledChange<TestRuntime>>::iter();
		let (_, scheduled_change_on_fork) = next_change.next().unwrap();
		assert_eq!(scheduled_change_on_fork.height, 3);

		// Sanity check to make sure we enacted the change on the canonical change
		assert_eq!(next_change.next(), None);
	})
}

// Order: 1, 2, 3, 2', 3'
//
//   / [2'] <- [3']
// [1] <- [2: S|0] <- [3]
//
// Not allowed to import 3
// Fine to import 2' and 3'
#[test]
fn fork_allows_importing_on_different_fork_while_waiting_for_finality_proof() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, Some(0)), Ok(())),
			(
				Type::Header(3, 1, None, None),
				Err(ImportError::AwaitingFinalityProof.into()),
			),
			(Type::Header(2, 2, Some((1, 1)), None), Ok(())),
			(Type::Header(3, 2, None, None), Ok(())),
		];

		create_chain(&mut storage, &mut chain);
	})
}

// Order: 1, 2, 2', F2, 3, 3'
//
//   / [2'] <- [3']
// [1] <- [2: F] <- [3]
//
// In our current implementation we're allowed to keep building on fork 2 for as long as our hearts'
// content. However, we'll never be able to finalize anything on that fork. We'd have to check for
// ancestry with `best_finalized` on every import which will get expensive.
//
// I think this is fine as long as we run pruning every so often to clean up these dead forks.
#[test]
fn fork_allows_importing_on_different_fork_past_finalized_header() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, Some(0)), Ok(())),
			(Type::Header(2, 2, Some((1, 1)), None), Ok(())),
			(Type::Finality(2, 1), Ok(())),
			(Type::Header(3, 1, None, None), Ok(())),
			(Type::Header(3, 2, None, None), Ok(())),
		];

		create_chain(&mut storage, &mut chain);
	})
}

// Order: 1, 2, 3, 4, 3', 4'
//
//                  / [3': E] <- [4']
// [1] <- [2: S|1] <- [3: E] <- [4]
//
// Not allowed to import {4|4'}
#[test]
fn fork_can_track_scheduled_changes_across_forks() {
	run_test(|| {
		let mut storage = PalletStorage::<TestRuntime>::new();

		let mut chain = vec![
			(Type::Header(1, 1, None, None), Ok(())),
			(Type::Header(2, 1, None, Some(1)), Ok(())),
			(Type::Header(3, 1, None, None), Ok(())),
			(
				Type::Header(4, 1, None, None),
				Err(ImportError::AwaitingFinalityProof.into()),
			),
			(Type::Header(3, 2, Some((2, 1)), None), Ok(())),
			(
				Type::Header(4, 2, None, None),
				Err(ImportError::AwaitingFinalityProof.into()),
			),
		];

		create_chain(&mut storage, &mut chain);
	})
}

#[derive(Debug, PartialEq)]
enum TestError {
	Import(ImportError),
	Finality(FinalizationError),
}

impl From<ImportError> for TestError {
	fn from(e: ImportError) -> Self {
		TestError::Import(e)
	}
}

impl From<FinalizationError> for TestError {
	fn from(e: FinalizationError) -> Self {
		TestError::Finality(e)
	}
}

// Builds a fork-aware representation of a blockchain given a list of headers.
//
// Takes a list of headers and finality proof operations which will be applied in order. The
// expected outcome for each operation is also required.
//
// The first header in the list will be used as the genesis header and will be manually imported
// into storage.
fn create_chain<S>(storage: &mut S, chain: &mut Vec<(Type, Result<(), TestError>)>)
where
	S: BridgeStorage<Header = TestHeader> + Clone,
{
	let mut map = BTreeMap::new();
	let mut verifier = Verifier {
		storage: storage.clone(),
	};
	initialize_genesis(storage, &mut map, chain.remove(0).0);

	for h in chain {
		match h {
			(Type::Header(num, fork_id, does_fork, schedules_change), expected_result) => {
				// If we've never seen this fork before
				if !map.contains_key(&fork_id) {
					// Let's get the info about where to start the fork
					if let Some((parent_num, forked_from_id)) = does_fork {
						let fork = &*map.get(&forked_from_id).unwrap();
						let parent = fork
							.iter()
							.find(|h| h.number == *parent_num)
							.expect("Trying to fork on a parent which doesn't exist");

						let mut header = test_header(*num);
						header.parent_hash = parent.hash();
						header.state_root = [*fork_id as u8; 32].into();

						if let Some(delay) = schedules_change {
							header.digest = change_log(*delay);
						}

						// Try and import into storage
						let res = verifier
							.import_header(header.hash(), header.clone())
							.map_err(TestError::Import);
						assert_eq!(
							res, *expected_result,
							"Expected {:?} while importing header ({}, {}), got {:?}",
							*expected_result, *num, *fork_id, res,
						);

						// Let's mark the header down in a new fork
						if res.is_ok() {
							map.insert(*fork_id, vec![header]);
						}
					}
				} else {
					// We've seen this fork before so let's append our new header to it
					let parent_hash = {
						let fork = &*map.get(&fork_id).unwrap();
						fork.last().unwrap().hash()
					};

					let mut header = test_header(*num);
					header.parent_hash = parent_hash;

					// Doing this to make sure headers at the same height but on
					// different forks have different hashes
					header.state_root = [*fork_id as u8; 32].into();

					if let Some(delay) = schedules_change {
						header.digest = change_log(*delay);
					}

					let res = verifier
						.import_header(header.hash(), header.clone())
						.map_err(TestError::Import);
					assert_eq!(
						res, *expected_result,
						"Expected {:?} while importing header ({}, {}), got {:?}",
						*expected_result, *num, *fork_id, res,
					);

					if res.is_ok() {
						map.get_mut(&fork_id).unwrap().push(header);
					}
				}
			}
			(Type::Finality(num, fork_id), expected_result) => {
				let header = map[fork_id]
					.iter()
					.find(|h| h.number == *num)
					.expect("Trying to finalize block that doesn't exist");

				// This is technically equivocating (accepting the same justification on the same
				// `grandpa_round`).
				//
				// See for more: https://github.com/paritytech/parity-bridges-common/issues/430
				let grandpa_round = 1;
				let set_id = 1;
				let authorities = authority_list();
				let justification = make_justification_for_header(header, grandpa_round, set_id, &authorities).encode();

				let res = verifier
					.import_finality_proof(header.hash(), justification.into())
					.map_err(TestError::Finality);
				assert_eq!(
					res, *expected_result,
					"Expected {:?} while importing finality proof for header ({}, {}), got {:?}",
					*expected_result, *num, *fork_id, res,
				);
			}
		}
	}

	for (key, value) in map.iter() {
		println!("{}: {:#?}", key, value);
	}
}

fn initialize_genesis<S>(storage: &mut S, map: &mut BTreeMap<TestNumber, Vec<TestHeader>>, genesis: Type)
where
	S: BridgeStorage<Header = TestHeader>,
{
	if let Type::Header(num, fork_id, None, None) = genesis {
		let genesis = test_header(num);
		map.insert(fork_id, vec![genesis.clone()]);

		let genesis = ImportedHeader {
			header: genesis,
			requires_justification: false,
			is_finalized: true,
			signal_hash: None,
		};

		<BestFinalized<TestRuntime>>::put(genesis.hash());
		storage.write_header(&genesis);
	} else {
		panic!("Unexpected genesis block format {:#?}", genesis)
	}

	let set_id = 1;
	let authorities = authority_list();
	let authority_set = AuthoritySet::new(authorities, set_id);
	storage.update_current_authority_set(authority_set);
}

pub(crate) fn change_log(delay: u64) -> Digest<TestHash> {
	let consensus_log = ConsensusLog::<TestNumber>::ScheduledChange(sp_finality_grandpa::ScheduledChange {
		next_authorities: vec![(alice(), 1), (bob(), 1)],
		delay,
	});

	Digest::<TestHash> {
		logs: vec![DigestItem::Consensus(GRANDPA_ENGINE_ID, consensus_log.encode())],
	}
}
