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
use sp_runtime::traits::{Bounded, Saturating, Zero, CheckedAdd};
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
	/// The statement that must be signed to participate in the purchase process.
	type Statement: Get<&'static [u8]>;
	/// The purchase limit for low contributing users.
	type PurchaseLimit: Get<BalanceOf<Self>>;
}

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

/// The kind of a statement an account needs to make for a claim to be valid.
#[derive(Encode, Decode, Clone, Copy, Eq, PartialEq, RuntimeDebug)]
pub enum AccountValidity {
	/// Account is not valid.
	Invalid,
	/// Account has initiated the account creation process.
	Initiated,
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
			Self::Initiated => false,
			Self::Pending => false,
			Self::ValidLow => true,
			Self::ValidHigh => true,
			Self::Completed => false,
		}
	}

	fn max_amount<T: Trait>(&self) -> BalanceOf<T> {
		match self {
			Self::Invalid => Zero::zero(),
			Self::Initiated => Zero::zero(),
			Self::Pending => Zero::zero(),
			Self::ValidLow => T::PurchaseLimit::get(),
			Self::ValidHigh => BalanceOf::<T>::max_value(),
			Self::Completed => Zero::zero(),
		}
	}
}

/// All information about an account regarding the purchase of DOTs.
#[derive(Encode, Decode, Default, Clone, Eq, PartialEq, RuntimeDebug)]
pub struct AccountStatus<Balance> {
	validity: AccountValidity,
	free_balance: Balance,
	locked_balance: Balance,
	signature: Vec<u8>,
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
		/// Someone's purchase balance was updated. (Free, Locked)
		BalanceUpdated(AccountId, Balance, Balance),
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
		/// An overflow occurred when doing calculations.
		Overflow,
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
			signature: Vec<u8>
		) {
			T::ValidityOrigin::ensure_origin(origin)?;
			ensure!(system::Module::<T>::is_dead_account(&who), Error::<T>::ExistingAccount);
			ensure!(!Accounts::<T>::contains_key(&who), Error::<T>::ExistingAccount);

			// Verify the signature provided is valid for the statement.
			Self::verify_signature(&who, &signature)?;

			// Create a new pending account.
			let status = AccountStatus {
				validity: AccountValidity::Initiated,
				signature,
				free_balance: Zero::zero(),
				locked_balance: Zero::zero(),
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
				free_balance: BalanceOf<T>,
				locked_balance: BalanceOf<T>,
		) {
			T::ValidityOrigin::ensure_origin(origin)?;

			Accounts::<T>::try_mutate(&who, |status: &mut AccountStatus<BalanceOf<T>>| -> DispatchResult {
				let max_amount = status.validity.max_amount::<T>();
				let total_balance = free_balance.checked_add(&locked_balance).ok_or(Error::<T>::Overflow)?;
				// TODO: Maybe remove this check, and trust the origin to be adding the right values.
				ensure!(total_balance <= max_amount, Error::<T>::InvalidBalance);
				status.free_balance = free_balance;
				status.locked_balance = locked_balance;
				Ok(())
			})?;
			Self::deposit_event(RawEvent::BalanceUpdated(who, free_balance, locked_balance));
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
				let total_balance = status.free_balance.checked_add(&status.locked_balance).ok_or(Error::<T>::Overflow)?;
				// TODO: Possibly remove this check and trust the origin to check this before they payout.
				ensure!(total_balance <= status.validity.max_amount::<T>(), Error::<T>::InvalidBalance);

				// Transfer funds from the payment account into the purchasing user.
				// TODO: This is totally safe? No chance of reentrancy?
				let payment_account = PaymentAccount::<T>::get();
				T::Currency::transfer(&payment_account, &who, total_balance, ExistenceRequirement::AllowDeath)?;

				if !status.locked_balance.is_zero() {
					// Account did not exist before this point, thus it should have no existing vesting schedule.
					// So this function should never fail, however if it does, not much we can do about it at
					// this point.
					let unlock_block = system::Module::<T>::block_number().saturating_add(T::VestingTime::get());
					let _ = T::VestingSchedule::add_vesting_schedule(
						// Apply vesting schedule to this user
						&who,
						// For this much amount
						status.locked_balance,
						// Unlocking the full amount after one block
						status.locked_balance,
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
	fn verify_signature(who: &T::AccountId, signature: &[u8]) -> Result<(), DispatchError> {
		// sr25519 always expects a 64 byte signature.
		ensure!(signature.len() == 64, Error::<T>::InvalidSignature);
		let signature = sr25519::Signature::from_slice(signature);

		// In Polkadot, the AccountId is always the same as the 32 byte public key.
		let account_bytes: [u8; 32] = account_to_bytes(who)?;
		let public_key = sr25519::Public::from_raw(account_bytes);

		let message = T::Statement::get();

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
		pub Statement: &'static [u8] = b"Hello, World!";
		pub const PurchaseLimit: u64 = 100;
	}

	ord_parameter_types! {
		pub const ValidityOrigin: AccountId = AccountId32::from([0u8; 32]);
		pub const PaymentOrigin: AccountId = AccountId32::from([1u8; 32]);
	}

	impl Trait for Test {
		type Event = ();
		type Currency = Balances;
		type VestingSchedule = Vesting;
		type VestingTime = VestingTime;
		type ValidityOrigin = system::EnsureSignedBy<ValidityOrigin, AccountId>;
		type PaymentOrigin = system::EnsureSignedBy<PaymentOrigin, AccountId>;
		type Statement = Statement;
		type PurchaseLimit = PurchaseLimit;
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

	fn alice_signature() -> [u8; 64] {
		// Signing the "Hello, World!" with Alice using PolkadotJS, removing `0x` prefix
		hex_literal::hex!("66b14944807202317fbc26001dd883726a9a756eab804e8d15f1a38bcde4a1591abd720ebe22df3cf1e7bd3a640727691b7fe697c23256760ec166b78c07b886")
	}

	fn bob_signature() -> [u8; 64] {
		// Signing the "Hello, World!" with Bob using PolkadotJS, removing `0x` prefix
		hex_literal::hex!("aced3b06fb3b7862c38ace0ba36fad20012dd0246b92760813b5fe9cee97c973f15db6b74fceeaae0012c3f708ee13a3dd3446403243be01d1d01ddf3b54448a")
	}

	fn validity_origin() -> AccountId {
		ValidityOrigin::get()
	}

	#[test]
	fn signature_verification_works() {
		new_test_ext().execute_with(|| {
			let alice = alice();
			let alice_signature = alice_signature();
			assert_ok!(Purchase::verify_signature(&alice, &alice_signature));

			let bob = bob();
			let bob_signature = bob_signature();
			assert_ok!(Purchase::verify_signature(&bob, &bob_signature));

			// Mixing and matching fails
			assert_noop!(Purchase::verify_signature(&alice, &bob_signature), Error::<Test>::InvalidSignature);
			assert_noop!(Purchase::verify_signature(&bob, &alice_signature), Error::<Test>::InvalidSignature);
		});
	}

	#[test]
	fn account_creation_works() {
		new_test_ext().execute_with(|| {
			assert!(!Accounts::<Test>::contains_key(alice()));
			assert_ok!(Purchase::create_account(Origin::signed(validity_origin()), alice(), alice_signature().to_vec()));
			assert_eq!(
				Accounts::<Test>::get(alice()),
				AccountStatus {
					validity: AccountValidity::Initiated,
					free_balance: Zero::zero(),
					locked_balance: Zero::zero(),
					signature: alice_signature().to_vec(),
				}
			);
		});
	}

	#[test]
	fn account_creation_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Wrong Origin
			assert_noop!(
				Purchase::create_account(Origin::signed(alice()), alice(), alice_signature().to_vec()),
				BadOrigin,
			);

			// Wrong Account/Signature
			assert_noop!(
				Purchase::create_account(Origin::signed(validity_origin()), alice(), bob_signature().to_vec()),
				Error::<Test>::InvalidSignature,
			);

			// Active Account
			Balances::make_free_balance_be(&alice(), 100);
			assert_noop!(
				Purchase::create_account(Origin::signed(validity_origin()), alice(), alice_signature().to_vec()),
				Error::<Test>::ExistingAccount,
			);

			// Duplicate Purchasing Account
			assert_ok!(
				Purchase::create_account(Origin::signed(validity_origin()), bob(), bob_signature().to_vec())
			);
			assert_noop!(
				Purchase::create_account(Origin::signed(validity_origin()), bob(), bob_signature().to_vec()),
				Error::<Test>::ExistingAccount,
			);
		});
	}

	#[test]
	fn update_validity_status_works() {
		new_test_ext().execute_with(|| {
			// Alice account is created.
			assert_ok!(Purchase::create_account(Origin::signed(validity_origin()), alice(), alice_signature().to_vec()));
			// She submits KYC, and we update the status to `Pending`.
			assert_ok!(Purchase::update_validity_status(
				Origin::signed(validity_origin()),
				alice(),
				AccountValidity::Pending,
			));
			// KYC comes back negative, so we mark the account invalid.
			assert_ok!(Purchase::update_validity_status(
				Origin::signed(validity_origin()),
				alice(),
				AccountValidity::Invalid,
			));
			assert_eq!(
				Accounts::<Test>::get(alice()),
				AccountStatus {
					validity: AccountValidity::Invalid,
					free_balance: Zero::zero(),
					locked_balance: Zero::zero(),
					signature: alice_signature().to_vec(),
				}
			);
			// She fixes it, we mark her account valid.
			assert_ok!(Purchase::update_validity_status(
				Origin::signed(validity_origin()),
				alice(),
				AccountValidity::ValidLow,
			));
			assert_eq!(
				Accounts::<Test>::get(alice()),
				AccountStatus {
					validity: AccountValidity::ValidLow,
					free_balance: Zero::zero(),
					locked_balance: Zero::zero(),
					signature: alice_signature().to_vec(),
				}
			);
		});
	}

	#[test]
	fn update_validity_status_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Inactive Account
			assert_noop!(Purchase::update_validity_status(
				Origin::signed(validity_origin()),
				alice(),
				AccountValidity::Pending,
			), Error::<Test>::InvalidAccount);
			// Already Completed
			assert_ok!(Purchase::create_account(
				Origin::signed(validity_origin()),
				alice(),
				alice_signature().to_vec(),
			));
			assert_ok!(Purchase::update_validity_status(
				Origin::signed(validity_origin()),
				alice(),
				AccountValidity::Completed,
			));
			assert_noop!(Purchase::update_validity_status(
				Origin::signed(validity_origin()),
				alice(),
				AccountValidity::Pending,
			), Error::<Test>::AlreadyCompleted);
		});
	}

	#[test]
	fn update_balance_works() {
		new_test_ext().execute_with(|| {
			// Alice account is created
			assert_ok!(Purchase::create_account(Origin::signed(validity_origin()), alice(), alice_signature().to_vec()));
			// And approved for basic contribution
			assert_ok!(Purchase::update_validity_status(
				Origin::signed(validity_origin()),
				alice(),
				AccountValidity::ValidLow,
			));
			// We set a balance on the user based on the payment they made. 50 locked, 50 free.
			assert_ok!(Purchase::update_balance(
				Origin::signed(validity_origin()),
				alice(),
				50,
				50,
			));
			assert_eq!(
				Accounts::<Test>::get(alice()),
				AccountStatus {
					validity: AccountValidity::ValidLow,
					free_balance: 50,
					locked_balance: 50,
					signature: alice_signature().to_vec(),
				}
			);
			// We can update the balance based on new information.
			assert_ok!(Purchase::update_balance(
				Origin::signed(validity_origin()),
				alice(),
				25,
				50,
			));
			assert_eq!(
				Accounts::<Test>::get(alice()),
				AccountStatus {
					validity: AccountValidity::ValidLow,
					free_balance: 25,
					locked_balance: 50,
					signature: alice_signature().to_vec(),
				}
			);
		});
	}

	#[test]
	fn update_balance_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Inactive Account
			assert_noop!(Purchase::update_balance(
				Origin::signed(validity_origin()),
				alice(),
				50,
				50,
			), Error::<Test>::InvalidBalance);

			// Value too high
			assert_ok!(Purchase::create_account(Origin::signed(validity_origin()), alice(), alice_signature().to_vec()));
			assert_ok!(Purchase::update_validity_status(
				Origin::signed(validity_origin()),
				alice(),
				AccountValidity::ValidLow,
			));
			assert_noop!(Purchase::update_balance(
				Origin::signed(validity_origin()),
				alice(),
				51,
				50,
			), Error::<Test>::InvalidBalance);
		});
	}
}
