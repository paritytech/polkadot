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

use rstd::{prelude::*, result};
#[cfg(any(feature = "std", test))]
use rstd::marker::PhantomData;
use sr_io::print;
use codec::{Encode, Decode};

use sr_primitives::{
	weights::{SimpleDispatchInfo, DispatchInfo},
	transaction_validity::{TransactionValidityError, ValidTransaction, TransactionValidity},
	traits::{Hash as HashT, StaticLookup, SignedExtension}
};

use srml_support::{
	decl_storage, decl_module, decl_event, ensure, StorageValue, StorageMap,
	dispatch::{Result, IsSubType}, traits::{Get, Currency, ReservableCurrency}
};
use system::{self, ensure_root, ensure_signed};
use primitives::parachain::{
	Id as ParaId, CollatorId, Scheduling, LOWEST_USER_ID, OnSwap, Info as ParaInfo, ActiveParas
};
use crate::parachains;
use sr_primitives::transaction_validity::InvalidTransaction;

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
		ensure!(!Paras::exists(id), "Parachain already exists");
		if let Scheduling::Always = info.scheduling {
			Parachains::mutate(|parachains|
				match parachains.binary_search(&id) {
					Ok(_) => Err("Parachain already exists"),
					Err(idx) => {
						parachains.insert(idx, id);
						Ok(())
					}
				}
			)?;
		}
		Paras::insert(id, info);
		<parachains::Module<T>>::initialize_para(id, code, initial_head_data);
		Ok(())
	}

	fn deregister_para(id: ParaId) -> Result {
		let info = Paras::take(id).ok_or("Invalid id")?;
		if let Scheduling::Always = info.scheduling {
			Parachains::mutate(|parachains|
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

pub trait Trait: parachains::Trait {
	/// The overarching event type.
	type Event: From<Event> + Into<<Self as system::Trait>::Event>;

	/// The aggregated origin type must support the parachains origin. We require that we can
	/// infallibly convert between this origin and the system origin, but in reality, they're the
	/// same type, we just can't express that to the Rust type system without writing a `where`
	/// clause everywhere.
	type Origin: From<<Self as system::Trait>::Origin>
		+ Into<result::Result<parachains::Origin, <Self as Trait>::Origin>>;

	/// The system's currency for parathread payment.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// The deposit to be paid to run a parathread.
	type ParathreadDeposit: Get<BalanceOf<Self>>;

	/// Handler for when two ParaIds are swapped.
	type OnSwap: OnSwap;
}

/// The number of items in the parathread queue, aka the number of blocks in advance to schedule
/// parachain execution.
const QUEUE_SIZE: usize = 2;

/// The number of rotations that you will have as grace if you miss a block.
const MAX_RETRIES: u32 = 3;

decl_storage! {
	trait Store for Module<T: Trait> as Registrar {
		// Ordered vector of all parachain IDs.
		Parachains: Vec<ParaId>;

		/// The number of threads to schedule per block.
		ThreadCount: u32;

		/// An array of the queue of set of threads scheduled for the coming blocks; ordered by
		/// para ID. There can be no duplicates of para ID in each list item.
		SelectedThreads: Vec<Vec<(ParaId, CollatorId)>>;

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
		PendingSwap: map ParaId => Option<ParaId>;

		/// Map of all registered parathreads/chains.
		Paras get(paras): map ParaId => Option<ParaInfo>;

		/// The current queue for parathreads that should be retried.
		RetryQueue get(retry_queue): [Vec<(ParaId, CollatorId)>; MAX_RETRIES as usize];

		/// Some if we are scheduling a retry in this block, along with the number of previous
		/// retries. None if not. If `Some` then the last para of `Active` will be the retried
		/// thread. If None, then it will be the last normally scheduled parathread (or the last
		/// parachain if there are no parathreads).
		Retrying: Option<u32>;
	}
	add_extra_genesis {
		config(parachains): Vec<(ParaId, Vec<u8>, Vec<u8>)>;
		config(_phdata): PhantomData<T>;
		build(build::<T>);
	}
}

#[cfg(feature = "std")]
fn build<T: Trait>(config: &GenesisConfig<T>) {
	let mut p = config.parachains.clone();
	p.sort_unstable_by_key(|&(ref id, _, _)| *id);
	p.dedup_by_key(|&mut (ref id, _, _)| *id);

	let only_ids: Vec<ParaId> = p.iter().map(|&(ref id, _, _)| id).cloned().collect();

	Parachains::put(&only_ids);

	for (id, code, genesis) in p {
		Paras::insert(id, &primitives::parachain::PARACHAIN_INFO);
		// no ingress -- a chain cannot be routed to until it is live.
		<parachains::Code>::insert(&id, &code);
		<parachains::Heads>::insert(&id, &genesis);
		<parachains::Watermarks<T>>::insert(&id, &sr_primitives::traits::Zero::zero());
		// Save initial parachains in registrar
		Paras::insert(id, ParaInfo { scheduling: Scheduling::Always })

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

		/// Reset the number of parathreads that can pay to be scheduled in a single block.
		///
		/// - `count`: The number of parathreads.
		///
		/// Must be called from Root origin.
		fn set_thread_count(origin, count: u32) {
			ensure_root(origin)?;
			ThreadCount::put(count);
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

			let _ = <Self as Registrar<T::AccountId>>::
				register_para(id, info, code, initial_head_data);
		}

		/// Place a bid for a parathread to be progressed in the next block.
		///
		/// This is a kind of special transaction that should by heavily prioritized in the
		/// transaction pool according to the `value`; only `ThreadCount` of them may be presented
		/// in any single block.
		fn select_parathread(origin,
			#[compact] _id: ParaId,
			_collator: CollatorId,
			_head_data_hash: T::Hash
		) {
			ensure_signed(origin)?;
			// Everything else is checked for in the transaction `SignedExtension`.
		}

		/// Deregister a parathread and retrieve the deposit.
		///
		/// Must be sent from a `Parachain` origin which is currently a parathread.
		///
		/// Ensure that before calling this that any funds you want emptied from the parathread's
		/// account is moved out; after this it will be impossible to retrieve them (without
		/// governance intervention).
		fn deregister_parathread(origin,
			debtor: <T::Lookup as StaticLookup>::Source
		) {
			let id = parachains::ensure_parachain(<T as Trait>::Origin::from(origin))?;
			let debtor = T::Lookup::lookup(debtor)?;

			let info = Paras::get(id).ok_or("invalid id")?;
			if let Scheduling::Dynamic = info.scheduling {} else { Err("invalid parathread id")? }

			<Self as Registrar<T::AccountId>>::deregister_para(id)?;

			let _ = T::Currency::unreserve(&debtor, T::ParathreadDeposit::get());
		}

		// TODO: swapping or removing a parathread should clear its para ID from all schedule queues

		/// Swap a parachain with another parachain or parathread. The origin must be a `Parachain`.
		/// The swap will happen only if there is already an opposite swap pending. If there is not,
		/// the swap will be stored in the pending swaps map, ready for a later confirmatory swap.
		///
		/// The `ParaId`s remain mapped to the same head data and code so external code can rely on
		/// `ParaId` to be a long-term identifier of a notional "parachain". However, their
		/// scheduling info (i.e. whether they're a parathread or parachain), auction information
		/// and the auction deposit are switched.
		fn swap(origin, #[compact] other: ParaId) {
			let id = parachains::ensure_parachain(<T as Trait>::Origin::from(origin))?;

			if PendingSwap::get(other) == Some(id) {
				// actually do the swap.
				T::OnSwap::can_swap(id, other)?;
				Paras::mutate(id, |i|
					Paras::mutate(other, |j|
						rstd::mem::swap(i, j)
					)
				);
				let _ = T::OnSwap::on_swap(id, other);
			} else {
				PendingSwap::insert(id, other);
			}
		}

		/// Block initializer. Clears SelectedThreads and constructs/replaces Active.
		fn on_initialize() {
			let next_up = SelectedThreads::mutate(|t| {
				let r = if t.len() >= QUEUE_SIZE {
					t.remove(0)
				} else {
					vec![]
				};
				if t.len() < QUEUE_SIZE {
					t.push(vec![]);
				}
				r
			});
			// mutable so that we can replace with `None` if parathread appears in new schedule.
			let mut retrying = Self::take_next_retry();
			if let Some(((para, _), _)) = retrying {
				// this isn't really ideal: better would be if there were an earlier pass that set
				// retrying to the first item in the Missed queue that isn't already scheduled, but
				// this is potentially O(m*n) in terms of missed queue size and parathread pool size.
				if next_up.iter().any(|x| x.0 == para) {
					retrying = None
				}
			}
			// We store these two for block finalisation to help work out which, if any, threads
			// missed their slot.
			if let Some(ref r) = retrying {
				Retrying::put(r.1);
			}

			let paras = Parachains::get().into_iter()
				.map(|id| (id, None))
				.chain(next_up.into_iter().chain(retrying.into_iter().map(|x| x.0))
					// collator needs wrapping to be merged into Active.
					.map(|(para, collator)| (para, Some(collator)))
				).collect::<Vec<_>>();
			println!("Active paras: {:?}/{:?}", Parachains::get(), paras);
			Active::put(paras);
		}

		fn on_finalize() {
			let retrying = Retrying::take();
			// a block without this will panic, but let's not panic here.
			if let Some(proceeded_vec) = parachains::DidUpdate::get() {
				// We can't remove the non-parathread items as with Active, so we reverse everything
				// so that we safely ignore permanent chains (which are at the beginning of
				// `DidUpdate`).
				let mut proceeded = proceeded_vec.into_iter().rev();
				let mut i = proceeded.next();

				let mut active = Active::get().into_iter()
					.rev()
					// Skip any permanent parachains (i.e. without collators).
					.take_while(|i| i.1.is_some())
					.map(|i| (i.0, i.1.expect("previous line guarantees this is_some(); qed")));

				// Check the parathread that we are retrying, if any.
				if let Some(retries) = retrying {
					// There was a retry. `active.next()` is guaranteed to be the retried para.
					if let Some(sched) = active.next() {
						match i.clone() {
							// If it got a block in, move onto next item.
							Some(para) if para == sched.0 => i = proceeded.next(),
							// If it missed its slot, then queue for retry.
							_ => Self::retry_later(sched, retries + 1),
						}
					} else {
						print("UNREACHABLE! No active chain when retry exists");
					}
				}

				// Check all normally scheduled parathreads: the rest of the `active` iterator are
				// just normally scheduled parathreads (albeit in reverse order).
				for sched in active {
					match i {
						// Scheduled parachain proceeded properly. Move onto next item.
						Some(para) if para == sched.0 => i = proceeded.next(),
						// Scheduled parachain missed their block. Queue for retry.
						_ => Self::retry_later(sched, 0),
					}
				}
			}
		}
	}
}

decl_event!{
	pub enum Event {
		/// A parathread was registered; its new ID is supplied.
		ParathreadRegistered(ParaId),

		/// The parathread of the supplied ID was de-registered.
		ParathreadDeregistered(ParaId),
	}
}

impl<T: Trait> Module<T> {
	pub fn ensure_thread_id(id: ParaId) -> Option<ParaInfo> {
		Paras::get(id).and_then(|info| if let Scheduling::Dynamic = info.scheduling {
			Some(info)
		} else {
			None
		})
	}

	fn retry_later(sched: (ParaId, CollatorId), retries: u32) {
		if retries < MAX_RETRIES {
			RetryQueue::mutate(|q| q[retries as usize].push(sched));
		}
	}

	fn take_next_retry() -> Option<((ParaId, CollatorId), u32)> {
		RetryQueue::mutate(|q| {
			for (i, q) in q.iter_mut().enumerate() {
				if !q.is_empty() {
					return Some((q.remove(0), i as u32));
				}
			}
			None
		})
	}
}

impl<T: Trait> ActiveParas for Module<T> {
	fn active_paras() -> Vec<(ParaId, Option<CollatorId>)> {
		Active::get()
	}
}

/// Ensure that parathread selections happen prioritized by fees.
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

/// Custom validity errors used in Polkadot while validating transactions.
#[repr(u8)]
pub enum Error {
	/// Parathread ID has already been submitted for this block.
	Duplicate = 0,
	/// Parathread ID does not identify a parathread.
	InvalidId = 1,
}

impl<T: Trait + Send + Sync> SignedExtension for LimitParathreadCommits<T> where
	<T as system::Trait>::Call: IsSubType<Module<T>, T>
{
	type AccountId = T::AccountId;
	type Call = <T as system::Trait>::Call;
	type AdditionalSigned = ();
	type Pre = ();

	fn additional_signed(&self)
		-> rstd::result::Result<Self::AdditionalSigned, TransactionValidityError>
	{
		Ok(())
	}

	fn validate(
		&self,
		_who: &Self::AccountId,
		call: &Self::Call,
		_info: DispatchInfo,
		_len: usize,
	) -> TransactionValidity {
		let mut r = ValidTransaction::default();
		if let Some(local_call) = call.is_sub_type() {
			if let Call::select_parathread(id, collator, hash) = local_call {
				// ensure that the para ID is actually a parathread.
				let e = TransactionValidityError::from(InvalidTransaction::Custom(Error::InvalidId as u8));
				<Module<T>>::ensure_thread_id(*id).ok_or(e)?;

				// ensure that we haven't already had a full complement of selected parathreads.
				let mut upcoming_selected_threads = SelectedThreads::get();
				if upcoming_selected_threads.is_empty() {
					upcoming_selected_threads.push(vec![]);
				}
				let i = upcoming_selected_threads.len() - 1;
				let selected_threads = &mut upcoming_selected_threads[i];
				let thread_count = ThreadCount::get() as usize;
				ensure!(
					selected_threads.len() < thread_count,
					InvalidTransaction::ExhaustsResources.into()
				);

				// ensure that this is not selecting a duplicate parathread ID
				let e = TransactionValidityError::from(InvalidTransaction::Custom(Error::Duplicate as u8));
				let pos = selected_threads
					.binary_search_by(|&(ref other_id, _)| other_id.cmp(id))
					.err()
					.ok_or(e)?;

				// ensure that this is a live bid (i.e. that the thread's chain head matches)
				let e = TransactionValidityError::from(InvalidTransaction::Custom(Error::InvalidId as u8));
				let head = <parachains::Module<T>>::parachain_head(id).ok_or(e)?;
				let actual = T::Hashing::hash(&head);
				ensure!(&actual == hash, InvalidTransaction::Stale.into());

				// updated the selected threads.
				selected_threads.insert(pos, (*id, collator.clone()));
				rstd::mem::drop(selected_threads);
				SelectedThreads::put(upcoming_selected_threads);

				// provides the state-transition for this head-data-hash; this should cue the pool
				// to throw out competing transactions with lesser fees.
				r.provides = vec![hash.encode()];
			}
		}
		Ok(r)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sr_io::{TestExternalities, with_externalities};
	use substrate_primitives::{H256, Blake2Hasher};
	use sr_primitives::{
		traits::{BlakeTwo256, IdentityLookup, ConvertInto, OnInitialize, OnFinalize, Dispatchable},
		testing::{UintAuthorityId, Header}, Perbill
	};
	use primitives::{
		parachain::{ValidatorId, Info as ParaInfo, Scheduling, LOWEST_USER_ID},
		Balance, BlockNumber,
	};
	use srml_support::{impl_outer_origin, impl_outer_dispatch, assert_ok, parameter_types};
	use keyring::Sr25519Keyring;

	use crate::parachains;
	use crate::slots;
	use crate::attestations;

	impl_outer_origin! {
		pub enum Origin for Test {
			parachains,
		}
	}

	impl_outer_dispatch! {
		pub enum Call for Test where origin: Origin {
			parachains::Parachains,
			registrar::Registrar,
		}
	}

	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub const MaximumBlockWeight: u32 = 4 * 1024 * 1024;
		pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	}
	impl system::Trait for Test {
		type Origin = Origin;
		type Call = Call;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type WeightMultiplierUpdate = ();
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
	}

	parameter_types! {
		pub const ExistentialDeposit: Balance = 0;
		pub const TransferFee: Balance = 0;
		pub const CreationFee: Balance = 0;
		pub const TransactionBaseFee: Balance = 0;
		pub const TransactionByteFee: Balance = 0;
	}

	impl balances::Trait for Test {
		type Balance = Balance;
		type OnFreeBalanceZero = ();
		type OnNewAccount = ();
		type Event = ();
		type TransactionPayment = ();
		type DustRemoval = ();
		type TransferPayment = ();
		type ExistentialDeposit = ExistentialDeposit;
		type TransferFee = TransferFee;
		type CreationFee = CreationFee;
		type TransactionBaseFee = TransactionBaseFee;
		type TransactionByteFee = TransactionByteFee;
		type WeightToFee = ConvertInto;
	}

	parameter_types!{
		pub const LeasePeriod: u64 = 10;
		pub const EndingPeriod: u64 = 3;
	}

	impl slots::Trait for Test {
		type Event = ();
		type Currency = balances::Module<Test>;
		type Parachains = Registrar;
		type EndingPeriod = EndingPeriod;
		type LeasePeriod = LeasePeriod;
	}

	parameter_types!{
		pub const AttestationPeriod: BlockNumber = 100;
	}

	impl attestations::Trait for Test {
		type AttestationPeriod = AttestationPeriod;
		type ValidatorIdentities = parachains::ValidatorIdentities<Test>;
		type RewardAttestation = ();
	}

	parameter_types! {
		pub const Period: BlockNumber = 1;
		pub const Offset: BlockNumber = 0;
	}

	impl session::Trait for Test {
		type OnSessionEnding = ();
		type Keys = UintAuthorityId;
		type ShouldEndSession = session::PeriodicSessions<Period, Offset>;
		type SessionHandler = ();
		type Event = ();
		type SelectInitialValidators = ();
		type ValidatorId = u64;
		type ValidatorIdOf = ();
	}

	impl parachains::Trait for Test {
		type Origin = Origin;
		type Call = Call;
		type ParachainCurrency = balances::Module<Test>;
		type ActiveParachains = Registrar;
		type Registrar = Registrar;
	}

	parameter_types! {
		pub const ParathreadDeposit: Balance = 10;
	}

	impl Trait for Test {
		type Event = ();
		type Origin = Origin;
		type Currency = balances::Module<Test>;
		type ParathreadDeposit = ParathreadDeposit;
		type OnSwap = slots::Module<Test>;
	}

	type Balances = balances::Module<Test>;
	type Parachains = parachains::Module<Test>;
	type System = system::Module<Test>;
	type Registrar = Module<Test>;

	fn new_test_ext(parachains: Vec<(ParaId, Vec<u8>, Vec<u8>)>) -> TestExternalities<Blake2Hasher> {
		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();

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
			.map(|(i, _k)| (i as u64, UintAuthorityId(i as u64)))
			.collect();

		let authorities: Vec<_> = authority_keys.iter().map(|k| ValidatorId::from(k.public())).collect();

		let balances: Vec<_> = (0..authority_keys.len()).map(|i| (i as u64, 10_000_000)).collect();

		parachains::GenesisConfig {
			authorities: authorities.clone(),
		}.assimilate_storage(&mut t).unwrap();

		GenesisConfig::<Test> {
			parachains,
			_phdata: Default::default(),
		}.assimilate_storage(&mut t).unwrap();

		session::GenesisConfig::<Test> {
			keys: session_keys,
		}.assimilate_storage(&mut t).unwrap();

		balances::GenesisConfig::<Test> {
			balances,
			vesting: vec![],
		}.assimilate_storage(&mut t).unwrap();

		t.into()
	}

	fn init_block() {
		println!("Initializing {}", System::block_number());
		System::on_initialize(System::block_number());
		Registrar::on_initialize(System::block_number());
		Parachains::on_initialize(System::block_number());
	}

	fn run_to_block(n: u64) {
		println!("Running until block {}", n);
		while System::block_number() < n {
			if System::block_number() > 1 {
				println!("Finalizing {}", System::block_number());
				if !parachains::DidUpdate::exists() {
					println!("Null heads update");
					assert_ok!(Parachains::set_heads(system::RawOrigin::None.into(), vec![]));
				}
				Parachains::on_finalize(System::block_number());
				Registrar::on_finalize(System::block_number());
				System::on_finalize(System::block_number());
			}
			System::set_block_number(System::block_number() + 1);
			init_block();
		}
	}

	#[test]
	fn register_deregister_chains_works() {
		let parachains = vec![
			(1u32.into(), vec![1; 3], vec![1; 3]),
		];

		with_externalities(&mut new_test_ext(parachains), || {
			run_to_block(2);

			// Genesis registration works
			assert_eq!(Registrar::active_paras(), vec![(1u32.into(), None)]);
			assert_eq!(
				Registrar::paras(&1u32.into()),
				Some(ParaInfo { scheduling: Scheduling::Always })
			);
			assert_eq!(Parachains::parachain_code(&1u32.into()), Some(vec![1; 3]));

			// Register a new parachain
			assert_ok!(Registrar::register_para(
				Origin::ROOT,
				2u32.into(),
				vec![2; 3],
				vec![2; 3],
				ParaInfo { scheduling: Scheduling::Always }
			));

			let orig_bal = Balances::free_balance(&3u64.into());
			// Register a new parathread
			assert_ok!(Registrar::register_parathead(
				Origin::signed(3u64.into()),
				vec![3; 3],
				vec![3; 3],
			));
			// deposit should be taken (reserved)
			assert_eq!(Balances::free_balance(&3u64.into()) + ParathreadDeposit::get(), orig_bal);
			assert_eq!(Balances::reserved_balance(&3u64.into()), ParathreadDeposit::get());

			run_to_block(3);

			// New paras are registered
			assert_eq!(Registrar::active_paras(), vec![(1u32.into(), None), (2u32.into(), None)]);
			assert_eq!(
				Registrar::paras(&2u32.into()),
				Some(ParaInfo { scheduling: Scheduling::Always })
			);
			assert_eq!(
				Registrar::paras(&LOWEST_USER_ID),
				Some(ParaInfo { scheduling: Scheduling::Dynamic })
			);
			assert_eq!(Parachains::parachain_code(&2u32.into()), Some(vec![2; 3]));
			assert_eq!(Parachains::parachain_code(&LOWEST_USER_ID), Some(vec![3; 3]));

			assert_ok!(Registrar::deregister_para(Origin::ROOT, 2u32.into()));
			assert_ok!(Registrar::deregister_parathread(
				parachains::Origin::Parachain(LOWEST_USER_ID).into(),
				3u32.into()
			));
			// reserved balance should be returned.
			assert_eq!(Balances::free_balance(&3u64.into()), orig_bal);
			assert_eq!(Balances::reserved_balance(&3u64.into()), 0);

			run_to_block(4);

			assert_eq!(Registrar::active_paras(), vec![(1u32.into(), None)]);
			assert_eq!(Registrar::paras(&2u32.into()), None);
			assert_eq!(Parachains::parachain_code(&2u32.into()), None);
			assert_eq!(Registrar::paras(&LOWEST_USER_ID), None);
			assert_eq!(Parachains::parachain_code(&LOWEST_USER_ID), None);
		});
	}

	#[test]
	fn parathread_scheduling_works() {
		with_externalities(&mut new_test_ext(vec![]), || {
			Registrar::set_thread_count(Origin::ROOT, 1);

			run_to_block(2);

			// Register a new parathread
			assert_ok!(Registrar::register_parathead(
				Origin::signed(3u64),
				vec![3; 3],
				vec![3; 3],
			));

			run_to_block(3);

			// transaction submitted to get parathread progressed.
			let tx: LimitParathreadCommits<Test> = LimitParathreadCommits(Default::default());
			let col: CollatorId = Sr25519Keyring::One.public().into();
			let hdh = BlakeTwo256::hash(&vec![3; 3]);
			let inner_call = super::Call::select_parathread(LOWEST_USER_ID, col.clone(), hdh);
			let call = Call::Registrar(inner_call);
			let origin = 4u64;
			assert!(tx.validate(&origin, &call, Default::default(), 0).is_ok());
			assert_ok!(call.dispatch(Origin::signed(origin)));

			run_to_block(5);

			assert_eq!(Registrar::active_paras(), vec![(LOWEST_USER_ID, Some(col))]);

			assert_ok!(Parachains::set_heads(vec![]));
		});
	}

	// TODO: check for error conditions along the way.

	#[test]
	fn parathread_rescheduling_works() {
		with_externalities(&mut new_test_ext(vec![]), || {
			Registrar::set_thread_count(Origin::ROOT, 1);

			run_to_block(2);

			// Register some parathreads.
			assert_ok!(Registrar::register_parathead(Origin::signed(3), vec![3; 3], vec![3; 3]));
			assert_ok!(Registrar::register_parathead(Origin::signed(4), vec![4; 3], vec![4; 3]));
			assert_ok!(Registrar::register_parathead(Origin::signed(5), vec![5; 3], vec![5; 3]));

			run_to_block(3);

			// transaction submitted to get parathread progressed.
			let tx: LimitParathreadCommits<Test> = LimitParathreadCommits(Default::default());
			let col: CollatorId = Sr25519Keyring::One.public().into();
			let hdh = BlakeTwo256::hash(&vec![3; 3]);
			let inner_call = super::Call::select_parathread(LOWEST_USER_ID, col.clone(), hdh);
			let call = Call::Registrar(inner_call);
			let origin = 4u64;
			assert!(tx.validate(&origin, &call, Default::default(), 0).is_ok());
			assert_ok!(call.dispatch(Origin::signed(origin)));

			// 4x: the initial time it was scheduled, plus 3 retries.
			for n in 5..9 {
				run_to_block(n);
				assert_eq!(Registrar::active_paras(), vec![(LOWEST_USER_ID, Some(col.clone()))]);
			}

			// missed too many times. dropped.
			run_to_block(9);
			assert_eq!(Registrar::active_paras(), vec![]);
		});
	}
}
