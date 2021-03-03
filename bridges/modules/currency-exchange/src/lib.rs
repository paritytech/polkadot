// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Runtime module that allows tokens exchange between two bridged chains.

#![cfg_attr(not(feature = "std"), no_std)]

use bp_currency_exchange::{
	CurrencyConverter, DepositInto, Error as ExchangeError, MaybeLockFundsTransaction, RecipientsMap,
};
use bp_header_chain::InclusionProofVerifier;
use frame_support::{decl_error, decl_module, decl_storage, ensure};
use sp_runtime::DispatchResult;

#[cfg(feature = "runtime-benchmarks")]
pub mod benchmarking;

/// Called when transaction is submitted to the exchange module.
pub trait OnTransactionSubmitted<AccountId> {
	/// Called when valid transaction is submitted and accepted by the module.
	fn on_valid_transaction_submitted(submitter: AccountId);
}

/// The module configuration trait
pub trait Config<I = DefaultInstance>: frame_system::Config {
	/// Handler for transaction submission result.
	type OnTransactionSubmitted: OnTransactionSubmitted<Self::AccountId>;
	/// Represents the blockchain that we'll be exchanging currency with.
	type PeerBlockchain: InclusionProofVerifier;
	/// Peer blockchain transaction parser.
	type PeerMaybeLockFundsTransaction: MaybeLockFundsTransaction<
		Transaction = <Self::PeerBlockchain as InclusionProofVerifier>::Transaction,
	>;
	/// Map between blockchains recipients.
	type RecipientsMap: RecipientsMap<
		PeerRecipient = <Self::PeerMaybeLockFundsTransaction as MaybeLockFundsTransaction>::Recipient,
		Recipient = Self::AccountId,
	>;
	/// This blockchain currency amount type.
	type Amount;
	/// Converter from peer blockchain currency type into current blockchain currency type.
	type CurrencyConverter: CurrencyConverter<
		SourceAmount = <Self::PeerMaybeLockFundsTransaction as MaybeLockFundsTransaction>::Amount,
		TargetAmount = Self::Amount,
	>;
	/// Something that could grant money.
	type DepositInto: DepositInto<Recipient = Self::AccountId, Amount = Self::Amount>;
}

decl_error! {
	pub enum Error for Module<T: Config<I>, I: Instance> {
		/// Invalid peer blockchain transaction provided.
		InvalidTransaction,
		/// Peer transaction has invalid amount.
		InvalidAmount,
		/// Peer transaction has invalid recipient.
		InvalidRecipient,
		/// Cannot map from peer recipient to this blockchain recipient.
		FailedToMapRecipients,
		/// Failed to convert from peer blockchain currency to this blockhain currency.
		FailedToConvertCurrency,
		/// Deposit has failed.
		DepositFailed,
		/// Deposit has partially failed (changes to recipient account were made).
		DepositPartiallyFailed,
		/// Transaction is not finalized.
		UnfinalizedTransaction,
		/// Transaction funds are already claimed.
		AlreadyClaimed,
	}
}

decl_module! {
	pub struct Module<T: Config<I>, I: Instance = DefaultInstance> for enum Call where origin: T::Origin {
		/// Imports lock fund transaction of the peer blockchain.
		#[weight = 0] // TODO: update me (https://github.com/paritytech/parity-bridges-common/issues/78)
		pub fn import_peer_transaction(
			origin,
			proof: <<T as Config<I>>::PeerBlockchain as InclusionProofVerifier>::TransactionInclusionProof,
		) -> DispatchResult {
			let submitter = frame_system::ensure_signed(origin)?;

			// verify and parse transaction proof
			let deposit = prepare_deposit_details::<T, I>(&proof)?;

			// make sure to update the mapping if we deposit successfully to avoid double spending,
			// i.e. whenever `deposit_into` is successful we MUST update `Transfers`.
			{
				// if any changes were made to the storage, we can't just return error here, because
				// otherwise the same proof may be imported again
				let deposit_result = T::DepositInto::deposit_into(deposit.recipient, deposit.amount);
				match deposit_result {
					Ok(_) => (),
					Err(ExchangeError::DepositPartiallyFailed) => (),
					Err(error) => return Err(Error::<T, I>::from(error).into()),
				}
				Transfers::<T, I>::insert(&deposit.transfer_id, ())
			}

			// reward submitter for providing valid message
			T::OnTransactionSubmitted::on_valid_transaction_submitted(submitter);

			frame_support::debug::trace!(
				target: "runtime",
				"Completed currency exchange: {:?}",
				deposit.transfer_id,
			);

			Ok(())
		}
	}
}

decl_storage! {
	trait Store for Module<T: Config<I>, I: Instance = DefaultInstance> as Bridge {
		/// All transfers that have already been claimed.
		Transfers: map hasher(blake2_128_concat) <T::PeerMaybeLockFundsTransaction as MaybeLockFundsTransaction>::Id => ();
	}
}

impl<T: Config<I>, I: Instance> Module<T, I> {
	/// Returns true if currency exchange module is able to import given transaction proof in
	/// its current state.
	pub fn filter_transaction_proof(
		proof: &<T::PeerBlockchain as InclusionProofVerifier>::TransactionInclusionProof,
	) -> bool {
		if let Err(err) = prepare_deposit_details::<T, I>(proof) {
			frame_support::debug::trace!(
				target: "runtime",
				"Can't accept exchange transaction: {:?}",
				err,
			);

			return false;
		}

		true
	}
}

impl<T: Config<I>, I: Instance> From<ExchangeError> for Error<T, I> {
	fn from(error: ExchangeError) -> Self {
		match error {
			ExchangeError::InvalidTransaction => Error::InvalidTransaction,
			ExchangeError::InvalidAmount => Error::InvalidAmount,
			ExchangeError::InvalidRecipient => Error::InvalidRecipient,
			ExchangeError::FailedToMapRecipients => Error::FailedToMapRecipients,
			ExchangeError::FailedToConvertCurrency => Error::FailedToConvertCurrency,
			ExchangeError::DepositFailed => Error::DepositFailed,
			ExchangeError::DepositPartiallyFailed => Error::DepositPartiallyFailed,
		}
	}
}

impl<AccountId> OnTransactionSubmitted<AccountId> for () {
	fn on_valid_transaction_submitted(_: AccountId) {}
}

/// Exchange deposit details.
struct DepositDetails<T: Config<I>, I: Instance> {
	/// Transfer id.
	pub transfer_id: <T::PeerMaybeLockFundsTransaction as MaybeLockFundsTransaction>::Id,
	/// Transfer recipient.
	pub recipient: <T::RecipientsMap as RecipientsMap>::Recipient,
	/// Transfer amount.
	pub amount: <T::CurrencyConverter as CurrencyConverter>::TargetAmount,
}

/// Verify and parse transaction proof, preparing everything required for importing
/// this transaction proof.
fn prepare_deposit_details<T: Config<I>, I: Instance>(
	proof: &<<T as Config<I>>::PeerBlockchain as InclusionProofVerifier>::TransactionInclusionProof,
) -> Result<DepositDetails<T, I>, Error<T, I>> {
	// ensure that transaction is included in finalized block that we know of
	let transaction = <T as Config<I>>::PeerBlockchain::verify_transaction_inclusion_proof(proof)
		.ok_or(Error::<T, I>::UnfinalizedTransaction)?;

	// parse transaction
	let transaction =
		<T as Config<I>>::PeerMaybeLockFundsTransaction::parse(&transaction).map_err(Error::<T, I>::from)?;
	let transfer_id = transaction.id;
	ensure!(
		!Transfers::<T, I>::contains_key(&transfer_id),
		Error::<T, I>::AlreadyClaimed
	);

	// grant recipient
	let recipient = T::RecipientsMap::map(transaction.recipient).map_err(Error::<T, I>::from)?;
	let amount = T::CurrencyConverter::convert(transaction.amount).map_err(Error::<T, I>::from)?;

	Ok(DepositDetails {
		transfer_id,
		recipient,
		amount,
	})
}

#[cfg(test)]
mod tests {
	// From construct_runtime macro
	#![allow(clippy::from_over_into)]

	use super::*;
	use bp_currency_exchange::LockFundsTransaction;
	use frame_support::{assert_noop, assert_ok, construct_runtime, parameter_types, weights::Weight};
	use sp_core::H256;
	use sp_runtime::{
		testing::Header,
		traits::{BlakeTwo256, IdentityLookup},
		Perbill,
	};

	type AccountId = u64;

	const INVALID_TRANSACTION_ID: u64 = 100;
	const ALREADY_CLAIMED_TRANSACTION_ID: u64 = 101;
	const UNKNOWN_RECIPIENT_ID: u64 = 0;
	const INVALID_AMOUNT: u64 = 0;
	const MAX_DEPOSIT_AMOUNT: u64 = 1000;
	const SUBMITTER: u64 = 2000;

	type RawTransaction = LockFundsTransaction<u64, u64, u64>;

	pub struct DummyTransactionSubmissionHandler;

	impl OnTransactionSubmitted<AccountId> for DummyTransactionSubmissionHandler {
		fn on_valid_transaction_submitted(submitter: AccountId) {
			Transfers::<TestRuntime, DefaultInstance>::insert(submitter, ());
		}
	}

	pub struct DummyBlockchain;

	impl InclusionProofVerifier for DummyBlockchain {
		type Transaction = RawTransaction;
		type TransactionInclusionProof = (bool, RawTransaction);

		fn verify_transaction_inclusion_proof(proof: &Self::TransactionInclusionProof) -> Option<RawTransaction> {
			if proof.0 {
				Some(proof.1.clone())
			} else {
				None
			}
		}
	}

	pub struct DummyTransaction;

	impl MaybeLockFundsTransaction for DummyTransaction {
		type Transaction = RawTransaction;
		type Id = u64;
		type Recipient = AccountId;
		type Amount = u64;

		fn parse(tx: &Self::Transaction) -> bp_currency_exchange::Result<RawTransaction> {
			match tx.id {
				INVALID_TRANSACTION_ID => Err(ExchangeError::InvalidTransaction),
				_ => Ok(tx.clone()),
			}
		}
	}

	pub struct DummyRecipientsMap;

	impl RecipientsMap for DummyRecipientsMap {
		type PeerRecipient = AccountId;
		type Recipient = AccountId;

		fn map(peer_recipient: Self::PeerRecipient) -> bp_currency_exchange::Result<Self::Recipient> {
			match peer_recipient {
				UNKNOWN_RECIPIENT_ID => Err(ExchangeError::FailedToMapRecipients),
				_ => Ok(peer_recipient * 10),
			}
		}
	}

	pub struct DummyCurrencyConverter;

	impl CurrencyConverter for DummyCurrencyConverter {
		type SourceAmount = u64;
		type TargetAmount = u64;

		fn convert(amount: Self::SourceAmount) -> bp_currency_exchange::Result<Self::TargetAmount> {
			match amount {
				INVALID_AMOUNT => Err(ExchangeError::FailedToConvertCurrency),
				_ => Ok(amount * 10),
			}
		}
	}

	pub struct DummyDepositInto;

	impl DepositInto for DummyDepositInto {
		type Recipient = AccountId;
		type Amount = u64;

		fn deposit_into(_recipient: Self::Recipient, amount: Self::Amount) -> bp_currency_exchange::Result<()> {
			match amount {
				amount if amount < MAX_DEPOSIT_AMOUNT * 10 => Ok(()),
				amount if amount == MAX_DEPOSIT_AMOUNT * 10 => Err(ExchangeError::DepositPartiallyFailed),
				_ => Err(ExchangeError::DepositFailed),
			}
		}
	}

	type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<TestRuntime>;
	type Block = frame_system::mocking::MockBlock<TestRuntime>;
	use crate as pallet_bridge_currency_exchange;

	construct_runtime! {
		pub enum TestRuntime where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			System: frame_system::{Module, Call, Config, Storage, Event<T>},
			Exchange: pallet_bridge_currency_exchange::{Module},
		}
	}

	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const MaximumBlockWeight: Weight = 1024;
		pub const MaximumBlockLength: u32 = 2 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::one();
	}

	impl frame_system::Config for TestRuntime {
		type Origin = Origin;
		type Index = u64;
		type Call = Call;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = AccountId;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = ();
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type BaseCallFilter = ();
		type SystemWeightInfo = ();
		type BlockWeights = ();
		type BlockLength = ();
		type DbWeight = ();
		type SS58Prefix = ();
	}

	impl Config for TestRuntime {
		type OnTransactionSubmitted = DummyTransactionSubmissionHandler;
		type PeerBlockchain = DummyBlockchain;
		type PeerMaybeLockFundsTransaction = DummyTransaction;
		type RecipientsMap = DummyRecipientsMap;
		type Amount = u64;
		type CurrencyConverter = DummyCurrencyConverter;
		type DepositInto = DummyDepositInto;
	}

	fn new_test_ext() -> sp_io::TestExternalities {
		let t = frame_system::GenesisConfig::default()
			.build_storage::<TestRuntime>()
			.unwrap();
		sp_io::TestExternalities::new(t)
	}

	fn transaction(id: u64) -> RawTransaction {
		RawTransaction {
			id,
			recipient: 1,
			amount: 2,
		}
	}

	#[test]
	fn unfinalized_transaction_rejected() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Exchange::import_peer_transaction(Origin::signed(SUBMITTER), (false, transaction(0))),
				Error::<TestRuntime, DefaultInstance>::UnfinalizedTransaction,
			);
		});
	}

	#[test]
	fn invalid_transaction_rejected() {
		new_test_ext().execute_with(|| {
			assert_noop!(
				Exchange::import_peer_transaction(
					Origin::signed(SUBMITTER),
					(true, transaction(INVALID_TRANSACTION_ID)),
				),
				Error::<TestRuntime, DefaultInstance>::InvalidTransaction,
			);
		});
	}

	#[test]
	fn claimed_transaction_rejected() {
		new_test_ext().execute_with(|| {
			<Exchange as crate::Store>::Transfers::insert(ALREADY_CLAIMED_TRANSACTION_ID, ());
			assert_noop!(
				Exchange::import_peer_transaction(
					Origin::signed(SUBMITTER),
					(true, transaction(ALREADY_CLAIMED_TRANSACTION_ID)),
				),
				Error::<TestRuntime, DefaultInstance>::AlreadyClaimed,
			);
		});
	}

	#[test]
	fn transaction_with_unknown_recipient_rejected() {
		new_test_ext().execute_with(|| {
			let mut transaction = transaction(0);
			transaction.recipient = UNKNOWN_RECIPIENT_ID;
			assert_noop!(
				Exchange::import_peer_transaction(Origin::signed(SUBMITTER), (true, transaction)),
				Error::<TestRuntime, DefaultInstance>::FailedToMapRecipients,
			);
		});
	}

	#[test]
	fn transaction_with_invalid_amount_rejected() {
		new_test_ext().execute_with(|| {
			let mut transaction = transaction(0);
			transaction.amount = INVALID_AMOUNT;
			assert_noop!(
				Exchange::import_peer_transaction(Origin::signed(SUBMITTER), (true, transaction)),
				Error::<TestRuntime, DefaultInstance>::FailedToConvertCurrency,
			);
		});
	}

	#[test]
	fn transaction_with_invalid_deposit_rejected() {
		new_test_ext().execute_with(|| {
			let mut transaction = transaction(0);
			transaction.amount = MAX_DEPOSIT_AMOUNT + 1;
			assert_noop!(
				Exchange::import_peer_transaction(Origin::signed(SUBMITTER), (true, transaction)),
				Error::<TestRuntime, DefaultInstance>::DepositFailed,
			);
		});
	}

	#[test]
	fn valid_transaction_accepted_even_if_deposit_partially_fails() {
		new_test_ext().execute_with(|| {
			let mut transaction = transaction(0);
			transaction.amount = MAX_DEPOSIT_AMOUNT;
			assert_ok!(Exchange::import_peer_transaction(
				Origin::signed(SUBMITTER),
				(true, transaction),
			),);

			// ensure that the transfer has been marked as completed
			assert!(<Exchange as crate::Store>::Transfers::contains_key(0u64));
			// ensure that submitter has been rewarded
			assert!(<Exchange as crate::Store>::Transfers::contains_key(SUBMITTER));
		});
	}

	#[test]
	fn valid_transaction_accepted() {
		new_test_ext().execute_with(|| {
			assert_ok!(Exchange::import_peer_transaction(
				Origin::signed(SUBMITTER),
				(true, transaction(0)),
			),);

			// ensure that the transfer has been marked as completed
			assert!(<Exchange as crate::Store>::Transfers::contains_key(0u64));
			// ensure that submitter has been rewarded
			assert!(<Exchange as crate::Store>::Transfers::contains_key(SUBMITTER));
		});
	}
}
