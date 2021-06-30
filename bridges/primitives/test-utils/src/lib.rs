// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! Utilities for testing runtime code.

#![cfg_attr(not(feature = "std"), no_std)]

use bp_header_chain::justification::GrandpaJustification;
use codec::Encode;
use sp_application_crypto::TryFrom;
use sp_finality_grandpa::{AuthorityId, AuthorityWeight};
use sp_finality_grandpa::{AuthoritySignature, SetId};
use sp_runtime::traits::{Header as HeaderT, One, Zero};
use sp_std::prelude::*;

// Re-export all our test account utilities
pub use keyring::*;

mod keyring;

pub const TEST_GRANDPA_ROUND: u64 = 1;
pub const TEST_GRANDPA_SET_ID: SetId = 1;

/// Configuration parameters when generating test GRANDPA justifications.
#[derive(Clone)]
pub struct JustificationGeneratorParams<H> {
	/// The header which we want to finalize.
	pub header: H,
	/// The GRANDPA round number for the current authority set.
	pub round: u64,
	/// The current authority set ID.
	pub set_id: SetId,
	/// The current GRANDPA authority set.
	///
	/// The size of the set will determine the number of pre-commits in our justification.
	pub authorities: Vec<(Account, AuthorityWeight)>,
	/// The total number of precommit ancestors in the `votes_ancestries` field our justification.
	///
	/// These may be distributed among many different forks.
	pub ancestors: u32,
	/// The number of forks.
	///
	/// Useful for creating a "worst-case" scenario in which each authority is on its own fork.
	pub forks: u32,
}

impl<H: HeaderT> Default for JustificationGeneratorParams<H> {
	fn default() -> Self {
		Self {
			header: test_header(One::one()),
			round: TEST_GRANDPA_ROUND,
			set_id: TEST_GRANDPA_SET_ID,
			authorities: test_keyring(),
			ancestors: 2,
			forks: 1,
		}
	}
}

/// Make a valid GRANDPA justification with sensible defaults
pub fn make_default_justification<H: HeaderT>(header: &H) -> GrandpaJustification<H> {
	let params = JustificationGeneratorParams::<H> {
		header: header.clone(),
		..Default::default()
	};

	make_justification_for_header(params)
}

/// Generate justifications in a way where we are able to tune the number of pre-commits
/// and vote ancestries which are included in the justification.
///
/// This is useful for benchmarkings where we want to generate valid justifications with
/// a specific number of pre-commits (tuned with the number of "authorities") and/or a specific
/// number of vote ancestries (tuned with the "votes" parameter).
///
/// Note: This needs at least three authorities or else the verifier will complain about
/// being given an invalid commit.
pub fn make_justification_for_header<H: HeaderT>(params: JustificationGeneratorParams<H>) -> GrandpaJustification<H> {
	let JustificationGeneratorParams {
		header,
		round,
		set_id,
		authorities,
		mut ancestors,
		forks,
	} = params;
	let (target_hash, target_number) = (header.hash(), *header.number());
	let mut votes_ancestries = vec![];
	let mut precommits = vec![];

	assert!(forks != 0, "Need at least one fork to have a chain..");
	assert!(
		forks as usize <= authorities.len(),
		"If we have more forks than authorities we can't create valid pre-commits for all the forks."
	);

	// Roughly, how many vote ancestries do we want per fork
	let target_depth = (ancestors + forks - 1) / forks;

	let mut unsigned_precommits = vec![];
	for i in 0..forks {
		let depth = if ancestors >= target_depth {
			ancestors -= target_depth;
			target_depth
		} else {
			ancestors
		};

		// Note: Adding 1 to account for the target header
		let chain = generate_chain(i as u32, depth + 1, &header);

		// We don't include our finality target header in the vote ancestries
		for child in &chain[1..] {
			votes_ancestries.push(child.clone());
		}

		// The header we need to use when pre-commiting is the one at the highest height
		// on our chain.
		let precommit_candidate = chain.last().map(|h| (h.hash(), *h.number())).unwrap();
		unsigned_precommits.push(precommit_candidate);
	}

	for (i, (id, _weight)) in authorities.iter().enumerate() {
		// Assign authorities to sign pre-commits in a round-robin fashion
		let target = unsigned_precommits[i % forks as usize];
		let precommit = signed_precommit::<H>(id, target, round, set_id);

		precommits.push(precommit);
	}

	GrandpaJustification {
		round,
		commit: finality_grandpa::Commit {
			target_hash,
			target_number,
			precommits,
		},
		votes_ancestries,
	}
}

fn generate_chain<H: HeaderT>(fork_id: u32, depth: u32, ancestor: &H) -> Vec<H> {
	let mut headers = vec![ancestor.clone()];

	for i in 1..depth {
		let parent = &headers[(i - 1) as usize];
		let (hash, num) = (parent.hash(), *parent.number());

		let mut header = test_header::<H>(num + One::one());
		header.set_parent_hash(hash);

		// Modifying the digest so headers at the same height but in different forks have different
		// hashes
		header
			.digest_mut()
			.logs
			.push(sp_runtime::DigestItem::Other(fork_id.encode()));

		headers.push(header);
	}

	headers
}

/// Create signed precommit with given target.
pub fn signed_precommit<H: HeaderT>(
	signer: &Account,
	target: (H::Hash, H::Number),
	round: u64,
	set_id: SetId,
) -> finality_grandpa::SignedPrecommit<H::Hash, H::Number, AuthoritySignature, AuthorityId> {
	let precommit = finality_grandpa::Precommit {
		target_hash: target.0,
		target_number: target.1,
	};

	let encoded =
		sp_finality_grandpa::localized_payload(round, set_id, &finality_grandpa::Message::Precommit(precommit.clone()));

	let signature = signer.sign(&encoded);
	let raw_signature: Vec<u8> = signature.to_bytes().into();

	// Need to wrap our signature and id types that they match what our `SignedPrecommit` is expecting
	let signature = AuthoritySignature::try_from(raw_signature).expect(
		"We know our Keypair is good,
		so our signature must also be good.",
	);
	let id = (*signer).into();

	finality_grandpa::SignedPrecommit {
		precommit,
		signature,
		id,
	}
}

/// Get a header for testing.
///
/// The correct parent hash will be used if given a non-zero header.
pub fn test_header<H: HeaderT>(number: H::Number) -> H {
	let default = |num| {
		H::new(
			num,
			Default::default(),
			Default::default(),
			Default::default(),
			Default::default(),
		)
	};

	let mut header = default(number);
	if number != Zero::zero() {
		let parent_hash = default(number - One::one()).hash();
		header.set_parent_hash(parent_hash);
	}

	header
}

/// Convenience function for generating a Header ID at a given block number.
pub fn header_id<H: HeaderT>(index: u8) -> (H::Hash, H::Number) {
	(test_header::<H>(index.into()).hash(), index.into())
}
