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

//! Pallet to handle parathread/parachain registration and related fund management.
//! In essence this is a simple wrapper around `paras`.

use frame_support::{
	dispatch::DispatchResult,
	ensure,
	pallet_prelude::Weight,
	traits::{Currency, Get, ReservableCurrency},
};
use frame_system::{self, ensure_root, ensure_signed};
use primitives::{HeadData, Id as ParaId, ValidationCode, LOWEST_PUBLIC_ID};
use runtime_parachains::{
	configuration, ensure_parachain,
	paras::{self, ParaGenesisArgs},
	Origin, ParaLifecycle,
};
use sp_std::{prelude::*, result};

use crate::traits::{OnSwap, Registrar};
pub use pallet::*;
use parity_scale_codec::{Decode, Encode};
use runtime_parachains::paras::ParaKind;
use scale_info::TypeInfo;
use sp_runtime::{
	traits::{CheckedSub, Saturating},
	RuntimeDebug,
};

#[derive(Encode, Decode, Clone, PartialEq, Eq, Default, RuntimeDebug, TypeInfo)]
pub struct ParaInfo<Account, Balance> {
	/// The account that has placed a deposit for registering this para.
	pub(crate) manager: Account,
	/// The amount reserved by the `manager` account for the registration.
	deposit: Balance,
	/// Whether the para registration should be locked from being controlled by the manager.
	locked: bool,
}

type BalanceOf<T> =
	<<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

pub trait WeightInfo {
	fn reserve() -> Weight;
	fn register() -> Weight;
	fn force_register() -> Weight;
	fn deregister() -> Weight;
	fn swap() -> Weight;
	fn schedule_code_upgrade(b: u32) -> Weight;
	fn set_current_head(b: u32) -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn reserve() -> Weight {
		Weight::zero()
	}
	fn register() -> Weight {
		Weight::zero()
	}
	fn force_register() -> Weight {
		Weight::zero()
	}
	fn deregister() -> Weight {
		Weight::zero()
	}
	fn swap() -> Weight {
		Weight::zero()
	}
	fn schedule_code_upgrade(_b: u32) -> Weight {
		Weight::zero()
	}
	fn set_current_head(_b: u32) -> Weight {
		Weight::zero()
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

	#[pallet::config]
	#[pallet::disable_frame_system_supertrait_check]
	pub trait Config: configuration::Config + paras::Config {
		/// The overarching event type.
		type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

		/// The aggregated origin type must support the `parachains` origin. We require that we can
		/// infallibly convert between this origin and the system origin, but in reality, they're the
		/// same type, we just can't express that to the Rust type system without writing a `where`
		/// clause everywhere.
		type RuntimeOrigin: From<<Self as frame_system::Config>::RuntimeOrigin>
			+ Into<result::Result<Origin, <Self as Config>::RuntimeOrigin>>;

		/// The system's currency for parathread payment.
		type Currency: ReservableCurrency<Self::AccountId>;

		/// Runtime hook for when a parachain and parathread swap.
		type OnSwap: crate::traits::OnSwap;

		/// The deposit to be paid to run a parathread.
		/// This should include the cost for storing the genesis head and validation code.
		#[pallet::constant]
		type ParaDeposit: Get<BalanceOf<Self>>;

		/// The deposit to be paid per byte stored on chain.
		#[pallet::constant]
		type DataDepositPerByte: Get<BalanceOf<Self>>;

		/// Weight Information for the Extrinsics in the Pallet
		type WeightInfo: WeightInfo;
	}

	#[pallet::event]
	#[pallet::generate_deposit(pub(super) fn deposit_event)]
	pub enum Event<T: Config> {
		Registered { para_id: ParaId, manager: T::AccountId },
		Deregistered { para_id: ParaId },
		Reserved { para_id: ParaId, who: T::AccountId },
	}

	#[pallet::error]
	pub enum Error<T> {
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
		/// Para is locked from manipulation by the manager. Must use parachain or relay chain governance.
		ParaLocked,
		/// The ID given for registration has not been reserved.
		NotReserved,
		/// Registering parachain with empty code is not allowed.
		EmptyCode,
		/// Cannot perform a parachain slot / lifecycle swap. Check that the state of both paras are
		/// correct for the swap to work.
		CannotSwap,
	}

	/// Pending swap operations.
	#[pallet::storage]
	pub(super) type PendingSwap<T> = StorageMap<_, Twox64Concat, ParaId, ParaId>;

	/// Amount held on deposit for each para and the original depositor.
	///
	/// The given account ID is responsible for registering the code and initial head data, but may only do
	/// so if it isn't yet registered. (After that, it's up to governance to do so.)
	#[pallet::storage]
	pub type Paras<T: Config> =
		StorageMap<_, Twox64Concat, ParaId, ParaInfo<T::AccountId, BalanceOf<T>>>;

	/// The next free `ParaId`.
	#[pallet::storage]
	pub type NextFreeParaId<T> = StorageValue<_, ParaId, ValueQuery>;

	#[pallet::genesis_config]
	pub struct GenesisConfig {
		pub next_free_para_id: ParaId,
	}

	#[cfg(feature = "std")]
	impl Default for GenesisConfig {
		fn default() -> Self {
			GenesisConfig { next_free_para_id: LOWEST_PUBLIC_ID }
		}
	}

	#[pallet::genesis_build]
	impl<T: Config> GenesisBuild<T> for GenesisConfig {
		fn build(&self) {
			NextFreeParaId::<T>::put(self.next_free_para_id);
		}
	}

	#[pallet::hooks]
	impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {}

	#[pallet::call]
	impl<T: Config> Pallet<T> {
		/// Register head data and validation code for a reserved Para Id.
		///
		/// ## Arguments
		/// - `origin`: Must be called by a `Signed` origin.
		/// - `id`: The para ID. Must be owned/managed by the `origin` signing account.
		/// - `genesis_head`: The genesis head data of the parachain/thread.
		/// - `validation_code`: The initial validation code of the parachain/thread.
		///
		/// ## Deposits/Fees
		/// The origin signed account must reserve a corresponding deposit for the registration. Anything already
		/// reserved previously for this para ID is accounted for.
		///
		/// ## Events
		/// The `Registered` event is emitted in case of success.
		#[pallet::call_index(0)]
		#[pallet::weight(<T as Config>::WeightInfo::register())]
		pub fn register(
			origin: OriginFor<T>,
			id: ParaId,
			genesis_head: HeadData,
			validation_code: ValidationCode,
		) -> DispatchResult {
			let who = ensure_signed(origin)?;
			Self::do_register(who, None, id, genesis_head, validation_code, true)?;
			Ok(())
		}

		/// Force the registration of a Para Id on the relay chain.
		///
		/// This function must be called by a Root origin.
		///
		/// The deposit taken can be specified for this registration. Any `ParaId`
		/// can be registered, including sub-1000 IDs which are System Parachains.
		#[pallet::call_index(1)]
		#[pallet::weight(<T as Config>::WeightInfo::force_register())]
		pub fn force_register(
			origin: OriginFor<T>,
			who: T::AccountId,
			deposit: BalanceOf<T>,
			id: ParaId,
			genesis_head: HeadData,
			validation_code: ValidationCode,
		) -> DispatchResult {
			ensure_root(origin)?;
			Self::do_register(who, Some(deposit), id, genesis_head, validation_code, false)
		}

		/// Deregister a Para Id, freeing all data and returning any deposit.
		///
		/// The caller must be Root, the `para` owner, or the `para` itself. The para must be a parathread.
		#[pallet::call_index(2)]
		#[pallet::weight(<T as Config>::WeightInfo::deregister())]
		pub fn deregister(origin: OriginFor<T>, id: ParaId) -> DispatchResult {
			Self::ensure_root_para_or_owner(origin, id)?;
			Self::do_deregister(id)
		}

		/// Swap a parachain with another parachain or parathread.
		///
		/// The origin must be Root, the `para` owner, or the `para` itself.
		///
		/// The swap will happen only if there is already an opposite swap pending. If there is not,
		/// the swap will be stored in the pending swaps map, ready for a later confirmatory swap.
		///
		/// The `ParaId`s remain mapped to the same head data and code so external code can rely on
		/// `ParaId` to be a long-term identifier of a notional "parachain". However, their
		/// scheduling info (i.e. whether they're a parathread or parachain), auction information
		/// and the auction deposit are switched.
		#[pallet::call_index(3)]
		#[pallet::weight(<T as Config>::WeightInfo::swap())]
		pub fn swap(origin: OriginFor<T>, id: ParaId, other: ParaId) -> DispatchResult {
			Self::ensure_root_para_or_owner(origin, id)?;

			// If `id` and `other` is the same id, we treat this as a "clear" function, and exit
			// early, since swapping the same id would otherwise be a noop.
			if id == other {
				PendingSwap::<T>::remove(id);
				return Ok(())
			}

			// Sanity check that `id` is even a para.
			let id_lifecycle =
				paras::Pallet::<T>::lifecycle(id).ok_or(Error::<T>::NotRegistered)?;

			if PendingSwap::<T>::get(other) == Some(id) {
				let other_lifecycle =
					paras::Pallet::<T>::lifecycle(other).ok_or(Error::<T>::NotRegistered)?;
				// identify which is a parachain and which is a parathread
				if id_lifecycle == ParaLifecycle::Parachain &&
					other_lifecycle == ParaLifecycle::Parathread
				{
					Self::do_thread_and_chain_swap(id, other);
				} else if id_lifecycle == ParaLifecycle::Parathread &&
					other_lifecycle == ParaLifecycle::Parachain
				{
					Self::do_thread_and_chain_swap(other, id);
				} else if id_lifecycle == ParaLifecycle::Parachain &&
					other_lifecycle == ParaLifecycle::Parachain
				{
					// If both chains are currently parachains, there is nothing funny we
					// need to do for their lifecycle management, just swap the underlying
					// data.
					T::OnSwap::on_swap(id, other);
				} else {
					return Err(Error::<T>::CannotSwap.into())
				}
				PendingSwap::<T>::remove(other);
			} else {
				PendingSwap::<T>::insert(id, other);
			}

			Ok(())
		}

		/// Remove a manager lock from a para. This will allow the manager of a
		/// previously locked para to deregister or swap a para without using governance.
		///
		/// Can only be called by the Root origin or the parachain.
		#[pallet::call_index(4)]
		#[pallet::weight(T::DbWeight::get().reads_writes(1, 1))]
		pub fn remove_lock(origin: OriginFor<T>, para: ParaId) -> DispatchResult {
			Self::ensure_root_or_para(origin, para)?;
			<Self as Registrar>::remove_lock(para);
			Ok(())
		}

		/// Reserve a Para Id on the relay chain.
		///
		/// This function will reserve a new Para Id to be owned/managed by the origin account.
		/// The origin account is able to register head data and validation code using `register` to create
		/// a parathread. Using the Slots pallet, a parathread can then be upgraded to get a parachain slot.
		///
		/// ## Arguments
		/// - `origin`: Must be called by a `Signed` origin. Becomes the manager/owner of the new para ID.
		///
		/// ## Deposits/Fees
		/// The origin must reserve a deposit of `ParaDeposit` for the registration.
		///
		/// ## Events
		/// The `Reserved` event is emitted in case of success, which provides the ID reserved for use.
		#[pallet::call_index(5)]
		#[pallet::weight(<T as Config>::WeightInfo::reserve())]
		pub fn reserve(origin: OriginFor<T>) -> DispatchResult {
			let who = ensure_signed(origin)?;
			let id = NextFreeParaId::<T>::get().max(LOWEST_PUBLIC_ID);
			Self::do_reserve(who, None, id)?;
			NextFreeParaId::<T>::set(id + 1);
			Ok(())
		}

		/// Add a manager lock from a para. This will prevent the manager of a
		/// para to deregister or swap a para.
		///
		/// Can be called by Root, the parachain, or the parachain manager if the parachain is unlocked.
		#[pallet::call_index(6)]
		#[pallet::weight(T::DbWeight::get().reads_writes(1, 1))]
		pub fn add_lock(origin: OriginFor<T>, para: ParaId) -> DispatchResult {
			Self::ensure_root_para_or_owner(origin, para)?;
			<Self as Registrar>::apply_lock(para);
			Ok(())
		}

		/// Schedule a parachain upgrade.
		///
		/// Can be called by Root, the parachain, or the parachain manager if the parachain is unlocked.
		#[pallet::call_index(7)]
		#[pallet::weight(<T as Config>::WeightInfo::schedule_code_upgrade(new_code.0.len() as u32))]
		pub fn schedule_code_upgrade(
			origin: OriginFor<T>,
			para: ParaId,
			new_code: ValidationCode,
		) -> DispatchResult {
			Self::ensure_root_para_or_owner(origin, para)?;
			runtime_parachains::schedule_code_upgrade::<T>(para, new_code)?;
			Ok(())
		}

		/// Set the parachain's current head.
		///
		/// Can be called by Root, the parachain, or the parachain manager if the parachain is unlocked.
		#[pallet::call_index(8)]
		#[pallet::weight(<T as Config>::WeightInfo::set_current_head(new_head.0.len() as u32))]
		pub fn set_current_head(
			origin: OriginFor<T>,
			para: ParaId,
			new_head: HeadData,
		) -> DispatchResult {
			Self::ensure_root_para_or_owner(origin, para)?;
			runtime_parachains::set_current_head::<T>(para, new_head);
			Ok(())
		}
	}
}

impl<T: Config> Registrar for Pallet<T> {
	type AccountId = T::AccountId;

	/// Return the manager `AccountId` of a para if one exists.
	fn manager_of(id: ParaId) -> Option<T::AccountId> {
		Some(Paras::<T>::get(id)?.manager)
	}

	// All parachains. Ordered ascending by ParaId. Parathreads are not included.
	fn parachains() -> Vec<ParaId> {
		paras::Pallet::<T>::parachains()
	}

	// Return if a para is a parathread
	fn is_parathread(id: ParaId) -> bool {
		paras::Pallet::<T>::is_parathread(id)
	}

	// Return if a para is a parachain
	fn is_parachain(id: ParaId) -> bool {
		paras::Pallet::<T>::is_parachain(id)
	}

	// Apply a lock to the parachain.
	fn apply_lock(id: ParaId) {
		Paras::<T>::mutate(id, |x| x.as_mut().map(|mut info| info.locked = true));
	}

	// Remove a lock from the parachain.
	fn remove_lock(id: ParaId) {
		Paras::<T>::mutate(id, |x| x.as_mut().map(|mut info| info.locked = false));
	}

	// Register a Para ID under control of `manager`.
	//
	// Note this is a backend registration API, so verification of ParaId
	// is not done here to prevent.
	fn register(
		manager: T::AccountId,
		id: ParaId,
		genesis_head: HeadData,
		validation_code: ValidationCode,
	) -> DispatchResult {
		Self::do_register(manager, None, id, genesis_head, validation_code, false)
	}

	// Deregister a Para ID, free any data, and return any deposits.
	fn deregister(id: ParaId) -> DispatchResult {
		Self::do_deregister(id)
	}

	// Upgrade a registered parathread into a parachain.
	fn make_parachain(id: ParaId) -> DispatchResult {
		// Para backend should think this is a parathread...
		ensure!(
			paras::Pallet::<T>::lifecycle(id) == Some(ParaLifecycle::Parathread),
			Error::<T>::NotParathread
		);
		runtime_parachains::schedule_parathread_upgrade::<T>(id)
			.map_err(|_| Error::<T>::CannotUpgrade)?;
		// Once a para has upgraded to a parachain, it can no longer be managed by the owner.
		// Intentionally, the flag stays with the para even after downgrade.
		Self::apply_lock(id);
		Ok(())
	}

	// Downgrade a registered para into a parathread.
	fn make_parathread(id: ParaId) -> DispatchResult {
		// Para backend should think this is a parachain...
		ensure!(
			paras::Pallet::<T>::lifecycle(id) == Some(ParaLifecycle::Parachain),
			Error::<T>::NotParachain
		);
		runtime_parachains::schedule_parachain_downgrade::<T>(id)
			.map_err(|_| Error::<T>::CannotDowngrade)?;
		Ok(())
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn worst_head_data() -> HeadData {
		let max_head_size = configuration::Pallet::<T>::config().max_head_data_size;
		assert!(max_head_size > 0, "max_head_data can't be zero for generating worst head data.");
		vec![0u8; max_head_size as usize].into()
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn worst_validation_code() -> ValidationCode {
		let max_code_size = configuration::Pallet::<T>::config().max_code_size;
		assert!(max_code_size > 0, "max_code_size can't be zero for generating worst code data.");
		let validation_code = vec![0u8; max_code_size as usize];
		validation_code.into()
	}

	#[cfg(any(feature = "runtime-benchmarks", test))]
	fn execute_pending_transitions() {
		use runtime_parachains::shared;
		shared::Pallet::<T>::set_session_index(shared::Pallet::<T>::scheduled_session());
		paras::Pallet::<T>::test_on_new_session();
	}
}

impl<T: Config> Pallet<T> {
	/// Ensure the origin is one of Root, the `para` owner, or the `para` itself.
	/// If the origin is the `para` owner, the `para` must be unlocked.
	fn ensure_root_para_or_owner(
		origin: <T as frame_system::Config>::RuntimeOrigin,
		id: ParaId,
	) -> DispatchResult {
		ensure_signed(origin.clone())
			.map_err(|e| e.into())
			.and_then(|who| -> DispatchResult {
				let para_info = Paras::<T>::get(id).ok_or(Error::<T>::NotRegistered)?;
				ensure!(!para_info.locked, Error::<T>::ParaLocked);
				ensure!(para_info.manager == who, Error::<T>::NotOwner);
				Ok(())
			})
			.or_else(|_| -> DispatchResult { Self::ensure_root_or_para(origin, id) })
	}

	/// Ensure the origin is one of Root or the `para` itself.
	fn ensure_root_or_para(
		origin: <T as frame_system::Config>::RuntimeOrigin,
		id: ParaId,
	) -> DispatchResult {
		if let Ok(caller_id) = ensure_parachain(<T as Config>::RuntimeOrigin::from(origin.clone()))
		{
			// Check if matching para id...
			ensure!(caller_id == id, Error::<T>::NotOwner);
		} else {
			// Check if root...
			ensure_root(origin.clone())?;
		}
		Ok(())
	}

	fn do_reserve(
		who: T::AccountId,
		deposit_override: Option<BalanceOf<T>>,
		id: ParaId,
	) -> DispatchResult {
		ensure!(!Paras::<T>::contains_key(id), Error::<T>::AlreadyRegistered);
		ensure!(paras::Pallet::<T>::lifecycle(id).is_none(), Error::<T>::AlreadyRegistered);

		let deposit = deposit_override.unwrap_or_else(T::ParaDeposit::get);
		<T as Config>::Currency::reserve(&who, deposit)?;
		let info = ParaInfo { manager: who.clone(), deposit, locked: false };

		Paras::<T>::insert(id, info);
		Self::deposit_event(Event::<T>::Reserved { para_id: id, who });
		Ok(())
	}

	/// Attempt to register a new Para Id under management of `who` in the
	/// system with the given information.
	fn do_register(
		who: T::AccountId,
		deposit_override: Option<BalanceOf<T>>,
		id: ParaId,
		genesis_head: HeadData,
		validation_code: ValidationCode,
		ensure_reserved: bool,
	) -> DispatchResult {
		let deposited = if let Some(para_data) = Paras::<T>::get(id) {
			ensure!(para_data.manager == who, Error::<T>::NotOwner);
			ensure!(!para_data.locked, Error::<T>::ParaLocked);
			para_data.deposit
		} else {
			ensure!(!ensure_reserved, Error::<T>::NotReserved);
			Default::default()
		};
		ensure!(paras::Pallet::<T>::lifecycle(id).is_none(), Error::<T>::AlreadyRegistered);
		let (genesis, deposit) =
			Self::validate_onboarding_data(genesis_head, validation_code, ParaKind::Parathread)?;
		let deposit = deposit_override.unwrap_or(deposit);

		if let Some(additional) = deposit.checked_sub(&deposited) {
			<T as Config>::Currency::reserve(&who, additional)?;
		} else if let Some(rebate) = deposited.checked_sub(&deposit) {
			<T as Config>::Currency::unreserve(&who, rebate);
		};
		let info = ParaInfo { manager: who.clone(), deposit, locked: false };

		Paras::<T>::insert(id, info);
		// We check above that para has no lifecycle, so this should not fail.
		let res = runtime_parachains::schedule_para_initialize::<T>(id, genesis);
		debug_assert!(res.is_ok());
		Self::deposit_event(Event::<T>::Registered { para_id: id, manager: who });
		Ok(())
	}

	/// Deregister a Para Id, freeing all data returning any deposit.
	fn do_deregister(id: ParaId) -> DispatchResult {
		match paras::Pallet::<T>::lifecycle(id) {
			// Para must be a parathread, or not exist at all.
			Some(ParaLifecycle::Parathread) | None => {},
			_ => return Err(Error::<T>::NotParathread.into()),
		}
		runtime_parachains::schedule_para_cleanup::<T>(id)
			.map_err(|_| Error::<T>::CannotDeregister)?;

		if let Some(info) = Paras::<T>::take(&id) {
			<T as Config>::Currency::unreserve(&info.manager, info.deposit);
		}

		PendingSwap::<T>::remove(id);
		Self::deposit_event(Event::<T>::Deregistered { para_id: id });
		Ok(())
	}

	/// Verifies the onboarding data is valid for a para.
	///
	/// Returns `ParaGenesisArgs` and the deposit needed for the data.
	fn validate_onboarding_data(
		genesis_head: HeadData,
		validation_code: ValidationCode,
		para_kind: ParaKind,
	) -> Result<(ParaGenesisArgs, BalanceOf<T>), sp_runtime::DispatchError> {
		let config = configuration::Pallet::<T>::config();
		ensure!(validation_code.0.len() > 0, Error::<T>::EmptyCode);
		ensure!(validation_code.0.len() <= config.max_code_size as usize, Error::<T>::CodeTooLarge);
		ensure!(
			genesis_head.0.len() <= config.max_head_data_size as usize,
			Error::<T>::HeadDataTooLarge
		);

		let per_byte_fee = T::DataDepositPerByte::get();
		let deposit = T::ParaDeposit::get()
			.saturating_add(per_byte_fee.saturating_mul((genesis_head.0.len() as u32).into()))
			.saturating_add(per_byte_fee.saturating_mul((validation_code.0.len() as u32).into()));

		Ok((ParaGenesisArgs { genesis_head, validation_code, para_kind }, deposit))
	}

	/// Swap a parachain and parathread, which involves scheduling an appropriate lifecycle update.
	fn do_thread_and_chain_swap(to_downgrade: ParaId, to_upgrade: ParaId) {
		let res1 = runtime_parachains::schedule_parachain_downgrade::<T>(to_downgrade);
		debug_assert!(res1.is_ok());
		let res2 = runtime_parachains::schedule_parathread_upgrade::<T>(to_upgrade);
		debug_assert!(res2.is_ok());
		T::OnSwap::on_swap(to_upgrade, to_downgrade);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{paras_registrar, traits::Registrar as RegistrarTrait};
	use frame_support::{
		assert_noop, assert_ok,
		error::BadOrigin,
		parameter_types,
		traits::{GenesisBuild, OnFinalize, OnInitialize},
	};
	use frame_system::limits;
	use pallet_balances::Error as BalancesError;
	use primitives::{Balance, BlockNumber, Header};
	use runtime_parachains::{configuration, origin, shared};
	use sp_core::H256;
	use sp_io::TestExternalities;
	use sp_runtime::{
		traits::{BlakeTwo256, IdentityLookup},
		transaction_validity::TransactionPriority,
		Perbill,
	};
	use sp_std::collections::btree_map::BTreeMap;

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
			Configuration: configuration::{Pallet, Call, Storage, Config<T>},
			Parachains: paras::{Pallet, Call, Storage, Config, Event},
			ParasShared: shared::{Pallet, Call, Storage},
			Registrar: paras_registrar::{Pallet, Call, Storage, Event<T>},
			ParachainsOrigin: origin::{Pallet, Origin},
		}
	);

	impl<C> frame_system::offchain::SendTransactionTypes<C> for Test
	where
		RuntimeCall: From<C>,
	{
		type Extrinsic = UncheckedExtrinsic;
		type OverarchingCall = RuntimeCall;
	}

	const NORMAL_RATIO: Perbill = Perbill::from_percent(75);
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub BlockWeights: limits::BlockWeights =
			frame_system::limits::BlockWeights::simple_max(Weight::from_parts(1024, u64::MAX));
		pub BlockLength: limits::BlockLength =
			limits::BlockLength::max_with_normal_ratio(4 * 1024 * 1024, NORMAL_RATIO);
	}

	impl frame_system::Config for Test {
		type BaseCallFilter = frame_support::traits::Everything;
		type RuntimeOrigin = RuntimeOrigin;
		type RuntimeCall = RuntimeCall;
		type Index = u64;
		type BlockNumber = BlockNumber;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type RuntimeEvent = RuntimeEvent;
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
		type OnSetCode = ();
		type MaxConsumers = frame_support::traits::ConstU32<16>;
	}

	parameter_types! {
		pub const ExistentialDeposit: Balance = 1;
	}

	impl pallet_balances::Config for Test {
		type Balance = u128;
		type DustRemoval = ();
		type RuntimeEvent = RuntimeEvent;
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type MaxLocks = ();
		type MaxReserves = ();
		type ReserveIdentifier = [u8; 8];
		type WeightInfo = ();
	}

	impl shared::Config for Test {}

	impl origin::Config for Test {}

	parameter_types! {
		pub const ParasUnsignedPriority: TransactionPriority = TransactionPriority::max_value();
	}

	impl paras::Config for Test {
		type RuntimeEvent = RuntimeEvent;
		type WeightInfo = paras::TestWeightInfo;
		type UnsignedPriority = ParasUnsignedPriority;
		type NextSessionRotation = crate::mock::TestNextSessionRotation;
	}

	impl configuration::Config for Test {
		type WeightInfo = configuration::TestWeightInfo;
	}

	parameter_types! {
		pub const ParaDeposit: Balance = 10;
		pub const DataDepositPerByte: Balance = 1;
		pub const MaxRetries: u32 = 3;
	}

	impl Config for Test {
		type RuntimeOrigin = RuntimeOrigin;
		type RuntimeEvent = RuntimeEvent;
		type Currency = Balances;
		type OnSwap = MockSwap;
		type ParaDeposit = ParaDeposit;
		type DataDepositPerByte = DataDepositPerByte;
		type WeightInfo = TestWeightInfo;
	}

	pub fn new_test_ext() -> TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();

		GenesisBuild::<Test>::assimilate_storage(
			&configuration::GenesisConfig {
				config: configuration::HostConfiguration {
					max_code_size: 2 * 1024 * 1024,      // 2 MB
					max_head_data_size: 1 * 1024 * 1024, // 1 MB
					..Default::default()
				},
			},
			&mut t,
		)
		.unwrap();

		pallet_balances::GenesisConfig::<Test> { balances: vec![(1, 10_000_000), (2, 10_000_000)] }
			.assimilate_storage(&mut t)
			.unwrap();

		t.into()
	}

	parameter_types! {
		pub static SwapData: BTreeMap<ParaId, u64> = BTreeMap::new();
	}

	pub struct MockSwap;
	impl OnSwap for MockSwap {
		fn on_swap(one: ParaId, other: ParaId) {
			let mut swap_data = SwapData::get();
			let one_data = swap_data.remove(&one).unwrap_or_default();
			let other_data = swap_data.remove(&other).unwrap_or_default();
			swap_data.insert(one, other_data);
			swap_data.insert(other, one_data);
			SwapData::set(swap_data);
		}
	}

	const BLOCKS_PER_SESSION: u32 = 3;

	fn run_to_block(n: BlockNumber) {
		// NOTE that this function only simulates modules of interest. Depending on new pallet may
		// require adding it here.
		assert!(System::block_number() < n);
		while System::block_number() < n {
			let b = System::block_number();

			if System::block_number() > 1 {
				System::on_finalize(System::block_number());
			}
			// Session change every 3 blocks.
			if (b + 1) % BLOCKS_PER_SESSION == 0 {
				shared::Pallet::<Test>::set_session_index(
					shared::Pallet::<Test>::session_index() + 1,
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
		let validation_code = vec![0u8; size as usize];
		ValidationCode(validation_code)
	}

	fn para_origin(id: ParaId) -> RuntimeOrigin {
		runtime_parachains::Origin::Parachain(id).into()
	}

	fn max_code_size() -> u32 {
		Configuration::config().max_code_size
	}

	fn max_head_size() -> u32 {
		Configuration::config().max_head_data_size
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(PendingSwap::<Test>::get(&ParaId::from(0u32)), None);
			assert_eq!(Paras::<Test>::get(&ParaId::from(0u32)), None);
		});
	}

	#[test]
	fn end_to_end_scenario_works() {
		new_test_ext().execute_with(|| {
			let para_id = LOWEST_PUBLIC_ID;
			run_to_block(1);
			// first para is not yet registered
			assert!(!Parachains::is_parathread(para_id));
			// We register the Para ID
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(1),
				para_id,
				test_genesis_head(32),
				test_validation_code(32),
			));
			run_to_session(2);
			// It is now a parathread.
			assert!(Parachains::is_parathread(para_id));
			assert!(!Parachains::is_parachain(para_id));
			// Some other external process will elevate parathread to parachain
			assert_ok!(Registrar::make_parachain(para_id));
			run_to_session(4);
			// It is now a parachain.
			assert!(!Parachains::is_parathread(para_id));
			assert!(Parachains::is_parachain(para_id));
			// Turn it back into a parathread
			assert_ok!(Registrar::make_parathread(para_id));
			run_to_session(6);
			assert!(Parachains::is_parathread(para_id));
			assert!(!Parachains::is_parachain(para_id));
			// Deregister it
			assert_ok!(Registrar::deregister(RuntimeOrigin::root(), para_id,));
			run_to_session(8);
			// It is nothing
			assert!(!Parachains::is_parathread(para_id));
			assert!(!Parachains::is_parachain(para_id));
		});
	}

	#[test]
	fn register_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			let para_id = LOWEST_PUBLIC_ID;
			assert!(!Parachains::is_parathread(para_id));
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
			assert_eq!(Balances::reserved_balance(&1), <Test as Config>::ParaDeposit::get());
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(1),
				para_id,
				test_genesis_head(32),
				test_validation_code(32),
			));
			run_to_session(2);
			assert!(Parachains::is_parathread(para_id));
			assert_eq!(
				Balances::reserved_balance(&1),
				<Test as Config>::ParaDeposit::get() +
					64 * <Test as Config>::DataDepositPerByte::get()
			);
		});
	}

	#[test]
	fn register_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			let para_id = LOWEST_PUBLIC_ID;

			assert_noop!(
				Registrar::register(
					RuntimeOrigin::signed(1),
					para_id,
					test_genesis_head(max_head_size() as usize),
					test_validation_code(max_code_size() as usize),
				),
				Error::<Test>::NotReserved
			);

			// Successfully register para
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));

			assert_noop!(
				Registrar::register(
					RuntimeOrigin::signed(2),
					para_id,
					test_genesis_head(max_head_size() as usize),
					test_validation_code(max_code_size() as usize),
				),
				Error::<Test>::NotOwner
			);

			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(1),
				para_id,
				test_genesis_head(max_head_size() as usize),
				test_validation_code(max_code_size() as usize),
			));

			run_to_session(2);

			assert_ok!(Registrar::deregister(RuntimeOrigin::root(), para_id));

			// Can't do it again
			assert_noop!(
				Registrar::register(
					RuntimeOrigin::signed(1),
					para_id,
					test_genesis_head(max_head_size() as usize),
					test_validation_code(max_code_size() as usize),
				),
				Error::<Test>::NotReserved
			);

			// Head Size Check
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(2)));
			assert_noop!(
				Registrar::register(
					RuntimeOrigin::signed(2),
					para_id + 1,
					test_genesis_head((max_head_size() + 1) as usize),
					test_validation_code(max_code_size() as usize),
				),
				Error::<Test>::HeadDataTooLarge
			);

			// Code Size Check
			assert_noop!(
				Registrar::register(
					RuntimeOrigin::signed(2),
					para_id + 1,
					test_genesis_head(max_head_size() as usize),
					test_validation_code((max_code_size() + 1) as usize),
				),
				Error::<Test>::CodeTooLarge
			);

			// Needs enough funds for deposit
			assert_noop!(
				Registrar::reserve(RuntimeOrigin::signed(1337)),
				BalancesError::<Test, _>::InsufficientBalance
			);
		});
	}

	#[test]
	fn deregister_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			let para_id = LOWEST_PUBLIC_ID;
			assert!(!Parachains::is_parathread(para_id));
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(1),
				para_id,
				test_genesis_head(32),
				test_validation_code(32),
			));
			run_to_session(2);
			assert!(Parachains::is_parathread(para_id));
			assert_ok!(Registrar::deregister(RuntimeOrigin::root(), para_id,));
			run_to_session(4);
			assert!(paras::Pallet::<Test>::lifecycle(para_id).is_none());
			assert_eq!(Balances::reserved_balance(&1), 0);
		});
	}

	#[test]
	fn deregister_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			let para_id = LOWEST_PUBLIC_ID;
			assert!(!Parachains::is_parathread(para_id));
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(1),
				para_id,
				test_genesis_head(32),
				test_validation_code(32),
			));
			run_to_session(2);
			assert!(Parachains::is_parathread(para_id));
			// Owner check
			assert_noop!(Registrar::deregister(RuntimeOrigin::signed(2), para_id,), BadOrigin);
			assert_ok!(Registrar::make_parachain(para_id));
			run_to_session(4);
			// Cant directly deregister parachain
			assert_noop!(
				Registrar::deregister(RuntimeOrigin::root(), para_id,),
				Error::<Test>::NotParathread
			);
		});
	}

	#[test]
	fn swap_works() {
		new_test_ext().execute_with(|| {
			// Successfully register first two parachains
			let para_1 = LOWEST_PUBLIC_ID;
			let para_2 = LOWEST_PUBLIC_ID + 1;
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(1),
				para_1,
				test_genesis_head(max_head_size() as usize),
				test_validation_code(max_code_size() as usize),
			));
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(2)));
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(2),
				para_2,
				test_genesis_head(max_head_size() as usize),
				test_validation_code(max_code_size() as usize),
			));
			run_to_session(2);

			// Upgrade para 1 into a parachain
			assert_ok!(Registrar::make_parachain(para_1));

			// Set some mock swap data.
			let mut swap_data = SwapData::get();
			swap_data.insert(para_1, 69);
			swap_data.insert(para_2, 1337);
			SwapData::set(swap_data);

			run_to_session(4);

			// Roles are as we expect
			assert!(Parachains::is_parachain(para_1));
			assert!(!Parachains::is_parathread(para_1));
			assert!(!Parachains::is_parachain(para_2));
			assert!(Parachains::is_parathread(para_2));

			// Both paras initiate a swap
			assert_ok!(Registrar::swap(para_origin(para_1), para_1, para_2,));
			assert_ok!(Registrar::swap(para_origin(para_2), para_2, para_1,));

			run_to_session(6);

			// Roles are swapped
			assert!(!Parachains::is_parachain(para_1));
			assert!(Parachains::is_parathread(para_1));
			assert!(Parachains::is_parachain(para_2));
			assert!(!Parachains::is_parathread(para_2));

			// Data is swapped
			assert_eq!(SwapData::get().get(&para_1).unwrap(), &1337);
			assert_eq!(SwapData::get().get(&para_2).unwrap(), &69);
		});
	}

	#[test]
	fn para_lock_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
			let para_id = LOWEST_PUBLIC_ID;
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(1),
				para_id,
				vec![1; 3].into(),
				vec![1, 2, 3].into(),
			));

			assert_noop!(Registrar::add_lock(RuntimeOrigin::signed(2), para_id), BadOrigin);
			// Once they begin onboarding, we lock them in.
			assert_ok!(Registrar::add_lock(RuntimeOrigin::signed(1), para_id));
			// Owner cannot pass origin check when checking lock
			assert_noop!(
				Registrar::ensure_root_para_or_owner(RuntimeOrigin::signed(1), para_id),
				BadOrigin
			);
			// Owner cannot remove lock.
			assert_noop!(Registrar::remove_lock(RuntimeOrigin::signed(1), para_id), BadOrigin);
			// Para can.
			assert_ok!(Registrar::remove_lock(para_origin(para_id), para_id));
			// Owner can pass origin check again
			assert_ok!(Registrar::ensure_root_para_or_owner(RuntimeOrigin::signed(1), para_id));
		});
	}

	#[test]
	fn swap_handles_bad_states() {
		new_test_ext().execute_with(|| {
			let para_1 = LOWEST_PUBLIC_ID;
			let para_2 = LOWEST_PUBLIC_ID + 1;
			run_to_block(1);
			// paras are not yet registered
			assert!(!Parachains::is_parathread(para_1));
			assert!(!Parachains::is_parathread(para_2));

			// Cannot even start a swap
			assert_noop!(
				Registrar::swap(RuntimeOrigin::root(), para_1, para_2),
				Error::<Test>::NotRegistered
			);

			// We register Paras 1 and 2
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(1)));
			assert_ok!(Registrar::reserve(RuntimeOrigin::signed(2)));
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(1),
				para_1,
				test_genesis_head(32),
				test_validation_code(32),
			));
			assert_ok!(Registrar::register(
				RuntimeOrigin::signed(2),
				para_2,
				test_genesis_head(32),
				test_validation_code(32),
			));

			// Cannot swap
			assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
			assert_noop!(
				Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
				Error::<Test>::CannotSwap
			);

			run_to_session(2);

			// They are now a parathread.
			assert!(Parachains::is_parathread(para_1));
			assert!(Parachains::is_parathread(para_2));

			// Cannot swap
			assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
			assert_noop!(
				Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
				Error::<Test>::CannotSwap
			);

			// Some other external process will elevate one parathread to parachain
			assert_ok!(Registrar::make_parachain(para_1));

			// Cannot swap
			assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
			assert_noop!(
				Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
				Error::<Test>::CannotSwap
			);

			run_to_session(3);

			// Cannot swap
			assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
			assert_noop!(
				Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
				Error::<Test>::CannotSwap
			);

			run_to_session(4);

			// It is now a parachain.
			assert!(Parachains::is_parachain(para_1));
			assert!(Parachains::is_parathread(para_2));

			// Swap works here.
			assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
			assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_2, para_1));

			run_to_session(5);

			// Cannot swap
			assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
			assert_noop!(
				Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
				Error::<Test>::CannotSwap
			);

			run_to_session(6);

			// Swap worked!
			assert!(Parachains::is_parachain(para_2));
			assert!(Parachains::is_parathread(para_1));

			// Something starts to downgrade a para
			assert_ok!(Registrar::make_parathread(para_2));

			run_to_session(7);

			// Cannot swap
			assert_ok!(Registrar::swap(RuntimeOrigin::root(), para_1, para_2));
			assert_noop!(
				Registrar::swap(RuntimeOrigin::root(), para_2, para_1),
				Error::<Test>::CannotSwap
			);

			run_to_session(8);

			assert!(Parachains::is_parathread(para_1));
			assert!(Parachains::is_parathread(para_2));
		});
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking {
	use super::{Pallet as Registrar, *};
	use crate::traits::Registrar as RegistrarT;
	use frame_support::assert_ok;
	use frame_system::RawOrigin;
	use primitives::{MAX_CODE_SIZE, MAX_HEAD_DATA_SIZE};
	use runtime_parachains::{paras, shared, Origin as ParaOrigin};
	use sp_runtime::traits::Bounded;

	use frame_benchmarking::{account, benchmarks, whitelisted_caller};

	fn assert_last_event<T: Config>(generic_event: <T as Config>::RuntimeEvent) {
		let events = frame_system::Pallet::<T>::events();
		let system_event: <T as frame_system::Config>::RuntimeEvent = generic_event.into();
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
		assert_ok!(Registrar::<T>::reserve(RawOrigin::Signed(caller.clone()).into()));
		assert_ok!(Registrar::<T>::register(
			RawOrigin::Signed(caller).into(),
			para,
			genesis_head,
			validation_code
		));
		return para
	}

	fn para_origin(id: u32) -> ParaOrigin {
		ParaOrigin::Parachain(id.into())
	}

	// This function moves forward to the next scheduled session for parachain lifecycle upgrades.
	fn next_scheduled_session<T: Config>() {
		shared::Pallet::<T>::set_session_index(shared::Pallet::<T>::scheduled_session());
		paras::Pallet::<T>::test_on_new_session();
	}

	benchmarks! {
		where_clause { where ParaOrigin: Into<<T as frame_system::Config>::RuntimeOrigin> }

		reserve {
			let caller: T::AccountId = whitelisted_caller();
			T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		}: _(RawOrigin::Signed(caller.clone()))
		verify {
			assert_last_event::<T>(Event::<T>::Reserved { para_id: LOWEST_PUBLIC_ID, who: caller }.into());
			assert!(Paras::<T>::get(LOWEST_PUBLIC_ID).is_some());
			assert_eq!(paras::Pallet::<T>::lifecycle(LOWEST_PUBLIC_ID), None);
		}

		register {
			let para = LOWEST_PUBLIC_ID;
			let genesis_head = Registrar::<T>::worst_head_data();
			let validation_code = Registrar::<T>::worst_validation_code();
			let caller: T::AccountId = whitelisted_caller();
			T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
			assert_ok!(Registrar::<T>::reserve(RawOrigin::Signed(caller.clone()).into()));
		}: _(RawOrigin::Signed(caller.clone()), para, genesis_head, validation_code)
		verify {
			assert_last_event::<T>(Event::<T>::Registered{ para_id: para, manager: caller }.into());
			assert_eq!(paras::Pallet::<T>::lifecycle(para), Some(ParaLifecycle::Onboarding));
			next_scheduled_session::<T>();
			assert_eq!(paras::Pallet::<T>::lifecycle(para), Some(ParaLifecycle::Parathread));
		}

		force_register {
			let manager: T::AccountId = account("manager", 0, 0);
			let deposit = 0u32.into();
			let para = ParaId::from(69);
			let genesis_head = Registrar::<T>::worst_head_data();
			let validation_code = Registrar::<T>::worst_validation_code();
		}: _(RawOrigin::Root, manager.clone(), deposit, para, genesis_head, validation_code)
		verify {
			assert_last_event::<T>(Event::<T>::Registered { para_id: para, manager }.into());
			assert_eq!(paras::Pallet::<T>::lifecycle(para), Some(ParaLifecycle::Onboarding));
			next_scheduled_session::<T>();
			assert_eq!(paras::Pallet::<T>::lifecycle(para), Some(ParaLifecycle::Parathread));
		}

		deregister {
			let para = register_para::<T>(LOWEST_PUBLIC_ID.into());
			next_scheduled_session::<T>();
			let caller: T::AccountId = whitelisted_caller();
		}: _(RawOrigin::Signed(caller), para)
		verify {
			assert_last_event::<T>(Event::<T>::Deregistered { para_id: para }.into());
		}

		swap {
			let parathread = register_para::<T>(LOWEST_PUBLIC_ID.into());
			let parachain = register_para::<T>((LOWEST_PUBLIC_ID + 1).into());

			let parachain_origin = para_origin(parachain.into());

			// Actually finish registration process
			next_scheduled_session::<T>();

			// Upgrade the parachain
			Registrar::<T>::make_parachain(parachain)?;
			next_scheduled_session::<T>();

			assert_eq!(paras::Pallet::<T>::lifecycle(parachain), Some(ParaLifecycle::Parachain));
			assert_eq!(paras::Pallet::<T>::lifecycle(parathread), Some(ParaLifecycle::Parathread));

			let caller: T::AccountId = whitelisted_caller();
			Registrar::<T>::swap(parachain_origin.into(), parachain, parathread)?;
		}: _(RawOrigin::Signed(caller.clone()), parathread, parachain)
		verify {
			next_scheduled_session::<T>();
			// Swapped!
			assert_eq!(paras::Pallet::<T>::lifecycle(parachain), Some(ParaLifecycle::Parathread));
			assert_eq!(paras::Pallet::<T>::lifecycle(parathread), Some(ParaLifecycle::Parachain));
		}

		schedule_code_upgrade {
			let b in 1 .. MAX_CODE_SIZE;
			let new_code = ValidationCode(vec![0; b as usize]);
			let para_id = ParaId::from(1000);
		}: _(RawOrigin::Root, para_id, new_code)

		set_current_head {
			let b in 1 .. MAX_HEAD_DATA_SIZE;
			let new_head = HeadData(vec![0; b as usize]);
			let para_id = ParaId::from(1000);
		}: _(RawOrigin::Root, para_id, new_head)

		impl_benchmark_test_suite!(
			Registrar,
			crate::integration_tests::new_test_ext(),
			crate::integration_tests::Test,
		);
	}
}
