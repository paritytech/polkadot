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

//! Auctioning system to determine the set of Parachains in operation. This includes logic for the auctioning
//! mechanism, for locking balance as part of the "payment", and to provide the requisite information for commissioning
//! and decommissioning them.

use rstd::{prelude::*, mem::swap, convert::TryInto};
use sr_io::blake2_256;
use sr_primitives::traits::{CheckedSub, StaticLookup, Zero, As};
use codec::Decode;
use srml_support::{decl_module, decl_storage, decl_event, StorageValue, StorageMap,
	traits::{Currency, ReservableCurrency, WithdrawReason, ExistenceRequirement}};
use primitives::parachain::AccountIdConversion;
use crate::parachains::ParachainRegistrar;
use system::ensure_signed;
use crate::slot_range::{SlotRange, SLOT_RANGE_COUNT};
use super::Get;

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;
type ParaIdOf<T> = <<T as Trait>::Parachains as ParachainRegistrar<<T as system::Trait>::AccountId>>::ParaId;

/// The module's configuration trait.
pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;

	/// The currency type used for bidding.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// The parachain registrar type.
	type Parachains: ParachainRegistrar<Self::AccountId>;

	/// The number of blocks over which an auction may be retroactively ended.
	type EndingPeriod: Get<Self::BlockNumber>;

	/// The number of blocks over which a single period lasts.
	type LeasePeriod: Get<Self::BlockNumber>;
}

pub type SubId = u32;
pub type AuctionIndex = u32;

#[derive(Clone, Eq, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct NewBidder<AccountId> {
	who: AccountId,
	sub: SubId,
}

/// The desired target of a bidder in an auction.
#[derive(Clone, Eq, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Bidder<AccountId, ParaId> {
	/// An account ID, funds coming from that account.
	New(NewBidder<AccountId>),

	/// An existing parachain, funds coming from the amount locked as part of a previous bid topped up with funds
	/// administered by the parachain.
	Existing(ParaId),
}

impl<AccountId: Clone, ParaId: AccountIdConversion<AccountId>> Bidder<AccountId, ParaId> {
	fn account_id(&self) -> AccountId {
		match self {
			Bidder::New(new_bidder) => new_bidder.who.clone(),
			Bidder::Existing(para_id) => para_id.into_account(),
		}
	}
}

#[derive(Clone, Eq, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum IncomingParachain<AccountId> {
	Unset(NewBidder<AccountId>),
	Deploy { code_hash: [u8; 32], initial_head_data: Vec<u8> },
}

type LeasePeriodOf<T> = <T as system::Trait>::BlockNumber;
type WinningData<T> = [Option<(Bidder<<T as system::Trait>::AccountId, ParaIdOf<T>>, BalanceOf<T>)>; SLOT_RANGE_COUNT];
type WinnersData<T> = Vec<(Option<NewBidder<<T as system::Trait>::AccountId>>, ParaIdOf<T>, BalanceOf<T>, SlotRange)>;

// TODO:
// events
// tests
// docs

/// This module's storage items.
decl_storage! {
	trait Store for Module<T: Trait> as Slots {

		/// The winning bids for each of the 10 ranges at each block in the final Ending Period of the current
		/// auction.
		pub Winning get(winning): map u32 => Option<WinningData<T>>;

		/// Amounts currently reserved in the accounts of the current winning bidders.
		pub ReservedAmounts get(reserved_amounts): map Bidder<T::AccountId, ParaIdOf<T>> => Option<BalanceOf<T>>;

		/// All `ParaId` values that are managed by this module. This includes chains that are not yet deployed (but
		/// have won an auction in the future).
		pub ManagedIds get(managed_ids): Vec<ParaIdOf<T>>;

		/// Winners of a given auction; deleted once all four slots have been used.
		pub Onboarding get(onboarding): map LeasePeriodOf<T> => Vec<(ParaIdOf<T>, IncomingParachain<T::AccountId>)>;

		/// Offboarding account; currency held on deposit for the parachain gets placed here if the parachain gets
		/// completely offboarded.
		pub Offboarding get(offboarding): map ParaIdOf<T> => T::AccountId;

		/// Various amounts on deposit for each parachain. The actual amount locked on its behalf at any time is the
		/// maximum item in this list. The first item in the list is the amount locked for the current Lease Period.
		/// The default value (an empty list) implies that the parachain no longer exists (or never existed).
		pub Deposits get(deposits): map ParaIdOf<T> => Vec<BalanceOf<T>>;

		/// Information relating to the current auction, if there is one. Right now it's just the initial Lease Period
		/// that it's for.
		pub AuctionInfo get(auction_info): Option<(LeasePeriodOf<T>, T::BlockNumber)>;

		/// Map from Blake2 hash to preimage for any code hashes contained in `Winners`.
		CodeHash: map [u8; 32] => Vec<u8>;

		/// The number of auctions that been started so far.
		pub AuctionCounter get(auction_counter): AuctionIndex;
	}
}

decl_event!(
	pub enum Event<T> where AccountId = <T as system::Trait>::AccountId {
		Nothing(AccountId),
	}
);

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		fn deposit_event<T>() = default;

		fn on_initialize(n: T::BlockNumber) {
			let lease_period = T::LeasePeriod::get();
			let lease_period_index: LeasePeriodOf<T> = (n / lease_period).into();

			if let Some((winning_ranges, auction_lease_period_index)) = Self::check_end(n) {
				// Auction is ended now. We have the winning ranges and the lease period index which acts as the offset.

				// unreserve all amounts
				for (bidder, _) in winning_ranges.iter().filter_map(|x| x.as_ref()) {
					if let Some(amount) = <ReservedAmounts<T>>::take(bidder) {
						T::Currency::unreserve(&bidder.account_id(), amount);
					}
				}

				// figure out the actual winners
				let winners = Self::calculate_winners(winning_ranges, T::Parachains::new_id);

				// go through winners and deduct their bid.
				for (maybe_new_deploy, para_id, amount, range) in winners.into_iter() {
					match maybe_new_deploy {
						Some(bidder) => {
							// for new deployments we ensure the full amount is deducted. This should always succeed as
							// we just unreserved the same amount.
							if T::Currency::withdraw(
								&bidder.who,
								amount,
								WithdrawReason::Fee,
								ExistenceRequirement::AllowDeath
							).is_err() {
								continue;
							}

							// add para IDs of any chains that will be newly deployed to our set of managed IDs
							<ManagedIds<T>>::mutate(|m| m.push(para_id.clone()));

							// add deployment record
							<Onboarding<T>>::mutate(auction_lease_period_index, |starters|
								starters.push((para_id.clone(), IncomingParachain::Unset(bidder)))
							);
						}
						None =>
							// for renewals it's more complicated. we just reserve any extra on top of what we already
							// have held on deposit for them.
							if let Some(additional) = amount.checked_sub(&Self::deposit_held(&para_id)) {
								if T::Currency::withdraw(
									&para_id.into_account(),
									additional,
									WithdrawReason::Fee,
									ExistenceRequirement::AllowDeath
								).is_err() {
									continue;
								}
							},
					}

					// update deposit held so it is `amount` for the new lease period indices.
					let current: u64 = lease_period_index.as_();
					let current = current as usize;
					let index: u64 = auction_lease_period_index.as_();
					let index = index as usize;
					if let Some(offset) = index.checked_sub(current) {
						let pair = range.as_pair();
						let pair = (pair.0 as usize + offset + 1, pair.1 as usize + offset + 2);
						<Deposits<T>>::mutate(para_id, |d| {
							if d.len() < pair.0 {
								d.resize_with(pair.0, Default::default);
							}
							for i in pair.0 .. pair.1 {
								if d.len() >= i {
									continue
								}
								d.push(amount);
							}
						});
					}
				}
			}

			// if beginning a new lease period...
			if (n % lease_period).is_zero() {
				println!("New lease period {:?} at block {:?}", lease_period_index, n);
				// bump off old deposits and unreserve accordingly.
				<ManagedIds<T>>::mutate(|ids| {
					let new = ids.drain(..).filter(|id| {
						let mut d = <Deposits<T>>::get(id);
						if !d.is_empty() {
							// ^^ should always be true.

							if d.len() == 1 {
								// decommission
								println!("Decommissioning {:?}", id);
								let _ = T::Parachains::deregister_parachain(id.clone());
								println!("Offboarding {:?} DOTs to {:?}", d[0], <Offboarding<T>>::get(id));
								T::Currency::deposit_creating(&<Offboarding<T>>::take(id), d[0]);
								<Deposits<T>>::remove(id);
								false
							} else {
								// continuing
								println!("Continuing {:?}", id);
								let outgoing = d[0];
								d.remove(0);
								<Deposits<T>>::insert(id, &d);
								let new_held = d.into_iter().max().unwrap_or_default();
								if let Some(rebate) = outgoing.checked_sub(&new_held) {
									T::Currency::deposit_creating(
										&id.into_account(),
										rebate
									);
								}
								true
							}
						} else {
							false
						}
					}).collect::<Vec<_>>();
					*ids = new;
				});

				// create new chains.
				for (para_id, data) in <Onboarding<T>>::take(lease_period_index) {
					match data {
						IncomingParachain::Unset(_) => {
							// Parachain not set by the time it should start!
							// Slot lost.
						}
						IncomingParachain::Deploy{code_hash, initial_head_data} => {
							let code = <CodeHash<T>>::take(code_hash);
							let _ = T::Parachains::register_parachain(para_id, code, initial_head_data);
							// ^^ not much we can do if it fails for some reason.
						}
					}
				}
			}
		}

		fn on_finalize(now: T::BlockNumber) {
			// if auction is_ending() copy previous block's winning to this block's winning if it's unset.
			if let Some(offset) = Self::is_ending(now) {
				if !<Winning<T>>::exists(&offset) {
					<Winning<T>>::insert(
						offset,
						offset.checked_sub(1).and_then(<Winning<T>>::get).unwrap_or_default()
					);
				}
			}
		}

		/// Begin an auction.
		fn new_auction(#[compact] duration: T::BlockNumber, #[compact] lease_period_index: LeasePeriodOf<T>) {
			ensure!(!Self::is_in_progress(), "auction already in progress");

			// Bump the counter.
			<AuctionCounter<T>>::mutate(|n| *n += 1);

			// set the info
			<AuctionInfo<T>>::put((lease_period_index, <system::Module<T>>::block_number() + duration));
		}

		/// Make a new bid from an account (including a parachain account) for deploying a new parachain.
		fn bid(origin,
			#[compact] sub: SubId,
			#[compact] auction_index: AuctionIndex,
			#[compact] first_slot: LeasePeriodOf<T>,
			#[compact] last_slot: LeasePeriodOf<T>,
			#[compact] amount: BalanceOf<T>
		) {
			let who = ensure_signed(origin)?;
			let bidder = Bidder::New(NewBidder{who: who.clone(), sub});
			Self::handle_bid(who, bidder, auction_index, first_slot, last_slot, amount)?;
		}

		/// Make a new bid from a parachain account for renewing that (pre-existing) parachain.
		///
		/// The origin must be a parachain account.
		fn bid_renew(origin,
			#[compact] auction_index: AuctionIndex,
			#[compact] first_slot: LeasePeriodOf<T>,
			#[compact] last_slot: LeasePeriodOf<T>,
			#[compact] amount: BalanceOf<T>
		) {
			let who = ensure_signed(origin)?;
			let para_id = <ParaIdOf<T>>::try_from_account(&who).ok_or("account is not a parachain")?;
			let bidder = Bidder::Existing(para_id);
			Self::handle_bid(who, bidder, auction_index, first_slot, last_slot, amount)?;
		}

		fn set_offboarding(origin, dest: <T::Lookup as StaticLookup>::Source) {
			let who = ensure_signed(origin)?;
			let dest = T::Lookup::lookup(dest)?;
			let para_id = <ParaIdOf<T>>::try_from_account(&who)
				.ok_or("not a parachain origin")?;
			<Offboarding<T>>::insert(para_id, dest);
		}

		fn set_deploy_data(origin,
			#[compact] sub: SubId,
			#[compact] lease_period_index: LeasePeriodOf<T>,
			code: Vec<u8>,
			initial_head_data: Vec<u8>
		) {
			let who = ensure_signed(origin)?;
			let code_hash = blake2_256(&code);
			ensure!(!<CodeHash<T>>::exists(&code_hash), "Parachain code blob already in use");

			let ours = IncomingParachain::Unset(NewBidder{who: who.clone(), sub});
			<Onboarding<T>>::mutate(lease_period_index, |starters|
				if let Some(item) = starters.iter_mut().find(|ref x| x.1 == ours) {
					<CodeHash<T>>::insert(&code_hash, &code);
					item.1 = IncomingParachain::Deploy{code_hash, initial_head_data}
				}
			)
		}
	}
}

impl<T: Trait> Module<T> {
	/// Deposit currently held for a particular parachain that we administer.
	fn deposit_held(para_id: &ParaIdOf<T>) -> BalanceOf<T> {
		<Deposits<T>>::get(para_id).into_iter().max().unwrap_or_else(Zero::zero)
	}

	/// True if an auction is in progress.
	fn is_in_progress() -> bool {
		<AuctionInfo<T>>::exists()
	}

	/// True if an auction is in its final ending period.
	fn is_ending(now: T::BlockNumber) -> Option<u32> {
		if let Some((_, early_end)) = <AuctionInfo<T>>::get() {
			if let Some(after_early_end) = now.checked_sub(&early_end) {
				if after_early_end < T::EndingPeriod::get() {
					return Some(after_early_end.as_() as u32)
				}
			}
		}
		None
	}

	/// Some when the auction's end is known (with the end block number). None if it is unknown. If Some then
	/// the block number must be at most the previous block and at least the previous block minus
	/// `T::EndingPeriod::get()`.
	fn check_end(now: T::BlockNumber) -> Option<(WinningData<T>, LeasePeriodOf<T>)> {
		if let Some((lease_period_index, early_end)) = <AuctionInfo<T>>::get() {
			if early_end + T::EndingPeriod::get() == now {
				// Just ended!
				let ending_period: u32 = T::EndingPeriod::get().as_() as u32;
				let offset = u32::decode(&mut<system::Module<T>>::random_seed().as_ref())
					.expect("secure hashes are always greater than 4 bytes; qed") % ending_period;
				let res = <Winning<T>>::get(offset).unwrap_or_default();
				for i in 0..ending_period {
					<Winning<T>>::remove(i)
				}
				<AuctionInfo<T>>::kill();
				return Some((res, lease_period_index))
			}
		}
		None
	}

	/// Actually handle a bid.
	fn handle_bid(
		who: T::AccountId,
		bidder: Bidder<T::AccountId, ParaIdOf<T>>,
		auction_index: u32,
		first_slot: LeasePeriodOf<T>,
		last_slot: LeasePeriodOf<T>,
		amount: BalanceOf<T>
	) -> Result<(), &'static str> {
		// bidding on latest auction.
		ensure!(auction_index == <AuctionCounter<T>>::get(), "not current auction");
		// assume it's actually an auction (this should never fail because of above).
		let (first_lease_period, _) = <AuctionInfo<T>>::get().ok_or("not an auction")?;

		let range = SlotRange::new_bounded(first_lease_period, first_slot, last_slot)?;
		let range_index = range as u8 as usize;
		let offset = Self::is_ending(<system::Module<T>>::block_number()).unwrap_or_default();
		let mut current_winning = <Winning<T>>::get(offset)
			.or_else(|| offset.checked_sub(1).and_then(<Winning<T>>::get))
			.unwrap_or_default();
		if current_winning[range_index].as_ref().map_or(true, |last| amount > last.1) {
			// this must overlap with all existing ranges that we're winning or it's invalid.
			ensure!(current_winning.iter()
				.enumerate()
				.all(|(i, x)| x.as_ref().map_or(true, |(w, _)| w != &bidder || range.intersects(i.try_into()
					.expect("array has SLOT_RANGE_COUNT items; index never reaches that value; qed")
				))),
				"bidder winning non-intersecting range"
			);

			// new winner - reserve the additional amount and record
			let deposit_held = if let Bidder::Existing(ref bidder_para_id) = bidder {
				Self::deposit_held(bidder_para_id)
			} else {
				Zero::zero()
			};
			let already_reserved = <ReservedAmounts<T>>::get(&bidder).unwrap_or_default() + deposit_held;
			if let Some(additional) = amount.checked_sub(&already_reserved) {
				T::Currency::reserve(&who, additional)?;
				<ReservedAmounts<T>>::insert(&bidder, amount);
			}
			let mut outgoing_winner = Some((bidder, amount));
			swap(&mut current_winning[range_index], &mut outgoing_winner);
			if let Some((who, _)) = outgoing_winner {
				if current_winning.iter().filter_map(Option::as_ref).all(|&(ref other, _)| other != &who) {
					// previous bidder no longer in winning set - unreserve their bid
					if let Some(amount) = <ReservedAmounts<T>>::take(&who) {
						// it really should be reserved; if it's not, then there's not much we can do here.
						let _ = T::Currency::unreserve(&who.account_id(), amount);
					}
				}
			}
			<Winning<T>>::insert(offset, &current_winning);
		}
		Ok(())
	}

	/// Calculate the final winners from the winning slots.
	fn calculate_winners(mut winning: WinningData<T>, new_id: impl Fn() -> ParaIdOf<T>) -> WinnersData<T> {
		let winning_ranges = {
			let mut best_winners_ending_at: [(Vec<SlotRange>, BalanceOf<T>); 4] = Default::default();
			let best_bid = |range: SlotRange| {
				winning[range as u8 as usize].as_ref()
					.map(|(_, amount)| *amount * <BalanceOf<T>>::sa(range.len() as u64))
			};
			for i in 0..4 {
				let r = SlotRange::new_bounded(0, 0, i).expect("`i < 4`; qed");
				if let Some(bid) = best_bid(r) {
					best_winners_ending_at[i] = (vec![r], bid);
				}
				for j in 0..i {
					let r = SlotRange::new_bounded(0, j + 1, i).expect("`i < 4`; `j < i`; `j + 1 < 4`; qed");
					if let Some(mut bid) = best_bid(r) {
						bid += best_winners_ending_at[j].1;
						if bid > best_winners_ending_at[i].1 {
							let mut new_winners = best_winners_ending_at[j].0.clone();
							new_winners.push(r);
							best_winners_ending_at[i] = (new_winners, bid);
						}
					}
				}
			}
			let [_, _, _, (winning_ranges, _)] = best_winners_ending_at;
			winning_ranges
		};

		winning_ranges.into_iter().map(|r| {
			let mut final_winner = (Bidder::Existing(Default::default()), Default::default());
			swap(&mut final_winner, winning[r as u8 as usize].as_mut()
				.expect("none values are filtered out in previous logic; qed"));
			let (slot_winner, bid) = final_winner;
			match slot_winner {
				Bidder::New(new_bidder) => (Some(new_bidder), new_id(), bid, r),
				Bidder::Existing(para_id) => (None, para_id, bid, r),
			}
		}).collect::<Vec<_>>()
	}
}


/// tests for this module
#[cfg(test)]
mod tests {
	use super::*;
	use std::{result::Result, collections::HashMap, cell::RefCell};

	use substrate_primitives::{Blake2Hasher, H256};
	use sr_io::with_externalities;
	use sr_primitives::{
		BuildStorage,
		traits::{BlakeTwo256, IdentityLookup, OnInitialize, OnFinalize},
		testing::{Digest, DigestItem, Header}
	};
	use srml_support::{impl_outer_origin, assert_ok};
	use balances;
	use primitives::parachain::Id as ParaId;

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	// For testing the module, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of modules we want to use.
	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	impl system::Trait for Test {
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type Digest = Digest;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type Log = DigestItem;
	}

	impl balances::Trait for Test {
		type Balance = u64;
		type OnFreeBalanceZero = ();
		type OnNewAccount = ();
		type TransactionPayment = ();
		type TransferPayment = ();
		type DustRemoval = ();
		type Event = ();
	}

	thread_local! {
		pub static PARACHAIN_COUNT: RefCell<u32> = RefCell::new(0);
		pub static PARACHAINS: RefCell<HashMap<u32, (Vec<u8>, Vec<u8>)>> = RefCell::new(HashMap::new());
	}

	pub struct TestParachains;
	impl ParachainRegistrar<u64> for TestParachains {
		type ParaId = ParaId;
		fn new_id() -> Self::ParaId {
			PARACHAIN_COUNT.with(|p| {
				*p.borrow_mut() += 1;
				(*p.borrow_mut() - 1).into()
			})
		}
		fn register_parachain(id: Self::ParaId, code: Vec<u8>, initial_head_data: Vec<u8>) -> Result<(), &'static str> {
			PARACHAINS.with(|p| {
				if p.borrow_mut().contains_key(&id.into_inner()) {
					return Err("ID already exists")
				}
				p.borrow_mut().insert(id.into_inner(), (code, initial_head_data));
				Ok(())
			})
		}
		fn deregister_parachain(id: Self::ParaId) -> Result<(), &'static str> {
			PARACHAINS.with(|p| {
				if p.borrow_mut().contains_key(&id.into_inner()) {
					return Err("ID doesn't exist")
				}
				p.borrow_mut().remove(&id.into_inner());
				Ok(())
			})
		}
	}

	fn reset_count() {
		PARACHAIN_COUNT.with(|p| *p.borrow_mut() = 0);
	}

	fn with_parachains<T>(f: impl FnOnce(&HashMap<u32, (Vec<u8>, Vec<u8>)>) -> T) -> T {
		PARACHAINS.with(|p| f(&*p.borrow()))
	}

	parameter_types!{
		pub const LeasePeriod: u64 = 10;
		pub const EndingPeriod: u64 = 3;
	}

	impl Trait for Test {
		type Event = ();
		type Currency = Balances;
		type Parachains = TestParachains;
		type LeasePeriod = LeasePeriod;
		type EndingPeriod = EndingPeriod;
	}

	type System = system::Module<Test>;
	type Balances = balances::Module<Test>;
	type Slots = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mock up.
	fn new_test_ext() -> sr_io::TestExternalities<Blake2Hasher> {
		let mut t = system::GenesisConfig::<Test>::default().build_storage().unwrap().0;
		t.extend(balances::GenesisConfig::<Test>{
			transaction_base_fee: 0,
			transaction_byte_fee: 0,
			balances: vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50), (6, 60)],
			existential_deposit: 0,
			transfer_fee: 0,
			creation_fee: 0,
			vesting: vec![],
		}.build_storage().unwrap().0);
		t.into()
	}

	fn run_to_block(n: u64) {
		while System::block_number() < n {
			Slots::on_finalize(System::block_number());
			Balances::on_finalize(System::block_number());
			System::on_finalize(System::block_number());
			System::set_block_number(System::block_number() + 1);
			System::on_initialize(System::block_number());
			Balances::on_initialize(System::block_number());
			Slots::on_initialize(System::block_number());
		}
	}

	#[test]
	fn basic_setup_works() {
		with_externalities(&mut new_test_ext(), || {
			assert_eq!(Slots::auction_counter(), 0);
			assert_eq!(Slots::deposit_held(&0u32.into()), 0);
			assert_eq!(Slots::is_in_progress(), false);
			assert_eq!(Slots::is_ending(System::block_number()), None);

			run_to_block(10);

			assert_eq!(Slots::auction_counter(), 0);
			assert_eq!(Slots::deposit_held(&0u32.into()), 0);
			assert_eq!(Slots::is_in_progress(), false);
			assert_eq!(Slots::is_ending(System::block_number()), None);
		});
	}

	#[test]
	fn can_start_auction() {
		with_externalities(&mut new_test_ext(), || {
			run_to_block(1);

			assert_ok!(Slots::new_auction(5, 1));

			assert_eq!(Slots::auction_counter(), 1);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), None);
		});
	}

	#[test]
	fn auction_proceeds_correctly() {
		with_externalities(&mut new_test_ext(), || {
			run_to_block(1);

			assert_ok!(Slots::new_auction(5, 1));

			assert_eq!(Slots::auction_counter(), 1);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), None);

			run_to_block(2);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), None);

			run_to_block(3);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), None);

			run_to_block(4);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), None);

			run_to_block(5);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), None);

			run_to_block(6);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), Some(0));

			run_to_block(7);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), Some(1));

			run_to_block(8);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), Some(2));

			run_to_block(9);
			assert_eq!(Slots::is_in_progress(), false);
			assert_eq!(Slots::is_ending(System::block_number()), None);
		});
	}

	#[test]
	fn can_win_auction() {
		with_externalities(&mut new_test_ext(), || {
			run_to_block(1);

			assert_ok!(Slots::new_auction(5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 1));
			assert_eq!(Balances::reserved_balance(&1), 1);
			assert_eq!(Balances::free_balance(&1), 9);

			run_to_block(9);
			assert_eq!(Slots::onboarding(1), vec![(0.into(), IncomingParachain::Unset(NewBidder { who: 1, sub: 0 }))]);
			assert_eq!(Slots::deposit_held(&0.into()), 1);
			assert_eq!(Balances::reserved_balance(&1), 0);
			assert_eq!(Balances::free_balance(&1), 9);
		});
	}

	#[test]
	fn offboarding_works() {
		with_externalities(&mut new_test_ext(), || {
			run_to_block(1);
			assert_ok!(Slots::new_auction(5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 1));

			run_to_block(9);
			assert_eq!(Slots::deposit_held(&0.into()), 1);
			assert_eq!(Slots::deposits(&0.into())[0], 0);

			run_to_block(49);
			assert_eq!(Slots::deposit_held(&0.into()), 1);
			assert_ok!(Slots::set_offboarding(Origin::signed(ParaId::from(0).into_account()), 10));

			run_to_block(50);
			assert_eq!(Slots::deposit_held(&0.into()), 0);
			assert_eq!(Balances::free_balance(&10), 1);
		});
	}

	#[test]
	fn onboarding_works() {
		with_externalities(&mut new_test_ext(), || {
			run_to_block(1);
			assert_ok!(Slots::new_auction(5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 1));

			run_to_block(9);
			assert_ok!(Slots::set_deploy_data(Origin::signed(1), 0, 1, vec![42], vec![69]));

			run_to_block(10);
			with_parachains(|p| {
				assert_eq!(p.len(), 1);
				assert_eq!(p[&0], (vec![42], vec![69]));
			});
		});
	}

	#[test]
	fn multiple_bids_work_pre_ending() {
		with_externalities(&mut new_test_ext(), || {
			run_to_block(1);

			assert_ok!(Slots::new_auction(5, 1));

			for i in 1..6 {
				run_to_block(i);
				assert_ok!(Slots::bid(Origin::signed(i), 0, 1, 1, 4, i));
				for j in 1..6 {
					assert_eq!(Balances::reserved_balance(&j), if j == i { j } else { 0 });
					assert_eq!(Balances::free_balance(&j), if j == i { j * 9 } else { j * 10 });
				}
			}

			run_to_block(9);
			assert_eq!(Slots::onboarding(1), vec![(0.into(), IncomingParachain::Unset(NewBidder { who: 5, sub: 0 }))]);
			assert_eq!(Slots::deposit_held(&0.into()), 5);
			assert_eq!(Balances::reserved_balance(&5), 0);
			assert_eq!(Balances::free_balance(&5), 45);
		});
	}

	#[test]
	fn multiple_bids_work_post_ending() {
		with_externalities(&mut new_test_ext(), || {
			run_to_block(1);

			assert_ok!(Slots::new_auction(5, 1));

			for i in 1..6 {
				run_to_block(i + 3);
				assert_ok!(Slots::bid(Origin::signed(i), 0, 1, 1, 4, i));
				for j in 1..6 {
					assert_eq!(Balances::reserved_balance(&j), if j == i { j } else { 0 });
					assert_eq!(Balances::free_balance(&j), if j == i { j * 9 } else { j * 10 });
				}
			}

			run_to_block(9);
			assert_eq!(Slots::onboarding(1), vec![(0.into(), IncomingParachain::Unset(NewBidder { who: 3, sub: 0 }))]);
			assert_eq!(Slots::deposit_held(&0.into()), 3);
			assert_eq!(Balances::reserved_balance(&3), 0);
			assert_eq!(Balances::free_balance(&3), 27);
		});
	}

	#[test]
	fn calculate_winners_works() {
		let mut winning = [
			/*0..0*/
			Some((Bidder::New(NewBidder{who: 2, sub: 0}), 2)),
			/*0..1*/
			None,
			/*0..2*/
			None,
			/*0..3*/
			Some((Bidder::New(NewBidder{who: 1, sub: 0}), 1)),
			/*1..1*/
			Some((Bidder::New(NewBidder{who: 3, sub: 0}), 1)),
			/*1..2*/
			None,
			/*1..3*/
			None,
			/*2..2*/
			//Some((Bidder::New(NewBidder{who: 4, sub: 0}), 1)),
			Some((Bidder::New(NewBidder{who: 1, sub: 0}), 53)),
			/*2..3*/
			None,
			/*3..3*/
			Some((Bidder::New(NewBidder{who: 5, sub: 0}), 1)),
		];
		let winners = vec![
			(Some(NewBidder{who: 2,sub: 0}), 0.into(), 2, SlotRange::ZeroZero),
			(Some(NewBidder{who: 3,sub: 0}), 1.into(), 1, SlotRange::OneOne),
			(Some(NewBidder{who: 1,sub: 0}), 2.into(), 53, SlotRange::TwoTwo),
			(Some(NewBidder{who: 5,sub: 0}), 3.into(), 1, SlotRange::ThreeThree)
		];

		assert_eq!(Slots::calculate_winners(winning.clone(), TestParachains::new_id), winners);

		reset_count();
		winning[SlotRange::ZeroThree as u8 as usize] = Some((Bidder::New(NewBidder{who: 1, sub: 0}), 2));
		let winners = vec![
			(Some(NewBidder{who: 2,sub: 0}), 0.into(), 2, SlotRange::ZeroZero),
			(Some(NewBidder{who: 3,sub: 0}), 1.into(), 1, SlotRange::OneOne),
			(Some(NewBidder{who: 1,sub: 0}), 2.into(), 53, SlotRange::TwoTwo),
			(Some(NewBidder{who: 5,sub: 0}), 3.into(), 1, SlotRange::ThreeThree)
		];
		assert_eq!(Slots::calculate_winners(winning.clone(), TestParachains::new_id), winners);

		reset_count();
		winning[SlotRange::ZeroOne as u8 as usize] = Some((Bidder::New(NewBidder{who: 4, sub: 0}), 3));
		let winners = vec![
			(Some(NewBidder{who: 4,sub: 0}), 0.into(), 3, SlotRange::ZeroOne),
			(Some(NewBidder{who: 1,sub: 0}), 1.into(), 53, SlotRange::TwoTwo),
			(Some(NewBidder{who: 5,sub: 0}), 2.into(), 1, SlotRange::ThreeThree)
		];
		assert_eq!(Slots::calculate_winners(winning.clone(), TestParachains::new_id), winners);
	}
}
