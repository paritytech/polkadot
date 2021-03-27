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
//!
//! Unlike other crates in the `primitives` folder, this crate does *not* need to compile in a
//! `no_std` environment. This is fine because this code should only be used, as the name implies,
//! in tests.

use bp_header_chain::justification::GrandpaJustification;
use finality_grandpa::voter_set::VoterSet;
use sp_finality_grandpa::{AuthorityId, AuthorityList, AuthorityWeight};
use sp_finality_grandpa::{AuthoritySignature, SetId};
use sp_keyring::Ed25519Keyring;
use sp_runtime::traits::Header as HeaderT;
use sp_runtime::traits::{One, Zero};

pub const TEST_GRANDPA_ROUND: u64 = 1;
pub const TEST_GRANDPA_SET_ID: SetId = 1;

/// Get a valid Grandpa justification for a header given a Grandpa round, authority set ID, and
/// authority list.
pub fn make_justification_for_header<H: HeaderT>(
	header: &H,
	round: u64,
	set_id: SetId,
	authorities: &[(AuthorityId, AuthorityWeight)],
) -> GrandpaJustification<H> {
	let (target_hash, target_number) = (header.hash(), *header.number());
	let mut precommits = vec![];
	let mut votes_ancestries = vec![];

	// We want to make sure that the header included in the vote ancestries
	// is actually related to our target header
	let mut precommit_header = test_header::<H>(target_number + One::one());
	precommit_header.set_parent_hash(target_hash);

	// I'm using the same header for all the voters since it doesn't matter as long
	// as they all vote on blocks _ahead_ of the one we're interested in finalizing
	for (id, _weight) in authorities.iter() {
		let signer = extract_keyring(&id);
		let precommit = signed_precommit::<H>(
			signer,
			(precommit_header.hash(), *precommit_header.number()),
			round,
			set_id,
		);
		precommits.push(precommit);
		votes_ancestries.push(precommit_header.clone());
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

fn signed_precommit<H: HeaderT>(
	signer: Ed25519Keyring,
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
	let signature = signer.sign(&encoded[..]).into();
	finality_grandpa::SignedPrecommit {
		precommit,
		signature,
		id: signer.public().into(),
	}
}

/// Get a header for testing.
///
/// The correct parent hash will be used if given a non-zero header.
pub fn test_header<H: HeaderT>(number: H::Number) -> H {
	let mut header = H::new(
		number,
		Default::default(),
		Default::default(),
		Default::default(),
		Default::default(),
	);

	if number != Zero::zero() {
		let parent_hash = test_header::<H>(number - One::one()).hash();
		header.set_parent_hash(parent_hash);
	}

	header
}

/// Convenience function for generating a Header ID at a given block number.
pub fn header_id<H: HeaderT>(index: u8) -> (H::Hash, H::Number) {
	(test_header::<H>(index.into()).hash(), index.into())
}

/// Get the identity of a test account given an ED25519 Public key.
pub fn extract_keyring(id: &AuthorityId) -> Ed25519Keyring {
	let mut raw_public = [0; 32];
	raw_public.copy_from_slice(id.as_ref());
	Ed25519Keyring::from_raw_public(raw_public).unwrap()
}

/// Get a valid set of voters for a Grandpa round.
pub fn voter_set() -> VoterSet<AuthorityId> {
	VoterSet::new(authority_list()).unwrap()
}

/// Convenience function to get a list of Grandpa authorities.
pub fn authority_list() -> AuthorityList {
	vec![(alice(), 1), (bob(), 1), (charlie(), 1)]
}

/// Get the Public key of the Alice test account.
pub fn alice() -> AuthorityId {
	Ed25519Keyring::Alice.public().into()
}

/// Get the Public key of the Bob test account.
pub fn bob() -> AuthorityId {
	Ed25519Keyring::Bob.public().into()
}

/// Get the Public key of the Charlie test account.
pub fn charlie() -> AuthorityId {
	Ed25519Keyring::Charlie.public().into()
}
