// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! A pallet for proposing a parachain for Rococo.
//!
//! This pallet works as registration of parachains for Rococo. The idea is to have
//! the registration of community provides parachains being handled by this pallet.
//! People will be able to propose their parachain for registration. This proposal
//! will need to be improved by some priviledged account. After approval the workflow
//! is the following:
//!
//! 1. On start of the next session the pallet announces the new relay chain validators.
//!
//! 2. The session after announcing the new relay chain validators, they will be active. At the
//!    switch to this session, the parachain will be registered and is allowed to produce blocks.
//!
//! When deregistering a parachain, we basically reverse the operations.

use frame_support::{
	decl_event, decl_error, decl_module, traits::{Get, ReservableCurrency, EnsureOrigin, Currency},
	decl_storage, ensure, IterableStorageMap,
};
use primitives::v1::{Id as ParaId, HeadData, ValidationCode};
use polkadot_parachain::primitives::AccountIdConversion;
use frame_system::{ensure_signed, EnsureOneOf, EnsureSigned};
use sp_runtime::Either;
use sp_staking::SessionIndex;
use sp_std::vec::Vec;
use runtime_parachains::paras::ParaGenesisArgs;

type EnsurePriviledgedOrSigned<T> = EnsureOneOf<
	<T as frame_system::Config>::AccountId,
	<T as Config>::PriviledgedOrigin,
	EnsureSigned<<T as frame_system::Config>::AccountId>
>;

type Session<T> = pallet_session::Module<T>;

type BalanceOf<T> = <T as pallet_balances::Config>::Balance;

/// Configuration for the parachain proposer.
pub trait Config: pallet_session::Config
	+ pallet_balances::Config
	+ pallet_balances::Config
	+ runtime_parachains::paras::Config
	+ runtime_parachains::dmp::Config
	+ runtime_parachains::ump::Config
	+ runtime_parachains::hrmp::Config
{
	/// The overreaching event type.
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;

	/// The maximum name length of a parachain.
	type MaxNameLength: Get<u32>;

	/// The amount that should be deposited when creating a proposal.
	type ProposeDeposit: Get<BalanceOf<Self>>;

	/// Priviledged origin that can approve/cancel/deregister parachain and proposals.
	type PriviledgedOrigin: EnsureOrigin<<Self as frame_system::Config>::Origin>;
}

/// A proposal for adding a parachain to the relay chain.
#[derive(parity_scale_codec::Encode, parity_scale_codec::Decode)]
struct Proposal<AccountId, ValidatorId, Balance> {
	/// The account that proposed this parachain.
	proposer: AccountId,
	/// The genesis head state of the parachain.
	genesis_head: HeadData,
	/// The validators for the relay chain provided by the parachain.
	validators: Vec<ValidatorId>,
	/// The name of the parachain.
	name: Vec<u8>,
	/// The balance that the parachain should receive.
	balance: Balance,
}

/// Information about the registered parachain.
#[derive(parity_scale_codec::Encode, parity_scale_codec::Decode)]
struct RegisteredParachainInfo<AccountId, ValidatorId> {
	/// The validators for the relay chain provided by the parachain.
	validators: Vec<ValidatorId>,
	/// The account that proposed the parachain.
	proposer: AccountId,
}

decl_event! {
	pub enum Event<T> where ValidatorId = <T as pallet_session::Config>::ValidatorId {
		/// A parachain was proposed for registration.
		ParachainProposed(Vec<u8>, ParaId),
		/// A parachain was approved and is scheduled for being activated.
		ParachainApproved(ParaId),
		/// A parachain was registered and is now running.
		ParachainRegistered(ParaId),
		/// New validators were added to the set.
		ValidatorsRegistered(Vec<ValidatorId>),
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// The name of the parachain is too long.
		NameTooLong,
		/// The requested parachain id is already registered.
		ParachainIdAlreadyTaken,
		/// The requested parachain id is already proposed for another parachain.
		ParachainIdAlreadyProposed,
		/// Could not find the parachain proposal.
		ProposalNotFound,
		/// Not authorized to do a certain operation.
		NotAuthorized,
		/// A validator is already registered in the active validator set.
		ValidatorAlreadyRegistered,
		/// No information about the registered parachain found.
		ParachainInfoNotFound,
		/// Parachain is already approved for registration.
		ParachainAlreadyApproved,
		/// Parachain is already scheduled for registration.
		ParachainAlreadyScheduled,
		/// The given WASM blob is definitley not valid.
		DefinitelyNotWasm,
		/// Registration requires at least one validator.
		AtLeastOneValidatorRequired,
		/// Couldn't schedule parachain cleanup.
		CouldntCleanup,
	}
}

decl_storage! {
	trait Store for Module<T: Config> as ParachainProposer {
		/// All the proposals.
		Proposals: map hasher(twox_64_concat) ParaId => Option<Proposal<T::AccountId, T::ValidatorId, BalanceOf<T>>>;
		/// The validation WASM code of the parachain.
		ParachainValidationCode: map hasher(twox_64_concat) ParaId => Option<ValidationCode>;
		/// Proposals that are approved.
		ApprovedProposals: Vec<ParaId>;
		/// Proposals that are scheduled at for a fixed session to be applied.
		ScheduledProposals: map hasher(twox_64_concat) SessionIndex => Vec<ParaId>;
		/// Information about the registered parachains.
		ParachainInfo: map hasher(twox_64_concat) ParaId => Option<RegisteredParachainInfo<T::AccountId, T::ValidatorId>>;
		/// Validators that should be retired, because their Parachain was deregistered.
		ValidatorsToRetire: Vec<T::ValidatorId>;
		/// Validators that should be added.
		ValidatorsToAdd: Vec<T::ValidatorId>;
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;

		/// The maximum name length of a parachain.
		const MaxNameLength: u32 = T::MaxNameLength::get();

		/// The deposit that will be reserved when proposing a parachain.
		const ProposeDeposit: BalanceOf<T> = T::ProposeDeposit::get();

		fn deposit_event() = default;

		/// Propose a new parachain
		///
		/// This requires:
		/// - `para_id`: The id of the parachain.
		/// - `name`: The name of the parachain.
		/// - `validation_function`: The wasm runtime of the parachain.
		/// - `initial_head_state`: The genesis state of the parachain.
		/// - `validators`: Validators that will validate for the relay chain, needs to be at least one.
		/// - `balance`: The initial balance of the parachain on the relay chain.
		///
		/// It will reserve a deposit from the sender account over the lifetime of the chain.
		#[weight = 1_000_000]
		fn propose_parachain(
			origin,
			para_id: ParaId,
			name: Vec<u8>,
			validation_code: ValidationCode,
			genesis_head: HeadData,
			validators: Vec<T::ValidatorId>,
			balance: BalanceOf<T>,
		) {
			let who = ensure_signed(origin)?;

			ensure!(name.len() <= T::MaxNameLength::get() as usize, Error::<T>::NameTooLong);
			ensure!(validators.len() > 0, Error::<T>::AtLeastOneValidatorRequired);
			ensure!(!Proposals::<T>::contains_key(&para_id), Error::<T>::ParachainIdAlreadyProposed);
			ensure!(
				runtime_parachains::paras::Module::<T>::lifecycle(para_id).is_none(),
				Error::<T>::ParachainIdAlreadyTaken,
			);
			ensure!(validation_code.0.starts_with(runtime_common::WASM_MAGIC), Error::<T>::DefinitelyNotWasm);

			let active_validators = Session::<T>::validators();
			let validators_to_retire = ValidatorsToRetire::<T>::get();
			ensure!(
				validators.iter().all(|v| !active_validators.contains(v) || validators_to_retire.contains(v)),
				Error::<T>::ValidatorAlreadyRegistered,
			);

			Proposals::<T>::iter().try_for_each(|(_, prop)|
				if validators.iter().all(|v| !prop.validators.contains(v)) {
					Ok(())
				} else {
					Err(Error::<T>::ValidatorAlreadyRegistered)
				}
			)?;

			pallet_balances::Module::<T>::reserve(&who, T::ProposeDeposit::get())?;

			let proposal = Proposal {
				name: name.clone(),
				proposer: who,
				validators: validators.into(),
				genesis_head,
				balance,
			};

			Proposals::<T>::insert(para_id, proposal);
			ParachainValidationCode::insert(para_id, validation_code);

			Self::deposit_event(RawEvent::ParachainProposed(name, para_id));
		}

		/// Approve a parachain proposal.
		#[weight = 100_000]
		fn approve_proposal(
			origin,
			para_id: ParaId,
		) {
			T::PriviledgedOrigin::ensure_origin(origin)?;

			ensure!(Proposals::<T>::contains_key(&para_id), Error::<T>::ProposalNotFound);

			Self::is_approved_or_scheduled(para_id)?;

			ApprovedProposals::append(para_id);

			Self::deposit_event(RawEvent::ParachainApproved(para_id));
		}

		/// Cancel a parachain proposal.
		///
		/// This also unreserves the deposit.
		#[weight = 100_000]
		fn cancel_proposal(origin, para_id: ParaId) {
			let who = match EnsurePriviledgedOrSigned::<T>::ensure_origin(origin)? {
				Either::Left(_) => None,
				Either::Right(who) => Some(who),
			};

			Self::is_approved_or_scheduled(para_id)?;

			let proposal = Proposals::<T>::get(&para_id).ok_or(Error::<T>::ProposalNotFound)?;

			if let Some(who) = who {
				ensure!(who == proposal.proposer, Error::<T>::NotAuthorized);
			}

			Proposals::<T>::remove(&para_id);
			ParachainValidationCode::remove(&para_id);

			pallet_balances::Module::<T>::unreserve(&proposal.proposer, T::ProposeDeposit::get());
		}

		/// Deregister a parachain that was already successfully registered in the relay chain.
		#[weight = 100_000]
		fn deregister_parachain(origin, para_id: ParaId) {
			let who = match EnsurePriviledgedOrSigned::<T>::ensure_origin(origin)? {
				Either::Left(_) => None,
				Either::Right(who) => Some(who),
			};

			let info = ParachainInfo::<T>::get(&para_id).ok_or(Error::<T>::ParachainInfoNotFound)?;

			if let Some(who) = who {
				ensure!(who == info.proposer, Error::<T>::NotAuthorized);
			}
			runtime_parachains::schedule_para_cleanup::<T>(para_id).map_err(|_| Error::<T>::CouldntCleanup)?;

			ParachainInfo::<T>::remove(&para_id);
			info.validators.into_iter().for_each(|v| ValidatorsToRetire::<T>::append(v));

			pallet_balances::Module::<T>::unreserve(&info.proposer, T::ProposeDeposit::get());
		}

		/// Add new validators to the set.
		#[weight = 100_000]
		fn register_validators(
			origin,
			validators: Vec<T::ValidatorId>,
		) {
			T::PriviledgedOrigin::ensure_origin(origin)?;

			validators.clone().into_iter().for_each(|v| ValidatorsToAdd::<T>::append(v));

			Self::deposit_event(RawEvent::ValidatorsRegistered(validators));
		}
	}
}

impl<T: Config> Module<T> {
	/// Returns wether the given `para_id` approval is approved or already scheduled.
	fn is_approved_or_scheduled(para_id: ParaId) -> frame_support::dispatch::DispatchResult {
		if ApprovedProposals::get().iter().any(|p| *p == para_id) {
			return Err(Error::<T>::ParachainAlreadyApproved.into())
		}

		if ScheduledProposals::get(&Session::<T>::current_index() + 1).iter().any(|p| *p == para_id) {
			return Err(Error::<T>::ParachainAlreadyScheduled.into())
		}

		Ok(())
	}
}

impl<T: Config> pallet_session::SessionManager<T::ValidatorId> for Module<T> {
	fn new_session(new_index: SessionIndex) -> Option<Vec<T::ValidatorId>> {
		if new_index <= 1 {
			return None;
		}

		let proposals = ApprovedProposals::take();

		let mut validators = Session::<T>::validators();

		ValidatorsToRetire::<T>::take().iter().for_each(|v| {
			if let Some(pos) = validators.iter().position(|r| r == v) {
				validators.swap_remove(pos);
			}
		});

		// Schedule all approved proposals
		for (id, proposal) in proposals.iter().filter_map(|id| Proposals::<T>::get(&id).map(|p| (id, p))) {
			ScheduledProposals::append(new_index, id);

			let validation_code = ParachainValidationCode::get(&id)?;
			let genesis = ParaGenesisArgs {
				genesis_head: proposal.genesis_head,
				validation_code,
				parachain: true,
			};

			// Not much we can do if this fails...
			let _ = runtime_parachains::schedule_para_initialize::<T>(*id, genesis);

			validators.extend(proposal.validators);
		}

		ValidatorsToAdd::<T>::take().into_iter().for_each(|v| {
			if !validators.contains(&v) {
				validators.push(v);
			}
		});

		Some(validators)
	}

	fn end_session(_: SessionIndex) {}

	fn start_session(start_index: SessionIndex) {
		let proposals = ScheduledProposals::take(&start_index);

		// Register all parachains that are allowed to start with the new session.
		for (id, proposal) in proposals.iter().filter_map(|id| Proposals::<T>::take(&id).map(|p| (id, p))) {
			Self::deposit_event(RawEvent::ParachainRegistered(*id));

			// Add some funds to the Parachain
			let _ = pallet_balances::Module::<T>::deposit_creating(&id.into_account(), proposal.balance);

			let info = RegisteredParachainInfo {
				proposer: proposal.proposer,
				validators: proposal.validators,
			};
			ParachainInfo::<T>::insert(id, info);
		}
	}
}

impl<T: Config> pallet_session::historical::SessionManager<T::ValidatorId, ()> for Module<T> {
	fn new_session(
		new_index: SessionIndex,
	) -> Option<Vec<(T::ValidatorId, ())>> {
		<Self as pallet_session::SessionManager<_>>::new_session(new_index)
			.map(|r| r.into_iter().map(|v| (v, Default::default())).collect())
	}

	fn start_session(start_index: SessionIndex) {
		<Self as pallet_session::SessionManager<_>>::start_session(start_index)
	}

	fn end_session(end_index: SessionIndex) {
		<Self as pallet_session::SessionManager<_>>::end_session(end_index)
	}
}
