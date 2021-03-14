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
	pallet_prelude::Weight,
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
	Origin, ParaLifecycle,
};

use crate::traits::{Registrar, OnSwap};
use parity_scale_codec::{Encode, Decode};
use sp_runtime::{RuntimeDebug, traits::Saturating};

#[derive(Encode, Decode, Clone, PartialEq, Eq, Default, RuntimeDebug)]
pub struct ParaInfo<Account, Balance> {
	pub(crate) manager: Account,
	deposit: Balance,
}

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

pub trait WeightInfo {
	fn register() -> Weight;
	fn deregister() -> Weight;
	fn swap() -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn register() -> Weight { 0 }
	fn deregister() -> Weight { 0 }
	fn swap() -> Weight { 0 }
}

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

	/// The deposit to be paid to run a parathread.
	/// This should include the cost for storing the genesis head and validation code.
	type ParaDeposit: Get<BalanceOf<Self>>;

	/// The deposit to be paid per byte stored on chain.
	type DataDepositPerByte: Get<BalanceOf<Self>>;

	/// The maximum size for the validation code.
	type MaxCodeSize: Get<u32>;

	/// The maximum size for the head data.
	type MaxHeadSize: Get<u32>;

	/// Weight Information for the Extrinsics in the Pallet
	type WeightInfo: WeightInfo;
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
		/// Cannot deregister para
		CannotDeregister,
		/// Cannot schedule downgrade of parachain to parathread
		CannotDowngrade,
		/// Cannot schedule upgrade of parathread to parachain
		CannotUpgrade,
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;

		const ParaDeposit: BalanceOf<T> = T::ParaDeposit::get();
		const DataDepositPerByte: BalanceOf<T> = T::DataDepositPerByte::get();
		const MaxCodeSize: u32 = T::MaxCodeSize::get();
		const MaxHeadSize: u32 = T::MaxHeadSize::get();

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
		#[weight = T::WeightInfo::register()]
		pub fn register(
			origin,
			id: ParaId,
			genesis_head: HeadData,
			validation_code: ValidationCode,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			Self::do_register(who, id, genesis_head, validation_code)
		}

		/// Deregister a Para Id, freeing all data and returning any deposit.
		///
		/// The caller must be the para itself or Root and the para must be a parathread.
		#[weight = T::WeightInfo::deregister()]
		pub fn deregister(origin, id: ParaId) -> DispatchResult {
			match ensure_root(origin.clone()) {
				Ok(_) => {},
				Err(_) => {
					let caller_id = ensure_parachain(<T as Config>::Origin::from(origin))?;
					ensure!(caller_id == id, Error::<T>::NotOwner);
				},
			};

			Self::do_deregister(id)
		}

		/// Swap a parachain with another parachain or parathread. The origin must be a `Parachain`.
		/// The swap will happen only if there is already an opposite swap pending. If there is not,
		/// the swap will be stored in the pending swaps map, ready for a later confirmatory swap.
		///
		/// The `ParaId`s remain mapped to the same head data and code so external code can rely on
		/// `ParaId` to be a long-term identifier of a notional "parachain". However, their
		/// scheduling info (i.e. whether they're a parathread or parachain), auction information
		/// and the auction deposit are switched.
		#[weight = T::WeightInfo::swap()]
		pub fn swap(origin, other: ParaId) {
			let id = ensure_parachain(<T as Config>::Origin::from(origin))?;
			if PendingSwap::get(other) == Some(id) {
				if let Some(other_lifecycle) = paras::Module::<T>::lifecycle(other) {
					if let Some(id_lifecycle) = paras::Module::<T>::lifecycle(id) {
						// identify which is a parachain and which is a parathread
						if id_lifecycle.is_parachain() && other_lifecycle.is_parathread() {
							// We check that both paras are in an appropriate lifecycle for a swap,
							// so these should never fail.
							let res1 = runtime_parachains::schedule_parachain_downgrade::<T>(id);
							debug_assert!(res1.is_ok());
							let res2 = runtime_parachains::schedule_parathread_upgrade::<T>(other);
							debug_assert!(res2.is_ok());
							T::OnSwap::on_swap(id, other);
						} else if id_lifecycle.is_parathread() && other_lifecycle.is_parachain() {
							// We check that both paras are in an appropriate lifecycle for a swap,
							// so these should never fail.
							let res1 = runtime_parachains::schedule_parachain_downgrade::<T>(other);
							debug_assert!(res1.is_ok());
							let res2 = runtime_parachains::schedule_parathread_upgrade::<T>(id);
							debug_assert!(res2.is_ok());
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

	/// Return the manager `AccountId` of a para if one exists.
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

	// Register a Para ID under control of `manager`.
	fn register(
		manager: T::AccountId,
		id: ParaId,
		genesis_head: HeadData,
		validation_code: ValidationCode,
	) -> DispatchResult {
		Self::do_register(manager, id, genesis_head, validation_code)
	}

	// Deregister a Para ID, free any data, and return any deposits.
	fn deregister(id: ParaId) -> DispatchResult {
		Self::do_deregister(id)
	}

	// Upgrade a registered parathread into a parachain.
	fn make_parachain(id: ParaId) -> DispatchResult {
		// Para backend should think this is a parathread...
		ensure!(paras::Module::<T>::lifecycle(id) == Some(ParaLifecycle::Parathread), Error::<T>::NotParathread);
		runtime_parachains::schedule_parathread_upgrade::<T>(id).map_err(|_| Error::<T>::CannotUpgrade)?;
		Ok(())
	}

	// Downgrade a registered para into a parathread.
	fn make_parathread(id: ParaId) -> DispatchResult {
		// Para backend should think this is a parachain...
		ensure!(paras::Module::<T>::lifecycle(id) == Some(ParaLifecycle::Parachain), Error::<T>::NotParachain);
		runtime_parachains::schedule_parachain_downgrade::<T>(id).map_err(|_| Error::<T>::CannotDowngrade)?;
		Ok(())
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn worst_head_data() -> HeadData {
		// TODO: Figure a way to allow bigger head data in benchmarks?
		let max_head_size = (T::MaxHeadSize::get()).min(1 * 1024 * 1024);
		vec![0u8; max_head_size as usize].into()
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn worst_validation_code() -> ValidationCode {
		// TODO: Figure a way to allow bigger wasm in benchmarks?
		let max_code_size = (T::MaxCodeSize::get()).min(4 * 1024 * 1024);
		let mut validation_code = vec![0u8; max_code_size as usize];
		// Replace first bytes of code with "WASM_MAGIC" to pass validation test.
		let _ = validation_code.splice(
			..crate::WASM_MAGIC.len(),
			crate::WASM_MAGIC.iter().cloned(),
		).collect::<Vec<_>>();
		validation_code.into()
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn execute_pending_transitions() {
		use runtime_parachains::shared;
		shared::Module::<T>::set_session_index(
			shared::Module::<T>::scheduled_session()
		);
		paras::Module::<T>::test_on_new_session();
	}
}

impl<T: Config> Module<T> {
	/// Attempt to register a new Para Id under management of `who` in the
	/// system with the given information.
	fn do_register(
		who: T::AccountId,
		id: ParaId,
		genesis_head: HeadData,
		validation_code: ValidationCode,
	) -> DispatchResult {
		ensure!(!Paras::<T>::contains_key(id), Error::<T>::AlreadyRegistered);
		ensure!(paras::Module::<T>::lifecycle(id).is_none(), Error::<T>::AlreadyRegistered);
		let (genesis, deposit) = Self::validate_onboarding_data(
			genesis_head,
			validation_code,
			false
		)?;

		<T as Config>::Currency::reserve(&who, deposit)?;
		let info = ParaInfo {
			manager: who.clone(),
			deposit: deposit,
		};

		Paras::<T>::insert(id, info);
		// We check above that para has no lifecycle, so this should not fail.
		let res = runtime_parachains::schedule_para_initialize::<T>(id, genesis);
		debug_assert!(res.is_ok());
		Self::deposit_event(RawEvent::Registered(id, who));
		Ok(())
	}

	/// Deregister a Para Id, freeing all data returning any deposit.
	fn do_deregister(id: ParaId) -> DispatchResult {
		ensure!(paras::Module::<T>::lifecycle(id) == Some(ParaLifecycle::Parathread), Error::<T>::NotParathread);
		runtime_parachains::schedule_para_cleanup::<T>(id).map_err(|_| Error::<T>::CannotDeregister)?;

		if let Some(info) = Paras::<T>::take(&id) {
			<T as Config>::Currency::unreserve(&info.manager, info.deposit);
		}

		PendingSwap::remove(id);
		Self::deposit_event(RawEvent::Deregistered(id));
		Ok(())
	}

	/// Verifies the onboarding data is valid for a para.
	///
	/// Returns `ParaGenesisArgs` and the deposit needed for the data.
	fn validate_onboarding_data(
		genesis_head: HeadData,
		validation_code: ValidationCode,
		parachain: bool,
	) -> Result<(ParaGenesisArgs, BalanceOf<T>), sp_runtime::DispatchError> {
		ensure!(validation_code.0.len() <= T::MaxCodeSize::get() as usize, Error::<T>::CodeTooLarge);
		ensure!(genesis_head.0.len() <= T::MaxHeadSize::get() as usize, Error::<T>::HeadDataTooLarge);
		ensure!(validation_code.0.starts_with(WASM_MAGIC), Error::<T>::DefinitelyNotWasm);

		let per_byte_fee = T::DataDepositPerByte::get();
		let deposit = T::ParaDeposit::get()
			.saturating_add(
				per_byte_fee.saturating_mul((genesis_head.0.len() as u32).into())
			).saturating_add(
				per_byte_fee.saturating_mul((validation_code.0.len() as u32).into())
			);

		Ok((ParaGenesisArgs {
			genesis_head,
			validation_code,
			parachain,
		}, deposit))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_io::TestExternalities;
	use sp_core::H256;
	use sp_runtime::{
		traits::{
			BlakeTwo256, IdentityLookup,
		}, Perbill,
	};
	use primitives::v1::{Balance, BlockNumber, Header};
	use frame_system::limits;
	use frame_support::{
		traits::{OnInitialize, OnFinalize},
		error::BadOrigin,
		assert_ok, assert_noop, parameter_types,
	};
	use runtime_parachains::{configuration, shared};
	use pallet_balances::Error as BalancesError;
	use crate::traits::Registrar as RegistrarTrait;
	use crate::paras_registrar;

	type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
	type Block = frame_system::mocking::MockBlock<Test>;

	frame_support::construct_runtime!(
		pub enum Test where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			System: frame_system::{Module, Call, Config, Storage, Event<T>},
			Balances: pallet_balances::{Module, Call, Storage, Config<T>, Event<T>},
			Parachains: paras::{Module, Origin, Call, Storage, Config<T>},
			Registrar: paras_registrar::{Module, Call, Storage, Event<T>},
		}
	);

	const NORMAL_RATIO: Perbill = Perbill::from_percent(75);
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub BlockWeights: limits::BlockWeights =
			frame_system::limits::BlockWeights::simple_max(1024);
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
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type DbWeight = ();
		type BlockWeights = BlockWeights;
		type BlockLength = BlockLength;
		type Version = ();
		type PalletInfo = PalletInfo;
		type AccountData = pallet_balances::AccountData<u128>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
	}

	parameter_types! {
		pub const ExistentialDeposit: Balance = 1;
	}

	impl pallet_balances::Config for Test {
		type Balance = u128;
		type DustRemoval = ();
		type Event = Event;
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type MaxLocks = ();
		type WeightInfo = ();
	}

	impl shared::Config for Test {}

	impl paras::Config for Test {
		type Origin = Origin;
	}

	impl configuration::Config for Test { }

	parameter_types! {
		pub const ParaDeposit: Balance = 10;
		pub const DataDepositPerByte: Balance = 1;
		pub const QueueSize: usize = 2;
		pub const MaxRetries: u32 = 3;
		pub const MaxCodeSize: u32 = 100;
		pub const MaxHeadSize: u32 = 100;
	}

	impl Config for Test {
		type Event = Event;
		type Origin = Origin;
		type Currency = Balances;
		type OnSwap = ();
		type ParaDeposit = ParaDeposit;
		type DataDepositPerByte = DataDepositPerByte;
		type MaxCodeSize = MaxCodeSize;
		type MaxHeadSize = MaxHeadSize;
		type WeightInfo = TestWeightInfo;
	}

	pub fn new_test_ext() -> TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();

		pallet_balances::GenesisConfig::<Test> {
			balances: vec![(1, 10_000_000), (2, 10_000_000)],
		}.assimilate_storage(&mut t).unwrap();

		t.into()
	}

	const BLOCKS_PER_SESSION: u32 = 3;

	fn run_to_block(n: BlockNumber) {
		// NOTE that this function only simulates modules of interest. Depending on new module may
		// require adding it here.
		assert!(System::block_number() < n);
		while System::block_number() < n {
			let b = System::block_number();

			if System::block_number() > 1 {
				System::on_finalize(System::block_number());
			}
			// Session change every 3 blocks.
			if (b + 1) % BLOCKS_PER_SESSION == 0 {
				shared::Module::<Test>::set_session_index(
					shared::Module::<Test>::session_index() + 1
				);
				Parachains::test_on_new_session();
			}
			System::set_block_number(b + 1);
			System::on_initialize(System::block_number());
		}
	}

	fn run_to_session(n: BlockNumber) {
		let block_number = n * BLOCKS_PER_SESSION;
		run_to_block(block_number);
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
			run_to_session(2);
			// It is now a parathread.
			assert!(Parachains::is_parathread(32.into()));
			assert!(!Parachains::is_parachain(32.into()));
			// Some other external process will elevate parathread to parachain
			assert_ok!(Registrar::make_parachain(32.into()));
			run_to_session(4);
			// It is now a parachain.
			assert!(!Parachains::is_parathread(32.into()));
			assert!(Parachains::is_parachain(32.into()));
			// Turn it back into a parathread
			assert_ok!(Registrar::make_parathread(32.into()));
			run_to_session(6);
			assert!(Parachains::is_parathread(32.into()));
			assert!(!Parachains::is_parachain(32.into()));
			// Deregister it
			assert_ok!(Registrar::deregister(
				Origin::root(),
				32.into(),
			));
			run_to_session(8);
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
			run_to_session(2);
			assert!(Parachains::is_parathread(32.into()));
			assert_eq!(
				Balances::reserved_balance(&1),
				<Test as Config>::ParaDeposit::get() + 64 * <Test as Config>::DataDepositPerByte::get()
			);
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

			run_to_session(2);

			assert_ok!(Registrar::deregister(Origin::root(), 32u32.into()));

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
			assert_eq!(
				Balances::reserved_balance(&1),
				<Test as Config>::ParaDeposit::get() + 64 * <Test as Config>::DataDepositPerByte::get()
			);
			run_to_session(2);
			assert!(Parachains::is_parathread(32.into()));
			assert_ok!(Registrar::deregister(
				Origin::root(),
				32.into(),
			));
			run_to_session(4);
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
			run_to_session(2);
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
			run_to_session(4);
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
			run_to_session(2);

			// Upgrade 23 into a parachain
			assert_ok!(Registrar::make_parachain(23.into()));

			run_to_session(4);

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

			run_to_session(6);

			// Deregister a parathread that was originally a parachain
			assert_eq!(Parachains::lifecycle(23u32.into()), Some(ParaLifecycle::Parathread));
			assert_ok!(Registrar::deregister(runtime_parachains::Origin::Parachain(23u32.into()).into(), 23u32.into()));

			run_to_block(21);

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
			run_to_block(1);

			assert_ok!(Registrar::register(
				Origin::signed(1),
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));

			// 2 session changes to fully onboard.
			run_to_session(2);

			assert_eq!(Parachains::lifecycle(1u32.into()), Some(ParaLifecycle::Parathread));
			assert_ok!(Registrar::deregister(Origin::root(), 1u32.into()));

			// Cannot register while it is offboarding.
			run_to_session(3);

			assert_noop!(Registrar::register(
				Origin::signed(1),
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			), Error::<Test>::AlreadyRegistered);

			// By session 4, it is offboarded, and we can register again.
			run_to_session(4);

			assert_ok!(Registrar::register(
				Origin::signed(1),
				1u32.into(),
				vec![1; 3].into(),
				WASM_MAGIC.to_vec().into(),
			));
		});
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking {
	use super::{*, Module as Registrar};
	use frame_system::RawOrigin;
	use frame_support::assert_ok;
	use sp_runtime::traits::Bounded;
	use crate::traits::{Registrar as RegistrarT};
	use runtime_parachains::{paras, shared, Origin as ParaOrigin};

	use frame_benchmarking::{benchmarks, whitelisted_caller};

	fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
		let events = frame_system::Module::<T>::events();
		let system_event: <T as frame_system::Config>::Event = generic_event.into();
		// compare to the last event record
		let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
		assert_eq!(event, &system_event);
	}

	fn register_para<T: Config>(id: u32) -> ParaId {
		let para = ParaId::from(id);
		let genesis_head = Registrar::<T>::worst_head_data();
		let validation_code = Registrar::<T>::worst_validation_code();
		let caller: T::AccountId = whitelisted_caller();
		T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		assert_ok!(Registrar::<T>::register(RawOrigin::Signed(caller).into(), para, genesis_head, validation_code));
		return para;
	}

	fn para_origin(id: u32) -> ParaOrigin {
		ParaOrigin::Parachain(id.into())
	}

	// This function moves forward to the next scheduled session for parachain lifecycle upgrades.
	fn next_scheduled_session<T: Config>() {
		shared::Module::<T>::set_session_index(
			shared::Module::<T>::scheduled_session()
		);
		paras::Module::<T>::test_on_new_session();
	}

	benchmarks! {
		where_clause { where ParaOrigin: Into<<T as frame_system::Config>::Origin> }

		register {
			let para = ParaId::from(1337);
			let genesis_head = Registrar::<T>::worst_head_data();
			let validation_code = Registrar::<T>::worst_validation_code();
			let caller: T::AccountId = whitelisted_caller();
			T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		}: _(RawOrigin::Signed(caller.clone()), para, genesis_head, validation_code)
		verify {
			assert_last_event::<T>(RawEvent::Registered(para, caller).into());
			assert_eq!(paras::Module::<T>::lifecycle(para), Some(ParaLifecycle::Onboarding));
			next_scheduled_session::<T>();
			assert_eq!(paras::Module::<T>::lifecycle(para), Some(ParaLifecycle::Parathread));
		}

		deregister {
			let para = register_para::<T>(1337);
			next_scheduled_session::<T>();
		}: _(RawOrigin::Root, para)
		verify {
			assert_last_event::<T>(RawEvent::Deregistered(para).into());
		}

		swap {
			let parathread = register_para::<T>(1337);
			let parachain = register_para::<T>(1338);

			let parathread_origin = para_origin(1337);
			let parachain_origin = para_origin(1338);

			// Actually finish registration process
			next_scheduled_session::<T>();

			// Upgrade the parachain
			Registrar::<T>::make_parachain(parachain)?;
			next_scheduled_session::<T>();

			assert_eq!(paras::Module::<T>::lifecycle(parachain), Some(ParaLifecycle::Parachain));
			assert_eq!(paras::Module::<T>::lifecycle(parathread), Some(ParaLifecycle::Parathread));

			Registrar::<T>::swap(parathread_origin.into(), parachain)?;
		}: {
			Registrar::<T>::swap(parachain_origin.into(), parathread)?;
		} verify {
			next_scheduled_session::<T>();
			// Swapped!
			assert_eq!(paras::Module::<T>::lifecycle(parachain), Some(ParaLifecycle::Parathread));
			assert_eq!(paras::Module::<T>::lifecycle(parathread), Some(ParaLifecycle::Parachain));
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use crate::integration_tests::{new_test_ext, Test};
		use frame_support::assert_ok;

		#[test]
		fn test_benchmarks() {
			new_test_ext().execute_with(|| {
				assert_ok!(test_benchmark_register::<Test>());
				assert_ok!(test_benchmark_deregister::<Test>());
				assert_ok!(test_benchmark_swap::<Test>());
			});
		}
	}
}
