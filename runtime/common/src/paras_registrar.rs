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
	decl_storage, decl_module, decl_error, decl_event, ensure,
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
	ensure_parachain,
	Origin, ParachainCleanup, ParaLifecycle,
};

use crate::traits::{Registrar, OnSwap};
use parity_scale_codec::{Encode, Decode};
use sp_runtime::RuntimeDebug;

#[derive(Encode, Decode, Clone, PartialEq, Eq, Default, RuntimeDebug)]
pub struct ParaInfo<Account, Balance> {
	pub(crate) manager: Account,
	deposit: Balance,
}

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

pub trait Config: paras::Config {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;

	/// The aggregated origin type must support the `parachains` origin. We require that we can
	/// infallibly convert between this origin and the system origin, but in reality, they're the
	/// same type, we just can't express that to the Rust type system without writing a `where`
	/// clause everywhere.
	type Origin: From<<Self as frame_system::Config>::Origin>
		+ Into<result::Result<Origin, <Self as Config>::Origin>>;

	/// The system's currency for parathread payment.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// Runtime hook for when a parachain and parathread swap.
	type OnSwap: crate::traits::OnSwap;

	/// Runtime hook for when a parachain should be cleaned up.
	type ParachainCleanup: runtime_parachains::ParachainCleanup;

	/// The deposit to be paid to run a parathread.
	/// This should include the cost for storing the genesis head and validation code.
	type ParaDeposit: Get<BalanceOf<Self>>;

	/// The maximum size for the validation code.
	type MaxCodeSize: Get<u32>;

	/// The maximum size for the head data.
	type MaxHeadSize: Get<u32>;
}

decl_storage! {
	trait Store for Module<T: Config> as Registrar {
		/// Pending swap operations.
		PendingSwap: map hasher(twox_64_concat) ParaId => Option<ParaId>;

		/// Amount held on deposit for each para and the original depositor.
		///
		/// The given account ID is responsible for registering the code and initial head data, but may only do
		/// so if it isn't yet registered. (After that, it's up to governance to do so.)
		pub Paras: map hasher(twox_64_concat) ParaId => Option<ParaInfo<T::AccountId, BalanceOf<T>>>;
	}
}

decl_event! {
	pub enum Event<T> where
		AccountId = <T as frame_system::Config>::AccountId,
		ParaId = ParaId,
	{
		Registered(ParaId, AccountId),
		Deregistered(ParaId),
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// The ID is not registered.
		NotRegistered,
		/// The ID is already registered.
		AlreadyRegistered,
		/// The caller is not the owner of this Id.
		NotOwner,
		/// Invalid para code size.
		CodeTooLarge,
		/// Invalid para head data size.
		HeadDataTooLarge,
		/// The validation code provided doesn't start with the Wasm file magic string.
		DefinitelyNotWasm,
		/// Para is not a Parachain.
		NotParachain,
		/// Para is not a Parathread.
		NotParathread,
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;

		fn deposit_event() = default;

		/// Register a Para Id on the relay chain.
		///
		/// This function will queue the new Para Id to be a parathread.
		/// Using the Slots pallet, a parathread can then be upgraded to get a
		/// parachain slot.
		///
		/// This function must be called by a signed origin.
		///
		/// The origin must pay a deposit for the registration information,
		/// including the genesis information and validation code.
		#[weight = 0]
		fn register(
			origin,
			id: ParaId,
			genesis_head: HeadData,
			validation_code: ValidationCode,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			ensure!(!Paras::<T>::contains_key(id), Error::<T>::AlreadyRegistered);
			ensure!(paras::Module::<T>::lifecycle(id).is_none(), Error::<T>::AlreadyRegistered);
			let genesis = Self::validate_onboarding_data(
				genesis_head,
				validation_code,
				false
			)?;

			<T as Config>::Currency::reserve(&who, T::ParaDeposit::get())?;
			let info = ParaInfo {
				manager: who.clone(),
				deposit: T::ParaDeposit::get(),
			};

			Paras::<T>::insert(id, info);
			runtime_parachains::schedule_para_initialize::<T>(id, genesis);
			Self::deposit_event(RawEvent::Registered(id, who));
			Ok(())
		}

		/// Deregister a Para Id, freeing all data and returning any deposit.
		///
		/// The caller must be the para itself or Root and the para must be a parathread.
		#[weight = 0]
		fn deregister(origin, id: ParaId) -> DispatchResult {
			match ensure_root(origin.clone()) {
				Ok(_) => {},
				Err(_) => {
					let caller_id = ensure_parachain(<T as Config>::Origin::from(origin))?;
					ensure!(caller_id == id, Error::<T>::NotOwner);
				},
			};

			ensure!(paras::Module::<T>::lifecycle(id) == Some(ParaLifecycle::Parathread), Error::<T>::NotParathread);

			if let Some(info) = Paras::<T>::take(&id) {
				<T as Config>::Currency::unreserve(&info.manager, info.deposit);
			}

			T::ParachainCleanup::schedule_para_cleanup(id);
			PendingSwap::remove(id);
			Self::deposit_event(RawEvent::Deregistered(id));
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
				if let Some(other_lifecycle) = paras::Module::<T>::lifecycle(other) {
					if let Some(id_lifecycle) = paras::Module::<T>::lifecycle(id) {
						// identify which is a parachain and which is a parathread
						if id_lifecycle.is_parachain() && other_lifecycle.is_parathread() {
							runtime_parachains::schedule_parachain_downgrade::<T>(id);
							runtime_parachains::schedule_parathread_upgrade::<T>(other);
							T::OnSwap::on_swap(id, other);
						} else if id_lifecycle.is_parathread() && other_lifecycle.is_parachain() {
							runtime_parachains::schedule_parachain_downgrade::<T>(other);
							runtime_parachains::schedule_parathread_upgrade::<T>(id);
							T::OnSwap::on_swap(id, other);
						}

						PendingSwap::remove(other);
					}
				}
			} else {
				PendingSwap::insert(id, other);
			}
		}
	}
}

impl<T: Config> Registrar for Module<T> {
	type AccountId = T::AccountId;

	fn manager_of(id: ParaId) -> Option<T::AccountId> {
		Some(Paras::<T>::get(id)?.manager)
	}

	// All parachains. Ordered ascending by ParaId. Parathreads are not included.
	fn parachains() -> Vec<ParaId> {
		paras::Module::<T>::parachains()
	}

	// Return if a para is a parathread
	fn is_parathread(id: ParaId) -> bool {
		paras::Module::<T>::is_parathread(id)
	}

	// Return if a para is a parachain
	fn is_parachain(id: ParaId) -> bool {
		paras::Module::<T>::is_parachain(id)
	}

	// Upgrade a registered parathread into a parachain.
	fn make_parachain(id: ParaId) -> DispatchResult {
		// Para backend should think this is a parathread...
		ensure!(paras::Module::<T>::lifecycle(id) == Some(ParaLifecycle::Parathread), Error::<T>::NotParathread);
		runtime_parachains::schedule_parathread_upgrade::<T>(id);
		Ok(())
	}

	// Downgrade a registered para into a parathread.
	fn make_parathread(id: ParaId) -> DispatchResult {
		// Para backend should think this is a parachain...
		ensure!(paras::Module::<T>::lifecycle(id) == Some(ParaLifecycle::Parachain), Error::<T>::NotParachain);
		runtime_parachains::schedule_parachain_downgrade::<T>(id);
		Ok(())
	}
}

impl<T: Config> Module<T> {
	/// Verifies the onboarding data is valid for a para.
	fn validate_onboarding_data(
		genesis_head: HeadData,
		validation_code: ValidationCode,
		parachain: bool,
	) -> Result<ParaGenesisArgs, sp_runtime::DispatchError> {
		ensure!(validation_code.0.len() <= T::MaxCodeSize::get() as usize, Error::<T>::CodeTooLarge);
		ensure!(genesis_head.0.len() <= T::MaxHeadSize::get() as usize, Error::<T>::HeadDataTooLarge);
		ensure!(validation_code.0.starts_with(WASM_MAGIC), Error::<T>::DefinitelyNotWasm);

		Ok(ParaGenesisArgs {
			genesis_head,
			validation_code,
			parachain,
		})
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
		Balance, BlockNumber, Header, Signature, AuthorityDiscoveryId, ValidatorIndex,
	};
	use frame_system::limits;
	use frame_support::{
		traits::{Randomness, OnInitialize, OnFinalize},
		error::BadOrigin,
		impl_outer_origin, impl_outer_dispatch, parameter_types,
		assert_ok, assert_noop
	};
	use keyring::Sr25519Keyring;
	use runtime_parachains::{initializer, configuration, inclusion, session_info, scheduler, dmp, ump, hrmp};
	use pallet_balances::Error as BalancesError;
	use crate::traits::Registrar as RegistrarTrait;
	use frame_support::traits::OneSessionHandler;

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
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
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
		type Currency = pallet_balances::Module<Test>;
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

	pub struct TestRewardValidators;

	impl inclusion::RewardValidators for TestRewardValidators {
		fn reward_backing(_: impl IntoIterator<Item = ValidatorIndex>) { }
		fn reward_bitfields(_: impl IntoIterator<Item = ValidatorIndex>) { }
	}

	impl inclusion::Config for Test {
		type Event = ();
		type RewardValidators = TestRewardValidators;
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
		pub const ParaDeposit: Balance = 10;
		pub const QueueSize: usize = 2;
		pub const MaxRetries: u32 = 3;
		pub const MaxCodeSize: u32 = 100;
		pub const MaxHeadSize: u32 = 100;
	}

	impl Config for Test {
		type Event = ();
		type Origin = Origin;
		type Currency = pallet_balances::Module<Test>;
		type ParachainCleanup = Parachains;
		type OnSwap = ();
		type ParaDeposit = ParaDeposit;
		type MaxCodeSize = MaxCodeSize;
		type MaxHeadSize = MaxHeadSize;
	}

	type Parachains = paras::Module<Test>;
	type Inclusion = inclusion::Module<Test>;
	type System = frame_system::Module<Test>;
	type Registrar = Module<Test>;
	type Session = pallet_session::Module<Test>;
	type Staking = pallet_staking::Module<Test>;
	type Initializer = initializer::Module<Test>;
	type Balances = pallet_balances::Module<Test>;

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

		let balances: Vec<_> = (0..authority_keys.len()).map(|i| (i as u64, 10_000_000)).collect();

		pallet_balances::GenesisConfig::<Test> {
			balances,
		}.assimilate_storage(&mut t).unwrap();

		// stashes are the index.
		let session_keys: Vec<_> = authority_keys.iter().enumerate()
			.map(|(i, _k)| (i as u64, i as u64, UintAuthorityId(i as u64)))
			.collect();

		pallet_session::GenesisConfig::<Test> {
			keys: session_keys,
		}.assimilate_storage(&mut t).unwrap();

		t.into()
	}

	fn run_to_block(n: BlockNumber) {
		// NOTE that this function only simulates modules of interest. Depending on new module may
		// require adding it here.
		println!("Running until block {}", n);
		while System::block_number() < n {
			let b = System::block_number();

			if System::block_number() > 1 {
				println!("Finalizing {}", System::block_number());
				System::on_finalize(System::block_number());
				Initializer::on_finalize(System::block_number());
			}
			// Session change every 3 blocks.
			if (b + 1) % 3 == 0 {
				println!("New session at {}", System::block_number());
				Initializer::on_new_session(
					false,
					Vec::new().into_iter(),
					Vec::new().into_iter(),
				);
			}
			System::set_block_number(b + 1);
			println!("Initializing {}", System::block_number());
			System::on_initialize(System::block_number());
			Initializer::on_initialize(System::block_number());
		}
	}

	fn test_genesis_head(size: usize) -> HeadData {
		HeadData(vec![0u8; size])
	}

	fn test_validation_code(size: usize) -> ValidationCode {
		let mut validation_code = vec![0u8; size as usize];
		// Replace first bytes of code with "WASM_MAGIC" to pass validation test.
		let _ = validation_code.splice(
			..crate::WASM_MAGIC.len(),
			crate::WASM_MAGIC.iter().cloned(),
		).collect::<Vec<_>>();
		ValidationCode(validation_code)
	}

	fn para_origin(id: ParaId) -> Origin {
		runtime_parachains::Origin::Parachain(id).into()
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(PendingSwap::get(&ParaId::from(0u32)), None);
			assert_eq!(Paras::<Test>::get(&ParaId::from(0u32)), None);
		});
	}

	#[test]
	fn end_to_end_scenario_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			// 32 is not yet registered
			assert!(!Parachains::is_parathread(32.into()));
			// We register the Para ID
			assert_ok!(Registrar::register(
				Origin::signed(1),
				32.into(),
				test_genesis_head(32),
				test_validation_code(32),
			));
			run_to_block(4); // Move to the next session
			// It is now a parathread.
			assert!(Parachains::is_parathread(32.into()));
			assert!(!Parachains::is_parachain(32.into()));
			// Some other external process will elevate parathread to parachain
			assert_ok!(Registrar::make_parachain(32.into()));
			run_to_block(8); // Move to the next session
			// It is now a parachain.
			assert!(!Parachains::is_parathread(32.into()));
			assert!(Parachains::is_parachain(32.into()));
			// Turn it back into a parathread
			assert_ok!(Registrar::make_parathread(32.into()));
			run_to_block(12); // Move to the next session
			assert!(Parachains::is_parathread(32.into()));
			assert!(!Parachains::is_parachain(32.into()));
			// Deregister it
			assert_ok!(Registrar::deregister(
				Origin::root(),
				32.into(),
			));
			run_to_block(16); // Move to the next session
			// It is nothing
			assert!(!Parachains::is_parathread(32.into()));
			assert!(!Parachains::is_parachain(32.into()));
		});
	}

	#[test]
	fn register_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert!(!Parachains::is_parathread(32.into()));
			assert_ok!(Registrar::register(
				Origin::signed(1),
				32.into(),
				test_genesis_head(32),
				test_validation_code(32),
			));
			run_to_block(4); // Move to the next session
			assert!(Parachains::is_parathread(32.into()));
			assert_eq!(Balances::reserved_balance(&1), <Test as Config>::ParaDeposit::get());
		});
	}

	#[test]
	fn register_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Successfully register 32
			assert_ok!(Registrar::register(
				Origin::signed(1),
				32.into(),
				test_genesis_head(<Test as super::Config>::MaxHeadSize::get() as usize),
				test_validation_code(<Test as super::Config>::MaxCodeSize::get() as usize),
			));

			// Can't do it again
			assert_noop!(Registrar::register(
				Origin::signed(1),
				32.into(),
				test_genesis_head(<Test as super::Config>::MaxHeadSize::get() as usize),
				test_validation_code(<Test as super::Config>::MaxCodeSize::get() as usize),
			), Error::<Test>::AlreadyRegistered);

			// Head Size Check
			assert_noop!(Registrar::register(
				Origin::signed(2),
				23.into(),
				test_genesis_head((<Test as super::Config>::MaxHeadSize::get() + 1) as usize),
				test_validation_code(<Test as super::Config>::MaxCodeSize::get() as usize),
			), Error::<Test>::HeadDataTooLarge);

			// Code Size Check
			assert_noop!(Registrar::register(
				Origin::signed(2),
				23.into(),
				test_genesis_head(<Test as super::Config>::MaxHeadSize::get() as usize),
				test_validation_code((<Test as super::Config>::MaxCodeSize::get() + 1) as usize),
			), Error::<Test>::CodeTooLarge);

			// Needs enough funds for deposit
			assert_noop!(Registrar::register(
				Origin::signed(1337),
				23.into(),
				test_genesis_head(<Test as super::Config>::MaxHeadSize::get() as usize),
				test_validation_code(<Test as super::Config>::MaxCodeSize::get() as usize),
			), BalancesError::<Test, _>::InsufficientBalance);
		});
	}

	#[test]
	fn deregister_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert!(!Parachains::is_parathread(32.into()));
			assert_ok!(Registrar::register(
				Origin::signed(1),
				32.into(),
				test_genesis_head(32),
				test_validation_code(32),
			));
			assert_eq!(Balances::reserved_balance(&1), <Test as Config>::ParaDeposit::get());
			run_to_block(4); // Move to the next session
			assert!(Parachains::is_parathread(32.into()));
			assert_ok!(Registrar::deregister(
				Origin::root(),
				32.into(),
			));
			run_to_block(8); // Move to the next session
			assert!(paras::Module::<Test>::lifecycle(32.into()).is_none());
			assert_eq!(Balances::reserved_balance(&1), 0);
		});
	}

	#[test]
	fn deregister_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert!(!Parachains::is_parathread(32.into()));
			assert_ok!(Registrar::register(
				Origin::signed(1),
				32.into(),
				test_genesis_head(32),
				test_validation_code(32),
			));
			run_to_block(4); // Move to the next session
			assert!(Parachains::is_parathread(32.into()));
			// Origin check
			assert_noop!(Registrar::deregister(
				Origin::signed(1),
				32.into(),
			), BadOrigin);
			// not registered
			assert_noop!(Registrar::deregister(
				Origin::root(),
				33.into(),
			), Error::<Test>::NotParathread);
			assert_ok!(Registrar::make_parachain(32.into()));
			run_to_block(8); // Move to the next session
			// Cant directly deregister parachain
			assert_noop!(Registrar::deregister(
				Origin::root(),
				32.into(),
			), Error::<Test>::NotParathread);
		});
	}

	#[test]
	fn swap_works() {
		new_test_ext().execute_with(|| {
			// Successfully register 23 and 32
			assert_ok!(Registrar::register(
				Origin::signed(1),
				23.into(),
				test_genesis_head(<Test as super::Config>::MaxHeadSize::get() as usize),
				test_validation_code(<Test as super::Config>::MaxCodeSize::get() as usize),
			));
			assert_ok!(Registrar::register(
				Origin::signed(2),
				32.into(),
				test_genesis_head(<Test as super::Config>::MaxHeadSize::get() as usize),
				test_validation_code(<Test as super::Config>::MaxCodeSize::get() as usize),
			));
			run_to_block(4); // Move to the next session

			// Upgrade 23 into a parachain
			assert_ok!(Registrar::make_parachain(23.into()));

			run_to_block(8); // Move to the next session

			// Roles are as we expect
			assert!(Parachains::is_parachain(23.into()));
			assert!(!Parachains::is_parathread(23.into()));
			assert!(!Parachains::is_parachain(32.into()));
			assert!(Parachains::is_parathread(32.into()));

			// Both paras initiate a swap
			assert_ok!(Registrar::swap(
				para_origin(23.into()),
				32.into()
			));
			assert_ok!(Registrar::swap(
				para_origin(32.into()),
				23.into()
			));

			run_to_block(12); // Move to the next session

			// Roles are swapped
			assert!(!Parachains::is_parachain(23.into()));
			assert!(Parachains::is_parathread(23.into()));
			assert!(Parachains::is_parachain(32.into()));
			assert!(!Parachains::is_parathread(32.into()));
		});
	}

	#[test]
	fn cannot_register_until_para_is_cleaned_up() {
		new_test_ext().execute_with(|| {
			run_to_block(2);

			assert_ok!(Registrar::register(
				Origin::signed(1),
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));

			run_to_block(4);

			assert_ok!(Registrar::deregister(Origin::root(), 1u32.into()));
			run_to_block(5);

			assert_noop!(Registrar::register(
				Origin::signed(1),
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			), Error::<Test>::AlreadyRegistered);

			// The session will be changed on the 6th block, as part of finalization. The change
			// will be observed on the 7th.
			run_to_block(7);
			assert_ok!(Registrar::register(
				Origin::signed(1),
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));
		});
	}
}
