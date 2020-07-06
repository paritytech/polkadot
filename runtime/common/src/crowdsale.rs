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

//! Module to process public crowdsale of DOTs.

use codec::{Encode, Decode};
use sp_runtime::{RuntimeDebug, DispatchResult, DispatchError};
use sp_runtime::traits::{Bounded, Zero};
use frame_support::{decl_event, decl_storage, decl_module, decl_error, ensure};
use frame_support::traits::{EnsureOrigin, IsDeadAccount, Currency};
use sp_core::sr25519;

/// Configuration trait.
pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;
	/// Balances
	type Currency: Currency<Self::AccountId>;
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
	/// Account has completed the crowdsale process.
	Completed,
}

impl Default for AccountValidity {
	fn default() -> Self {
		AccountValidity::Invalid
	}
}

impl AccountValidity {
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

/// All information about an account regarding the crowdsale.
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
		/// Someone's crowdsale balance was updated
		BalanceUpdated(AccountId, Balance, bool),
	}
);

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Account is not currently valid to use.
		InvalidAccount,
		/// Account used in the crowdsale already exists.
		ExistingAccount,
		/// Provided signature is invalid
		InvalidSignature,
		/// Account has already completed the crowdsale process.
		AlreadyCompleted,
		/// Balance provided for the account is not valid.
		InvalidBalance,
	}
}

decl_storage! {
	trait Store for Module<T: Trait> as Crowdsale {
		Accounts: map hasher(blake2_128_concat) T::AccountId => AccountStatus<BalanceOf<T>>;
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
				Ok(())
			})?;
			Self::deposit_event(RawEvent::BalanceUpdated(who, balance, locked));
		}

		/// Pay the user and complete the crowdsale process.
		///
		/// We check tht the account is valid for a balance transfer at this point.
		///
		/// Origin must match the `ValidityOrigin`.
		#[weight = 0]
		fn payout(origin, who: T::AccountId) {
			T::PaymentOrigin::ensure_origin(origin)?;
			// TODO
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

	use sp_core::H256;
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are required.
	use sp_runtime::{Perbill, traits::{BlakeTwo256, IdentityLookup}, testing::Header};
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
			crowdsale::Crowdsale,
		}
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
		type BaseCallFilter = ();
		type Origin = Origin;
		type Call = Call;
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

	ord_parameter_types! {
		pub const Six: u64 = 6;
		pub const Seven: u64 = 7;
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

	impl Trait for Test {
		type Event = ();
		type Currency = Balances;
		type ValidityOrigin = system::EnsureSignedBy<Six, u64>;
		type PaymentOrigin = system::EnsureSignedBy<Seven, u64>;
	}

	type System = system::Module<Test>;
	type Balances = balances::Module<Test>;
	type Crowdsale = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
		t.into()
	}

	// #[test]
	// fn set_account_validity_works() {
	// 	new_test_ext().execute_with(|| {
	// 		// User initially has no validity statement, by default, they are `Invalid`.
	// 		assert_eq!(ValidityStatements::<Test>::get(42), AccountValidity::Invalid);
	// 		// Origin must be the `ValidityOrigin`
	// 		assert_noop!(Crowdsale::set_account_validity(Origin::signed(1), 42, AccountValidity::ValidLow), BadOrigin);
	// 		assert_ok!(Crowdsale::set_account_validity(Origin::signed(6), 42, AccountValidity::ValidLow));
	// 		// Account is updated
	// 		assert_eq!(ValidityStatements::<Test>::get(42), AccountValidity::ValidLow);
	// 		// We update validity statement
	// 		assert_ok!(Crowdsale::set_account_validity(Origin::signed(6), 42, AccountValidity::Invalid));
	// 		assert_eq!(ValidityStatements::<Test>::get(42), AccountValidity::Invalid);
	// 	});
	// }

	// #[test]
	// fn set_account_validity_handles_basic_errors() {
	// 	new_test_ext().execute_with(|| {
	// 		// Create an "existing account"
	// 		Balances::make_free_balance_be(&42, 500);
	// 		// Origin must be the `ValidityOrigin`
	// 		assert_noop!(Crowdsale::set_account_validity(Origin::signed(1), 42, AccountValidity::ValidLow), BadOrigin);
	// 		// Account must be dead
	// 		assert_noop!(Crowdsale::set_account_validity(Origin::signed(6), 42, AccountValidity::ValidLow), Error::<Test>::ExistingAccount);
	// 	});
	// }
}
