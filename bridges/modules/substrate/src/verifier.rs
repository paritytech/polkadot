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

//! The verifier's role is to check the validity of headers being imported, and also determine if
//! they can be finalized.
//!
//! When importing headers it performs checks to ensure that no invariants are broken (like
//! importing the same header twice). When it imports finality proofs it will ensure that the proof
//! has been signed off by the correct GRANDPA authorities, and also enact any authority set changes
//! if required.

use crate::storage::{ImportedHeader, ScheduledChange};
use crate::BridgeStorage;

use bp_header_chain::{justification::verify_justification, AuthoritySet};
use finality_grandpa::voter_set::VoterSet;
use sp_finality_grandpa::{ConsensusLog, GRANDPA_ENGINE_ID};
use sp_runtime::generic::OpaqueDigestItemId;
use sp_runtime::traits::{CheckedAdd, Header as HeaderT, One};
use sp_runtime::RuntimeDebug;
use sp_std::{prelude::Vec, vec};

/// The finality proof used by the pallet.
///
/// For a Substrate based chain using GRANDPA this will
/// be an encoded GRANDPA Justification.
#[derive(RuntimeDebug)]
pub struct FinalityProof(Vec<u8>);

impl From<&[u8]> for FinalityProof {
	fn from(proof: &[u8]) -> Self {
		Self(proof.to_vec())
	}
}

impl From<Vec<u8>> for FinalityProof {
	fn from(proof: Vec<u8>) -> Self {
		Self(proof)
	}
}

/// Errors which can happen while importing a header.
#[derive(RuntimeDebug, PartialEq)]
pub enum ImportError {
	/// This header is at the same height or older than our latest finalized block, thus not useful.
	OldHeader,
	/// This header has already been imported by the pallet.
	HeaderAlreadyExists,
	/// We're missing a parent for this header.
	MissingParent,
	/// The number of the header does not follow its parent's number.
	InvalidChildNumber,
	/// The height of the next authority set change overflowed.
	ScheduledHeightOverflow,
	/// Received an authority set which was invalid in some way, such as
	/// the authority weights being empty or overflowing the `AuthorityWeight`
	/// type.
	InvalidAuthoritySet,
	/// This header is not allowed to be imported since an ancestor requires a finality proof.
	///
	/// This can happen if an ancestor is supposed to enact an authority set change.
	AwaitingFinalityProof,
	/// This header schedules an authority set change even though we're still waiting
	/// for an old authority set change to be enacted on this fork.
	PendingAuthoritySetChange,
}

/// Errors which can happen while verifying a headers finality.
#[derive(RuntimeDebug, PartialEq)]
pub enum FinalizationError {
	/// This header has never been imported by the pallet.
	UnknownHeader,
	/// Trying to prematurely import a justification
	PrematureJustification,
	/// We failed to verify this header's ancestry.
	AncestryCheckFailed,
	/// This header is at the same height or older than our latest finalized block, thus not useful.
	OldHeader,
	/// The given justification was not able to finalize the given header.
	///
	/// There are several reasons why this might happen, such as the justification being
	/// signed by the wrong authority set, being given alongside an unexpected header,
	/// or failing ancestry checks.
	InvalidJustification,
}

/// Used to verify imported headers and their finality status.
#[derive(RuntimeDebug)]
pub struct Verifier<S> {
	pub storage: S,
}

impl<S, H> Verifier<S>
where
	S: BridgeStorage<Header = H>,
	H: HeaderT,
	H::Number: finality_grandpa::BlockNumberOps,
{
	/// Import a header to the pallet.
	///
	/// Will perform some basic checks to make sure that this header doesn't break any assumptions
	/// such as being on a different finalized fork.
	pub fn import_header(&mut self, hash: H::Hash, header: H) -> Result<(), ImportError> {
		let best_finalized = self.storage.best_finalized_header();

		if header.number() <= best_finalized.number() {
			return Err(ImportError::OldHeader);
		}

		if self.storage.header_exists(hash) {
			return Err(ImportError::HeaderAlreadyExists);
		}

		let parent_header = self
			.storage
			.header_by_hash(*header.parent_hash())
			.ok_or(ImportError::MissingParent)?;

		let parent_number = *parent_header.number();
		if parent_number + One::one() != *header.number() {
			return Err(ImportError::InvalidChildNumber);
		}

		// A header requires a justification if it enacts an authority set change. We don't
		// need to act on it right away (we'll update the set once the header gets finalized), but
		// we need to make a note of it.
		//
		// Note: This assumes that we can only have one authority set change pending per fork at a
		// time. While this is not strictly true of GRANDPA (it can have multiple pending changes,
		// even across forks), this assumption simplifies our tracking of authority set changes.
		let mut signal_hash = parent_header.signal_hash;
		let scheduled_change = find_scheduled_change(&header);

		// Check if our fork is expecting an authority set change
		let requires_justification = if let Some(hash) = signal_hash {
			const PROOF: &str = "If the header has a signal hash it means there's an accompanying set
							change in storage, therefore this must always be valid.";
			let pending_change = self.storage.scheduled_set_change(hash).expect(PROOF);

			if scheduled_change.is_some() {
				return Err(ImportError::PendingAuthoritySetChange);
			}

			if *header.number() > pending_change.height {
				return Err(ImportError::AwaitingFinalityProof);
			}

			pending_change.height == *header.number()
		} else {
			// Since we don't currently have a pending authority set change let's check if the header
			// contains a log indicating when the next change should be.
			if let Some(change) = scheduled_change {
				let mut total_weight = 0u64;

				for (_id, weight) in &change.next_authorities {
					total_weight = total_weight
						.checked_add(*weight)
						.ok_or(ImportError::InvalidAuthoritySet)?;
				}

				// If none of the authorities have a weight associated with them the
				// set is essentially empty. We don't want that.
				if total_weight == 0 {
					return Err(ImportError::InvalidAuthoritySet);
				}

				let next_set = AuthoritySet {
					authorities: change.next_authorities,
					set_id: self.storage.current_authority_set().set_id + 1,
				};

				let height = (*header.number())
					.checked_add(&change.delay)
					.ok_or(ImportError::ScheduledHeightOverflow)?;

				let scheduled_change = ScheduledChange {
					authority_set: next_set,
					height,
				};

				// Note: It's important that the signal hash is updated if a header schedules a
				// change or else we end up with inconsistencies in other places.
				signal_hash = Some(hash);
				self.storage.schedule_next_set_change(hash, scheduled_change);

				// If the delay is 0 this header will enact the change it signaled
				height == *header.number()
			} else {
				false
			}
		};

		self.storage.write_header(&ImportedHeader {
			header,
			requires_justification,
			is_finalized: false,
			signal_hash,
		});

		Ok(())
	}

	/// Verify that a previously imported header can be finalized with the given GRANDPA finality
	/// proof. If the header enacts an authority set change the change will be applied once the
	/// header has been finalized.
	pub fn import_finality_proof(&mut self, hash: H::Hash, proof: FinalityProof) -> Result<(), FinalizationError> {
		// Make sure that we've previously imported this header
		let header = self
			.storage
			.header_by_hash(hash)
			.ok_or(FinalizationError::UnknownHeader)?;

		// We don't want to finalize an ancestor of an already finalized
		// header, this would be inconsistent
		let last_finalized = self.storage.best_finalized_header();
		if header.number() <= last_finalized.number() {
			return Err(FinalizationError::OldHeader);
		}

		let current_authority_set = self.storage.current_authority_set();
		let voter_set = VoterSet::new(current_authority_set.authorities).expect(
			"We verified the correctness of the authority list during header import,
			before writing them to storage. This must always be valid.",
		);
		verify_justification::<H>(
			(hash, *header.number()),
			current_authority_set.set_id,
			voter_set,
			&proof.0,
		)
		.map_err(|_| FinalizationError::InvalidJustification)?;
		frame_support::debug::trace!("Received valid justification for {:?}", header);

		frame_support::debug::trace!(
			"Checking ancestry for headers between {:?} and {:?}",
			last_finalized,
			header
		);
		let mut finalized_headers =
			if let Some(ancestors) = headers_between(&self.storage, last_finalized, header.clone()) {
				// Since we only try and finalize headers with a height strictly greater
				// than `best_finalized` if `headers_between` returns Some we must have
				// at least one element. If we don't something's gone wrong, so best
				// to die before we write to storage.
				assert_eq!(
					ancestors.is_empty(),
					false,
					"Empty ancestry list returned from `headers_between()`",
				);

				// Check if any of our ancestors `requires_justification` a.k.a schedule authority
				// set changes. If they're still waiting to be finalized we must reject this
				// justification. We don't include our current header in this check.
				//
				// We do this because it is important to to import justifications _in order_,
				// otherwise we risk finalizing headers on competing chains.
				let requires_justification = ancestors.iter().skip(1).find(|h| h.requires_justification);
				if requires_justification.is_some() {
					return Err(FinalizationError::PrematureJustification);
				}

				ancestors
			} else {
				return Err(FinalizationError::AncestryCheckFailed);
			};

		// If the current header was marked as `requires_justification` it means that it enacts a
		// new authority set change. When we finalize the header we need to update the current
		// authority set.
		if header.requires_justification {
			const SIGNAL_HASH_PROOF: &str = "When we import a header we only mark it as
			`requires_justification` if we have checked that it contains a signal hash. Therefore
			this must always be valid.";

			const ENACT_SET_PROOF: &str =
				"Headers must only be marked as `requires_justification` if there's a scheduled change in storage.";

			// If we are unable to enact an authority set it means our storage entry for scheduled
			// changes is missing. Best to crash since this is likely a bug.
			let _ = self
				.storage
				.enact_authority_set(header.signal_hash.expect(SIGNAL_HASH_PROOF))
				.expect(ENACT_SET_PROOF);
		}

		for header in finalized_headers.iter_mut() {
			header.is_finalized = true;
			header.requires_justification = false;
			header.signal_hash = None;
			self.storage.write_header(header);
		}

		self.storage.update_best_finalized(hash);

		Ok(())
	}
}

/// Returns the lineage of headers between [child, ancestor)
fn headers_between<S, H>(
	storage: &S,
	ancestor: ImportedHeader<H>,
	child: ImportedHeader<H>,
) -> Option<Vec<ImportedHeader<H>>>
where
	S: BridgeStorage<Header = H>,
	H: HeaderT,
{
	let mut ancestors = vec![];
	let mut current_header = child;

	while ancestor.hash() != current_header.hash() {
		// We've gotten to the same height and we're not related
		if ancestor.number() >= current_header.number() {
			return None;
		}

		let parent = storage.header_by_hash(*current_header.parent_hash());
		ancestors.push(current_header);
		current_header = match parent {
			Some(h) => h,
			None => return None,
		}
	}

	Some(ancestors)
}

pub(crate) fn find_scheduled_change<H: HeaderT>(header: &H) -> Option<sp_finality_grandpa::ScheduledChange<H::Number>> {
	let id = OpaqueDigestItemId::Consensus(&GRANDPA_ENGINE_ID);

	let filter_log = |log: ConsensusLog<H::Number>| match log {
		ConsensusLog::ScheduledChange(change) => Some(change),
		_ => None,
	};

	// find the first consensus digest with the right ID which converts to
	// the right kind of consensus log.
	header.digest().convert_first(|l| l.try_to(id).and_then(filter_log))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::*;
	use crate::{BestFinalized, BestHeight, HeaderId, ImportedHeaders, PalletStorage};
	use bp_test_utils::{alice, authority_list, bob, make_justification_for_header};
	use codec::Encode;
	use frame_support::{assert_err, assert_ok};
	use frame_support::{StorageMap, StorageValue};
	use sp_finality_grandpa::{AuthorityId, SetId};
	use sp_runtime::{Digest, DigestItem};

	fn schedule_next_change(
		authorities: Vec<AuthorityId>,
		set_id: SetId,
		height: TestNumber,
	) -> ScheduledChange<TestNumber> {
		let authorities = authorities.into_iter().map(|id| (id, 1u64)).collect();
		let authority_set = AuthoritySet::new(authorities, set_id);
		ScheduledChange { authority_set, height }
	}

	// Useful for quickly writing a chain of headers to storage
	// Input is expected in the form: vec![(num, requires_justification, is_finalized)]
	fn write_headers<S: BridgeStorage<Header = TestHeader>>(
		storage: &mut S,
		headers: Vec<(u64, bool, bool)>,
	) -> Vec<ImportedHeader<TestHeader>> {
		let mut imported_headers = vec![];
		let genesis = ImportedHeader {
			header: test_header(0),
			requires_justification: false,
			is_finalized: true,
			signal_hash: None,
		};

		<BestFinalized<TestRuntime>>::put(genesis.hash());
		storage.write_header(&genesis);
		imported_headers.push(genesis);

		for (num, requires_justification, is_finalized) in headers {
			let header = ImportedHeader {
				header: test_header(num),
				requires_justification,
				is_finalized,
				signal_hash: None,
			};

			storage.write_header(&header);
			imported_headers.push(header);
		}

		imported_headers
	}

	// Given a block number will generate a chain of headers which don't require justification and
	// are not considered to be finalized.
	fn write_default_headers<S: BridgeStorage<Header = TestHeader>>(
		storage: &mut S,
		headers: Vec<u64>,
	) -> Vec<ImportedHeader<TestHeader>> {
		let headers = headers.iter().map(|num| (*num, false, false)).collect();
		write_headers(storage, headers)
	}

	#[test]
	fn fails_to_import_old_header() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let parent = unfinalized_header(5);
			storage.write_header(&parent);
			storage.update_best_finalized(parent.hash());

			let header = test_header(1);
			let mut verifier = Verifier { storage };
			assert_err!(verifier.import_header(header.hash(), header), ImportError::OldHeader);
		})
	}

	#[test]
	fn fails_to_import_header_without_parent() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let parent = unfinalized_header(1);
			storage.write_header(&parent);
			storage.update_best_finalized(parent.hash());

			// By default the parent is `0x00`
			let header = TestHeader::new_from_number(2);

			let mut verifier = Verifier { storage };
			assert_err!(
				verifier.import_header(header.hash(), header),
				ImportError::MissingParent
			);
		})
	}

	#[test]
	fn fails_to_import_header_twice() {
		run_test(|| {
			let storage = PalletStorage::<TestRuntime>::new();
			let header = test_header(1);
			<BestFinalized<TestRuntime>>::put(header.hash());

			let imported_header = ImportedHeader {
				header: header.clone(),
				requires_justification: false,
				is_finalized: false,
				signal_hash: None,
			};
			<ImportedHeaders<TestRuntime>>::insert(header.hash(), &imported_header);

			let mut verifier = Verifier { storage };
			assert_err!(verifier.import_header(header.hash(), header), ImportError::OldHeader);
		})
	}

	#[test]
	fn succesfully_imports_valid_but_unfinalized_header() {
		run_test(|| {
			let storage = PalletStorage::<TestRuntime>::new();
			let parent = test_header(1);
			let parent_hash = parent.hash();
			<BestFinalized<TestRuntime>>::put(parent.hash());

			let imported_header = ImportedHeader {
				header: parent,
				requires_justification: false,
				is_finalized: true,
				signal_hash: None,
			};
			<ImportedHeaders<TestRuntime>>::insert(parent_hash, &imported_header);

			let header = test_header(2);
			let mut verifier = Verifier {
				storage: storage.clone(),
			};
			assert_ok!(verifier.import_header(header.hash(), header.clone()));

			let stored_header = storage
				.header_by_hash(header.hash())
				.expect("Should have been imported successfully");
			assert_eq!(stored_header.is_finalized, false);
			assert_eq!(stored_header.hash(), storage.best_headers()[0].hash);
		})
	}

	#[test]
	fn successfully_imports_two_different_headers_at_same_height() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();

			// We want to write the genesis header to storage
			let _ = write_headers(&mut storage, vec![]);

			// Both of these headers have the genesis header as their parent
			let header_on_fork1 = test_header(1);
			let mut header_on_fork2 = test_header(1);

			// We need to change _something_ to make it a different header
			header_on_fork2.state_root = [1; 32].into();

			let mut verifier = Verifier {
				storage: storage.clone(),
			};

			// It should be fine to import both
			assert_ok!(verifier.import_header(header_on_fork1.hash(), header_on_fork1.clone()));
			assert_ok!(verifier.import_header(header_on_fork2.hash(), header_on_fork2.clone()));

			// We should have two headers marked as being the best since they're
			// both at the same height
			let best_headers = storage.best_headers();
			assert_eq!(best_headers.len(), 2);
			assert_eq!(
				best_headers[0],
				HeaderId {
					number: *header_on_fork1.number(),
					hash: header_on_fork1.hash()
				}
			);
			assert_eq!(
				best_headers[1],
				HeaderId {
					number: *header_on_fork2.number(),
					hash: header_on_fork2.hash()
				}
			);
			assert_eq!(<BestHeight<TestRuntime>>::get(), 1);
		})
	}

	#[test]
	fn correctly_updates_the_best_header_given_a_better_header() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();

			// We want to write the genesis header to storage
			let _ = write_headers(&mut storage, vec![]);

			// Write two headers at the same height to storage.
			let best_header = test_header(1);
			let mut also_best_header = test_header(1);

			// We need to change _something_ to make it a different header
			also_best_header.state_root = [1; 32].into();

			let mut verifier = Verifier {
				storage: storage.clone(),
			};

			// It should be fine to import both
			assert_ok!(verifier.import_header(best_header.hash(), best_header.clone()));
			assert_ok!(verifier.import_header(also_best_header.hash(), also_best_header));

			// The headers we manually imported should have been marked as the best
			// upon writing to storage. Let's confirm that.
			assert_eq!(storage.best_headers().len(), 2);
			assert_eq!(<BestHeight<TestRuntime>>::get(), 1);

			// Now let's build something at a better height.
			let mut better_header = test_header(2);
			better_header.parent_hash = best_header.hash();

			assert_ok!(verifier.import_header(better_header.hash(), better_header.clone()));

			// Since `better_header` is the only one at height = 2 we should only have
			// a single "best header" now.
			let best_headers = storage.best_headers();
			assert_eq!(best_headers.len(), 1);
			assert_eq!(
				best_headers[0],
				HeaderId {
					number: *better_header.number(),
					hash: better_header.hash()
				}
			);
			assert_eq!(<BestHeight<TestRuntime>>::get(), 2);
		})
	}

	#[test]
	fn doesnt_write_best_header_twice_upon_finalization() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let _imported_headers = write_default_headers(&mut storage, vec![1]);

			let set_id = 1;
			let authorities = authority_list();
			let initial_authority_set = AuthoritySet::new(authorities.clone(), set_id);
			storage.update_current_authority_set(initial_authority_set);

			// Let's import our header
			let header = test_header(2);
			let mut verifier = Verifier {
				storage: storage.clone(),
			};
			assert_ok!(verifier.import_header(header.hash(), header.clone()));

			// Our header should be the only best header we have
			assert_eq!(storage.best_headers()[0].hash, header.hash());
			assert_eq!(storage.best_headers().len(), 1);

			// Now lets finalize our best header
			let grandpa_round = 1;
			let justification = make_justification_for_header(&header, grandpa_round, set_id, &authorities).encode();
			assert_ok!(verifier.import_finality_proof(header.hash(), justification.into()));

			// Our best header should only appear once in the list of best headers
			assert_eq!(storage.best_headers()[0].hash, header.hash());
			assert_eq!(storage.best_headers().len(), 1);
		})
	}

	#[test]
	fn related_headers_are_ancestors() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let mut imported_headers = write_default_headers(&mut storage, vec![1, 2, 3]);

			for header in imported_headers.iter() {
				assert!(storage.header_exists(header.hash()));
			}

			let ancestor = imported_headers.remove(0);
			let child = imported_headers.pop().unwrap();
			let ancestors = headers_between(&storage, ancestor, child);

			assert!(ancestors.is_some());
			assert_eq!(ancestors.unwrap().len(), 3);
		})
	}

	#[test]
	fn unrelated_headers_are_not_ancestors() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();

			let mut imported_headers = write_default_headers(&mut storage, vec![1, 2, 3]);
			for header in imported_headers.iter() {
				assert!(storage.header_exists(header.hash()));
			}

			// Need to give it a different parent_hash or else it'll be
			// related to our test genesis header
			let mut bad_ancestor = test_header(0);
			bad_ancestor.parent_hash = [1u8; 32].into();
			let bad_ancestor = ImportedHeader {
				header: bad_ancestor,
				requires_justification: false,
				is_finalized: false,
				signal_hash: None,
			};

			let child = imported_headers.pop().unwrap();
			let ancestors = headers_between(&storage, bad_ancestor, child);
			assert!(ancestors.is_none());
		})
	}

	#[test]
	fn ancestor_newer_than_child_is_not_related() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();

			let mut imported_headers = write_default_headers(&mut storage, vec![1, 2, 3]);
			for header in imported_headers.iter() {
				assert!(storage.header_exists(header.hash()));
			}

			// What if we have an "ancestor" that's newer than child?
			let new_ancestor = test_header(5);
			let new_ancestor = ImportedHeader {
				header: new_ancestor,
				requires_justification: false,
				is_finalized: false,
				signal_hash: None,
			};

			let child = imported_headers.pop().unwrap();
			let ancestors = headers_between(&storage, new_ancestor, child);
			assert!(ancestors.is_none());
		})
	}

	#[test]
	fn doesnt_import_header_which_schedules_change_with_invalid_authority_set() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let _imported_headers = write_default_headers(&mut storage, vec![1]);
			let mut header = test_header(2);

			// This is an *invalid* authority set because the combined weight of the
			// authorities is greater than `u64::MAX`
			let consensus_log = ConsensusLog::<TestNumber>::ScheduledChange(sp_finality_grandpa::ScheduledChange {
				next_authorities: vec![(alice(), u64::MAX), (bob(), u64::MAX)],
				delay: 0,
			});

			header.digest = Digest::<TestHash> {
				logs: vec![DigestItem::Consensus(GRANDPA_ENGINE_ID, consensus_log.encode())],
			};

			let mut verifier = Verifier { storage };

			assert_eq!(
				verifier.import_header(header.hash(), header).unwrap_err(),
				ImportError::InvalidAuthoritySet
			);
		})
	}

	#[test]
	fn finalizes_header_which_doesnt_enact_or_schedule_a_new_authority_set() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let _imported_headers = write_default_headers(&mut storage, vec![1]);

			// Nothing special about this header, yet GRANDPA may have created a justification
			// for it since it does that periodically
			let header = test_header(2);

			let set_id = 1;
			let authorities = authority_list();
			let authority_set = AuthoritySet::new(authorities.clone(), set_id);
			storage.update_current_authority_set(authority_set);

			// We'll need this justification to finalize the header
			let grandpa_round = 1;
			let justification = make_justification_for_header(&header, grandpa_round, set_id, &authorities).encode();

			let mut verifier = Verifier {
				storage: storage.clone(),
			};

			assert_ok!(verifier.import_header(header.hash(), header.clone()));
			assert_ok!(verifier.import_finality_proof(header.hash(), justification.into()));
			assert_eq!(storage.best_finalized_header().header, header);
		})
	}

	#[test]
	fn correctly_verifies_and_finalizes_chain_of_headers() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let imported_headers = write_default_headers(&mut storage, vec![1, 2]);
			let header = test_header(3);

			let set_id = 1;
			let authorities = authority_list();
			let authority_set = AuthoritySet {
				authorities: authorities.clone(),
				set_id,
			};
			storage.update_current_authority_set(authority_set);

			let grandpa_round = 1;
			let justification = make_justification_for_header(&header, grandpa_round, set_id, &authorities).encode();

			let mut verifier = Verifier {
				storage: storage.clone(),
			};
			assert!(verifier.import_header(header.hash(), header.clone()).is_ok());
			assert!(verifier
				.import_finality_proof(header.hash(), justification.into())
				.is_ok());

			// Make sure we marked the our headers as finalized
			assert!(storage.header_by_hash(imported_headers[1].hash()).unwrap().is_finalized);
			assert!(storage.header_by_hash(imported_headers[2].hash()).unwrap().is_finalized);
			assert!(storage.header_by_hash(header.hash()).unwrap().is_finalized);

			// Make sure the header at the highest height is the best finalized
			assert_eq!(storage.best_finalized_header().header, header);
		});
	}

	#[test]
	fn updates_authority_set_upon_finalizing_header_which_enacts_change() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let genesis_hash = write_headers(&mut storage, vec![])[0].hash();

			// We want this header to indicate that there's an upcoming set change on this fork
			let parent = ImportedHeader {
				header: test_header(1),
				requires_justification: false,
				is_finalized: false,
				signal_hash: Some(genesis_hash),
			};
			storage.write_header(&parent);

			let set_id = 1;
			let authorities = authority_list();
			let initial_authority_set = AuthoritySet::new(authorities.clone(), set_id);
			storage.update_current_authority_set(initial_authority_set);

			// This header enacts an authority set change upon finalization
			let header = test_header(2);

			let grandpa_round = 1;
			let justification = make_justification_for_header(&header, grandpa_round, set_id, &authorities).encode();

			// Schedule a change at the height of our header
			let set_id = 2;
			let height = *header.number();
			let authorities = vec![alice()];
			let change = schedule_next_change(authorities, set_id, height);
			storage.schedule_next_set_change(genesis_hash, change.clone());

			let mut verifier = Verifier {
				storage: storage.clone(),
			};

			assert_ok!(verifier.import_header(header.hash(), header.clone()));
			assert_eq!(storage.missing_justifications().len(), 1);
			assert_eq!(storage.missing_justifications()[0].hash, header.hash());

			assert_ok!(verifier.import_finality_proof(header.hash(), justification.into()));
			assert_eq!(storage.best_finalized_header().header, header);

			// Make sure that we have updated the set now that we've finalized our header
			assert_eq!(storage.current_authority_set(), change.authority_set);
			assert!(storage.missing_justifications().is_empty());
		})
	}

	#[test]
	fn importing_finality_proof_for_already_finalized_header_doesnt_work() {
		run_test(|| {
			let mut storage = PalletStorage::<TestRuntime>::new();
			let genesis = test_header(0);

			let genesis = ImportedHeader {
				header: genesis,
				requires_justification: false,
				is_finalized: true,
				signal_hash: None,
			};

			// Make sure that genesis is the best finalized header
			<BestFinalized<TestRuntime>>::put(genesis.hash());
			storage.write_header(&genesis);

			let mut verifier = Verifier { storage };

			// Now we want to try and import it again to see what happens
			assert_eq!(
				verifier
					.import_finality_proof(genesis.hash(), vec![4, 2].into())
					.unwrap_err(),
				FinalizationError::OldHeader
			);
		});
	}
}
