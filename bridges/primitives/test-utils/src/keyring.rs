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

//! Utilities for working with test accounts.

use ed25519_dalek::{Keypair, PublicKey, SecretKey, Signature};
use finality_grandpa::voter_set::VoterSet;
use parity_scale_codec::Encode;
use sp_application_crypto::Public;
use sp_finality_grandpa::{AuthorityId, AuthorityList, AuthorityWeight};
use sp_runtime::RuntimeDebug;
use sp_std::prelude::*;

/// Set of test accounts with friendly names.
pub const ALICE: Account = Account(0);
pub const BOB: Account = Account(1);
pub const CHARLIE: Account = Account(2);
pub const DAVE: Account = Account(3);
pub const EVE: Account = Account(4);
pub const FERDIE: Account = Account(5);

/// A test account which can be used to sign messages.
#[derive(RuntimeDebug, Clone, Copy)]
pub struct Account(pub u16);

impl Account {
	pub fn public(&self) -> PublicKey {
		(&self.secret()).into()
	}

	pub fn secret(&self) -> SecretKey {
		let data = self.0.encode();
		let mut bytes = [0_u8; 32];
		bytes[0..data.len()].copy_from_slice(&*data);
		SecretKey::from_bytes(&bytes).expect("A static array of the correct length is a known good.")
	}

	pub fn pair(&self) -> Keypair {
		let mut pair: [u8; 64] = [0; 64];

		let secret = self.secret();
		pair[..32].copy_from_slice(&secret.to_bytes());

		let public = self.public();
		pair[32..].copy_from_slice(&public.to_bytes());

		Keypair::from_bytes(&pair).expect("We expect the SecretKey to be good, so this must also be good.")
	}

	pub fn sign(&self, msg: &[u8]) -> Signature {
		use ed25519_dalek::Signer;
		self.pair().sign(msg)
	}
}

impl From<Account> for AuthorityId {
	fn from(p: Account) -> Self {
		AuthorityId::from_slice(&p.public().to_bytes())
	}
}

/// Get a valid set of voters for a Grandpa round.
pub fn voter_set() -> VoterSet<AuthorityId> {
	VoterSet::new(authority_list()).unwrap()
}

/// Convenience function to get a list of Grandpa authorities.
pub fn authority_list() -> AuthorityList {
	test_keyring()
		.iter()
		.map(|(id, w)| (AuthorityId::from(*id), *w))
		.collect()
}

/// Get the corresponding identities from the keyring for the "standard" authority set.
pub fn test_keyring() -> Vec<(Account, AuthorityWeight)> {
	vec![(ALICE, 1), (BOB, 1), (CHARLIE, 1)]
}

/// Get a list of "unique" accounts.
pub fn accounts(len: u16) -> Vec<Account> {
	(0..len).into_iter().map(Account).collect()
}
