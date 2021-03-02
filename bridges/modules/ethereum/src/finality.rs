// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

use crate::error::Error;
use crate::Storage;
use bp_eth_poa::{public_to_address, Address, AuraHeader, HeaderId, SealedEmptyStep, H256};
use codec::{Decode, Encode};
use sp_io::crypto::secp256k1_ecdsa_recover;
use sp_runtime::RuntimeDebug;
use sp_std::collections::{
	btree_map::{BTreeMap, Entry},
	btree_set::BTreeSet,
	vec_deque::VecDeque,
};
use sp_std::prelude::*;

/// Cached finality votes for given block.
#[derive(RuntimeDebug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct CachedFinalityVotes<Submitter> {
	/// True if we have stopped at best finalized block' sibling. This means
	/// that we are trying to finalize block from fork that has forked before
	/// best finalized.
	pub stopped_at_finalized_sibling: bool,
	/// Header ancestors that were read while we have been searching for
	/// cached votes entry. Newest header has index 0.
	pub unaccounted_ancestry: VecDeque<(HeaderId, Option<Submitter>, AuraHeader)>,
	/// Cached finality votes, if they have been found. The associated
	/// header is not included into `unaccounted_ancestry`.
	pub votes: Option<FinalityVotes<Submitter>>,
}

/// Finality effects.
#[derive(RuntimeDebug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct FinalityEffects<Submitter> {
	/// Finalized headers.
	pub finalized_headers: Vec<(HeaderId, Option<Submitter>)>,
	/// Finality votes used in computation.
	pub votes: FinalityVotes<Submitter>,
}

/// Finality votes for given block.
#[derive(RuntimeDebug, Decode, Encode)]
#[cfg_attr(test, derive(Clone, PartialEq))]
pub struct FinalityVotes<Submitter> {
	/// Number of votes per each validator.
	pub votes: BTreeMap<Address, u64>,
	/// Ancestry blocks with oldest ancestors at the beginning and newest at the
	/// end of the queue.
	pub ancestry: VecDeque<FinalityAncestor<Submitter>>,
}

/// Information about block ancestor that is used in computations.
#[derive(RuntimeDebug, Decode, Encode)]
#[cfg_attr(test, derive(Clone, Default, PartialEq))]
pub struct FinalityAncestor<Submitter> {
	/// Bock id.
	pub id: HeaderId,
	/// Block submitter.
	pub submitter: Option<Submitter>,
	/// Validators that have signed this block and empty steps on top
	/// of this block.
	pub signers: BTreeSet<Address>,
}

/// Tries to finalize blocks when given block is imported.
///
/// Returns numbers and hashes of finalized blocks in ascending order.
pub fn finalize_blocks<S: Storage>(
	storage: &S,
	best_finalized: HeaderId,
	header_validators: (HeaderId, &[Address]),
	id: HeaderId,
	submitter: Option<&S::Submitter>,
	header: &AuraHeader,
	two_thirds_majority_transition: u64,
) -> Result<FinalityEffects<S::Submitter>, Error> {
	// compute count of voters for every unfinalized block in ancestry
	let validators = header_validators.1.iter().collect();
	let votes = prepare_votes(
		header
			.parent_id()
			.map(|parent_id| {
				storage.cached_finality_votes(&parent_id, &best_finalized, |hash| {
					*hash == header_validators.0.hash || *hash == best_finalized.hash
				})
			})
			.unwrap_or_default(),
		best_finalized,
		&validators,
		id,
		header,
		submitter.cloned(),
	)?;

	// now let's iterate in reverse order && find just finalized blocks
	let mut finalized_headers = Vec::new();
	let mut current_votes = votes.votes.clone();
	for ancestor in &votes.ancestry {
		if !is_finalized(
			&validators,
			&current_votes,
			ancestor.id.number >= two_thirds_majority_transition,
		) {
			break;
		}

		remove_signers_votes(&ancestor.signers, &mut current_votes);
		finalized_headers.push((ancestor.id, ancestor.submitter.clone()));
	}

	Ok(FinalityEffects {
		finalized_headers,
		votes,
	})
}

/// Returns true if there are enough votes to treat this header as finalized.
fn is_finalized(
	validators: &BTreeSet<&Address>,
	votes: &BTreeMap<Address, u64>,
	requires_two_thirds_majority: bool,
) -> bool {
	(!requires_two_thirds_majority && votes.len() * 2 > validators.len())
		|| (requires_two_thirds_majority && votes.len() * 3 > validators.len() * 2)
}

/// Prepare 'votes' of header and its ancestors' signers.
pub(crate) fn prepare_votes<Submitter>(
	mut cached_votes: CachedFinalityVotes<Submitter>,
	best_finalized: HeaderId,
	validators: &BTreeSet<&Address>,
	id: HeaderId,
	header: &AuraHeader,
	submitter: Option<Submitter>,
) -> Result<FinalityVotes<Submitter>, Error> {
	// if we have reached finalized block sibling, then we're trying
	// to switch finalized blocks
	if cached_votes.stopped_at_finalized_sibling {
		return Err(Error::TryingToFinalizeSibling);
	}

	// this fn can only work with single validators set
	if !validators.contains(&header.author) {
		return Err(Error::NotValidator);
	}

	// now we have votes that were valid when some block B has been inserted
	// things may have changed a bit, but we do not need to read anything else
	// from the db, because we have ancestry
	// so the only thing we need to do is:
	// 1) remove votes from blocks that have been finalized after B has been inserted;
	// 2) add votes from B descendants
	let mut votes = cached_votes.votes.unwrap_or_default();

	// remove votes from finalized blocks
	while let Some(old_ancestor) = votes.ancestry.pop_front() {
		if old_ancestor.id.number > best_finalized.number {
			votes.ancestry.push_front(old_ancestor);
			break;
		}

		remove_signers_votes(&old_ancestor.signers, &mut votes.votes);
	}

	// add votes from new blocks
	let mut parent_empty_step_signers = empty_steps_signers(header);
	let mut unaccounted_ancestry = VecDeque::new();
	while let Some((ancestor_id, ancestor_submitter, ancestor)) = cached_votes.unaccounted_ancestry.pop_front() {
		let mut signers = empty_steps_signers(&ancestor);
		sp_std::mem::swap(&mut signers, &mut parent_empty_step_signers);
		signers.insert(ancestor.author);

		add_signers_votes(validators, &signers, &mut votes.votes)?;

		unaccounted_ancestry.push_front(FinalityAncestor {
			id: ancestor_id,
			submitter: ancestor_submitter,
			signers,
		});
	}
	votes.ancestry.extend(unaccounted_ancestry);

	// add votes from block itself
	let mut header_signers = BTreeSet::new();
	header_signers.insert(header.author);
	*votes.votes.entry(header.author).or_insert(0) += 1;
	votes.ancestry.push_back(FinalityAncestor {
		id,
		submitter,
		signers: header_signers,
	});

	Ok(votes)
}

/// Increase count of 'votes' for every passed signer.
/// Fails if at least one of signers is not in the `validators` set.
fn add_signers_votes(
	validators: &BTreeSet<&Address>,
	signers_to_add: &BTreeSet<Address>,
	votes: &mut BTreeMap<Address, u64>,
) -> Result<(), Error> {
	for signer in signers_to_add {
		if !validators.contains(signer) {
			return Err(Error::NotValidator);
		}

		*votes.entry(*signer).or_insert(0) += 1;
	}

	Ok(())
}

/// Decrease 'votes' count for every passed signer.
fn remove_signers_votes(signers_to_remove: &BTreeSet<Address>, votes: &mut BTreeMap<Address, u64>) {
	for signer in signers_to_remove {
		match votes.entry(*signer) {
			Entry::Occupied(mut entry) => {
				if *entry.get() <= 1 {
					entry.remove();
				} else {
					*entry.get_mut() -= 1;
				}
			}
			Entry::Vacant(_) => unreachable!("we only remove signers that have been added; qed"),
		}
	}
}

/// Returns unique set of empty steps signers.
fn empty_steps_signers(header: &AuraHeader) -> BTreeSet<Address> {
	header
		.empty_steps()
		.into_iter()
		.flatten()
		.filter_map(|step| empty_step_signer(&step, &header.parent_hash))
		.collect::<BTreeSet<_>>()
}

/// Returns author of empty step signature.
fn empty_step_signer(empty_step: &SealedEmptyStep, parent_hash: &H256) -> Option<Address> {
	let message = empty_step.message(parent_hash);
	secp256k1_ecdsa_recover(empty_step.signature.as_fixed_bytes(), message.as_fixed_bytes())
		.ok()
		.map(|public| public_to_address(&public))
}

impl<Submitter> Default for CachedFinalityVotes<Submitter> {
	fn default() -> Self {
		CachedFinalityVotes {
			stopped_at_finalized_sibling: false,
			unaccounted_ancestry: VecDeque::new(),
			votes: None,
		}
	}
}

impl<Submitter> Default for FinalityVotes<Submitter> {
	fn default() -> Self {
		FinalityVotes {
			votes: BTreeMap::new(),
			ancestry: VecDeque::new(),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{insert_header, run_test, validator, validators_addresses, HeaderBuilder, TestRuntime};
	use crate::{BridgeStorage, FinalityCache, HeaderToImport};
	use frame_support::StorageMap;

	const TOTAL_VALIDATORS: usize = 5;

	#[test]
	fn verifies_header_author() {
		run_test(TOTAL_VALIDATORS, |_| {
			assert_eq!(
				finalize_blocks(
					&BridgeStorage::<TestRuntime>::new(),
					Default::default(),
					(Default::default(), &[]),
					Default::default(),
					None,
					&AuraHeader::default(),
					0,
				),
				Err(Error::NotValidator),
			);
		});
	}

	#[test]
	fn finalize_blocks_works() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			// let's say we have 5 validators (we need 'votes' from 3 validators to achieve
			// finality)
			let mut storage = BridgeStorage::<TestRuntime>::new();

			// when header#1 is inserted, nothing is finalized (1 vote)
			let header1 = HeaderBuilder::with_parent(&ctx.genesis).sign_by(&validator(0));
			let id1 = header1.compute_id();
			let mut header_to_import = HeaderToImport {
				context: storage.import_context(None, &header1.parent_hash).unwrap(),
				is_best: true,
				id: id1,
				header: header1,
				total_difficulty: 0.into(),
				enacted_change: None,
				scheduled_change: None,
				finality_votes: Default::default(),
			};
			assert_eq!(
				finalize_blocks(
					&storage,
					ctx.genesis.compute_id(),
					(Default::default(), &ctx.addresses),
					id1,
					None,
					&header_to_import.header,
					u64::max_value(),
				)
				.map(|eff| eff.finalized_headers),
				Ok(Vec::new()),
			);
			storage.insert_header(header_to_import.clone());

			// when header#2 is inserted, nothing is finalized (2 votes)
			header_to_import.header = HeaderBuilder::with_parent_hash(id1.hash).sign_by(&validator(1));
			header_to_import.id = header_to_import.header.compute_id();
			let id2 = header_to_import.header.compute_id();
			assert_eq!(
				finalize_blocks(
					&storage,
					ctx.genesis.compute_id(),
					(Default::default(), &ctx.addresses),
					id2,
					None,
					&header_to_import.header,
					u64::max_value(),
				)
				.map(|eff| eff.finalized_headers),
				Ok(Vec::new()),
			);
			storage.insert_header(header_to_import.clone());

			// when header#3 is inserted, header#1 is finalized (3 votes)
			header_to_import.header = HeaderBuilder::with_parent_hash(id2.hash).sign_by(&validator(2));
			header_to_import.id = header_to_import.header.compute_id();
			let id3 = header_to_import.header.compute_id();
			assert_eq!(
				finalize_blocks(
					&storage,
					ctx.genesis.compute_id(),
					(Default::default(), &ctx.addresses),
					id3,
					None,
					&header_to_import.header,
					u64::max_value(),
				)
				.map(|eff| eff.finalized_headers),
				Ok(vec![(id1, None)]),
			);
			storage.insert_header(header_to_import);
		});
	}

	#[test]
	fn cached_votes_are_updated_with_ancestry() {
		// we're inserting header#5
		// cached votes are from header#3
		// header#4 has finalized header#1 and header#2
		// => when inserting header#5, we need to:
		// 1) remove votes from header#1 and header#2
		// 2) add votes from header#4 and header#5
		let validators = validators_addresses(5);
		let headers = (1..6)
			.map(|number| HeaderBuilder::with_number(number).sign_by(&validator(number as usize - 1)))
			.collect::<Vec<_>>();
		let ancestry = headers
			.iter()
			.map(|header| FinalityAncestor {
				id: header.compute_id(),
				signers: vec![header.author].into_iter().collect(),
				..Default::default()
			})
			.collect::<Vec<_>>();
		let header5 = headers[4].clone();
		assert_eq!(
			prepare_votes::<()>(
				CachedFinalityVotes {
					stopped_at_finalized_sibling: false,
					unaccounted_ancestry: vec![(headers[3].compute_id(), None, headers[3].clone()),]
						.into_iter()
						.collect(),
					votes: Some(FinalityVotes {
						votes: vec![(validators[0], 1), (validators[1], 1), (validators[2], 1),]
							.into_iter()
							.collect(),
						ancestry: ancestry[..3].iter().cloned().collect(),
					}),
				},
				headers[1].compute_id(),
				&validators.iter().collect(),
				header5.compute_id(),
				&header5,
				None,
			)
			.unwrap(),
			FinalityVotes {
				votes: vec![(validators[2], 1), (validators[3], 1), (validators[4], 1),]
					.into_iter()
					.collect(),
				ancestry: ancestry[2..].iter().cloned().collect(),
			},
		);
	}

	#[test]
	fn prepare_votes_respects_finality_cache() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			// we need signatures of 3 validators to finalize block
			let mut storage = BridgeStorage::<TestRuntime>::new();

			// headers 1..3 are signed by validator#0
			// headers 4..6 are signed by validator#1
			// headers 7..9 are signed by validator#2
			let mut hashes = Vec::new();
			let mut headers = Vec::new();
			let mut ancestry = Vec::new();
			let mut parent_hash = ctx.genesis.compute_hash();
			for i in 1..10 {
				let header = HeaderBuilder::with_parent_hash(parent_hash).sign_by(&validator((i - 1) / 3));
				let id = header.compute_id();
				insert_header(&mut storage, header.clone());
				hashes.push(id.hash);
				ancestry.push(FinalityAncestor {
					id: header.compute_id(),
					submitter: None,
					signers: vec![header.author].into_iter().collect(),
				});
				headers.push(header);
				parent_hash = id.hash;
			}

			// when we're inserting header#7 and last finalized header is 0:
			// check that votes at #7 are computed correctly without cache
			let expected_votes_at_7 = FinalityVotes {
				votes: vec![(ctx.addresses[0], 3), (ctx.addresses[1], 3), (ctx.addresses[2], 1)]
					.into_iter()
					.collect(),
				ancestry: ancestry[..7].iter().cloned().collect(),
			};
			let id7 = headers[6].compute_id();
			assert_eq!(
				prepare_votes(
					storage.cached_finality_votes(
						&headers.get(5).unwrap().compute_id(),
						&ctx.genesis.compute_id(),
						|_| false,
					),
					Default::default(),
					&ctx.addresses.iter().collect(),
					id7,
					headers.get(6).unwrap(),
					None,
				)
				.unwrap(),
				expected_votes_at_7,
			);

			// cached votes at #5
			let expected_votes_at_5 = FinalityVotes {
				votes: vec![(ctx.addresses[0], 3), (ctx.addresses[1], 2)].into_iter().collect(),
				ancestry: ancestry[..5].iter().cloned().collect(),
			};
			FinalityCache::<TestRuntime>::insert(hashes[4], expected_votes_at_5);

			// when we're inserting header#7 and last finalized header is 0:
			// check that votes at #7 are computed correctly with cache
			assert_eq!(
				prepare_votes(
					storage.cached_finality_votes(
						&headers.get(5).unwrap().compute_id(),
						&ctx.genesis.compute_id(),
						|_| false,
					),
					Default::default(),
					&ctx.addresses.iter().collect(),
					id7,
					headers.get(6).unwrap(),
					None,
				)
				.unwrap(),
				expected_votes_at_7,
			);

			// when we're inserting header#7 and last finalized header is 3:
			// check that votes at #7 are computed correctly with cache
			let expected_votes_at_7 = FinalityVotes {
				votes: vec![(ctx.addresses[1], 3), (ctx.addresses[2], 1)].into_iter().collect(),
				ancestry: ancestry[3..7].iter().cloned().collect(),
			};
			assert_eq!(
				prepare_votes(
					storage.cached_finality_votes(
						&headers.get(5).unwrap().compute_id(),
						&headers.get(2).unwrap().compute_id(),
						|hash| *hash == hashes[2],
					),
					headers[2].compute_id(),
					&ctx.addresses.iter().collect(),
					id7,
					headers.get(6).unwrap(),
					None,
				)
				.unwrap(),
				expected_votes_at_7,
			);
		});
	}

	#[test]
	fn prepare_votes_fails_when_finalized_sibling_is_in_ancestry() {
		assert_eq!(
			prepare_votes::<()>(
				CachedFinalityVotes {
					stopped_at_finalized_sibling: true,
					..Default::default()
				},
				Default::default(),
				&validators_addresses(3).iter().collect(),
				Default::default(),
				&Default::default(),
				None,
			),
			Err(Error::TryingToFinalizeSibling),
		);
	}
}
