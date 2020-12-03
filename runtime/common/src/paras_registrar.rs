// Copyright 2017-2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Module to handle parathread/parachain registration and related fund management.
//! In essence this is a simple wrapper around `paras`.

use crate::WASM_MAGIC;
use sp_std::{prelude::*, result};
use frame_support::{
	decl_storage, decl_module, decl_error, ensure,
	dispatch::DispatchResult,
	traits::{Get, Currency, ReservableCurrency},
};
use frame_system::{self, ensure_root, ensure_signed};
use primitives::v1::{
	Id as ParaId, ValidationCode, HeadData,
};
use runtime_parachains::{
	paras::{
		self,
		ParaGenesisArgs,
	},
	dmp, ump, hrmp,
	ensure_parachain,
	Origin,
};

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

pub trait Config: paras::Config + dmp::Config + ump::Config + hrmp::Config {
	/// The aggregated origin type must support the `parachains` origin. We require that we can
	/// infallibly convert between this origin and the system origin, but in reality, they're the
	/// same type, we just can't express that to the Rust type system without writing a `where`
	/// clause everywhere.
	type Origin: From<<Self as frame_system::Config>::Origin>
		+ Into<result::Result<Origin, <Self as Config>::Origin>>;

	/// The system's currency for parathread payment.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// The deposit to be paid to run a parathread.
	type ParathreadDeposit: Get<BalanceOf<Self>>;
}

decl_storage! {
	trait Store for Module<T: Config> as Registrar {
		/// Whether parathreads are enabled or not.
		ParathreadsRegistrationEnabled: bool;

		/// Pending swap operations.
		PendingSwap: map hasher(twox_64_concat) ParaId => Option<ParaId>;

		/// Map of all registered parathreads/chains.
		Paras get(fn paras): map hasher(twox_64_concat) ParaId => Option<bool>;

		/// Users who have paid a parathread's deposit.
		Debtors: map hasher(twox_64_concat) ParaId => T::AccountId;
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// Parachain already exists.
		ParaAlreadyExists,
		/// Invalid parachain ID.
		InvalidChainId,
		/// Invalid parathread ID.
		InvalidThreadId,
		/// Invalid para code size.
		CodeTooLarge,
		/// Invalid para head data size.
		HeadDataTooLarge,
		/// Parathreads registration is disabled.
		ParathreadsRegistrationDisabled,
		/// The validation code provided doesn't start with the Wasm file magic string.
		DefinitelyNotWasm,
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;

		/// Register a parathread with given code for immediate use.
		///
		/// Must be sent from a Signed origin that is able to have `ParathreadDeposit` reserved.
		/// `genesis_head` and `validation_code` are used to initalize the parathread's state.
		#[weight = 0]
		fn register_parathread(
			origin,
			id: ParaId,
			genesis_head: HeadData,
			validation_code: ValidationCode,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;

			ensure!(ParathreadsRegistrationEnabled::get(), Error::<T>::ParathreadsRegistrationDisabled);
			ensure!(validation_code.0.starts_with(WASM_MAGIC), Error::<T>::DefinitelyNotWasm);

			ensure!(!Paras::contains_key(id), Error::<T>::ParaAlreadyExists);

			let outgoing = <paras::Module<T>>::outgoing_paras();

			ensure!(outgoing.binary_search(&id).is_err(), Error::<T>::ParaAlreadyExists);

			<T as Config>::Currency::reserve(&who, T::ParathreadDeposit::get())?;
			<Debtors<T>>::insert(id, who);

			Paras::insert(id, false);

			let genesis = ParaGenesisArgs {
				genesis_head,
				validation_code,
				parachain: false,
			};

			runtime_parachains::schedule_para_initialize::<T>(id, genesis);

			Ok(())
		}

		/// Deregister a parathread and retreive the deposit.
		///
		/// Must be sent from a `Parachain` origin which is currently a parathread.
		///
		/// Ensure that before calling this that any funds you want emptied from the parathread's
		/// account is moved out; after this it will be impossible to retreive them (without
		/// governance intervention).
		#[weight = 0]
		fn deregister_parathread(origin) -> DispatchResult {
			let id = ensure_parachain(<T as Config>::Origin::from(origin))?;

			ensure!(ParathreadsRegistrationEnabled::get(), Error::<T>::ParathreadsRegistrationDisabled);

			let is_parachain = Paras::take(id).ok_or(Error::<T>::InvalidChainId)?;

			ensure!(!is_parachain, Error::<T>::InvalidThreadId);

			let debtor = <Debtors<T>>::take(id);
			let _ = <T as Config>::Currency::unreserve(&debtor, T::ParathreadDeposit::get());

			runtime_parachains::schedule_para_cleanup::<T>(id);

			Ok(())
		}

		#[weight = 0]
		fn enable_parathread_registration(origin) -> DispatchResult {
			ensure_root(origin)?;

			ParathreadsRegistrationEnabled::put(true);

			Ok(())
		}

		#[weight = 0]
		fn disable_parathread_registration(origin) -> DispatchResult {
			ensure_root(origin)?;

			ParathreadsRegistrationEnabled::put(false);

			Ok(())
		}


		/// Swap a parachain with another parachain or parathread. The origin must be a `Parachain`.
		/// The swap will happen only if there is already an opposite swap pending. If there is not,
		/// the swap will be stored in the pending swaps map, ready for a later confirmatory swap.
		///
		/// The `ParaId`s remain mapped to the same head data and code so external code can rely on
		/// `ParaId` to be a long-term identifier of a notional "parachain". However, their
		/// scheduling info (i.e. whether they're a parathread or parachain), auction information
		/// and the auction deposit are switched.
		#[weight = 0]
		fn swap(origin, other: ParaId) {
			let id = ensure_parachain(<T as Config>::Origin::from(origin))?;

			if PendingSwap::get(other) == Some(id) {
				// Remove intention to swap.
				PendingSwap::remove(other);

				Paras::mutate(id, |i|
					Paras::mutate(other, |j|
						sp_std::mem::swap(i, j)
					)
				);

				<Debtors<T>>::mutate(id, |i|
					<Debtors<T>>::mutate(other, |j|
						sp_std::mem::swap(i, j)
					)
				);
			} else {
				PendingSwap::insert(id, other);
			}
		}
	}
}

impl<T: Config> Module<T> {
	/// Register a parachain with given code. Must be called by root.
	/// Fails if given ID is already used.
	pub fn register_parachain(
		id: ParaId,
		genesis_head: HeadData,
		validation_code: ValidationCode,
	) -> DispatchResult {
		ensure!(!Paras::contains_key(id), Error::<T>::ParaAlreadyExists);
		ensure!(validation_code.0.starts_with(WASM_MAGIC), Error::<T>::DefinitelyNotWasm);

		let outgoing = <paras::Module<T>>::outgoing_paras();

		ensure!(outgoing.binary_search(&id).is_err(), Error::<T>::ParaAlreadyExists);

		Paras::insert(id, true);

		let genesis = ParaGenesisArgs {
			genesis_head,
			validation_code,
			parachain: true,
		};

		runtime_parachains::schedule_para_initialize::<T>(id, genesis);

		Ok(())
	}

	/// Deregister a parachain with the given ID. Must be called by root.
	pub fn deregister_parachain(id: ParaId) -> DispatchResult {
		let is_parachain = Paras::take(id).ok_or(Error::<T>::InvalidChainId)?;

		ensure!(is_parachain, Error::<T>::InvalidChainId);

		runtime_parachains::schedule_para_cleanup::<T>(id);

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_io::TestExternalities;
	use sp_core::H256;
	use sp_runtime::{
		traits::{
			BlakeTwo256, IdentityLookup, Extrinsic as ExtrinsicT,
		}, testing::{UintAuthorityId, TestXt}, Perbill, curve::PiecewiseLinear,
	};
	use primitives::v1::{
		Balance, BlockNumber, Header, Signature, AuthorityDiscoveryId,
	};
	use frame_system::limits;
	use frame_support::{
		traits::{Randomness, OnInitialize, OnFinalize},
		impl_outer_origin, impl_outer_dispatch, assert_ok, parameter_types,
	};
	use keyring::Sr25519Keyring;
	use runtime_parachains::{initializer, configuration, inclusion, session_info, scheduler, dmp, ump, hrmp};
	use pallet_session::OneSessionHandler;

	impl_outer_origin! {
		pub enum Origin for Test {
			runtime_parachains,
		}
	}

	impl_outer_dispatch! {
		pub enum Call for Test where origin: Origin {
			paras::Parachains,
			registrar::Registrar,
			staking::Staking,
		}
	}

	pallet_staking_reward_curve::build! {
		const REWARD_CURVE: PiecewiseLinear<'static> = curve!(
			min_inflation: 0_025_000,
			max_inflation: 0_100_000,
			ideal_stake: 0_500_000,
			falloff: 0_050_000,
			max_piece_count: 40,
			test_precision: 0_005_000,
		);
	}

	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	const NORMAL_RATIO: Perbill = Perbill::from_percent(75);
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub BlockWeights: limits::BlockWeights =
			limits::BlockWeights::with_sensible_defaults(4 * 1024 * 1024, NORMAL_RATIO);
		pub BlockLength: limits::BlockLength =
			limits::BlockLength::max_with_normal_ratio(4 * 1024 * 1024, NORMAL_RATIO);
	}

	impl frame_system::Config for Test {
		type BaseCallFilter = ();
		type Origin = Origin;
		type Call = Call;
		type Index = u64;
		type BlockNumber = BlockNumber;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type DbWeight = ();
		type BlockWeights = BlockWeights;
		type BlockLength = BlockLength;
		type Version = ();
		type PalletInfo = ();
		type AccountData = pallet_balances::AccountData<u128>;
		type OnNewAccount = ();
		type OnKilledAccount = Balances;
		type SystemWeightInfo = ();
	}

	impl<C> frame_system::offchain::SendTransactionTypes<C> for Test where
		Call: From<C>,
	{
		type OverarchingCall = Call;
		type Extrinsic = TestXt<Call, ()>;
	}

	parameter_types! {
		pub const ExistentialDeposit: Balance = 1;
	}

	impl pallet_balances::Config for Test {
		type Balance = u128;
		type DustRemoval = ();
		type Event = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type MaxLocks = ();
		type WeightInfo = ();
	}

	parameter_types!{
		pub const SlashDeferDuration: pallet_staking::EraIndex = 7;
		pub const AttestationPeriod: BlockNumber = 100;
		pub const MinimumPeriod: u64 = 3;
		pub const SessionsPerEra: sp_staking::SessionIndex = 6;
		pub const BondingDuration: pallet_staking::EraIndex = 28;
		pub const MaxNominatorRewardedPerValidator: u32 = 64;
	}

	parameter_types! {
		pub const Period: BlockNumber = 1;
		pub const Offset: BlockNumber = 0;
		pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(17);
		pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
	}

	impl pallet_session::Config for Test {
		type SessionManager = ();
		type Keys = UintAuthorityId;
		type ShouldEndSession = pallet_session::PeriodicSessions<Period, Offset>;
		type NextSessionRotation = pallet_session::PeriodicSessions<Period, Offset>;
		type SessionHandler = pallet_session::TestSessionHandler;
		type Event = ();
		type ValidatorId = u64;
		type ValidatorIdOf = ();
		type DisabledValidatorsThreshold = DisabledValidatorsThreshold;
		type WeightInfo = ();
	}

	parameter_types! {
		pub const MaxHeadDataSize: u32 = 100;
		pub const MaxCodeSize: u32 = 100;

		pub const ValidationUpgradeFrequency: BlockNumber = 10;
		pub const ValidationUpgradeDelay: BlockNumber = 2;
		pub const SlashPeriod: BlockNumber = 50;
		pub const ElectionLookahead: BlockNumber = 0;
		pub const StakingUnsignedPriority: u64 = u64::max_value() / 2;
	}

	impl pallet_staking::Config for Test {
		type RewardRemainder = ();
		type CurrencyToVote = frame_support::traits::SaturatingCurrencyToVote;
		type Event = ();
		type Currency = pallet_balances::Module<Test>;
		type Slash = ();
		type Reward = ();
		type SessionsPerEra = SessionsPerEra;
		type BondingDuration = BondingDuration;
		type SlashDeferDuration = SlashDeferDuration;
		type SlashCancelOrigin = frame_system::EnsureRoot<Self::AccountId>;
		type SessionInterface = Self;
		type UnixTime = pallet_timestamp::Module<Test>;
		type RewardCurve = RewardCurve;
		type MaxNominatorRewardedPerValidator = MaxNominatorRewardedPerValidator;
		type NextNewSession = Session;
		type ElectionLookahead = ElectionLookahead;
		type Call = Call;
		type UnsignedPriority = StakingUnsignedPriority;
		type MaxIterations = ();
		type MinSolutionScoreBump = ();
		type OffchainSolutionWeightLimit = ();
		type WeightInfo = ();
	}

	impl pallet_timestamp::Config for Test {
		type Moment = u64;
		type OnTimestampSet = ();
		type MinimumPeriod = MinimumPeriod;
		type WeightInfo = ();
	}

	impl dmp::Config for Test {}

	impl ump::Config for Test {
		type UmpSink = ();
	}

	impl hrmp::Config for Test {
		type Origin = Origin;
	}

	impl pallet_session::historical::Config for Test {
		type FullIdentification = pallet_staking::Exposure<u64, Balance>;
		type FullIdentificationOf = pallet_staking::ExposureOf<Self>;
	}

	// This is needed for a custom `AccountId` type which is `u64` in testing here.
	pub mod test_keys {
		use sp_core::crypto::KeyTypeId;

		pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"test");

		mod app {
			use super::super::Inclusion;
			use sp_application_crypto::{app_crypto, sr25519};

			app_crypto!(sr25519, super::KEY_TYPE);

			impl sp_runtime::traits::IdentifyAccount for Public {
				type AccountId = u64;

				fn into_account(self) -> Self::AccountId {
					let id = self.0.clone().into();
					Inclusion::validators().iter().position(|b| *b == id).unwrap() as u64
				}
			}
		}

		pub type ReporterId = app::Public;
	}

	impl paras::Config for Test {
		type Origin = Origin;
	}

	impl configuration::Config for Test { }

	impl inclusion::Config for Test {
		type Event = ();
	}

	impl session_info::AuthorityDiscoveryConfig for Test {
		fn authorities() -> Vec<AuthorityDiscoveryId> {
			Vec::new()
		}
	}

	impl session_info::Config for Test { }

	pub struct TestRandomness;

	impl Randomness<H256> for TestRandomness {
		fn random(_subject: &[u8]) -> H256 {
			Default::default()
		}
	}

	impl initializer::Config for Test {
		type Randomness = TestRandomness;
	}

	impl scheduler::Config for Test { }

	type Extrinsic = TestXt<Call, ()>;

	impl<LocalCall> frame_system::offchain::CreateSignedTransaction<LocalCall> for Test where
		Call: From<LocalCall>,
	{
		fn create_transaction<C: frame_system::offchain::AppCrypto<Self::Public, Self::Signature>>(
			call: Call,
			_public: test_keys::ReporterId,
			_account: <Test as frame_system::Config>::AccountId,
			nonce: <Test as frame_system::Config>::Index,
		) -> Option<(Call, <Extrinsic as ExtrinsicT>::SignaturePayload)> {
			Some((call, (nonce, ())))
		}
	}

	impl frame_system::offchain::SigningTypes for Test {
		type Public = test_keys::ReporterId;
		type Signature = Signature;
	}

	parameter_types! {
		pub const ParathreadDeposit: Balance = 10;
		pub const QueueSize: usize = 2;
		pub const MaxRetries: u32 = 3;
	}

	impl Config for Test {
		type Origin = Origin;
		type Currency = pallet_balances::Module<Test>;
		type ParathreadDeposit = ParathreadDeposit;
	}

	type Balances = pallet_balances::Module<Test>;
	type Parachains = paras::Module<Test>;
	type Inclusion = inclusion::Module<Test>;
	type System = frame_system::Module<Test>;
	type Registrar = Module<Test>;
	type Session = pallet_session::Module<Test>;
	type Staking = pallet_staking::Module<Test>;
	type Initializer = initializer::Module<Test>;

	fn new_test_ext() -> TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();

		let authority_keys = [
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Eve,
			Sr25519Keyring::Ferdie,
			Sr25519Keyring::One,
			Sr25519Keyring::Two,
		];

		// stashes are the index.
		let session_keys: Vec<_> = authority_keys.iter().enumerate()
			.map(|(i, _k)| (i as u64, i as u64, UintAuthorityId(i as u64)))
			.collect();

		let balances: Vec<_> = (0..authority_keys.len()).map(|i| (i as u64, 10_000_000)).collect();

		pallet_session::GenesisConfig::<Test> {
			keys: session_keys,
		}.assimilate_storage(&mut t).unwrap();

		pallet_balances::GenesisConfig::<Test> {
			balances,
		}.assimilate_storage(&mut t).unwrap();

		t.into()
	}

	fn init_block() {
		println!("Initializing {}", System::block_number());
		System::on_initialize(System::block_number());
		Initializer::on_initialize(System::block_number());
	}

	fn run_to_block(n: BlockNumber) {
		println!("Running until block {}", n);
		while System::block_number() < n {
			let b = System::block_number();

			if System::block_number() > 1 {
				println!("Finalizing {}", System::block_number());
				System::on_finalize(System::block_number());
			}
			// Session change every 3 blocks.
			if (b + 1) % 3 == 0 {
				Initializer::on_new_session(
					false,
					Vec::new().into_iter(),
					Vec::new().into_iter(),
				);
			}
			System::set_block_number(b + 1);
			init_block();
		}
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(PendingSwap::get(&ParaId::from(0u32)), None);
			assert_eq!(Paras::get(&ParaId::from(0u32)), None);
		});
	}

	#[test]
	fn register_deregister_chain_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Registrar::enable_parathread_registration(
				Origin::root(),
			));
			run_to_block(2);

			assert_ok!(Registrar::register_parachain(
				2u32.into(),
				vec![3; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));

			let orig_bal = Balances::free_balance(&3u64);

			// Register a new parathread
			assert_ok!(Registrar::register_parathread(
				Origin::signed(3u64),
				8u32.into(),
				vec![3; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));

			// deposit should be taken (reserved)
			assert_eq!(Balances::free_balance(3u64) + ParathreadDeposit::get(), orig_bal);
			assert_eq!(Balances::reserved_balance(3u64), ParathreadDeposit::get());

			run_to_block(3);

			assert_ok!(Registrar::deregister_parachain(2u32.into()));

			assert_ok!(Registrar::deregister_parathread(
				runtime_parachains::Origin::Parachain(8u32.into()).into()
			));

			// reserved balance should be returned.
			assert_eq!(Balances::free_balance(3u64), orig_bal);
			assert_eq!(Balances::reserved_balance(3u64), 0);
		});
	}

	#[test]
	fn swap_handles_funds_correctly() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Registrar::enable_parathread_registration(
				Origin::root(),
			));
			run_to_block(2);

			let initial_1_balance = Balances::free_balance(1);
			let initial_2_balance = Balances::free_balance(2);

			// User 1 register a new parathread
			assert_ok!(Registrar::register_parathread(
				Origin::signed(1),
				8u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));

			assert_ok!(Registrar::register_parachain(
				2u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));

			run_to_block(9);

			// Swap the parachain and parathread
			assert_ok!(Registrar::swap(runtime_parachains::Origin::Parachain(2u32.into()).into(), 8u32.into()));
			assert_ok!(Registrar::swap(runtime_parachains::Origin::Parachain(8u32.into()).into(), 2u32.into()));

			// Deregister a parathread that was originally a parachain
			assert_ok!(Registrar::deregister_parathread(runtime_parachains::Origin::Parachain(2u32.into()).into()));

			run_to_block(12);

			// Funds are correctly returned
			assert_eq!(Balances::free_balance(1), initial_1_balance);
			assert_eq!(Balances::free_balance(2), initial_2_balance);
		});
	}

	#[test]
	fn cannot_register_until_para_is_cleaned_up() {
		new_test_ext().execute_with(|| {
			run_to_block(2);

			assert_ok!(Registrar::register_parachain(
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));

			run_to_block(4);

			assert_ok!(Registrar::deregister_parachain(1u32.into()));
			run_to_block(5);

			assert!(Registrar::register_parachain(
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			).is_err());

			run_to_block(6);

			assert_ok!(Registrar::register_parachain(
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));
		});
	}
}
