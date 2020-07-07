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

//! Module to process purchase of DOTs.

use codec::{Encode, Decode};
use sp_runtime::{RuntimeDebug, DispatchResult, DispatchError};
use sp_runtime::traits::{Bounded, Saturating, Zero};
use frame_support::{decl_event, decl_storage, decl_module, decl_error, ensure};
use frame_support::traits::{
	EnsureOrigin, IsDeadAccount, Currency, ExistenceRequirement, VestingSchedule, Get
};
use sp_core::sr25519;

/// Configuration trait.
pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
	/// Balances Pallet
	type Currency: Currency<Self::AccountId>;
	/// Vesting Pallet
	type VestingSchedule: VestingSchedule<Self::AccountId, Moment=Self::BlockNumber, Currency=Self::Currency>;
	/// The number of blocks that the "locked" dots will be locked for.
	type VestingTime: Get<Self::BlockNumber>;
	/// The origin allowed to set account status.
	type ValidityOrigin: EnsureOrigin<Self::Origin>;
	/// The origin allowed to make final payments.
	type PaymentOrigin: EnsureOrigin<Self::Origin>;
}

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

/// The kind of a statement an account needs to make for a claim to be valid.
#[derive(Encode, Decode, Clone, Copy, Eq, PartialEq, RuntimeDebug)]
pub enum AccountValidity {
	/// Account is not valid.
	Invalid,
	/// Account is pending validation.
	Pending,
	/// Account is valid with a low contribution amount.
	ValidLow,
	/// Account is valid with a high contribution amount.
	ValidHigh,
	/// Account has completed the purchase process.
	Completed,
}

impl Default for AccountValidity {
	fn default() -> Self {
		AccountValidity::Invalid
	}
}

impl AccountValidity {
	fn is_valid(&self) -> bool {
		match self {
			Self::Invalid => false,
			Self::Pending => false,
			Self::ValidLow => true,
			Self::ValidHigh => true,
			Self::Completed => false,
		}
	}

	fn max_amount<T: Trait>(&self) -> BalanceOf<T> {
		match self {
			Self::Invalid => Zero::zero(),
			Self::Pending => Zero::zero(),
			Self::ValidLow => 100_000.into(),
			Self::ValidHigh => BalanceOf::<T>::max_value(),
			Self::Completed => Zero::zero(),
		}
	}
}

/// All information about an account regarding the purchase of DOTs.
#[derive(Encode, Decode, Default, Clone, Eq, PartialEq, RuntimeDebug)]
pub struct AccountStatus<Balance> {
	validity: AccountValidity,
	balance: Balance,
	locked: bool,
	statement_kind: StatementKind,
	signature: Vec<u8>,
}

/// The kind of a statement an account needs to make for a claim to be valid.
#[derive(Encode, Decode, Clone, Copy, Eq, PartialEq, RuntimeDebug)]
pub enum StatementKind {
	/// Statement required to be made by the regular sale participants.
	Regular,
	/// Statement required to be made by high value sale participants.
	High,
}

impl StatementKind {
	/// Convert this to the (English) statement it represents.
	fn to_text(self) -> &'static [u8] {
		match self {
			StatementKind::Regular => &b"Regular"[..],
			StatementKind::High => &b"High"[..],
		}
	}
}

impl Default for StatementKind {
	fn default() -> Self {
		StatementKind::Regular
	}
}

decl_event!(
	pub enum Event<T> where
		AccountId = <T as system::Trait>::AccountId,
		Balance = BalanceOf<T>,
	{
		/// A new account was created
		AccountCreated(AccountId),
		/// Someone's account validity was updated
		ValidityUpdated(AccountId, AccountValidity),
		/// Someone's account validity statement was removed
		ValidityRemoved(AccountId),
		/// Someone's purchase balance was updated
		BalanceUpdated(AccountId, Balance, bool),
	}
);

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Account is not currently valid to use.
		InvalidAccount,
		/// Account used in the purchase already exists.
		ExistingAccount,
		/// Provided signature is invalid
		InvalidSignature,
		/// Account has already completed the purchase process.
		AlreadyCompleted,
		/// Balance provided for the account is not valid.
		InvalidBalance,
	}
}

decl_storage! {
	trait Store for Module<T: Trait> as Purchase {
		// A map of all participants in the DOT purchase process.
		Accounts: map hasher(blake2_128_concat) T::AccountId => AccountStatus<BalanceOf<T>>;
		// The account that will be used to payout participants of the DOT purchase process.
		PaymentAccount: T::AccountId;
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		type Error = Error<T>;

		/// Deposit one of this module's events by using the default implementation.
		fn deposit_event() = default;

		/// Create a new account. Proof of existence through a valid signed message.
		///
		/// We check that the account does not exist at this stage.
		///
		/// Origin must match the `ValidityOrigin`.
		#[weight = 0]
		fn create_account(origin,
			who: T::AccountId,
			statement_kind: StatementKind,
			signature: Vec<u8>
		) {
			T::ValidityOrigin::ensure_origin(origin)?;
			ensure!(system::Module::<T>::is_dead_account(&who), Error::<T>::ExistingAccount);
			ensure!(!Accounts::<T>::contains_key(&who), Error::<T>::ExistingAccount);

			// Verify the signature provided is valid for the statement.
			Self::verify_signature(&who, statement_kind, &signature)?;

			// Create a new pending account.
			let status = AccountStatus {
				validity: AccountValidity::Pending,
				statement_kind,
				signature,
				balance: Zero::zero(),
				locked: false,
			};
			Accounts::<T>::insert(&who, status);
			Self::deposit_event(RawEvent::AccountCreated(who));
		}

		/// Update the validity status of an existing account. If set to completed, the account
		/// will no longer be able to continue through the crowdfund process.
		///
		/// We check tht the account exists at this stage, but has not completed the process.
		///
		/// Origin must match the `ValidityOrigin`.
		#[weight = 0]
		fn update_validity_status(origin,
			who: T::AccountId,
			validity: AccountValidity
		) {
			T::ValidityOrigin::ensure_origin(origin)?;
			ensure!(Accounts::<T>::contains_key(&who), Error::<T>::InvalidAccount);
			Accounts::<T>::try_mutate(&who, |status: &mut AccountStatus<BalanceOf<T>>| -> DispatchResult {
				ensure!(status.validity != AccountValidity::Completed, Error::<T>::AlreadyCompleted);
				status.validity = validity;
				Ok(())
			})?;
			Self::deposit_event(RawEvent::ValidityUpdated(who, validity));
		}

		/// Update the balance of a valid account.
		///
		/// We check tht the account is valid for a balance transfer at this point.
		///
		/// Origin must match the `ValidityOrigin`.
		#[weight = 0]
		fn update_balance(origin,
				who: T::AccountId,
				balance: BalanceOf<T>,
				locked: bool,
		) {
			T::ValidityOrigin::ensure_origin(origin)?;

			Accounts::<T>::try_mutate(&who, |status: &mut AccountStatus<BalanceOf<T>>| -> DispatchResult {
				let max_amount = status.validity.max_amount::<T>();
				ensure!(balance <= max_amount, Error::<T>::InvalidBalance);
				status.balance = balance;
				status.locked = locked;
				Ok(())
			})?;
			Self::deposit_event(RawEvent::BalanceUpdated(who, balance, locked));
		}

		/// Pay the user and complete the purchase process.
		///
		/// We reverify all assumptions about the state of an account, and complete the process.
		///
		/// Origin must match the `PaymentOrigin`.
		#[weight = 0]
		fn payout(origin, who: T::AccountId) {
			T::PaymentOrigin::ensure_origin(origin)?;

			// Account should not be active.
			ensure!(system::Module::<T>::is_dead_account(&who), Error::<T>::ExistingAccount);

			Accounts::<T>::try_mutate(&who, |status: &mut AccountStatus<BalanceOf<T>>| -> DispatchResult {
				// Account has a valid status (not Invalid, Pending, or Completed)...
				ensure!(status.validity.is_valid(), Error::<T>::InvalidAccount);
				// The balance we are going to transfer them matches their validity status
				ensure!(status.balance <= status.validity.max_amount::<T>(), Error::<T>::InvalidBalance);

				// Transfer funds from the payment account into the purchasing user.
				// TODO: This is totally safe? No chance of reentrancy?
				let payment_account = PaymentAccount::<T>::get();
				T::Currency::transfer(&payment_account, &who, status.balance, ExistenceRequirement::AllowDeath)?;

				if status.locked {
					// Account did not exist before this point, thus it should have no existing vesting schedule.
					// So this function should never fail, however if it does, not much we can do about it at
					// this point.
					let unlock_block = system::Module::<T>::block_number().saturating_add(T::VestingTime::get());
					let _ = T::VestingSchedule::add_vesting_schedule(
						// Apply vesting schedule to this user
						&who,
						// For this much amount
						status.balance,
						// Unlocking the full amount after one block
						status.balance,
						// When everything unlocks
						unlock_block
					);
				}

				// Setting the user account to `Completed` ends the purchase process for this user.
				status.validity = AccountValidity::Completed;
				Ok(())
			})?;
		}

		/* Admin Operations */

		/// Set the account that will be used to payout users in the DOT purchase process.
		///
		/// Origin must match the `PaymentOrigin`
		#[weight = 0]
		fn set_payout_account(origin, who: T::AccountId) {
			T::PaymentOrigin::ensure_origin(origin)?;
			// Possibly this is worse than having the caller account be the payment account?
			PaymentAccount::<T>::set(who);
		}
	}
}

impl<T: Trait> Module<T> {
	fn verify_signature(who: &T::AccountId, statement_kind: StatementKind, signature: &[u8]) -> Result<(), DispatchError> {
		// sr25519 always expects a 64 byte signature.
		ensure!(signature.len() == 64, Error::<T>::InvalidSignature);
		let signature = sr25519::Signature::from_slice(signature);

		// In Polkadot, the AccountId is always the same as the 32 byte public key.
		let account_bytes: [u8; 32] = account_to_bytes(who)?;
		let public_key = sr25519::Public::from_raw(account_bytes);

		let message = statement_kind.to_text();

		// Check if everything is good or not.
		match sr25519::verify_batch(vec![message], vec![&signature], vec![&public_key]) {
			true => Ok(()),
			false => Err(Error::<T>::InvalidSignature)?,
		}
	}
}

// This function converts a 32 byte AccountId to its byte-array equivalent form.
fn account_to_bytes<AccountId>(account: &AccountId) -> Result<[u8; 32], DispatchError>
	where AccountId: Encode,
{
	let account_vec = account.encode();
	ensure!(account_vec.len() == 32, "AccountId must be 32 bytes.");
	let mut bytes = [0; 32];
	bytes.copy_from_slice(&account_vec);
	Ok(bytes)
}

#[cfg(test)]
mod tests {
	use super::*;

	use sp_core::{H256, Pair, Public, crypto::AccountId32};
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are required.
	use sp_runtime::{
		Perbill, MultiSignature,
		traits::{BlakeTwo256, IdentityLookup, Identity, Verify, IdentifyAccount},
		testing::Header
	};
	use frame_support::{
		impl_outer_origin, impl_outer_dispatch, assert_ok, assert_noop, parameter_types,
		ord_parameter_types, dispatch::DispatchError::BadOrigin,
	};
	use frame_support::traits::Currency;

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	impl_outer_dispatch! {
		pub enum Call for Test where origin: Origin {
			purchase::Purchase,
		}
	}

	type AccountId = AccountId32;

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
		type BaseCallFilter = ();
		type Origin = Origin;
		type Call = Call;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = AccountId;
		type Lookup = IdentityLookup<AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type DbWeight = ();
		type BlockExecutionWeight = ();
		type ExtrinsicBaseWeight = ();
		type MaximumExtrinsicWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
		type ModuleToIndex = ();
		type AccountData = balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = Balances;
	}

	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
	}

	impl balances::Trait for Test {
		type Balance = u64;
		type Event = ();
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
	}

	parameter_types! {
		pub const MinVestedTransfer: u64 = 0;
	}

	impl vesting::Trait for Test {
		type Event = ();
		type Currency = Balances;
		type BlockNumberToBalance = Identity;
		type MinVestedTransfer = MinVestedTransfer;
	}

	parameter_types! {
		pub const VestingTime: u64 = 100;
	}

	impl Trait for Test {
		type Event = ();
		type Currency = Balances;
		type VestingSchedule = Vesting;
		type VestingTime = VestingTime;
		type ValidityOrigin = system::EnsureRoot<AccountId>;
		type PaymentOrigin = system::EnsureRoot<AccountId>;
	}

	type System = system::Module<Test>;
	type Balances = balances::Module<Test>;
	type Vesting = vesting::Module<Test>;
	type Purchase = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
		t.into()
	}

	type AccountPublic = <MultiSignature as Verify>::Signer;

	/// Helper function to generate a crypto pair from seed
	fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
		TPublic::Pair::from_string(&format!("//{}", seed), None)
			.expect("static values are valid; qed")
			.public()
	}

	/// Helper function to generate an account ID from seed
	fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId where
		AccountPublic: From<<TPublic::Pair as Pair>::Public>
	{
		AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
	}

	fn alice() -> AccountId {
		get_account_id_from_seed::<sr25519::Public>("Alice")
	}

	fn bob() -> AccountId {
		get_account_id_from_seed::<sr25519::Public>("Bob")
	}

	#[test]
	fn signature_verification_works() {
		new_test_ext().execute_with(|| {
			let alice = alice();
			let regular_statement = StatementKind::Regular;
			// Signing "Regular" with Alice using PolkadotJS, removing `0x` prefix
			let alice_regular_signature = hex_literal::hex!("76c9616b262d01b2fc64aad212a364bbb0c0a6744968ba189dde93496acc6d54b2a5b7af93ff57c114dd5641ae0510e4c78d74d60f432c9737bcb37369190d88");
			assert_ok!(Purchase::verify_signature(&alice, regular_statement, &alice_regular_signature));

			let bob = bob();
			let high_statement = StatementKind::High;
			// Signing "High" with Bob using PolkadotJS, removing `0x` prefix
			let bob_high_signature = hex_literal::hex!("fe00fd50ae126753d56cec06b1acb3fef65b9009707d28d700c76161eab7f550bf28f090953f2fcdf8422f9b1fad7869395d2704363a73147efaec98339f2f8d");
			assert_ok!(Purchase::verify_signature(&bob, high_statement, &bob_high_signature));

			// Mixing and matching fails
			assert_noop!(
				Purchase::verify_signature(&alice, high_statement, &bob_high_signature),
				Error::<Test>::InvalidSignature
			);
			assert_noop!(
				Purchase::verify_signature(&alice, high_statement, &alice_regular_signature),
				Error::<Test>::InvalidSignature
			);
			assert_noop!(
				Purchase::verify_signature(&alice, regular_statement, &bob_high_signature),
				Error::<Test>::InvalidSignature
			);
			assert_noop!(
				Purchase::verify_signature(&bob, high_statement, &alice_regular_signature),
				Error::<Test>::InvalidSignature
			);
			assert_noop!(
				Purchase::verify_signature(&bob, regular_statement, &bob_high_signature),
				Error::<Test>::InvalidSignature
			);
			assert_noop!(
				Purchase::verify_signature(&bob, regular_statement, &alice_regular_signature),
				Error::<Test>::InvalidSignature
			);
		});
	}
}
