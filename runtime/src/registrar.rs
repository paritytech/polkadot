// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! Main parachains logic. For now this is just the determination of which validators do what.

use rstd::prelude::*;
#[cfg(any(feature = "std", test))]
use rstd::marker::PhantomData;
use parity_codec::{Encode, Decode};
#[cfg(any(feature = "std", test))]
use sr_primitives::{StorageOverlay, ChildrenStorageOverlay};
use sr_primitives::{
	weights::{SimpleDispatchInfo, DispatchInfo}, transaction_validity::ValidTransaction,
	traits::{Hash as HashT, StaticLookup, DispatchError, SignedExtension}
};
#[cfg(feature = "std")]
use srml_support::storage::hashed::generator;
use srml_support::{
	decl_storage, decl_module, ensure,
	StorageValue, StorageMap, Dispatchable, dispatch::Result,
	traits::{Get, Currency}
};
use system::{self, ensure_root, ensure_signed};
use primitives::parachain::{
	Id as ParaId, CollatorId, Scheduling, LOWEST_USER_ID, OnSwap, Info as ParaInfo, ActiveParas
};
use crate::parachains;

/// Parachain registration API.
pub trait Registrar<AccountId> {
	/// Create a new unique parachain identity for later registration.
	fn new_id() -> ParaId;

	/// Register a parachain with given `code` and `initial_head_data`. `id` must not yet be registered or it will
	/// result in a error.
	fn register_para(
		id: ParaId,
		info: ParaInfo,
		code: Vec<u8>,
		initial_head_data: Vec<u8>
	) -> Result;

	/// Deregister a parachain with given `id`. If `id` is not currently registered, an error is returned.
	fn deregister_para(id: ParaId) -> Result;
}

impl<T: Trait> Registrar<T::AccountId> for Module<T> {
	fn new_id() -> ParaId {
		<NextFreeId>::mutate(|n| { let r = *n; *n = ParaId::from(u32::from(*n) + 1); r })
	}

	fn register_para(
		id: ParaId,
		info: ParaInfo,
		code: Vec<u8>,
		initial_head_data: Vec<u8>
	) -> Result {
		if let Scheduling::Always = info.scheduling {
			Parachains.mutate(|parachains|
				match parachains.binary_search(&id) {
					Ok(_) => Err("Parachain already exists"),
					Err(idx) => {
						parachains.insert(idx, id);
						Ok(())
					}
				}
			)?;
		}
		<parachains::Module<T>>::initialize_para(id, code, initial_head_data);
		Ok(())
	}

	fn deregister_para(id: ParaId) -> Result {
		let info = Paras::take(id).ok_or("Invalid id")?;
		if let Scheduling::Always = info.scheduling {
			Parachains.mutate(|parachains|
				parachains.binary_search(&id)
					.map(|index| parachains.remove(index))
					.map_err(|_| "Invalid id")
			)?;
		}
		<parachains::Module<T>>::cleanup_para(id);
		Ok(())
	}
}

type BalanceOf<T> =
	<<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;
type NegativeImbalanceOf<T> =
	<<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::NegativeImbalance;

pub trait Trait: parachains::Trait {
	/// The overarching event type.
	type Event: From<Event> + Into<<Self as system::Trait>::Event>;

	/// The system's currency for parathread payment.
	type Currency: Currency<Self::AccountId>;

	/// The deposit to be paid to run a parathread.
	type ParathreadDeposit: Get<BalanceOf<Self>>;

	/// Handler for when two ParaIds are swapped.
	type OnSwap: OnSwap;
}

decl_storage! {
	trait Store for Module<T: Trait> as Registrar {
		// Ordered vector of all parachain IDs.
		Parachains: Vec<ParaId>;

		/// The number of threads to schedule per block.
		ThreadCount: u32;

		/// Current set of threads the next block; ordered by para ID. There can be no duplicates
		/// of para ID in the list.
		SelectedThreads: Vec<(ParaId, CollatorId)>;

		/// Parathreads/chains scheduled for execution this block. If the collator ID is set, then
		/// a particular collator has already been chosen for the next block, and no other collator
		/// may provide the block.
		///
		/// Ordered as Parachains ++ Selected_Parathreads.
		Active: Vec<(ParaId, Option<CollatorId>)>;

		/// The next unused ParaId value. Start this high in order to keep low numbers for
		/// system-level chains.
		NextFreeId: ParaId = LOWEST_USER_ID;

		/// Pending swap operations.
		PendingSwap: map ParaId => ParaId;

		// Map of all registered parathreads/chains.
		Paras get(paras): map ParaId => Option<ParaInfo>;
	}
	add_extra_genesis {
		config(parachains): Vec<(ParaId, Vec<u8>, Vec<u8>)>;
		config(_phdata): PhantomData<T>;
		build(|storage: &mut StorageOverlay, _: &mut ChildrenStorageOverlay, config: &GenesisConfig<T>| {
			use sr_primitives::traits::Zero;

			let mut p = config.parachains.clone();
			p.sort_unstable_by_key(|&(ref id, _, _)| *id);
			p.dedup_by_key(|&mut (ref id, _, _)| *id);

			let only_ids: Vec<_> = p.iter().map(|&(ref id, _, _)| id).cloned().collect();

			<Parachains as generator::StorageValue<_>>::put(&only_ids, storage);

			for (id, code, genesis) in p {
				// no ingress -- a chain cannot be routed to until it is live.
				<parachains::Code as generator::StorageMap<_, _>>
					::insert(&id, &code, storage);
				<parachains::Heads as generator::StorageMap<_, _>>
					::insert(&id, &genesis, storage);
				<parachains::Watermarks<T> as generator::StorageMap<_, _>>
					::insert(&id, &Zero::zero(), storage);
			}
		});
	}
}

decl_module! {
	/// Parachains module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		/// Register a parachain with given code.
		/// Fails if given ID is already used.
		#[weight = SimpleDispatchInfo::FixedOperational(5_000_000)]
		pub fn register_para(origin,
			#[compact] id: ParaId,
			code: Vec<u8>,
			initial_head_data: Vec<u8>,
			info: ParaInfo
		) -> Result {
			ensure_root(origin)?;
			<Self as Registrar<T::AccountId>>::
				register_para(id, info, code, initial_head_data)
		}

		/// Deregister a parachain with given id
		#[weight = SimpleDispatchInfo::FixedOperational(10_000)]
		pub fn deregister_para(origin, #[compact] id: ParaId) -> Result {
			ensure_root(origin)?;
			<Self as Registrar<T::AccountId>>::deregister_para(id)
		}

		/// Register a parathread for immediate use.
		///
		/// Must be sent from a Signed origin that is able to have ParathreadDeposit reserved.
		/// `code` and `initial_head_data` are used to initialize the parathread's state.
		fn register_parathead(origin,
			code: Vec<u8>,
			initial_head_data: Vec<u8>
		) {
			let who = ensure_signed(origin)?;

			T::Currency::reserve(&who, T::ParathreadDeposit::get())?;

			let info = ParaInfo {
				scheduling: Scheduling::Dynamic,
			};
			let id = <Self as Registrar<T::AccountId>>::new_id();

			<Self as Registrar<T::AccountId>>::
				register_para(id, info, code, initial_head_data)
		}

		/// Place a bid for a parathread to be progressed in the next block.
		///
		/// This is a kind of special transaction that should by heavily prioritised in the
		/// transaction pool according to the `value`; only `ThreadCount` of them may be presented
		/// in any single block.
		fn select_parathread(origin,
			#[compact] id: ParaId,
			collator: CollatorId,
			head_data_hash: T::Hash
		) {
			ensure_signed(origin)?;
			// Everything else is checked for in the transaction `SignedExtension`.
		}

		/// Unregister a parathread and retrieve the deposit.
		///
		/// Must be sent from a `Parachain` origin which is currently a parathread.
		///
		/// Ensure that before calling this that any funds you want emptied from the parathread's
		/// account is moved out; after this it will be impossible to retrieve them (without
		/// governance intervention).
		fn deregister_parathread(origin,
			debtor: <T::Lookup as StaticLookup>::Source,
			#[compact] required_transfer: BalanceOf<T>
		) {
			let id = crate::parachains::ensure_parachain(origin)?;
			let debtor = T::Lookup::lookup(debtor)?;

			let info = Paras::get(id).ok_or("invalid id")?;
			if let Scheduling::Dynamic = info.scheduling {} else { Err("invalid parathread id")? }

			let _ = T::Currency::unreserve(&debtor, T::ParathreadDeposit::get());
			<Self as Registrar<T::AccountId>>::deregister_para(id);
		}

		/// Swap a parachain with another parachain or parathread. The origin must be a `Parachain`.
		/// The swap will happen only if there is already an opposite swap. If there is not, the
		/// swap will be stored in the pending swaps map, ready for a later confirmatory swap.
		///
		/// The `ParaId`s remain mapped to the same head data and code so external code can rely on
		/// `ParaId` to be a long-term identifier of a notional "parachain". However, their
		/// scheduling info (i.e. whether they're a parathread or parachain), auction information
		/// and the auction deposit are switched.
		fn swap(origin, #[compact] other: ParaId) {
			let id = crate::parachains::ensure_parachain(origin)?;

			let info = Paras::get(id).ok_or("invalid id")?;

			if PendingSwap::get(other) == Some(id) {
				// actually do the swap.
				T::OnSwap::can_swap(id, other)?;
				Paras::mutate(id, |i|
					Paras::mutate(other, |j|
						rstd::mem::swap(i, j)
					)
				);
				let _ = T::OnSwap::do_swap(id, other);
			} else {
				PendingSwap::insert(id, other);
			}
		}

		/// Block initializer. Clears SelectedThreads and constructs/replaces Active.
		fn on_initialize() {
			let paras = Parachains::get().into_iter()
				.map(|id| (id, None))
				.chain(SelectedThreads::take().into_iter()
					.map(|(who, id)| (who, Some(id))
				))
				.collect::<Vec<_>>();
			Active::put(paras);
		}
	}
}

pub enum Event {
	/// A parathread was registered; its new ID is supplied.
	ParathreadRegistered(ParaId),

	/// The parathread of the supplied ID was de-registered.
	ParathreadDeregistered(ParaId),
}

impl<T: Trait> Module<T> {
	pub fn ensure_thread_id(id: ParaId) -> rstd::result::Result<ParaInfo, &'static str> {
		let info = Paras::get(id).ok_or("invalid id")?;
		if let Scheduling::Dynamic = info.scheduling {} else { Err("invalid parathread id")? }
		Ok(info)
	}
}

impl<T: Trait> ActiveParas for Module<T> {
	fn active_paras() -> Vec<(ParaId, Option<CollatorId>)> {
		Active::get()
	}
}

use srml_support::dispatch::IsSubType;

/// Ensure that parathread selections happen prioritised by fees.
#[derive(Encode, Decode, Clone, Eq, PartialEq)]
pub struct LimitParathreadCommits<T: Trait + Send + Sync>(rstd::marker::PhantomData<T>) where
	<T as system::Trait>::Call: IsSubType<Module<T>, T>;

#[cfg(feature = "std")]
impl<T: Trait + Send + Sync> rstd::fmt::Debug for LimitParathreadCommits<T> where
	<T as system::Trait>::Call: IsSubType<Module<T>, T>
{
	fn fmt(&self, f: &mut rstd::fmt::Formatter) -> rstd::fmt::Result {
		write!(f, "LimitParathreadCommits<T>")
	}
}

impl<T: Trait + Send + Sync> SignedExtension for LimitParathreadCommits<T> where
	<T as system::Trait>::Call: IsSubType<Module<T>, T>
{
	type AccountId = T::AccountId;
	type Call = <T as system::Trait>::Call;
	type AdditionalSigned = ();

	fn validate(
		&self,
		call: &Self::Call,
		who: &Self::AccountId,
		info: DispatchInfo,
		len: usize,
	) -> rstd::result::Result<ValidTransaction, DispatchError> {
		if let Some(local_call) = call.is_aux_sub_type() {
			if let Call::select_parathread(id, collator, hash) = local_call {
				// ensure that the para ID is actually a parathread.
				Self::ensure_thread_id(id)?;

				// ensure that we haven't already had a full complement of selected parathreads.
				let mut selected_threads = <SelectedThreads<T>>::get();
				ensure!(selected_threads.len() < ThreadCount::get(), DispatchError::Overflow);

				// ensure that this is not selecting a duplicate parathread ID
				let pos = selected_threads
					.binary_search_by(|&(ref other_id, _)| other_id == id)
					.err()
					.ok_or(DispatchError::BadState)?;

				// ensure that this is a live bid (i.e. that the thread's chain head matches)
				let actual = T::Hashing::hash(&<parachains::Module<T>>::parachain_head(id));
				ensure!(actual == hash, DispatchError::Stale);

				// updated the selected threads.
				selected_threads.insert(pos, (id, collator));
				<SelectedThreads<T>>::put(selected_threads);
			}
		}
		Ok(Default::default())
	}
}
