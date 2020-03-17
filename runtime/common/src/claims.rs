// Copyright 2017-2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

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

use sp_std::prelude::*;
use sp_io::{hashing::keccak_256, crypto::secp256k1_ecdsa_recover};
use frame_support::{decl_event, decl_storage, decl_module, decl_error};
use frame_support::weights::SimpleDispatchInfo;
use frame_support::traits::{Currency, Get, VestingSchedule};
use system::{ensure_root, ensure_none};
use codec::{Encode, Decode};
#[cfg(feature = "std")]
use serde::{self, Serialize, Deserialize, Serializer, Deserializer};
#[cfg(feature = "std")]
use sp_runtime::traits::Zero;
use sp_runtime::traits::CheckedSub;
use sp_runtime::{
	RuntimeDebug, transaction_validity::{
		TransactionLongevity, TransactionValidity, ValidTransaction, InvalidTransaction
	},
};
use primitives::ValidityError;
use system;

type CurrencyOf<T> = <<T as Trait>::VestingSchedule as VestingSchedule<<T as system::Trait>::AccountId>>::Currency;
type BalanceOf<T> = <CurrencyOf<T> as Currency<<T as system::Trait>::AccountId>>::Balance;

/// Configuration trait.
pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
	type VestingSchedule: VestingSchedule<Self::AccountId, Moment=Self::BlockNumber>;
	type Prefix: Get<&'static [u8]>;
}

/// An Ethereum address (i.e. 20 bytes, used to represent an Ethereum account).
///
/// This gets serialized to the 0x-prefixed hex representation.
#[derive(Clone, Copy, PartialEq, Eq, Encode, Decode, Default, RuntimeDebug)]
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

impl sp_std::fmt::Debug for EcdsaSignature {
	fn fmt(&self, f: &mut sp_std::fmt::Formatter<'_>) -> sp_std::fmt::Result {
		write!(f, "EcdsaSignature({:?})", &self.0[..])
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

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Invalid Ethereum signature.
		InvalidEthereumSignature,
		/// Ethereum address has no claim.
		SignerHasNoClaim,
		/// The destination is already vesting and cannot be the target of a further claim.
		DestinationVesting,
		/// There's not enough in the pot to pay out some unvested amount. Generally implies a logic
		/// error.
		PotUnderflow,
	}
}

decl_storage! {
	// A macro for the Storage trait, and its implementation, for this module.
	// This allows for type-safe usage of the Substrate storage database, so you can
	// keep things around between blocks.
	trait Store for Module<T: Trait> as Claims {
		Claims get(claims) build(|config: &GenesisConfig<T>| {
			config.claims.iter().map(|(a, b)| (a.clone(), b.clone())).collect::<Vec<_>>()
		}): map hasher(identity) EthereumAddress => Option<BalanceOf<T>>;
		Total get(total) build(|config: &GenesisConfig<T>| {
			config.claims.iter().fold(Zero::zero(), |acc: BalanceOf<T>, &(_, n)| acc + n)
		}): BalanceOf<T>;
		/// Vesting schedule for a claim.
		/// First balance is the total amount that should be held for vesting.
		/// Second balance is how much should be unlocked per block.
		/// The block number is when the vesting should start.
		Vesting get(vesting) config():
			map hasher(identity) EthereumAddress
			=> Option<(BalanceOf<T>, BalanceOf<T>, T::BlockNumber)>;
	}
	add_extra_genesis {
		config(claims): Vec<(EthereumAddress, BalanceOf<T>)>;
	}
}

mod migration {
	use super::*;

	pub fn migrate<T: Trait>() {
		if let Ok(addresses) = Vec::<EthereumAddress>::decode(&mut &include_bytes!("./claims.scale")[..]) {
			for i in &addresses {
				Vesting::<T>::migrate_key_from_blake(i);
				Claims::<T>::migrate_key_from_blake(i);
			}
		}
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		type Error = Error<T>;

		/// The Prefix that is used in signed Ethereum messages for this network
		const Prefix: &[u8] = T::Prefix::get();

		/// Deposit one of this module's events by using the default implementation.
		fn deposit_event() = default;

		fn on_runtime_upgrade() {
			migration::migrate::<T>();
		}

		/// Make a claim to collect your DOTs.
		///
		/// The dispatch origin for this call must be _None_.
		///
		/// Unsigned Validation:
		/// A call to claim is deemed valid if the signature provided matches
		/// the expected signed message of:
		///
		/// > Ethereum Signed Message:
		/// > (configured prefix string)(address)
		///
		/// and `address` matches the `dest` account.
		///
		/// Parameters:
		/// - `dest`: The destination account to payout the claim.
		/// - `ethereum_signature`: The signature of an ethereum signed message
		///    matching the format described above.
		///
		/// <weight>
		/// The weight of this call is invariant over the input parameters.
		/// - One `eth_recover` operation which involves a keccak hash and a
		///   ecdsa recover.
		/// - Three storage reads to check if a claim exists for the user, to
		///   get the current pot size, to see if there exists a vesting schedule.
		/// - Up to one storage write for adding a new vesting schedule.
		/// - One `deposit_creating` Currency call.
		/// - One storage write to update the total.
		/// - Two storage removals for vesting and claims information.
		/// - One deposit event.
		///
		/// Total Complexity: O(1)
		/// </weight>
		#[weight = SimpleDispatchInfo::FixedNormal(1_000_000)]
		fn claim(origin, dest: T::AccountId, ethereum_signature: EcdsaSignature) {
			ensure_none(origin)?;

			let data = dest.using_encoded(to_ascii_hex);
			let signer = Self::eth_recover(&ethereum_signature, &data)
				.ok_or(Error::<T>::InvalidEthereumSignature)?;

			let balance_due = <Claims<T>>::get(&signer)
				.ok_or(Error::<T>::SignerHasNoClaim)?;

			let new_total = Self::total().checked_sub(&balance_due).ok_or(Error::<T>::PotUnderflow)?;

			// Check if this claim should have a vesting schedule.
			if let Some(vs) = <Vesting<T>>::get(&signer) {
				// If this fails, destination account already has a vesting schedule
				// applied to it, and this claim should not be processed.
				T::VestingSchedule::add_vesting_schedule(&dest, vs.0, vs.1, vs.2)
					.map_err(|_| Error::<T>::DestinationVesting)?;
			}

			CurrencyOf::<T>::deposit_creating(&dest, balance_due);
			<Total<T>>::put(new_total);
			<Claims<T>>::remove(&signer);
			<Vesting<T>>::remove(&signer);

			// Let's deposit an event to let the outside world know this happened.
			Self::deposit_event(RawEvent::Claimed(dest, signer, balance_due));
		}

		/// Mint a new claim to collect DOTs.
		///
		/// The dispatch origin for this call must be _Root_.
		///
		/// Parameters:
		/// - `who`: The Ethereum address allowed to collect this claim.
		/// - `value`: The number of DOTs that will be claimed.
		/// - `vesting_schedule`: An optional vesting schedule for these DOTs.
		///
		/// <weight>
		/// The weight of this call is invariant over the input parameters.
		/// - One storage mutate to increase the total claims available.
		/// - One storage write to add a new claim.
		/// - Up to one storage write to add a new vesting schedule.
		///
		/// Total Complexity: O(1)
		/// </weight>
		#[weight = SimpleDispatchInfo::FixedNormal(30_000)]
		fn mint_claim(origin,
			who: EthereumAddress,
			value: BalanceOf<T>,
			vesting_schedule: Option<(BalanceOf<T>, BalanceOf<T>, T::BlockNumber)>,
		) {
			ensure_root(origin)?;

			<Total<T>>::mutate(|t| *t += value);
			<Claims<T>>::insert(who, value);
			if let Some(vs) = vesting_schedule {
				<Vesting<T>>::insert(who, vs);
			}
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

#[allow(deprecated)] // Allow `ValidateUnsigned`
impl<T: Trait> sp_runtime::traits::ValidateUnsigned for Module<T> {
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

				if !<Claims<T>>::contains_key(&signer) {
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

#[cfg(any(test, feature = "runtime-benchmarks"))]
mod secp_utils {
	use super::*;
	use secp256k1;

	pub fn public(secret: &secp256k1::SecretKey) -> secp256k1::PublicKey {
		secp256k1::PublicKey::from_secret_key(secret)
	}
	pub fn eth(secret: &secp256k1::SecretKey) -> EthereumAddress {
		let mut res = EthereumAddress::default();
		res.0.copy_from_slice(&keccak_256(&public(secret).serialize()[1..65])[12..]);
		res
	}
	pub fn sig<T: Trait>(secret: &secp256k1::SecretKey, what: &[u8]) -> EcdsaSignature {
		let msg = keccak_256(&<super::Module<T>>::ethereum_signable_message(&to_ascii_hex(what)[..]));
		let (sig, recovery_id) = secp256k1::sign(&secp256k1::Message::parse(&msg), secret);
		let mut r = [0u8; 65];
		r[0..64].copy_from_slice(&sig.serialize()[..]);
		r[64] = recovery_id.serialize();
		EcdsaSignature(r)
	}
}

#[cfg(test)]
mod tests {
	use secp256k1;
	use hex_literal::hex;
	use super::*;
	use secp_utils::*;

	use sp_core::H256;
	use codec::Encode;
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are required.
	use sp_runtime::{Perbill, traits::{BlakeTwo256, IdentityLookup, Identity}, testing::Header};
	use frame_support::{
		impl_outer_origin, assert_ok, assert_err, assert_noop, parameter_types
	};
	use balances;

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
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
		type ModuleToIndex = ();
		type AccountData = balances::AccountData<u64>;
		type MigrateAccount = (); type OnNewAccount = ();
		type OnKilledAccount = Balances;
	}

	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
		pub const CreationFee: u64 = 0;
		pub const MinVestedTransfer: u64 = 0;
	}

	impl balances::Trait for Test {
		type Balance = u64;
		type Event = ();
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
	}

	impl vesting::Trait for Test {
		type Event = ();
		type Currency = Balances;
		type BlockNumberToBalance = Identity;
		type MinVestedTransfer = MinVestedTransfer;
	}

	parameter_types!{
		pub const Prefix: &'static [u8] = b"Pay RUSTs to the TEST account:";
	}

	impl Trait for Test {
		type Event = ();
		type VestingSchedule = Vesting;
		type Prefix = Prefix;
	}
	type System = system::Module<Test>;
	type Balances = balances::Module<Test>;
	type Vesting = vesting::Module<Test>;
	type Claims = Module<Test>;

	fn alice() -> secp256k1::SecretKey {
		secp256k1::SecretKey::parse(&keccak_256(b"Alice")).unwrap()
	}
	fn bob() -> secp256k1::SecretKey {
		secp256k1::SecretKey::parse(&keccak_256(b"Bob")).unwrap()
	}

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
		// We use default for brevity, but you can configure as desired if needed.
		balances::GenesisConfig::<Test>::default().assimilate_storage(&mut t).unwrap();
		GenesisConfig::<Test>{
			claims: vec![(eth(&alice()), 100)],
			vesting: vec![(eth(&alice()), (50, 10, 1))],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Claims::total(), 100);
			assert_eq!(Claims::claims(&eth(&alice())), Some(100));
			assert_eq!(Claims::claims(&EthereumAddress::default()), None);
			assert_eq!(Claims::vesting(&eth(&alice())), Some((50, 10, 1)));
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
			assert_eq!(Balances::free_balance(42), 0);
			assert_ok!(Claims::claim(Origin::NONE, 42, sig::<Test>(&alice(), &42u64.encode())));
			assert_eq!(Balances::free_balance(&42), 100);
			assert_eq!(Vesting::vesting_balance(&42), Some(50));
			assert_eq!(Claims::total(), 0);
		});
	}

	#[test]
	fn add_claim_works() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Claims::mint_claim(Origin::signed(42), eth(&bob()), 200, None),
				sp_runtime::traits::BadOrigin,
			);
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::claim(Origin::NONE, 69, sig::<Test>(&bob(), &69u64.encode())),
				Error::<Test>::SignerHasNoClaim,
			);
			assert_ok!(Claims::mint_claim(Origin::ROOT, eth(&bob()), 200, None));
			assert_eq!(Claims::total(), 300);
			assert_ok!(Claims::claim(Origin::NONE, 69, sig::<Test>(&bob(), &69u64.encode())));
			assert_eq!(Balances::free_balance(&69), 200);
			assert_eq!(Vesting::vesting_balance(&69), None);
			assert_eq!(Claims::total(), 100);
		});
	}

	#[test]
	fn add_claim_with_vesting_works() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Claims::mint_claim(Origin::signed(42), eth(&bob()), 200, Some((50, 10, 1))),
				sp_runtime::traits::BadOrigin,
			);
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::claim(Origin::NONE, 69, sig::<Test>(&bob(), &69u64.encode())),
				Error::<Test>::SignerHasNoClaim
			);
			assert_ok!(Claims::mint_claim(Origin::ROOT, eth(&bob()), 200, Some((50, 10, 1))));
			assert_ok!(Claims::claim(Origin::NONE, 69, sig::<Test>(&bob(), &69u64.encode())));
			assert_eq!(Balances::free_balance(&69), 200);
			assert_eq!(Vesting::vesting_balance(&69), Some(50));
		});
	}

	#[test]
	fn origin_signed_claiming_fail() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_err!(
				Claims::claim(Origin::signed(42), 42, sig::<Test>(&alice(), &42u64.encode())),
				sp_runtime::traits::BadOrigin,
			);
		});
	}

	#[test]
	fn double_claiming_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_ok!(Claims::claim(Origin::NONE, 42, sig::<Test>(&alice(), &42u64.encode())));
			assert_noop!(
				Claims::claim(Origin::NONE, 42, sig::<Test>(&alice(), &42u64.encode())),
				Error::<Test>::SignerHasNoClaim
			);
		});
	}

	#[test]
	fn claiming_while_vested_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Claims::total(), 100);
			// A user is already vested
			assert_ok!(<Test as Trait>::VestingSchedule::add_vesting_schedule(&69, 1000, 100, 10));
			CurrencyOf::<Test>::make_free_balance_be(&69, 1000);
			assert_eq!(Balances::free_balance(69), 1000);
			assert_ok!(Claims::mint_claim(Origin::ROOT, eth(&bob()), 200, Some((50, 10, 1))));
			// New total
			assert_eq!(Claims::total(), 300);

			// They should not be able to claim
			assert_noop!(
				Claims::claim(Origin::NONE, 69, sig::<Test>(&bob(), &69u64.encode())),
				Error::<Test>::DestinationVesting
			);
			// Everything should be unchanged
			assert_eq!(Claims::total(), 300);
			assert_eq!(Balances::free_balance(69), 1000);
			assert_eq!(Vesting::vesting_balance(&69), Some(1000));
		});
	}

	#[test]
	fn non_sender_sig_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::claim(Origin::NONE, 42, sig::<Test>(&alice(), &69u64.encode())),
				Error::<Test>::SignerHasNoClaim
			);
		});
	}

	#[test]
	fn non_claimant_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::claim(Origin::NONE, 42, sig::<Test>(&bob(), &69u64.encode())),
				Error::<Test>::SignerHasNoClaim
			);
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
		use sp_runtime::traits::ValidateUnsigned;

		new_test_ext().execute_with(|| {
			assert_eq!(
				<Module<Test>>::validate_unsigned(&Call::claim(1, sig::<Test>(&alice(), &1u64.encode()))),
				Ok(ValidTransaction {
					priority: 100,
					requires: vec![],
					provides: vec![("claims", eth(&alice())).encode()],
					longevity: TransactionLongevity::max_value(),
					propagate: true,
				})
			);
			assert_eq!(
				<Module<Test>>::validate_unsigned(&Call::claim(0, EcdsaSignature([0; 65]))),
				InvalidTransaction::Custom(ValidityError::InvalidEthereumSignature.into()).into(),
			);
			assert_eq!(
				<Module<Test>>::validate_unsigned(&Call::claim(1, sig::<Test>(&bob(), &1u64.encode()))),
				InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into()).into(),
			);
			assert_eq!(
				<Module<Test>>::validate_unsigned(&Call::claim(0, sig::<Test>(&bob(), &1u64.encode()))),
				InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into()).into(),
			);
		});
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking {
	use super::*;
	use secp_utils::*;
	use system::RawOrigin;
	use frame_benchmarking::{benchmarks, account};
	use sp_runtime::DispatchResult;
	use sp_runtime::traits::ValidateUnsigned;
	use crate::claims::Call;

	const SEED: u32 = 0;

	const MAX_CLAIMS: u32 = 10_000;
	const VALUE: u32 = 1_000_000;

	fn create_claim<T: Trait>(input: u32) -> DispatchResult {
		let secret_key = secp256k1::SecretKey::parse(&keccak_256(&input.encode())).unwrap();
		let eth_address = eth(&secret_key);
		let vesting = Some((100_000.into(), 1_000.into(), 100.into()));
		super::Module::<T>::mint_claim(RawOrigin::Root.into(), eth_address, VALUE.into(), vesting)?;
		Ok(())
	}

	benchmarks! {
		_ {
			// Create claims in storage.
			let c in 0 .. MAX_CLAIMS => create_claim::<T>(c)?;
		}

		// Benchmark `claim` for different users.
		claim {
			let u in 0 .. 1000;
			let secret_key = secp256k1::SecretKey::parse(&keccak_256(&u.encode())).unwrap();
			let eth_address = eth(&secret_key);
			let account: T::AccountId = account("user", u, SEED);
			let vesting = Some((100_000.into(), 1_000.into(), 100.into()));
			let signature = sig::<T>(&secret_key, &account.encode());
			super::Module::<T>::mint_claim(RawOrigin::Root.into(), eth_address, VALUE.into(), vesting)?;
		}: _(RawOrigin::None, account, signature)

		// Benchmark `mint_claim` when there already exists `c` claims in storage.
		mint_claim {
			let c in ...;
			let account = account("user", c, SEED);
			let vesting = Some((100_000.into(), 1_000.into(), 100.into()));
		}: _(RawOrigin::Root, account, VALUE.into(), vesting)

		// Benchmark the time it takes to execute `validate_unsigned`
		validate_unsigned {
			let c in ...;
			// Crate signature
			let secret_key = secp256k1::SecretKey::parse(&keccak_256(&c.encode())).unwrap();
			let account: T::AccountId = account("user", c, SEED);
			let signature = sig::<T>(&secret_key, &account.encode());
			let call = Call::<T>::claim(account, signature);
		}: {
			super::Module::<T>::validate_unsigned(&call)?
		}

		// Benchmark the time it takes to do `repeat` number of keccak256 hashes
		keccak256 {
			let i in 0 .. 10_000;
			let bytes = (i).encode();
		}: {
			for index in 0 .. i {
				let _hash = keccak_256(&bytes);
			}
		}

		// Benchmark the time it takes to do `repeat` number of `eth_recover`
		eth_recover {
			let i in 0 .. 1_000;
			// Crate signature
			let secret_key = secp256k1::SecretKey::parse(&keccak_256(&i.encode())).unwrap();
			let account: T::AccountId = account("user", i, SEED);
			let signature = sig::<T>(&secret_key, &account.encode());
			let data = account.using_encoded(to_ascii_hex);
		}: {
			for _ in 0 .. i {
				let _maybe_signer = super::Module::<T>::eth_recover(&signature, &data);
			}
		}
	}
}
