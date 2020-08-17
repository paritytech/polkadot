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

use frame_support::{
	decl_event, decl_error, decl_module, traits::{Get, ReservableCurrency, EnsureOrigin, Currency},
	decl_storage, ensure, IterableStorageMap,
};
use primitives::v0::{Id as ParaId, Info as ParaInfo, Scheduling, HeadData, ValidationCode};
use polkadot_parachain::primitives::AccountIdConversion;
use system::{ensure_signed, ensure_root, EnsureOneOf, EnsureRoot, EnsureSigned};
use sp_runtime::{Either, traits::BadOrigin};
use sp_staking::SessionIndex;
use sp_std::vec::Vec;
use runtime_common::registrar::Registrar;

type EnsureRootOrSigned<AccountId> = EnsureOneOf<AccountId, EnsureRoot<AccountId>, EnsureSigned<AccountId>>;

type Session<T> = session::Module<T>;

type BalanceOf<T> = <<T as runtime_common::registrar::Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

/// Configuration for the parachain proposer.
pub trait Trait: session::Trait + runtime_common::parachains::Trait + runtime_common::registrar::Trait {
	/// The overreaching event type.
	type Event: From<Event> + Into<<Self as system::Trait>::Event>;

	/// The maximum name length of a parachain.
	type MaxNameLength: Get<u32>;

	/// The amount that should be deposited when creating a proposal.
	type ProposeDeposit: Get<BalanceOf<Self>>;
}

/// A proposal for adding a parachain to the relay chain.
#[derive(codec::Encode, codec::Decode)]
struct Proposal<AccountId, ValidatorId, Balance> {
	/// The account that proposed this account.
	proposer: AccountId,
	/// The validation WASM code of the parachain.
	validation_function: Vec<u8>,
	/// The initial head state of the parachain.
	initial_head_state: HeadData,
	/// The validators for the relay chain provided by the parachain.
	validators: Vec<ValidatorId>,
	/// The name of the parachain.
	name: Vec<u8>,
	/// The balance that the parachain should receive.
	balance: Balance,
}

decl_event! {
	pub enum Event {
		/// A parachain was proposed for registration.
		ParachainProposed(Vec<u8>, ParaId),
		/// A parachain was approved and is scheduled for being activated.
		ParachainApproved(ParaId),
	}
}

decl_error! {
	pub enum Error for Module<T: Trait> {
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
	}
}

decl_storage! {
	trait Store for Module<T: Trait> as ParachainProposer {
		/// All the proposals.
		Proposals: map hasher(twox_64_concat) ParaId => Option<Proposal<T::AccountId, T::ValidatorId, BalanceOf<T>>>;
		/// Proposals that are approved.
		ApprovedProposals: Vec<ParaId>;
		/// Proposals that are scheduled at for a fixed session to be applied.
		ScheduledProposals: map hasher(twox_64_concat) SessionIndex => Vec<ParaId>;
		/// The validators that were registered for a parachain.
		ParachainValidators: map hasher(twox_64_concat) ParaId => Vec<T::ValidatorId>;
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin, system = system {
		type Error = Error<T>;

		/// The maximum name length of a parachain.
		const MaxNameLength: u32 = T::MaxNameLength::get();

		fn deposit_event() = default;

		#[weight = T::MaximumBlockWeight::get()]
		fn propose_parachain(
			origin,
			para_id: ParaId,
			name: Vec<u8>,
			validation_function: Vec<u8>,
			initial_head_state: HeadData,
			validators: [T::ValidatorId; 2],
			balance: BalanceOf<T>,
		) {
			let who = ensure_signed(origin)?;

			ensure!(name.len() < T::MaxNameLength::get() as usize, Error::<T>::NameTooLong);
			ensure!(!Proposals::<T>::contains_key(&para_id), Error::<T>::ParachainIdAlreadyProposed);
			ensure!(!runtime_common::parachains::Code::contains_key(&para_id), Error::<T>::ParachainIdAlreadyTaken);

			T::Currency::reserve(&who, T::ProposeDeposit::get())?;

			let active_validators = Session::<T>::validators();
			ensure!(
				active_validators.iter().all(|v| *v != validators[0] && *v != validators[1]),
				Error::<T>::ValidatorAlreadyRegistered,
			);
			Proposals::<T>::iter().try_for_each(|(_, prop)|
				if prop.validators.iter().all(|v| *v != validators[0] && *v != validators[1]) {
					Ok(())
				} else {
					Err(Error::<T>::ValidatorAlreadyRegistered)
				}
			)?;

			let proposal = Proposal {
				name: name.clone(),
				proposer: who,
				validators: validators.into(),
				initial_head_state,
				validation_function,
				balance,
			};

			Proposals::<T>::insert(para_id, proposal);

			Self::deposit_event(Event::ParachainProposed(name, para_id));
		}

		#[weight = 100_000]
		fn approve_parachain(
			origin,
			para_id: ParaId,
		) {
			ensure_root(origin)?;

			ApprovedProposals::append(para_id);

			Self::deposit_event(Event::ParachainApproved(para_id));
		}

		#[weight = 100_000]
		fn cancel_proposal(
			origin,
			para_id: ParaId,
		) {
			let who = match EnsureRootOrSigned::try_origin(origin).map_err(|_| BadOrigin)? {
				Either::Left(()) => None,
				Either::Right(who) => Some(who),
			};

			let proposal = Proposals::get(&para_id).ok_or(Error::<T>::ProposalNotFound)?;

			if let Some(who) = who {
				ensure!(who == proposal.proposer, Error::<T>::NotAuthorized);
			}

			Proposals::remove(&para_id);

			T::Currency::unreserve(&proposal.proposer, T::ProposeDeposit::get());
		}
	}
}

impl<T: Trait> session::SessionManager<T::ValidatorId> for Module<T> {
	fn new_session(new_index: SessionIndex) -> Option<Vec<T::ValidatorId>> {
		let proposals = ApprovedProposals::take();

		let mut validators = Session::<T>::validators();

		// Schedule all approved proposals
		for (id, proposal) in proposals.iter().filter_map(|id| Proposals::<T>::get(&id).map(|p| (id, p))) {
			ScheduledProposals::append(new_index, id);
			validators.extend(proposal.validators);
		}

		Some(validators)
	}

	fn end_session(_: SessionIndex) {}

	fn start_session(start_index: SessionIndex) {
		let proposals = ScheduledProposals::take(&start_index);

		// Register all parachains that are allowed to start with the new session.
		for (id, proposal) in proposals.iter().filter_map(|id| Proposals::<T>::take(&id).map(|p| (id, p))) {
			let info = ParaInfo {
				scheduling: Scheduling::Always,
			};

			// Ignore errors for now
			let _ = T::Registrar::register_para(
				*id,
				info,
				proposal.validation_function,
				proposal.initial_head_state,
			);

			// Add some funds to the Parachain
			let _ = T::Currency::deposit_creating(&id.into_account(), proposal.balance);

			ParachainValidators::insert(id, proposal.validators);
		}
	}
}
