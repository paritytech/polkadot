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

use rstd::{prelude::*, result, mem::swap, ops::Add};
use std::convert::{TryFrom, TryInto};
use sr_io::blake2_256;
use sr_primitives::traits::{CheckedSub, SimpleArithmetic, StaticLookup, Zero, As, Convert};
use codec::Decode;
use srml_support::{decl_module, decl_storage, decl_event, StorageValue, StorageMap,
	traits::{Currency, ReservableCurrency, WithdrawReason, ExistenceRequirement}};
use crate::parachains::ParachainRegistrar;
use system::ensure_signed;

/// A compactly represented sub-range from the series (0, 1, 2, 3).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Encode, Decode)]
#[repr(u8)]
pub enum SlotRange {
	/// Sub range from index 0 to index 0 inclusive.
	ZeroZero = 0,
	/// Sub range from index 0 to index 1 inclusive.
	ZeroOne = 1,
	/// Sub range from index 0 to index 2 inclusive.
	ZeroTwo = 2,
	/// Sub range from index 0 to index 3 inclusive.
	ZeroThree = 3,
	/// Sub range from index 1 to index 1 inclusive.
	OneOne = 4,
	/// Sub range from index 1 to index 2 inclusive.
	OneTwo = 5,
	/// Sub range from index 1 to index 3 inclusive.
	OneThree = 6,
	/// Sub range from index 2 to index 2 inclusive.
	TwoTwo = 7,
	/// Sub range from index 2 to index 3 inclusive.
	TwoThree = 8,
	/// Sub range from index 3 to index 3 inclusive.
	ThreeThree = 9,     // == SLOT_RANGE_COUNT - 1
}

/// Total number of possible sub ranges of slots.
const SLOT_RANGE_COUNT: usize = 10;

impl SlotRange {
	pub fn new_bounded<Index: Add<Output=Index> + CheckedSub + As<u64> + Copy + Ord>(
		initial: Index,
		first: Index,
		last: Index
	) -> result::Result<Self, &'static str> {
		if first > last || first < initial || last > initial + Index::sa(3) {
			return Err("Invalid range for this auction")
		}
		let count: u64 = (last.checked_sub(&first).ok_or("range ends before it begins")?).as_();
		let first: u64 = first.checked_sub(&initial).ok_or("range begins too early")?.as_();
		match first {
			0 => match count {
				0 => Some(SlotRange::ZeroZero),
				1 => Some(SlotRange::ZeroOne),
				2 => Some(SlotRange::ZeroTwo),
				3 => Some(SlotRange::ZeroThree),
				_ => None,
			},
			1 => match count {
				0 => Some(SlotRange::OneOne),
				1 => Some(SlotRange::OneTwo),
				2 => Some(SlotRange::OneThree),
				_ => None
			},
			2 => match count { 0 => Some(SlotRange::TwoTwo), 1 => Some(SlotRange::TwoThree), _ => None },
			3 => match count { 0 => Some(SlotRange::ThreeThree), _ => None },
			_ => return Err("range begins too late"),
		}.ok_or("range ends too late")
	}

	pub fn as_pair(&self) -> (u8, u8) {
		match self {
			SlotRange::ZeroZero => (0, 0),
			SlotRange::ZeroOne => (0, 1),
			SlotRange::ZeroTwo => (0, 2),
			SlotRange::ZeroThree => (0, 3),
			SlotRange::OneOne => (1, 1),
			SlotRange::OneTwo => (1, 2),
			SlotRange::OneThree => (1, 3),
			SlotRange::TwoTwo => (2, 2),
			SlotRange::TwoThree => (2, 3),
			SlotRange::ThreeThree => (3, 3),
		}
	}

	pub fn intersects(&self, other: SlotRange) -> bool {
		let a = self.as_pair();
		let b = other.as_pair();
		a.1 >= b.0 || b.1 >= a.0
	}

	pub fn len(&self) -> usize {
		match self {
			SlotRange::ZeroZero => 1,
			SlotRange::ZeroOne => 2,
			SlotRange::ZeroTwo => 3,
			SlotRange::ZeroThree => 4,
			SlotRange::OneOne => 1,
			SlotRange::OneTwo => 2,
			SlotRange::OneThree => 3,
			SlotRange::TwoTwo => 1,
			SlotRange::TwoThree => 2,
			SlotRange::ThreeThree => 1,
		}
	}
}

impl TryFrom<usize> for SlotRange {
	type Error = ();
	fn try_from(x: usize) -> Result<SlotRange, ()> {
		Ok(match x {
			0 => SlotRange::ZeroZero,
			1 => SlotRange::ZeroOne,
			2 => SlotRange::ZeroTwo,
			3 => SlotRange::ZeroThree,
			4 => SlotRange::OneOne,
			5 => SlotRange::OneTwo,
			6 => SlotRange::OneThree,
			7 => SlotRange::TwoTwo,
			8 => SlotRange::TwoThree,
			9 => SlotRange::ThreeThree,
			_ => return Err(()),
		})
	}
}

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;
type ParaIdOf<T> = <<T as Trait>::Parachains as ParachainRegistrar<<T as system::Trait>::AccountId>>::ParaId;
type AccountIdOfParaOf<T> = <<T as Trait>::Parachains as ParachainRegistrar<<T as system::Trait>::AccountId>>
	::AccountIdOfPara;
type ParaIdOfAccountOf<T> = <<T as Trait>::Parachains as ParachainRegistrar<<T as system::Trait>::AccountId>>
	::ParaIdOfAccount;

/// The module's configuration trait.
pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;

	/// The currency type used for bidding.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// The parachain registrar type.
	type Parachains: ParachainRegistrar<Self::AccountId>;
}

pub type SubId = u32;
pub type AuctionIndex = u32;

#[derive(Clone, Eq, PartialEq, Encode, Decode)]
pub struct NewBidder<AccountId> {
	who: AccountId,
	sub: SubId,
}

/// The desired target of a bidder in an auction.
#[derive(Clone, Eq, PartialEq, Encode, Decode)]
pub enum Bidder<AccountId, ParaId> {
	/// An account ID, funds coming from that account.
	New(NewBidder<AccountId>),

	/// An existing parachain, funds coming from the amount locked as part of a previous bid topped up with funds
	/// administered by the parachain.
	Existing(ParaId),
}

impl<AccountId: Clone, ParaId: Clone> Bidder<AccountId, ParaId> {
	fn account_id<AccountIdOfPara: Convert<ParaId, AccountId>>(&self) -> AccountId {
		match self {
			Bidder::New(new_bidder) => new_bidder.who.clone(),
			Bidder::Existing(para_id) => AccountIdOfPara::convert(para_id.clone()),
		}
	}
}

#[derive(Clone, Eq, PartialEq, Encode, Decode)]
pub enum IncomingParachain<AccountId> {
	Unset(NewBidder<AccountId>),
	Deploy { code_hash: [u8; 32], initial_head_data: Vec<u8> },
}

const ENDING_PERIOD: u32 = 1000;
const LEASE_PERIOD: u32 = 100000;

type LeasePeriodOf<T> = <T as system::Trait>::BlockNumber;
type WinningData<T> = [Option<(Bidder<<T as system::Trait>::AccountId, ParaIdOf<T>>, BalanceOf<T>)>; SLOT_RANGE_COUNT];
type WinnersData<T> = Vec<(Option<NewBidder<<T as system::Trait>::AccountId>>, ParaIdOf<T>, BalanceOf<T>, SlotRange)>;

// TODO:
// calculate_winners
// events
// tests

/// This module's storage items.
decl_storage! {
	trait Store for Module<T: Trait> as Slots {

		/// The winning bids for each of the 10 ranges at each block in the final `ENDING_PERIOD` of the current
		/// auction.
		pub Winning get(winning): map u32 => Option<WinningData<T>>;

		/// Amounts currently reserved in the accounts of the current winning bidders.
		pub ReservedAmounts: map Bidder<T::AccountId, ParaIdOf<T>> => Option<BalanceOf<T>>;

		/// All `ParaId` values that are managed by this module. This includes chains that are not yet deployed (but
		/// have won an auction in the future). Once a managed ID
		pub ManagedIds: Vec<ParaIdOf<T>>;

		/// Winners of a given auction; deleted once all four slots have been used.
		pub Onboarding: map LeasePeriodOf<T> => Vec<(ParaIdOf<T>, IncomingParachain<T::AccountId>)>;

		/// Offboarding account; currency held on deposit for the parachain gets placed here if the parachain gets
		/// completely offboarded.
		pub Offboarding: map ParaIdOf<T> => T::AccountId;

		/// Various amounts on deposit for each parachain. The actual amount locked on its behalf at any time is the
		/// maximum item in this list. The first item in the list is the amount locked for the current Lease Period.
		/// The default value (an empty list) implies that the parachain no longer exists (or never existed).
		pub Deposits: map ParaIdOf<T> => Vec<BalanceOf<T>>;

		/// Information relating to the current auction, if there is one. Right now it's just the initial Lease Period
		/// that it's for.
		pub AuctionInfo: Option<(LeasePeriodOf<T>, T::BlockNumber)>;

		/// Map from Blake2 hash to preimage for any code hashes contained in `Winners`.
		pub CodeHash: map [u8; 32] => Vec<u8>;

		/// The number of auctions that been started so far.
		pub AuctionCounter: AuctionIndex;
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
			let lease_period = T::BlockNumber::sa(LEASE_PERIOD as u64);
			let lease_period_index: LeasePeriodOf<T> = (n / lease_period).into();

			if let Some((winning_ranges, auction_lease_period_index)) = Self::check_end(n) {
				// Auction is ended now. We have the winning ranges and the lease period index which acts as the offset.

				// unreserve all amounts
				for (bidder, _) in winning_ranges.iter().filter_map(|x| x.as_ref()) {
					if let Some(amount) = <ReservedAmounts<T>>::take(bidder) {
						T::Currency::unreserve(&bidder.account_id::<AccountIdOfParaOf<T>>(), amount);
					}
				}

				// figure out the actual winners
				let winners = Self::calculate_winning(winning_ranges);

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
									&<AccountIdOfParaOf<T>>::convert(para_id.clone()),
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
						let pair = (pair.0 as usize + offset, pair.1 as usize + offset + 1);
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
				// bump off old deposits and unreserve accordingly.
				<ManagedIds<T>>::mutate(|ids| {
					let new = ids.drain(..).filter(|id| {
						let mut d = <Deposits<T>>::get(id);
						if !d.is_empty() {
							// ^^ should always be true.

							if d.len() == 1 {
								// decommission
								let _ = T::Parachains::deregister_parachain(id.clone());
								T::Currency::deposit_creating(&<Offboarding<T>>::take(id), d[0]);
								<Deposits<T>>::remove(id);
								false
							} else {
								// continuing
								let outgoing = d[0];
								d.remove(0);
								<Deposits<T>>::insert(id, &d);
								let new_held = d.into_iter().max().unwrap_or_default();
								if let Some(rebate) = outgoing.checked_sub(&new_held) {
									T::Currency::deposit_creating(
										&<AccountIdOfParaOf<T>>::convert(id.clone()),
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
			let para_id = <ParaIdOfAccountOf<T>>::convert(who.clone()).ok_or("account is not a parachain")?;
			let bidder = Bidder::Existing(para_id);
			Self::handle_bid(who, bidder, auction_index, first_slot, last_slot, amount)?;
		}

		fn set_offboarding(origin, dest: <T::Lookup as StaticLookup>::Source) {
			let who = ensure_signed(origin)?;
			let dest = T::Lookup::lookup(dest)?;
			let para_id = <ParaIdOfAccountOf<T>>::convert(who)
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
	fn deposit_held(para_id: &ParaIdOf<T>) -> BalanceOf<T> {
		<Deposits<T>>::get(para_id).into_iter().max().unwrap_or_else(Zero::zero)
	}
	fn is_in_progress() -> bool {
		<AuctionInfo<T>>::exists()
	}
	fn is_ending(now: T::BlockNumber) -> Option<u32> {
		if let Some((_, early_end)) = <AuctionInfo<T>>::get() {
			if let Some(after_early_end) = now.checked_sub(&early_end) {
				let after_early_end = after_early_end.as_() as u32;
				if after_early_end < ENDING_PERIOD {
					return Some(after_early_end)
				}
			}
		}
		None
	}
	/// Some when the auction's end is known (with the end block number). None if it is unknown. If Some then
	/// the block number must be at most the previous block and at least the previous block minus `ENDING_PERIOD`.
	fn check_end(now: T::BlockNumber) -> Option<(WinningData<T>, LeasePeriodOf<T>)> {
		if let Some((lease_period_index, early_end)) = <AuctionInfo<T>>::get() {
			if early_end + T::BlockNumber::sa(ENDING_PERIOD as u64) == now {
				// Just ended!
				let offset = u32::decode(&mut<system::Module<T>>::random_seed().as_ref())
					.expect("secure hashes are always greater than 4 bytes; qed") % ENDING_PERIOD;
				let res = <Winning<T>>::get(offset).unwrap_or_default();
				for i in 0..ENDING_PERIOD {
					<Winning<T>>::remove(i)
				}
				<AuctionInfo<T>>::kill();
				return Some((res, lease_period_index))
			}
		}
		None
	}

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
						let _ = T::Currency::unreserve(&who.account_id::<AccountIdOfParaOf<T>>(), amount);
					}
				}
			}
		}
		Ok(())
	}

	fn calculate_winning(mut winning: WinningData<T>) -> WinnersData<T> {
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
				Bidder::New(new_bidder) => (Some(new_bidder), T::Parachains::new_id(), bid, r),
				Bidder::Existing(para_id) => (None, para_id, bid, r),
			}
		}).collect::<Vec<_>>()
	}
}


/// tests for this module
#[cfg(test)]
mod tests {
	use super::*;

	use runtime_io::with_externalities;
	use primitives::{H256, Blake2Hasher};
	use support::{impl_outer_origin, assert_ok};
	use runtime_primitives::{
		BuildStorage,
		traits::{BlakeTwo256, IdentityLookup},
		testing::{Digest, DigestItem, Header}
	};

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
	impl Trait for Test {
		type Event = ();
	}
	type Slots = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> runtime_io::TestExternalities<Blake2Hasher> {
		system::GenesisConfig::<Test>::default().build_storage().unwrap().0.into()
	}

	#[test]
	fn it_works_for_default_value() {
		with_externalities(&mut new_test_ext(), || {
			// Just a dummy test for the dummy funtion `do_something`
			// calling the `do_something` function with a value 42
			assert_ok!(Slots::do_something(Origin::signed(1), 42));
			// asserting that the stored value is equal to what we stored
			assert_eq!(Slots::something(), Some(42));
		});
	}
}
