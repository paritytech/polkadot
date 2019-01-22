// Copyright 2017-2018 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! Module to process claims from Ethereum addresses.

use tiny_keccak::keccak256;
use secp256k1;
use srml_support::{StorageValue, StorageMap, dispatch::Result};
use system::ensure_signed;
use codec::Encode;
use sr_primitives::traits::Zero;

/// Configuration trait.
pub trait Trait: balances::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
}

type EthereumAddress = [u8; 20];
type EcdsaSignature = ([u8; 32], [u8; 32], i8);

/// An event in this module.
decl_event!(
	pub enum Event<T> where
		B = <T as balances::Trait>::Balance,
		A = <T as system::Trait>::AccountId
	{
		/// Someone claimed some DOTs.
		Claimed(A, EthereumAddress, B),
	}
);

decl_storage! {
	// A macro for the Storage trait, and its implementation, for this module.
	// This allows for type-safe usage of the Substrate storage database, so you can
	// keep things around between blocks.
	trait Store for Module<T: Trait> as Claims {
		Claims get(claims) build(|config: &GenesisConfig<T>| {
			config.claims.iter().map(|(a, b)| (a.clone(), b.clone())).collect::<Vec<_>>()
		}): map EthereumAddress => Option<T::Balance>;
		Total get(total) build(|config: &GenesisConfig<T>| {
			config.claims.iter().fold(Zero::zero(), |acc: T::Balance, &(_, n)| acc + n)
		}): T::Balance;
	}
	add_extra_genesis {
		config(claims): Vec<(EthereumAddress, T::Balance)>;
	}
}

fn ecdsa_recover(sig: &EcdsaSignature, msg: [u8; 32]) -> Option<[u8; 64]> {
	let pubkey = secp256k1::recover(
		&secp256k1::Message::parse(&msg),
		&(sig.0, sig.1).using_encoded(secp256k1::Signature::parse_slice).ok()?,
		&secp256k1::RecoveryId::parse(sig.2 as u8).ok()?
	).ok()?;
	let mut res = [0u8; 64];
	res.copy_from_slice(&pubkey.serialize()[0..64]);
	Some(res)
}

fn eth_recover(s: &EcdsaSignature, who: &[u8]) -> Option<EthereumAddress> {
	let mut v = b"\x19Ethereum Signed Message: 65\nPay DOTs to the Polkadot account:".to_vec();
	v.extend_from_slice(who);
	let msg = keccak256(&v[..]);
	let mut res = EthereumAddress::default();
	res.copy_from_slice(&keccak256(&ecdsa_recover(s, msg)?[..])[12..]);
	Some(res)
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		/// Deposit one of this module's events by using the default implementation.
		fn deposit_event<T>() = default;

		/// Make a claim.
		fn claim(origin, ethereum_signature: EcdsaSignature) -> Result {
			// This is a public call, so we ensure that the origin is some signed account.
			let sender = ensure_signed(origin)?;
			
			let signer = sender.using_encoded(|data|
					eth_recover(&ethereum_signature, data)
				).ok_or("Invalid Ethereum signature")?;
			
			let balance_due = <Claims<T>>::take(&signer)
				.ok_or("Ethereum address has no claim")?;
			
			<Total<T>>::mutate(|t| if *t < balance_due {
				panic!("Logic error: Pot less than the total of claims!")
			} else {
				*t -= balance_due
			});

			// Let's deposit an event to let the outside world know this happened.
			Self::deposit_event(RawEvent::Claimed(sender, signer, balance_due));

			// All good.
			Ok(())
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use sr_io::with_externalities;
	use substrate_primitives::{H256, Blake2Hasher};
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are requried.
	use sr_primitives::{
		BuildStorage, traits::{BlakeTwo256, IdentityLookup}, testing::{Digest, DigestItem, Header}
	};

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	// For testing the module, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of modules we want to use.
	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	impl system::Trait for Test {
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type Digest = Digest;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type Event = ();
		type Log = DigestItem;
	}
	impl balances::Trait for Test {
		type Balance = u64;
		type OnFreeBalanceZero = ();
		type OnNewAccount = ();
		type EnsureAccountLiquid = ();
		type Event = ();
	}
	impl Trait for Test {
		type Event = ();
	}
	type Claims = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> sr_io::TestExternalities<Blake2Hasher> {
		let mut t = system::GenesisConfig::<Test>::default().build_storage().unwrap().0;
		// We use default for brevity, but you can configure as desired if needed.
		t.extend(balances::GenesisConfig::<Test>::default().build_storage().unwrap().0);
		t.extend(GenesisConfig::<Test>{
			claims: vec![([1; 20], 100), ([2; 20], 200)],
		}.build_storage().unwrap().0);
		t.into()
	}

	#[test]
	fn it_works_for_optional_value() {
		with_externalities(&mut new_test_ext(), || {
			assert_eq!(Claims::total(), 300);
			assert_eq!(Claims::claims(&[1; 20]), Some(100));
			assert_eq!(Claims::claims(&[2; 20]), Some(200));
			assert_eq!(Claims::claims(&[0; 20]), None);
		});
	}
}
