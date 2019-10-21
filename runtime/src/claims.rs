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

use rstd::prelude::*;
use sr_io::{keccak_256, secp256k1_ecdsa_recover};
use srml_support::{decl_event, decl_storage, decl_module};
use srml_support::traits::{Currency, Get};
use system::ensure_none;
use codec::{Encode, Decode};
#[cfg(feature = "std")]
use serde::{self, Serialize, Deserialize, Serializer, Deserializer};
#[cfg(feature = "std")]
use sr_primitives::traits::Zero;
use sr_primitives::{
	weights::SimpleDispatchInfo, traits::ValidateUnsigned,
	transaction_validity::{
		TransactionLongevity, TransactionValidity, ValidTransaction, InvalidTransaction
	},
};
use primitives::ValidityError;
use system;

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

/// Configuration trait.
pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
	type Currency: Currency<Self::AccountId>;
	type Prefix: Get<&'static [u8]>;
}

/// An Ethereum address (i.e. 20 bytes, used to represent an Ethereum account).
///
/// This gets serialized to the 0x-prefixed hex representation.
#[derive(Clone, Copy, PartialEq, Eq, Encode, Decode, Default)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct EthereumAddress([u8; 20]);

#[cfg(feature = "std")]
impl Serialize for EthereumAddress {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
		let hex: String = rustc_hex::ToHex::to_hex(&self.0[..]);
		serializer.serialize_str(&format!("0x{}", hex))
	}
}

#[cfg(feature = "std")]
impl<'de> Deserialize<'de> for EthereumAddress {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
		let base_string = String::deserialize(deserializer)?;
		let offset = if base_string.starts_with("0x") { 2 } else { 0 };
		let s = &base_string[offset..];
		if s.len() != 40 {
			Err(serde::de::Error::custom("Bad length of Ethereum address (should be 42 including '0x')"))?;
		}
		let raw: Vec<u8> = rustc_hex::FromHex::from_hex(s)
			.map_err(|e| serde::de::Error::custom(format!("{:?}", e)))?;
		let mut r = Self::default();
		r.0.copy_from_slice(&raw);
		Ok(r)
	}
}

#[derive(Encode, Decode, Clone)]
pub struct EcdsaSignature(pub [u8; 65]);

impl PartialEq for EcdsaSignature {
	fn eq(&self, other: &Self) -> bool {
		&self.0[..] == &other.0[..]
	}
}

#[cfg(feature = "std")]
impl std::fmt::Debug for EcdsaSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", &self.0[..])
    }
}

decl_event!(
	pub enum Event<T> where
		Balance = BalanceOf<T>,
		AccountId = <T as system::Trait>::AccountId
	{
		/// Someone claimed some DOTs.
		Claimed(AccountId, EthereumAddress, Balance),
	}
);

decl_storage! {
	// A macro for the Storage trait, and its implementation, for this module.
	// This allows for type-safe usage of the Substrate storage database, so you can
	// keep things around between blocks.
	trait Store for Module<T: Trait> as Claims {
		Claims get(claims) build(|config: &GenesisConfig<T>| {
			config.claims.iter().map(|(a, b)| (a.clone(), b.clone())).collect::<Vec<_>>()
		}): map EthereumAddress => Option<BalanceOf<T>>;
		Total get(total) build(|config: &GenesisConfig<T>| {
			config.claims.iter().fold(Zero::zero(), |acc: BalanceOf<T>, &(_, n)| acc + n)
		}): BalanceOf<T>;
	}
	add_extra_genesis {
		config(claims): Vec<(EthereumAddress, BalanceOf<T>)>;
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		/// The Prefix that is used in signed Ethereum messages for this network
		const Prefix: &[u8] = T::Prefix::get();

		/// Deposit one of this module's events by using the default implementation.
		fn deposit_event() = default;

		/// Make a claim.
		#[weight = SimpleDispatchInfo::FixedNormal(1_000_000)]
		fn claim(origin, dest: T::AccountId, ethereum_signature: EcdsaSignature) {
			ensure_none(origin)?;

			let data = dest.using_encoded(to_ascii_hex);
			let signer = Self::eth_recover(&ethereum_signature, &data)
				.ok_or("Invalid Ethereum signature")?;

			let balance_due = <Claims<T>>::take(&signer)
				.ok_or("Ethereum address has no claim")?;

			<Total<T>>::mutate(|t| if *t < balance_due {
				panic!("Logic error: Pot less than the total of claims!")
			} else {
				*t -= balance_due
			});

			T::Currency::deposit_creating(&dest, balance_due);

			// Let's deposit an event to let the outside world know this happened.
			Self::deposit_event(RawEvent::Claimed(dest, signer, balance_due));
		}
	}
}

/// Converts the given binary data into ASCII-encoded hex. It will be twice the length.
fn to_ascii_hex(data: &[u8]) -> Vec<u8> {
	let mut r = Vec::with_capacity(data.len() * 2);
	let mut push_nibble = |n| r.push(if n < 10 { b'0' + n } else { b'a' - 10 + n });
	for &b in data.iter() {
		push_nibble(b / 16);
		push_nibble(b % 16);
	}
	r
}

impl<T: Trait> Module<T> {
	// Constructs the message that Ethereum RPC's `personal_sign` and `eth_sign` would sign.
	fn ethereum_signable_message(what: &[u8]) -> Vec<u8> {
		let prefix = T::Prefix::get();
		let mut l = prefix.len() + what.len();
		let mut rev = Vec::new();
		while l > 0 {
			rev.push(b'0' + (l % 10) as u8);
			l /= 10;
		}
		let mut v = b"\x19Ethereum Signed Message:\n".to_vec();
		v.extend(rev.into_iter().rev());
		v.extend_from_slice(&prefix[..]);
		v.extend_from_slice(what);
		v
	}

	// Attempts to recover the Ethereum address from a message signature signed by using
	// the Ethereum RPC's `personal_sign` and `eth_sign`.
	fn eth_recover(s: &EcdsaSignature, what: &[u8]) -> Option<EthereumAddress> {
		let msg = keccak_256(&Self::ethereum_signable_message(what));
		let mut res = EthereumAddress::default();
		res.0.copy_from_slice(&keccak_256(&secp256k1_ecdsa_recover(&s.0, &msg).ok()?[..])[12..]);
		Some(res)
	}
}

impl<T: Trait> ValidateUnsigned for Module<T> {
	type Call = Call<T>;

	fn validate_unsigned(call: &Self::Call) -> TransactionValidity {
		const PRIORITY: u64 = 100;

		match call {
			Call::claim(account, ethereum_signature) => {
				let data = account.using_encoded(to_ascii_hex);
				let maybe_signer = Self::eth_recover(&ethereum_signature, &data);
				let signer = if let Some(s) = maybe_signer {
					s
				} else {
					return InvalidTransaction::Custom(
						ValidityError::InvalidEthereumSignature.into(),
					).into();
				};

				if !<Claims<T>>::exists(&signer) {
					return Err(InvalidTransaction::Custom(
						ValidityError::SignerHasNoClaim.into(),
					).into());
				}

				Ok(ValidTransaction {
					priority: PRIORITY,
					requires: vec![],
					provides: vec![("claims", signer).encode()],
					longevity: TransactionLongevity::max_value(),
					propagate: true,
				})
			}
			_ => Err(InvalidTransaction::Call.into()),
		}
	}
}

#[cfg(test)]
mod tests {
	use secp256k1;
	use tiny_keccak::keccak256;
	use hex_literal::hex;
	use super::*;

	use substrate_primitives::H256;
	use codec::Encode;
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are required.
	use sr_primitives::{Perbill, traits::{BlakeTwo256, IdentityLookup}, testing::Header};
	use balances;
	use srml_support::{impl_outer_origin, assert_ok, assert_err, assert_noop, parameter_types};

	impl_outer_origin! {
		pub enum Origin for Test {}
	}
	// For testing the module, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of modules we want to use.
	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub const MaximumBlockWeight: u32 = 4 * 1024 * 1024;
		pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	}
	impl system::Trait for Test {
		type Origin = Origin;
		type Call = ();
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type AvailableBlockRatio = AvailableBlockRatio;
		type MaximumBlockLength = MaximumBlockLength;
		type Version = ();
	}

	parameter_types! {
		pub const ExistentialDeposit: u64 = 0;
		pub const TransferFee: u64 = 0;
		pub const CreationFee: u64 = 0;
	}

	impl balances::Trait for Test {
		type Balance = u64;
		type OnFreeBalanceZero = ();
		type OnNewAccount = ();
		type Event = ();
		type DustRemoval = ();
		type TransferPayment = ();
		type ExistentialDeposit = ExistentialDeposit;
		type TransferFee = TransferFee;
		type CreationFee = CreationFee;
	}

	parameter_types!{
		pub const Prefix: &'static [u8] = b"Pay RUSTs to the TEST account:";
	}

	impl Trait for Test {
		type Event = ();
		type Currency = Balances;
		type Prefix = Prefix;
	}
	type Balances = balances::Module<Test>;
	type Claims = Module<Test>;

	fn alice_secret() -> secp256k1::SecretKey {
		secp256k1::SecretKey::parse(&keccak256(b"Alice")).unwrap()
	}
	fn alice_public() -> secp256k1::PublicKey {
		secp256k1::PublicKey::from_secret_key(&alice_secret())
	}
	fn alice_eth() -> EthereumAddress {
		let mut res = EthereumAddress::default();
		res.0.copy_from_slice(&keccak256(&alice_public().serialize()[1..65])[12..]);
		res
	}
	fn alice_sig(what: &[u8]) -> EcdsaSignature {
		let msg = keccak256(&Claims::ethereum_signable_message(&to_ascii_hex(what)[..]));
		let (sig, recovery_id) = secp256k1::sign(&secp256k1::Message::parse(&msg), &alice_secret());
		let mut r = [0u8; 65];
		r[0..64].copy_from_slice(&sig.serialize()[..]);
		r[64] = recovery_id.serialize();
		EcdsaSignature(r)
	}
	fn bob_secret() -> secp256k1::SecretKey {
		secp256k1::SecretKey::parse(&keccak256(b"Bob")).unwrap()
	}
	fn bob_sig(what: &[u8]) -> EcdsaSignature {
		let msg = keccak256(&Claims::ethereum_signable_message(&to_ascii_hex(what)[..]));
		let (sig, recovery_id) = secp256k1::sign(&secp256k1::Message::parse(&msg), &bob_secret());
		let mut r = [0u8; 65];
		r[0..64].copy_from_slice(&sig.serialize()[..]);
		r[64] = recovery_id.serialize();
		EcdsaSignature(r)
	}

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> sr_io::TestExternalities {
		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
		// We use default for brevity, but you can configure as desired if needed.
		balances::GenesisConfig::<Test>::default().assimilate_storage(&mut t).unwrap();
		GenesisConfig::<Test>{
			claims: vec![(alice_eth(), 100)],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Claims::total(), 100);
			assert_eq!(Claims::claims(&alice_eth()), Some(100));
			assert_eq!(Claims::claims(&EthereumAddress::default()), None);
		});
	}

	#[test]
	fn serde_works() {
		let x = EthereumAddress(hex!["0123456789abcdef0123456789abcdef01234567"]);
		let y = serde_json::to_string(&x).unwrap();
		assert_eq!(y, "\"0x0123456789abcdef0123456789abcdef01234567\"");
		let z: EthereumAddress = serde_json::from_str(&y).unwrap();
		assert_eq!(x, z);
	}

	#[test]
	fn claiming_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(&42), 0);
			assert_ok!(Claims::claim(Origin::NONE, 42, alice_sig(&42u64.encode())));
			assert_eq!(Balances::free_balance(&42), 100);
		});
	}

	#[test]
	fn origin_signed_claiming_fail() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(&42), 0);
			assert_err!(
				Claims::claim(Origin::signed(42), 42, alice_sig(&42u64.encode())),
				"RequireNoOrigin",
			);
		});
	}

	#[test]
	fn double_claiming_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(&42), 0);
			assert_ok!(Claims::claim(Origin::NONE, 42, alice_sig(&42u64.encode())));
			assert_noop!(Claims::claim(Origin::NONE, 42, alice_sig(&42u64.encode())), "Ethereum address has no claim");
		});
	}

	#[test]
	fn non_sender_sig_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(&42), 0);
			assert_noop!(Claims::claim(Origin::NONE, 42, alice_sig(&69u64.encode())), "Ethereum address has no claim");
		});
	}

	#[test]
	fn non_claimant_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(&42), 0);
			assert_noop!(Claims::claim(Origin::NONE, 42, bob_sig(&69u64.encode())), "Ethereum address has no claim");
		});
	}

	#[test]
	fn real_eth_sig_works() {
		new_test_ext().execute_with(|| {
			// "Pay RUSTs to the TEST account:2a00000000000000"
			let sig = hex!["444023e89b67e67c0562ed0305d252a5dd12b2af5ac51d6d3cb69a0b486bc4b3191401802dc29d26d586221f7256cd3329fe82174bdf659baea149a40e1c495d1c"];
			let sig = EcdsaSignature(sig);
			let who = 42u64.using_encoded(to_ascii_hex);
			let signer = Claims::eth_recover(&sig, &who).unwrap();
			assert_eq!(signer.0, hex!["6d31165d5d932d571f3b44695653b46dcc327e84"]);
		});
	}

	#[test]
	fn validate_unsigned_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(
				<Module<Test>>::validate_unsigned(&Call::claim(1, alice_sig(&1u64.encode()))),
				Ok(ValidTransaction {
					priority: 100,
					requires: vec![],
					provides: vec![("claims", alice_eth()).encode()],
					longevity: TransactionLongevity::max_value(),
					propagate: true,
				})
			);
			assert_eq!(
				<Module<Test>>::validate_unsigned(&Call::claim(0, EcdsaSignature([0; 65]))),
				InvalidTransaction::Custom(ValidityError::InvalidEthereumSignature.into()).into(),
			);
			assert_eq!(
				<Module<Test>>::validate_unsigned(&Call::claim(1, bob_sig(&1u64.encode()))),
				InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into()).into(),
			);
			assert_eq!(
				<Module<Test>>::validate_unsigned(&Call::claim(0, bob_sig(&1u64.encode()))),
				InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into()).into(),
			);
		});
	}
}
