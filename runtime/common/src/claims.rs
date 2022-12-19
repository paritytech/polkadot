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

//! Pallet to process claims from Ethereum addresses.

use frame_support::{
	ensure,
	traits::{Currency, Get, IsSubType, VestingSchedule},
	weights::Weight,
};
pub use pallet::*;
use parity_scale_codec::{Decode, Encode};
use primitives::v2::ValidityError;
use scale_info::TypeInfo;
#[cfg(feature = "std")]
use serde::{self, Deserialize, Deserializer, Serialize, Serializer};
use sp_io::{crypto::secp256k1_ecdsa_recover, hashing::keccak_256};
#[cfg(feature = "std")]
use sp_runtime::traits::Zero;
use sp_runtime::{
	traits::{CheckedSub, DispatchInfoOf, SignedExtension},
	transaction_validity::{
		InvalidTransaction, TransactionValidity, TransactionValidityError, ValidTransaction,
	},
	RuntimeDebug,
};
use sp_std::{fmt::Debug, prelude::*};

type CurrencyOf<T> = <<T as Config>::VestingSchedule as VestingSchedule<
	<T as frame_system::Config>::AccountId,
>>::Currency;
type BalanceOf<T> = <CurrencyOf<T> as Currency<<T as frame_system::Config>::AccountId>>::Balance;

pub trait WeightInfo {
	fn claim() -> Weight;
	fn mint_claim() -> Weight;
	fn claim_attest() -> Weight;
	fn attest() -> Weight;
	fn move_claim() -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn claim() -> Weight {
		Weight::zero()
	}
	fn mint_claim() -> Weight {
		Weight::zero()
	}
	fn claim_attest() -> Weight {
		Weight::zero()
	}
	fn attest() -> Weight {
		Weight::zero()
	}
	fn move_claim() -> Weight {
		Weight::zero()
	}
}

/// The kind of statement an account needs to make for a claim to be valid.
#[derive(Encode, Decode, Clone, Copy, Eq, PartialEq, RuntimeDebug, TypeInfo)]
#[cfg_attr(feature = "std", derive(Serialize, Deserialize))]
pub enum StatementKind {
	/// Statement required to be made by non-SAFT holders.
	Regular,
	/// Statement required to be made by SAFT holders.
	Saft,
}

impl StatementKind {
	/// Convert this to the (English) statement it represents.
	fn to_text(self) -> &'static [u8] {
		match self {
			StatementKind::Regular =>
				&b"I hereby agree to the terms of the statement whose SHA-256 multihash is \
				Qmc1XYqT6S39WNp2UeiRUrZichUWUPpGEThDE6dAb3f6Ny. (This may be found at the URL: \
				https://statement.polkadot.network/regular.html)"[..],
			StatementKind::Saft =>
				&b"I hereby agree to the terms of the statement whose SHA-256 multihash is \
				QmXEkMahfhHJPzT3RjkXiZVFi77ZeVeuxtAjhojGRNYckz. (This may be found at the URL: \
				https://statement.polkadot.network/saft.html)"[..],
		}
	}
}

impl Default for StatementKind {
	fn default() -> Self {
		StatementKind::Regular
	}
}

/// An Ethereum address (i.e. 20 bytes, used to represent an Ethereum account).
///
/// This gets serialized to the 0x-prefixed hex representation.
#[derive(Clone, Copy, PartialEq, Eq, Encode, Decode, Default, RuntimeDebug, TypeInfo)]
pub struct EthereumAddress([u8; 20]);

#[cfg(feature = "std")]
impl Serialize for EthereumAddress {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: Serializer,
	{
		let hex: String = rustc_hex::ToHex::to_hex(&self.0[..]);
		serializer.serialize_str(&format!("0x{}", hex))
	}
}

#[cfg(feature = "std")]
impl<'de> Deserialize<'de> for EthereumAddress {
	fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
	where
		D: Deserializer<'de>,
	{
		let base_string = String::deserialize(deserializer)?;
		let offset = if base_string.starts_with("0x") { 2 } else { 0 };
		let s = &base_string[offset..];
		if s.len() != 40 {
			Err(serde::de::Error::custom(
				"Bad length of Ethereum address (should be 42 including '0x')",
			))?;
		}
		let raw: Vec<u8> = rustc_hex::FromHex::from_hex(s)
			.map_err(|e| serde::de::Error::custom(format!("{:?}", e)))?;
		let mut r = Self::default();
		r.0.copy_from_slice(&raw);
		Ok(r)
	}
}

#[derive(Encode, Decode, Clone, TypeInfo)]
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

#[frame_support::pallet]
pub mod pallet {
	use super::*;
	use frame_support::pallet_prelude::*;
	use frame_system::pallet_prelude::*;

	#[pallet::pallet]
	#[pallet::generate_store(pub(super) trait Store)]
	#[pallet::without_storage_info]
	pub struct Pallet<T>(_);

	/// Configuration trait.
	#[pallet::config]
	pub trait Config: frame_system::Config {
		/// The overarching event type.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
		type VestingSchedule: VestingSchedule<Self::AccountId, Moment = Self::BlockNumber>;
		#[pallet::constant]
		type Prefix: Get<&'static [u8]>;
		type MoveClaimOrigin: EnsureOrigin<Self::RuntimeOrigin>;
		type WeightInfo: WeightInfo;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		/// Someone claimed some DOTs.
		Claimed { who: T::AccountId, ethereum_address: EthereumAddress, amount: BalanceOf<T> },
	}

	#[pallet::error]
	pub enum Error<T> {
		/// Invalid Ethereum signature.
		InvalidEthereumSignature,
		/// Ethereum address has no claim.
		SignerHasNoClaim,
		/// Account ID sending transaction has no claim.
		SenderHasNoClaim,
		/// There's not enough in the pot to pay out some unvested amount. Generally implies a logic
		/// error.
		PotUnderflow,
		/// A needed statement was not included.
		InvalidStatement,
		/// The account already has a vested balance.
		VestedBalanceExists,
	}

	#[pallet::storage]
	#[pallet::getter(fn claims)]
	pub(super) type Claims<T: Config> = StorageMap<_, Identity, EthereumAddress, BalanceOf<T>>;

	#[pallet::storage]
	#[pallet::getter(fn total)]
	pub(super) type Total<T: Config> = StorageValue<_, BalanceOf<T>, ValueQuery>;

	/// Vesting schedule for a claim.
	/// First balance is the total amount that should be held for vesting.
	/// Second balance is how much should be unlocked per block.
	/// The block number is when the vesting should start.
	#[pallet::storage]
	#[pallet::getter(fn vesting)]
	pub(super) type Vesting<T: Config> =
		StorageMap<_, Identity, EthereumAddress, (BalanceOf<T>, BalanceOf<T>, T::BlockNumber)>;

	/// The statement kind that must be signed, if any.
	#[pallet::storage]
	pub(super) type Signing<T> = StorageMap<_, Identity, EthereumAddress, StatementKind>;

	/// Pre-claimed Ethereum accounts, by the Account ID that they are claimed to.
	#[pallet::storage]
	pub(super) type Preclaims<T: Config> = StorageMap<_, Identity, T::AccountId, EthereumAddress>;

	#[pallet::genesis_config]
	pub struct GenesisConfig<T: Config> {
		pub claims:
			Vec<(EthereumAddress, BalanceOf<T>, Option<T::AccountId>, Option<StatementKind>)>,
		pub vesting: Vec<(EthereumAddress, (BalanceOf<T>, BalanceOf<T>, T::BlockNumber))>,
	}

	#[cfg(feature = "std")]
	impl<T: Config> Default for GenesisConfig<T> {
		fn default() -> Self {
			GenesisConfig { claims: Default::default(), vesting: Default::default() }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
		fn build(&self) {
			// build `Claims`
			self.claims.iter().map(|(a, b, _, _)| (*a, *b)).for_each(|(a, b)| {
				Claims::<T>::insert(a, b);
			});
			// build `Total`
			Total::<T>::put(
				self.claims
					.iter()
					.fold(Zero::zero(), |acc: BalanceOf<T>, &(_, b, _, _)| acc + b),
			);
			// build `Vesting`
			self.vesting.iter().for_each(|(k, v)| {
				Vesting::<T>::insert(k, v);
			});
			// build `Signing`
			self.claims
				.iter()
				.filter_map(|(a, _, _, s)| Some((*a, (*s)?)))
				.for_each(|(a, s)| {
					Signing::<T>::insert(a, s);
				});
			// build `Preclaims`
			self.claims.iter().filter_map(|(a, _, i, _)| Some((i.clone()?, *a))).for_each(
				|(i, a)| {
					Preclaims::<T>::insert(i, a);
				},
			);
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
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
		/// Weight includes logic to validate unsigned `claim` call.
		///
		/// Total Complexity: O(1)
		/// </weight>
		#[pallet::weight(T::WeightInfo::claim())]
		pub fn claim(
			origin: OriginFor<T>,
			dest: T::AccountId,
			ethereum_signature: EcdsaSignature,
		) -> DispatchResult {
			ensure_none(origin)?;

			let data = dest.using_encoded(to_ascii_hex);
			let signer = Self::eth_recover(&ethereum_signature, &data, &[][..])
				.ok_or(Error::<T>::InvalidEthereumSignature)?;
			ensure!(Signing::<T>::get(&signer).is_none(), Error::<T>::InvalidStatement);

			Self::process_claim(signer, dest)?;
			Ok(())
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
		/// We assume worst case that both vesting and statement is being inserted.
		///
		/// Total Complexity: O(1)
		/// </weight>
		#[pallet::weight(T::WeightInfo::mint_claim())]
		pub fn mint_claim(
			origin: OriginFor<T>,
			who: EthereumAddress,
			value: BalanceOf<T>,
			vesting_schedule: Option<(BalanceOf<T>, BalanceOf<T>, T::BlockNumber)>,
			statement: Option<StatementKind>,
		) -> DispatchResult {
			ensure_root(origin)?;

			<Total<T>>::mutate(|t| *t += value);
			<Claims<T>>::insert(who, value);
			if let Some(vs) = vesting_schedule {
				<Vesting<T>>::insert(who, vs);
			}
			if let Some(s) = statement {
				Signing::<T>::insert(who, s);
			}
			Ok(())
		}

		/// Make a claim to collect your DOTs by signing a statement.
		///
		/// The dispatch origin for this call must be _None_.
		///
		/// Unsigned Validation:
		/// A call to `claim_attest` is deemed valid if the signature provided matches
		/// the expected signed message of:
		///
		/// > Ethereum Signed Message:
		/// > (configured prefix string)(address)(statement)
		///
		/// and `address` matches the `dest` account; the `statement` must match that which is
		/// expected according to your purchase arrangement.
		///
		/// Parameters:
		/// - `dest`: The destination account to payout the claim.
		/// - `ethereum_signature`: The signature of an ethereum signed message
		///    matching the format described above.
		/// - `statement`: The identity of the statement which is being attested to in the signature.
		///
		/// <weight>
		/// The weight of this call is invariant over the input parameters.
		/// Weight includes logic to validate unsigned `claim_attest` call.
		///
		/// Total Complexity: O(1)
		/// </weight>
		#[pallet::weight(T::WeightInfo::claim_attest())]
		pub fn claim_attest(
			origin: OriginFor<T>,
			dest: T::AccountId,
			ethereum_signature: EcdsaSignature,
			statement: Vec<u8>,
		) -> DispatchResult {
			ensure_none(origin)?;

			let data = dest.using_encoded(to_ascii_hex);
			let signer = Self::eth_recover(&ethereum_signature, &data, &statement)
				.ok_or(Error::<T>::InvalidEthereumSignature)?;
			if let Some(s) = Signing::<T>::get(signer) {
				ensure!(s.to_text() == &statement[..], Error::<T>::InvalidStatement);
			}
			Self::process_claim(signer, dest)?;
			Ok(())
		}

		/// Attest to a statement, needed to finalize the claims process.
		///
		/// WARNING: Insecure unless your chain includes `PrevalidateAttests` as a `SignedExtension`.
		///
		/// Unsigned Validation:
		/// A call to attest is deemed valid if the sender has a `Preclaim` registered
		/// and provides a `statement` which is expected for the account.
		///
		/// Parameters:
		/// - `statement`: The identity of the statement which is being attested to in the signature.
		///
		/// <weight>
		/// The weight of this call is invariant over the input parameters.
		/// Weight includes logic to do pre-validation on `attest` call.
		///
		/// Total Complexity: O(1)
		/// </weight>
		#[pallet::weight((
			T::WeightInfo::attest(),
			DispatchClass::Normal,
			Pays::No
		))]
		pub fn attest(origin: OriginFor<T>, statement: Vec<u8>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let signer = Preclaims::<T>::get(&who).ok_or(Error::<T>::SenderHasNoClaim)?;
			if let Some(s) = Signing::<T>::get(signer) {
				ensure!(s.to_text() == &statement[..], Error::<T>::InvalidStatement);
			}
			Self::process_claim(signer, who.clone())?;
			Preclaims::<T>::remove(&who);
			Ok(())
		}

		#[pallet::weight(T::WeightInfo::move_claim())]
		pub fn move_claim(
			origin: OriginFor<T>,
			old: EthereumAddress,
			new: EthereumAddress,
			maybe_preclaim: Option<T::AccountId>,
		) -> DispatchResultWithPostInfo {
			T::MoveClaimOrigin::try_origin(origin).map(|_| ()).or_else(ensure_root)?;

			Claims::<T>::take(&old).map(|c| Claims::<T>::insert(&new, c));
			Vesting::<T>::take(&old).map(|c| Vesting::<T>::insert(&new, c));
			Signing::<T>::take(&old).map(|c| Signing::<T>::insert(&new, c));
			maybe_preclaim.map(|preclaim| {
				Preclaims::<T>::mutate(&preclaim, |maybe_o| {
					if maybe_o.as_ref().map_or(false, |o| o == &old) {
						*maybe_o = Some(new)
					}
				})
			});
			Ok(Pays::No.into())
		}
	}

	#[pallet::validate_unsigned]
	impl<T: Config> ValidateUnsigned for Pallet<T> {
		type Call = Call<T>;

		fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
			const PRIORITY: u64 = 100;

			let (maybe_signer, maybe_statement) = match call {
				// <weight>
				// The weight of this logic is included in the `claim` dispatchable.
				// </weight>
				Call::claim { dest: account, ethereum_signature } => {
					let data = account.using_encoded(to_ascii_hex);
					(Self::eth_recover(&ethereum_signature, &data, &[][..]), None)
				},
				// <weight>
				// The weight of this logic is included in the `claim_attest` dispatchable.
				// </weight>
				Call::claim_attest { dest: account, ethereum_signature, statement } => {
					let data = account.using_encoded(to_ascii_hex);
					(
						Self::eth_recover(&ethereum_signature, &data, &statement),
						Some(statement.as_slice()),
					)
				},
				_ => return Err(InvalidTransaction::Call.into()),
			};

			let signer = maybe_signer.ok_or(InvalidTransaction::Custom(
				ValidityError::InvalidEthereumSignature.into(),
			))?;

			let e = InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into());
			ensure!(<Claims<T>>::contains_key(&signer), e);

			let e = InvalidTransaction::Custom(ValidityError::InvalidStatement.into());
			match Signing::<T>::get(signer) {
				None => ensure!(maybe_statement.is_none(), e),
				Some(s) => ensure!(Some(s.to_text()) == maybe_statement, e),
			}

			Ok(ValidTransaction {
				priority: PRIORITY,
				requires: vec![],
				provides: vec![("claims", signer).encode()],
				longevity: TransactionLongevity::max_value(),
				propagate: true,
			})
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

impl<T: Config> Pallet<T> {
	// Constructs the message that Ethereum RPC's `personal_sign` and `eth_sign` would sign.
	fn ethereum_signable_message(what: &[u8], extra: &[u8]) -> Vec<u8> {
		let prefix = T::Prefix::get();
		let mut l = prefix.len() + what.len() + extra.len();
		let mut rev = Vec::new();
		while l > 0 {
			rev.push(b'0' + (l % 10) as u8);
			l /= 10;
		}
		let mut v = b"\x19Ethereum Signed Message:\n".to_vec();
		v.extend(rev.into_iter().rev());
		v.extend_from_slice(prefix);
		v.extend_from_slice(what);
		v.extend_from_slice(extra);
		v
	}

	// Attempts to recover the Ethereum address from a message signature signed by using
	// the Ethereum RPC's `personal_sign` and `eth_sign`.
	fn eth_recover(s: &EcdsaSignature, what: &[u8], extra: &[u8]) -> Option<EthereumAddress> {
		let msg = keccak_256(&Self::ethereum_signable_message(what, extra));
		let mut res = EthereumAddress::default();
		res.0
			.copy_from_slice(&keccak_256(&secp256k1_ecdsa_recover(&s.0, &msg).ok()?[..])[12..]);
		Some(res)
	}

	fn process_claim(signer: EthereumAddress, dest: T::AccountId) -> sp_runtime::DispatchResult {
		let balance_due = <Claims<T>>::get(&signer).ok_or(Error::<T>::SignerHasNoClaim)?;

		let new_total = Self::total().checked_sub(&balance_due).ok_or(Error::<T>::PotUnderflow)?;

		let vesting = Vesting::<T>::get(&signer);
		if vesting.is_some() && T::VestingSchedule::vesting_balance(&dest).is_some() {
			return Err(Error::<T>::VestedBalanceExists.into())
		}

		// We first need to deposit the balance to ensure that the account exists.
		CurrencyOf::<T>::deposit_creating(&dest, balance_due);

		// Check if this claim should have a vesting schedule.
		if let Some(vs) = vesting {
			// This can only fail if the account already has a vesting schedule,
			// but this is checked above.
			T::VestingSchedule::add_vesting_schedule(&dest, vs.0, vs.1, vs.2)
				.expect("No other vesting schedule exists, as checked above; qed");
		}

		<Total<T>>::put(new_total);
		<Claims<T>>::remove(&signer);
		<Vesting<T>>::remove(&signer);
		Signing::<T>::remove(&signer);

		// Let's deposit an event to let the outside world know this happened.
		Self::deposit_event(Event::<T>::Claimed {
			who: dest,
			ethereum_address: signer,
			amount: balance_due,
		});

		Ok(())
	}
}

/// Validate `attest` calls prior to execution. Needed to avoid a DoS attack since they are
/// otherwise free to place on chain.
#[derive(Encode, Decode, Clone, Eq, PartialEq, TypeInfo)]
#[scale_info(skip_type_params(T))]
pub struct PrevalidateAttests<T: Config + Send + Sync>(sp_std::marker::PhantomData<T>)
where
	<T as frame_system::Config>::RuntimeCall: IsSubType<Call<T>>;

impl<T: Config + Send + Sync> Debug for PrevalidateAttests<T>
where
	<T as frame_system::Config>::RuntimeCall: IsSubType<Call<T>>,
{
	#[cfg(feature = "std")]
	fn fmt(&self, f: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		write!(f, "PrevalidateAttests")
	}

	#[cfg(not(feature = "std"))]
	fn fmt(&self, _: &mut sp_std::fmt::Formatter) -> sp_std::fmt::Result {
		Ok(())
	}
}

impl<T: Config + Send + Sync> PrevalidateAttests<T>
where
	<T as frame_system::Config>::RuntimeCall: IsSubType<Call<T>>,
{
	/// Create new `SignedExtension` to check runtime version.
	pub fn new() -> Self {
		Self(sp_std::marker::PhantomData)
	}
}

impl<T: Config + Send + Sync> SignedExtension for PrevalidateAttests<T>
where
	<T as frame_system::Config>::RuntimeCall: IsSubType<Call<T>>,
{
	type AccountId = T::AccountId;
	type Call = <T as frame_system::Config>::RuntimeCall;
	type AdditionalSigned = ();
	type Pre = ();
	const IDENTIFIER: &'static str = "PrevalidateAttests";

	fn additional_signed(&self) -> Result<Self::AdditionalSigned, TransactionValidityError> {
		Ok(())
	}

	fn pre_dispatch(
		self,
		who: &Self::AccountId,
		call: &Self::Call,
		info: &DispatchInfoOf<Self::Call>,
		len: usize,
	) -> Result<Self::Pre, TransactionValidityError> {
		self.validate(who, call, info, len).map(|_| ())
	}

	// <weight>
	// The weight of this logic is included in the `attest` dispatchable.
	// </weight>
	fn validate(
		&self,
		who: &Self::AccountId,
		call: &Self::Call,
		_info: &DispatchInfoOf<Self::Call>,
		_len: usize,
	) -> TransactionValidity {
		if let Some(local_call) = call.is_sub_type() {
			if let Call::attest { statement: attested_statement } = local_call {
				let signer = Preclaims::<T>::get(who)
					.ok_or(InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into()))?;
				if let Some(s) = Signing::<T>::get(signer) {
					let e = InvalidTransaction::Custom(ValidityError::InvalidStatement.into());
					ensure!(&attested_statement[..] == s.to_text(), e);
				}
			}
		}
		Ok(ValidTransaction::default())
	}
}

#[cfg(any(test, feature = "runtime-benchmarks"))]
mod secp_utils {
	use super::*;

	pub fn public(secret: &libsecp256k1::SecretKey) -> libsecp256k1::PublicKey {
		libsecp256k1::PublicKey::from_secret_key(secret)
	}
	pub fn eth(secret: &libsecp256k1::SecretKey) -> EthereumAddress {
		let mut res = EthereumAddress::default();
		res.0.copy_from_slice(&keccak_256(&public(secret).serialize()[1..65])[12..]);
		res
	}
	pub fn sig<T: Config>(
		secret: &libsecp256k1::SecretKey,
		what: &[u8],
		extra: &[u8],
	) -> EcdsaSignature {
		let msg = keccak_256(&<super::Pallet<T>>::ethereum_signable_message(
			&to_ascii_hex(what)[..],
			extra,
		));
		let (sig, recovery_id) = libsecp256k1::sign(&libsecp256k1::Message::parse(&msg), secret);
		let mut r = [0u8; 65];
		r[0..64].copy_from_slice(&sig.serialize()[..]);
		r[64] = recovery_id.serialize();
		EcdsaSignature(r)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use hex_literal::hex;
	use secp_utils::*;

	use parity_scale_codec::Encode;
	use sp_core::H256;
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are required.
	use crate::claims;
	use claims::Call as ClaimsCall;
	use frame_support::{
		assert_err, assert_noop, assert_ok,
		dispatch::{DispatchError::BadOrigin, GetDispatchInfo, Pays},
		ord_parameter_types, parameter_types,
		traits::{ExistenceRequirement, GenesisBuild, WithdrawReasons},
	};
	use pallet_balances;
	use sp_runtime::{
		testing::Header,
		traits::{BlakeTwo256, Identity, IdentityLookup},
		transaction_validity::TransactionLongevity,
	};

	type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
	type Block = frame_system::mocking::MockBlock<Test>;

	frame_support::construct_runtime!(
		pub enum Test where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
			Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
			Vesting: pallet_vesting::{Pallet, Call, Storage, Config<T>, Event<T>},
			Claims: claims::{Pallet, Call, Storage, Config<T>, Event<T>, ValidateUnsigned},
		}
	);

	parameter_types! {
		pub const BlockHashCount: u32 = 250;
	}
	impl frame_system::Config for Test {
		type BaseCallFilter = frame_support::traits::Everything;
		type BlockWeights = ();
		type BlockLength = ();
		type DbWeight = ();
		type RuntimeOrigin = RuntimeOrigin;
		type RuntimeCall = RuntimeCall;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type RuntimeEvent = RuntimeEvent;
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = pallet_balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
		type OnSetCode = ();
		type MaxConsumers = frame_support::traits::ConstU32<16>;
	}

	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
	}

	impl pallet_balances::Config for Test {
		type Balance = u64;
		type RuntimeEvent = RuntimeEvent;
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type MaxLocks = ();
		type MaxReserves = ();
		type ReserveIdentifier = [u8; 8];
		type WeightInfo = ();
	}

	parameter_types! {
		pub const MinVestedTransfer: u64 = 1;
		pub UnvestedFundsAllowedWithdrawReasons: WithdrawReasons =
			WithdrawReasons::except(WithdrawReasons::TRANSFER | WithdrawReasons::RESERVE);
	}

	impl pallet_vesting::Config for Test {
		type RuntimeEvent = RuntimeEvent;
		type Currency = Balances;
		type BlockNumberToBalance = Identity;
		type MinVestedTransfer = MinVestedTransfer;
		type WeightInfo = ();
		type UnvestedFundsAllowedWithdrawReasons = UnvestedFundsAllowedWithdrawReasons;
		const MAX_VESTING_SCHEDULES: u32 = 28;
	}

	parameter_types! {
		pub Prefix: &'static [u8] = b"Pay RUSTs to the TEST account:";
	}
	ord_parameter_types! {
		pub const Six: u64 = 6;
	}

	impl Config for Test {
		type RuntimeEvent = RuntimeEvent;
		type VestingSchedule = Vesting;
		type Prefix = Prefix;
		type MoveClaimOrigin = frame_system::EnsureSignedBy<Six, u64>;
		type WeightInfo = TestWeightInfo;
	}

	fn alice() -> libsecp256k1::SecretKey {
		libsecp256k1::SecretKey::parse(&keccak_256(b"Alice")).unwrap()
	}
	fn bob() -> libsecp256k1::SecretKey {
		libsecp256k1::SecretKey::parse(&keccak_256(b"Bob")).unwrap()
	}
	fn dave() -> libsecp256k1::SecretKey {
		libsecp256k1::SecretKey::parse(&keccak_256(b"Dave")).unwrap()
	}
	fn eve() -> libsecp256k1::SecretKey {
		libsecp256k1::SecretKey::parse(&keccak_256(b"Eve")).unwrap()
	}
	fn frank() -> libsecp256k1::SecretKey {
		libsecp256k1::SecretKey::parse(&keccak_256(b"Frank")).unwrap()
	}

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		// We use default for brevity, but you can configure as desired if needed.
		pallet_balances::GenesisConfig::<Test>::default()
			.assimilate_storage(&mut t)
			.unwrap();
		claims::GenesisConfig::<Test> {
			claims: vec![
				(eth(&alice()), 100, None, None),
				(eth(&dave()), 200, None, Some(StatementKind::Regular)),
				(eth(&eve()), 300, Some(42), Some(StatementKind::Saft)),
				(eth(&frank()), 400, Some(43), None),
			],
			vesting: vec![(eth(&alice()), (50, 10, 1))],
		}
		.assimilate_storage(&mut t)
		.unwrap();
		t.into()
	}

	fn total_claims() -> u64 {
		100 + 200 + 300 + 400
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Claims::total(), total_claims());
			assert_eq!(Claims::claims(&eth(&alice())), Some(100));
			assert_eq!(Claims::claims(&eth(&dave())), Some(200));
			assert_eq!(Claims::claims(&eth(&eve())), Some(300));
			assert_eq!(Claims::claims(&eth(&frank())), Some(400));
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
			assert_ok!(Claims::claim(
				RuntimeOrigin::none(),
				42,
				sig::<Test>(&alice(), &42u64.encode(), &[][..])
			));
			assert_eq!(Balances::free_balance(&42), 100);
			assert_eq!(Vesting::vesting_balance(&42), Some(50));
			assert_eq!(Claims::total(), total_claims() - 100);
		});
	}

	#[test]
	fn basic_claim_moving_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::move_claim(RuntimeOrigin::signed(1), eth(&alice()), eth(&bob()), None),
				BadOrigin
			);
			assert_ok!(Claims::move_claim(
				RuntimeOrigin::signed(6),
				eth(&alice()),
				eth(&bob()),
				None
			));
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					42,
					sig::<Test>(&alice(), &42u64.encode(), &[][..])
				),
				Error::<Test>::SignerHasNoClaim
			);
			assert_ok!(Claims::claim(
				RuntimeOrigin::none(),
				42,
				sig::<Test>(&bob(), &42u64.encode(), &[][..])
			));
			assert_eq!(Balances::free_balance(&42), 100);
			assert_eq!(Vesting::vesting_balance(&42), Some(50));
			assert_eq!(Claims::total(), total_claims() - 100);
		});
	}

	#[test]
	fn claim_attest_moving_works() {
		new_test_ext().execute_with(|| {
			assert_ok!(Claims::move_claim(
				RuntimeOrigin::signed(6),
				eth(&dave()),
				eth(&bob()),
				None
			));
			let s = sig::<Test>(&bob(), &42u64.encode(), StatementKind::Regular.to_text());
			assert_ok!(Claims::claim_attest(
				RuntimeOrigin::none(),
				42,
				s,
				StatementKind::Regular.to_text().to_vec()
			));
			assert_eq!(Balances::free_balance(&42), 200);
		});
	}

	#[test]
	fn attest_moving_works() {
		new_test_ext().execute_with(|| {
			assert_ok!(Claims::move_claim(
				RuntimeOrigin::signed(6),
				eth(&eve()),
				eth(&bob()),
				Some(42)
			));
			assert_ok!(Claims::attest(
				RuntimeOrigin::signed(42),
				StatementKind::Saft.to_text().to_vec()
			));
			assert_eq!(Balances::free_balance(&42), 300);
		});
	}

	#[test]
	fn claiming_does_not_bypass_signing() {
		new_test_ext().execute_with(|| {
			assert_ok!(Claims::claim(
				RuntimeOrigin::none(),
				42,
				sig::<Test>(&alice(), &42u64.encode(), &[][..])
			));
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					42,
					sig::<Test>(&dave(), &42u64.encode(), &[][..])
				),
				Error::<Test>::InvalidStatement,
			);
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					42,
					sig::<Test>(&eve(), &42u64.encode(), &[][..])
				),
				Error::<Test>::InvalidStatement,
			);
			assert_ok!(Claims::claim(
				RuntimeOrigin::none(),
				42,
				sig::<Test>(&frank(), &42u64.encode(), &[][..])
			));
		});
	}

	#[test]
	fn attest_claiming_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			let s = sig::<Test>(&dave(), &42u64.encode(), StatementKind::Saft.to_text());
			let r = Claims::claim_attest(
				RuntimeOrigin::none(),
				42,
				s.clone(),
				StatementKind::Saft.to_text().to_vec(),
			);
			assert_noop!(r, Error::<Test>::InvalidStatement);

			let r = Claims::claim_attest(
				RuntimeOrigin::none(),
				42,
				s,
				StatementKind::Regular.to_text().to_vec(),
			);
			assert_noop!(r, Error::<Test>::SignerHasNoClaim);
			// ^^^ we use ecdsa_recover, so an invalid signature just results in a random signer id
			// being recovered, which realistically will never have a claim.

			let s = sig::<Test>(&dave(), &42u64.encode(), StatementKind::Regular.to_text());
			assert_ok!(Claims::claim_attest(
				RuntimeOrigin::none(),
				42,
				s,
				StatementKind::Regular.to_text().to_vec()
			));
			assert_eq!(Balances::free_balance(&42), 200);
			assert_eq!(Claims::total(), total_claims() - 200);

			let s = sig::<Test>(&dave(), &42u64.encode(), StatementKind::Regular.to_text());
			let r = Claims::claim_attest(
				RuntimeOrigin::none(),
				42,
				s,
				StatementKind::Regular.to_text().to_vec(),
			);
			assert_noop!(r, Error::<Test>::SignerHasNoClaim);
		});
	}

	#[test]
	fn attesting_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::attest(RuntimeOrigin::signed(69), StatementKind::Saft.to_text().to_vec()),
				Error::<Test>::SenderHasNoClaim
			);
			assert_noop!(
				Claims::attest(
					RuntimeOrigin::signed(42),
					StatementKind::Regular.to_text().to_vec()
				),
				Error::<Test>::InvalidStatement
			);
			assert_ok!(Claims::attest(
				RuntimeOrigin::signed(42),
				StatementKind::Saft.to_text().to_vec()
			));
			assert_eq!(Balances::free_balance(&42), 300);
			assert_eq!(Claims::total(), total_claims() - 300);
		});
	}

	#[test]
	fn claim_cannot_clobber_preclaim() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			// Alice's claim is 100
			assert_ok!(Claims::claim(
				RuntimeOrigin::none(),
				42,
				sig::<Test>(&alice(), &42u64.encode(), &[][..])
			));
			assert_eq!(Balances::free_balance(&42), 100);
			// Eve's claim is 300 through Account 42
			assert_ok!(Claims::attest(
				RuntimeOrigin::signed(42),
				StatementKind::Saft.to_text().to_vec()
			));
			assert_eq!(Balances::free_balance(&42), 100 + 300);
			assert_eq!(Claims::total(), total_claims() - 400);
		});
	}

	#[test]
	fn valid_attest_transactions_are_free() {
		new_test_ext().execute_with(|| {
			let p = PrevalidateAttests::<Test>::new();
			let c = RuntimeCall::Claims(ClaimsCall::attest {
				statement: StatementKind::Saft.to_text().to_vec(),
			});
			let di = c.get_dispatch_info();
			assert_eq!(di.pays_fee, Pays::No);
			let r = p.validate(&42, &c, &di, 20);
			assert_eq!(r, TransactionValidity::Ok(ValidTransaction::default()));
		});
	}

	#[test]
	fn invalid_attest_transactions_are_recognized() {
		new_test_ext().execute_with(|| {
			let p = PrevalidateAttests::<Test>::new();
			let c = RuntimeCall::Claims(ClaimsCall::attest {
				statement: StatementKind::Regular.to_text().to_vec(),
			});
			let di = c.get_dispatch_info();
			let r = p.validate(&42, &c, &di, 20);
			assert!(r.is_err());
			let c = RuntimeCall::Claims(ClaimsCall::attest {
				statement: StatementKind::Saft.to_text().to_vec(),
			});
			let di = c.get_dispatch_info();
			let r = p.validate(&69, &c, &di, 20);
			assert!(r.is_err());
		});
	}

	#[test]
	fn cannot_bypass_attest_claiming() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			let s = sig::<Test>(&dave(), &42u64.encode(), &[]);
			let r = Claims::claim(RuntimeOrigin::none(), 42, s.clone());
			assert_noop!(r, Error::<Test>::InvalidStatement);
		});
	}

	#[test]
	fn add_claim_works() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Claims::mint_claim(RuntimeOrigin::signed(42), eth(&bob()), 200, None, None),
				sp_runtime::traits::BadOrigin,
			);
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					69,
					sig::<Test>(&bob(), &69u64.encode(), &[][..])
				),
				Error::<Test>::SignerHasNoClaim,
			);
			assert_ok!(Claims::mint_claim(RuntimeOrigin::root(), eth(&bob()), 200, None, None));
			assert_eq!(Claims::total(), total_claims() + 200);
			assert_ok!(Claims::claim(
				RuntimeOrigin::none(),
				69,
				sig::<Test>(&bob(), &69u64.encode(), &[][..])
			));
			assert_eq!(Balances::free_balance(&69), 200);
			assert_eq!(Vesting::vesting_balance(&69), None);
			assert_eq!(Claims::total(), total_claims());
		});
	}

	#[test]
	fn add_claim_with_vesting_works() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Claims::mint_claim(
					RuntimeOrigin::signed(42),
					eth(&bob()),
					200,
					Some((50, 10, 1)),
					None
				),
				sp_runtime::traits::BadOrigin,
			);
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					69,
					sig::<Test>(&bob(), &69u64.encode(), &[][..])
				),
				Error::<Test>::SignerHasNoClaim,
			);
			assert_ok!(Claims::mint_claim(
				RuntimeOrigin::root(),
				eth(&bob()),
				200,
				Some((50, 10, 1)),
				None
			));
			assert_ok!(Claims::claim(
				RuntimeOrigin::none(),
				69,
				sig::<Test>(&bob(), &69u64.encode(), &[][..])
			));
			assert_eq!(Balances::free_balance(&69), 200);
			assert_eq!(Vesting::vesting_balance(&69), Some(50));

			// Make sure we can not transfer the vested balance.
			assert_err!(
				<Balances as Currency<_>>::transfer(
					&69,
					&80,
					180,
					ExistenceRequirement::AllowDeath
				),
				pallet_balances::Error::<Test, _>::LiquidityRestrictions,
			);
		});
	}

	#[test]
	fn add_claim_with_statement_works() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Claims::mint_claim(
					RuntimeOrigin::signed(42),
					eth(&bob()),
					200,
					None,
					Some(StatementKind::Regular)
				),
				sp_runtime::traits::BadOrigin,
			);
			assert_eq!(Balances::free_balance(42), 0);
			let signature = sig::<Test>(&bob(), &69u64.encode(), StatementKind::Regular.to_text());
			assert_noop!(
				Claims::claim_attest(
					RuntimeOrigin::none(),
					69,
					signature.clone(),
					StatementKind::Regular.to_text().to_vec()
				),
				Error::<Test>::SignerHasNoClaim
			);
			assert_ok!(Claims::mint_claim(
				RuntimeOrigin::root(),
				eth(&bob()),
				200,
				None,
				Some(StatementKind::Regular)
			));
			assert_noop!(
				Claims::claim_attest(RuntimeOrigin::none(), 69, signature.clone(), vec![],),
				Error::<Test>::SignerHasNoClaim
			);
			assert_ok!(Claims::claim_attest(
				RuntimeOrigin::none(),
				69,
				signature.clone(),
				StatementKind::Regular.to_text().to_vec()
			));
			assert_eq!(Balances::free_balance(&69), 200);
		});
	}

	#[test]
	fn origin_signed_claiming_fail() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_err!(
				Claims::claim(
					RuntimeOrigin::signed(42),
					42,
					sig::<Test>(&alice(), &42u64.encode(), &[][..])
				),
				sp_runtime::traits::BadOrigin,
			);
		});
	}

	#[test]
	fn double_claiming_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_ok!(Claims::claim(
				RuntimeOrigin::none(),
				42,
				sig::<Test>(&alice(), &42u64.encode(), &[][..])
			));
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					42,
					sig::<Test>(&alice(), &42u64.encode(), &[][..])
				),
				Error::<Test>::SignerHasNoClaim
			);
		});
	}

	#[test]
	fn claiming_while_vested_doesnt_work() {
		new_test_ext().execute_with(|| {
			// A user is already vested
			assert_ok!(<Test as Config>::VestingSchedule::add_vesting_schedule(
				&69,
				total_claims(),
				100,
				10
			));
			CurrencyOf::<Test>::make_free_balance_be(&69, total_claims());
			assert_eq!(Balances::free_balance(69), total_claims());
			assert_ok!(Claims::mint_claim(
				RuntimeOrigin::root(),
				eth(&bob()),
				200,
				Some((50, 10, 1)),
				None
			));
			// New total
			assert_eq!(Claims::total(), total_claims() + 200);

			// They should not be able to claim
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					69,
					sig::<Test>(&bob(), &69u64.encode(), &[][..])
				),
				Error::<Test>::VestedBalanceExists,
			);
		});
	}

	#[test]
	fn non_sender_sig_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					42,
					sig::<Test>(&alice(), &69u64.encode(), &[][..])
				),
				Error::<Test>::SignerHasNoClaim
			);
		});
	}

	#[test]
	fn non_claimant_doesnt_work() {
		new_test_ext().execute_with(|| {
			assert_eq!(Balances::free_balance(42), 0);
			assert_noop!(
				Claims::claim(
					RuntimeOrigin::none(),
					42,
					sig::<Test>(&bob(), &69u64.encode(), &[][..])
				),
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
			let signer = Claims::eth_recover(&sig, &who, &[][..]).unwrap();
			assert_eq!(signer.0, hex!["6d31165d5d932d571f3b44695653b46dcc327e84"]);
		});
	}

	#[test]
	fn validate_unsigned_works() {
		use sp_runtime::traits::ValidateUnsigned;
		let source = sp_runtime::transaction_validity::TransactionSource::External;

		new_test_ext().execute_with(|| {
			assert_eq!(
				<Pallet<Test>>::validate_unsigned(
					source,
					&ClaimsCall::claim {
						dest: 1,
						ethereum_signature: sig::<Test>(&alice(), &1u64.encode(), &[][..])
					}
				),
				Ok(ValidTransaction {
					priority: 100,
					requires: vec![],
					provides: vec![("claims", eth(&alice())).encode()],
					longevity: TransactionLongevity::max_value(),
					propagate: true,
				})
			);
			assert_eq!(
				<Pallet<Test>>::validate_unsigned(
					source,
					&ClaimsCall::claim { dest: 0, ethereum_signature: EcdsaSignature([0; 65]) }
				),
				InvalidTransaction::Custom(ValidityError::InvalidEthereumSignature.into()).into(),
			);
			assert_eq!(
				<Pallet<Test>>::validate_unsigned(
					source,
					&ClaimsCall::claim {
						dest: 1,
						ethereum_signature: sig::<Test>(&bob(), &1u64.encode(), &[][..])
					}
				),
				InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into()).into(),
			);
			let s = sig::<Test>(&dave(), &1u64.encode(), StatementKind::Regular.to_text());
			let call = ClaimsCall::claim_attest {
				dest: 1,
				ethereum_signature: s,
				statement: StatementKind::Regular.to_text().to_vec(),
			};
			assert_eq!(
				<Pallet<Test>>::validate_unsigned(source, &call),
				Ok(ValidTransaction {
					priority: 100,
					requires: vec![],
					provides: vec![("claims", eth(&dave())).encode()],
					longevity: TransactionLongevity::max_value(),
					propagate: true,
				})
			);
			assert_eq!(
				<Pallet<Test>>::validate_unsigned(
					source,
					&ClaimsCall::claim_attest {
						dest: 1,
						ethereum_signature: EcdsaSignature([0; 65]),
						statement: StatementKind::Regular.to_text().to_vec()
					}
				),
				InvalidTransaction::Custom(ValidityError::InvalidEthereumSignature.into()).into(),
			);

			let s = sig::<Test>(&bob(), &1u64.encode(), StatementKind::Regular.to_text());
			let call = ClaimsCall::claim_attest {
				dest: 1,
				ethereum_signature: s,
				statement: StatementKind::Regular.to_text().to_vec(),
			};
			assert_eq!(
				<Pallet<Test>>::validate_unsigned(source, &call),
				InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into()).into(),
			);

			let s = sig::<Test>(&dave(), &1u64.encode(), StatementKind::Saft.to_text());
			let call = ClaimsCall::claim_attest {
				dest: 1,
				ethereum_signature: s,
				statement: StatementKind::Regular.to_text().to_vec(),
			};
			assert_eq!(
				<Pallet<Test>>::validate_unsigned(source, &call),
				InvalidTransaction::Custom(ValidityError::SignerHasNoClaim.into()).into(),
			);

			let s = sig::<Test>(&dave(), &1u64.encode(), StatementKind::Saft.to_text());
			let call = ClaimsCall::claim_attest {
				dest: 1,
				ethereum_signature: s,
				statement: StatementKind::Saft.to_text().to_vec(),
			};
			assert_eq!(
				<Pallet<Test>>::validate_unsigned(source, &call),
				InvalidTransaction::Custom(ValidityError::InvalidStatement.into()).into(),
			);
		});
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking {
	use super::*;
	use crate::claims::Call;
	use frame_benchmarking::{account, benchmarks};
	use frame_support::dispatch::UnfilteredDispatchable;
	use frame_system::RawOrigin;
	use secp_utils::*;
	use sp_runtime::{traits::ValidateUnsigned, DispatchResult};

	const SEED: u32 = 0;

	const MAX_CLAIMS: u32 = 10_000;
	const VALUE: u32 = 1_000_000;

	fn create_claim<T: Config>(input: u32) -> DispatchResult {
		let secret_key = libsecp256k1::SecretKey::parse(&keccak_256(&input.encode())).unwrap();
		let eth_address = eth(&secret_key);
		let vesting = Some((100_000u32.into(), 1_000u32.into(), 100u32.into()));
		super::Pallet::<T>::mint_claim(
			RawOrigin::Root.into(),
			eth_address,
			VALUE.into(),
			vesting,
			None,
		)?;
		Ok(())
	}

	fn create_claim_attest<T: Config>(input: u32) -> DispatchResult {
		let secret_key = libsecp256k1::SecretKey::parse(&keccak_256(&input.encode())).unwrap();
		let eth_address = eth(&secret_key);
		let vesting = Some((100_000u32.into(), 1_000u32.into(), 100u32.into()));
		super::Pallet::<T>::mint_claim(
			RawOrigin::Root.into(),
			eth_address,
			VALUE.into(),
			vesting,
			Some(Default::default()),
		)?;
		Ok(())
	}

	benchmarks! {
		// Benchmark `claim` including `validate_unsigned` logic.
		claim {
			let c = MAX_CLAIMS;

			for i in 0 .. c / 2 {
				create_claim::<T>(c)?;
				create_claim_attest::<T>(u32::MAX - c)?;
			}

			let secret_key = libsecp256k1::SecretKey::parse(&keccak_256(&c.encode())).unwrap();
			let eth_address = eth(&secret_key);
			let account: T::AccountId = account("user", c, SEED);
			let vesting = Some((100_000u32.into(), 1_000u32.into(), 100u32.into()));
			let signature = sig::<T>(&secret_key, &account.encode(), &[][..]);
			super::Pallet::<T>::mint_claim(RawOrigin::Root.into(), eth_address, VALUE.into(), vesting, None)?;
			assert_eq!(Claims::<T>::get(eth_address), Some(VALUE.into()));
			let source = sp_runtime::transaction_validity::TransactionSource::External;
			let call_enc = Call::<T>::claim {
				dest: account.clone(),
				ethereum_signature: signature.clone()
			}.encode();
		}: {
			let call = <Call<T> as Decode>::decode(&mut &*call_enc)
				.expect("call is encoded above, encoding must be correct");
			super::Pallet::<T>::validate_unsigned(source, &call).map_err(|e| -> &'static str { e.into() })?;
			call.dispatch_bypass_filter(RawOrigin::None.into())?;
		}
		verify {
			assert_eq!(Claims::<T>::get(eth_address), None);
		}

		// Benchmark `mint_claim` when there already exists `c` claims in storage.
		mint_claim {
			let c = MAX_CLAIMS;

			for i in 0 .. c / 2 {
				create_claim::<T>(c)?;
				create_claim_attest::<T>(u32::MAX - c)?;
			}

			let eth_address = account("eth_address", 0, SEED);
			let vesting = Some((100_000u32.into(), 1_000u32.into(), 100u32.into()));
			let statement = StatementKind::Regular;
		}: _(RawOrigin::Root, eth_address, VALUE.into(), vesting, Some(statement))
		verify {
			assert_eq!(Claims::<T>::get(eth_address), Some(VALUE.into()));
		}

		// Benchmark `claim_attest` including `validate_unsigned` logic.
		claim_attest {
			let c = MAX_CLAIMS;

			for i in 0 .. c / 2 {
				create_claim::<T>(c)?;
				create_claim_attest::<T>(u32::MAX - c)?;
			}

			// Crate signature
			let attest_c = u32::MAX - c;
			let secret_key = libsecp256k1::SecretKey::parse(&keccak_256(&attest_c.encode())).unwrap();
			let eth_address = eth(&secret_key);
			let account: T::AccountId = account("user", c, SEED);
			let vesting = Some((100_000u32.into(), 1_000u32.into(), 100u32.into()));
			let statement = StatementKind::Regular;
			let signature = sig::<T>(&secret_key, &account.encode(), statement.to_text());
			super::Pallet::<T>::mint_claim(RawOrigin::Root.into(), eth_address, VALUE.into(), vesting, Some(statement))?;
			assert_eq!(Claims::<T>::get(eth_address), Some(VALUE.into()));
			let call_enc = Call::<T>::claim_attest {
				dest: account.clone(),
				ethereum_signature: signature.clone(),
				statement: StatementKind::Regular.to_text().to_vec()
			}.encode();
			let source = sp_runtime::transaction_validity::TransactionSource::External;
		}: {
			let call = <Call<T> as Decode>::decode(&mut &*call_enc)
				.expect("call is encoded above, encoding must be correct");
			super::Pallet::<T>::validate_unsigned(source, &call).map_err(|e| -> &'static str { e.into() })?;
			call.dispatch_bypass_filter(RawOrigin::None.into())?;
		}
		verify {
			assert_eq!(Claims::<T>::get(eth_address), None);
		}

		// Benchmark `attest` including prevalidate logic.
		attest {
			let c = MAX_CLAIMS;

			for i in 0 .. c / 2 {
				create_claim::<T>(c)?;
				create_claim_attest::<T>(u32::MAX - c)?;
			}

			let attest_c = u32::MAX - c;
			let secret_key = libsecp256k1::SecretKey::parse(&keccak_256(&attest_c.encode())).unwrap();
			let eth_address = eth(&secret_key);
			let account: T::AccountId = account("user", c, SEED);
			let vesting = Some((100_000u32.into(), 1_000u32.into(), 100u32.into()));
			let statement = StatementKind::Regular;
			let signature = sig::<T>(&secret_key, &account.encode(), statement.to_text());
			super::Pallet::<T>::mint_claim(RawOrigin::Root.into(), eth_address, VALUE.into(), vesting, Some(statement))?;
			Preclaims::<T>::insert(&account, eth_address);
			assert_eq!(Claims::<T>::get(eth_address), Some(VALUE.into()));

			let call = super::Call::<T>::attest { statement: StatementKind::Regular.to_text().to_vec() };
			// We have to copy the validate statement here because of trait issues... :(
			let validate = |who: &T::AccountId, call: &super::Call<T>| -> DispatchResult {
				if let Call::attest{ statement: attested_statement } = call {
					let signer = Preclaims::<T>::get(who).ok_or("signer has no claim")?;
					if let Some(s) = Signing::<T>::get(signer) {
						ensure!(&attested_statement[..] == s.to_text(), "invalid statement");
					}
				}
				Ok(())
			};
			let call_enc = call.encode();
		}: {
			let call = <Call<T> as Decode>::decode(&mut &*call_enc)
				.expect("call is encoded above, encoding must be correct");
			validate(&account, &call)?;
			call.dispatch_bypass_filter(RawOrigin::Signed(account).into())?;
		}
		verify {
			assert_eq!(Claims::<T>::get(eth_address), None);
		}

		move_claim {
			let c = MAX_CLAIMS;

			for i in 0 .. c / 2 {
				create_claim::<T>(c)?;
				create_claim_attest::<T>(u32::MAX - c)?;
			}

			let attest_c = u32::MAX - c;
			let secret_key = libsecp256k1::SecretKey::parse(&keccak_256(&attest_c.encode())).unwrap();
			let eth_address = eth(&secret_key);

			let new_secret_key = libsecp256k1::SecretKey::parse(&keccak_256(&(u32::MAX/2).encode())).unwrap();
			let new_eth_address = eth(&new_secret_key);

			let account: T::AccountId = account("user", c, SEED);
			Preclaims::<T>::insert(&account, eth_address);

			assert!(Claims::<T>::contains_key(eth_address));
			assert!(!Claims::<T>::contains_key(new_eth_address));
		}: _(RawOrigin::Root, eth_address, new_eth_address, Some(account))
		verify {
			assert!(!Claims::<T>::contains_key(eth_address));
			assert!(Claims::<T>::contains_key(new_eth_address));
		}

		// Benchmark the time it takes to do `repeat` number of keccak256 hashes
		#[extra]
		keccak256 {
			let i in 0 .. 10_000;
			let bytes = (i).encode();
		}: {
			for index in 0 .. i {
				let _hash = keccak_256(&bytes);
			}
		}

		// Benchmark the time it takes to do `repeat` number of `eth_recover`
		#[extra]
		eth_recover {
			let i in 0 .. 1_000;
			// Crate signature
			let secret_key = libsecp256k1::SecretKey::parse(&keccak_256(&i.encode())).unwrap();
			let account: T::AccountId = account("user", i, SEED);
			let signature = sig::<T>(&secret_key, &account.encode(), &[][..]);
			let data = account.using_encoded(to_ascii_hex);
			let extra = StatementKind::default().to_text();
		}: {
			for _ in 0 .. i {
				assert!(super::Pallet::<T>::eth_recover(&signature, &data, extra).is_some());
			}
		}

		impl_benchmark_test_suite!(
			Pallet,
			crate::claims::tests::new_test_ext(),
			crate::claims::tests::Test,
		);
	}
}
