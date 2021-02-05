// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Auctioning system to determine the set of Parachains in operation. This includes logic for the
//! auctioning mechanism and for reserving balance as part of the "payment". Unreserving the balance
//! happens elsewhere.

use sp_std::{prelude::*, mem::swap, convert::TryInto};
use sp_runtime::traits::{CheckedSub, Zero, One, Saturating};
use parity_scale_codec::Decode;
use frame_support::{
	decl_module, decl_storage, decl_event, decl_error, ensure, dispatch::DispatchResult,
	traits::{Currency, ReservableCurrency, Get, Randomness, EnsureOrigin},
	weights::{DispatchClass, Weight},
};
use primitives::v1::Id as ParaId;
use frame_system::ensure_signed;
use crate::slot_range::{SlotRange, SLOT_RANGE_COUNT};
use crate::traits::{Leaser, LeaseError, Auctioneer};

type CurrencyOf<T> = <<T as Config>::Leaser as Leaser>::Currency;
type BalanceOf<T> = <<<T as Config>::Leaser as Leaser>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

/// The module's configuration trait.
pub trait Config: frame_system::Config {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;

	/// The number of blocks over which a single period lasts.
	type Leaser: Leaser<AccountId=Self::AccountId, LeasePeriod=Self::BlockNumber>;

	/// The number of blocks over which an auction may be retroactively ended.
	type EndingPeriod: Get<Self::BlockNumber>;

	/// Something that provides randomness in the runtime.
	type Randomness: Randomness<Self::Hash>;

	/// The origin which may initiate auctions.
	type InitiateOrigin: EnsureOrigin<Self::Origin>;
}

/// An auction index. We count auctions in this type.
pub type AuctionIndex = u32;

type LeasePeriodOf<T> = <<T as Config>::Leaser as Leaser>::LeasePeriod;
// Winning data type. This encodes the top bidders of each range together with their bid.
type WinningData<T> =
	[Option<(<T as frame_system::Config>::AccountId, ParaId, BalanceOf<T>)>; SLOT_RANGE_COUNT];
// Winners data type. This encodes each of the final winners of a parachain auction, the parachain
// index assigned to them, their winning bid and the range that they won.
type WinnersData<T> = Vec<(<T as frame_system::Config>::AccountId, ParaId, BalanceOf<T>, SlotRange)>;

// This module's storage items.
decl_storage! {
	trait Store for Module<T: Config> as Slots {
		/// Number of auctions started so far.
		pub AuctionCounter: AuctionIndex;

		/// Information relating to the current auction, if there is one.
		///
		/// The first item in the tuple is the lease period index that the first of the four
		/// contiguous lease periods on auction is for. The second is the block number when the
		/// auction will "begin to end", i.e. the first block of the Ending Period of the auction.
		pub AuctionInfo get(fn auction_info): Option<(LeasePeriodOf<T>, T::BlockNumber)>;

		/// Amounts currently reserved in the accounts of the bidders currently winning
		/// (sub-)ranges.
		pub ReservedAmounts get(fn reserved_amounts):
			map hasher(twox_64_concat) (T::AccountId, ParaId) => Option<BalanceOf<T>>;

		/// The winning bids for each of the 10 ranges at each block in the final Ending Period of
		/// the current auction. The map's key is the 0-based index into the Ending Period. The
		/// first block of the ending period is 0; the last is `EndingPeriod - 1`.
		pub Winning get(fn winning): map hasher(twox_64_concat) T::BlockNumber => Option<WinningData<T>>;
	}
}

decl_event!(
	pub enum Event<T> where
		AccountId = <T as frame_system::Config>::AccountId,
		BlockNumber = <T as frame_system::Config>::BlockNumber,
		LeasePeriod = LeasePeriodOf<T>,
		ParaId = ParaId,
		Balance = BalanceOf<T>,
	{
		/// An auction started. Provides its index and the block number where it will begin to
		/// close and the first lease period of the quadruplet that is auctioned.
		/// [auction_index, lease_period, ending]
		AuctionStarted(AuctionIndex, LeasePeriod, BlockNumber),
		/// An auction ended. All funds become unreserved. [auction_index]
		AuctionClosed(AuctionIndex),
		/// Someone won the right to deploy a parachain. Balance amount is deducted for deposit.
		/// [bidder, range, parachain_id, amount]
		WonDeploy(AccountId, SlotRange, ParaId, Balance),
		/// An existing parachain won the right to continue.
		/// First balance is the extra amount reseved. Second is the total amount reserved.
		/// [parachain_id, begin, count, total_amount]
		WonRenewal(ParaId, LeasePeriod, LeasePeriod, Balance),
		/// Funds were reserved for a winning bid. First balance is the extra amount reserved.
		/// Second is the total. [bidder, extra_reserved, total_amount]
		Reserved(AccountId, Balance, Balance),
		/// Funds were unreserved since bidder is no longer active. [bidder, amount]
		Unreserved(AccountId, Balance),
		/// Someone attempted to lease the same slot twice for a parachain. The amount is held in reserve
		/// but no parachain slot has been leased.
		/// \[parachain_id, leaser, amount\]
		ReserveConfiscated(ParaId, AccountId, Balance),
	}
);

decl_error! {
	pub enum Error for Module<T: Config> {
		/// This auction is already in progress.
		AuctionInProgress,
		/// The lease period is in the past.
		LeasePeriodInPast,
		/// The origin for this call must be a parachain.
		NotParaOrigin,
		/// The parachain ID is not on-boarding.
		ParaNotOnboarding,
		/// The origin for this call must be the origin who registered the parachain.
		InvalidOrigin,
		/// Parachain is already registered.
		AlreadyRegistered,
		/// The code must correspond to the hash.
		InvalidCode,
		/// Deployment data has not been set for this parachain.
		UnsetDeployData,
		/// The bid must overlap all intersecting ranges.
		NonIntersectingRange,
		/// Not a current auction.
		NotCurrentAuction,
		/// Not an auction.
		NotAuction,
		/// Given code size is too large.
		CodeTooLarge,
		/// Given initial head data is too large.
		HeadDataTooLarge,
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: T::Origin {
		type Error = Error<T>;

		fn deposit_event() = default;

		fn on_initialize(n: T::BlockNumber) -> Weight {
			// If the current auction was in its ending period last block, then ensure that the (sub-)range
			// winner information is duplicated from the previous block in case no bids happened in the
			// last block.
			if let Some(offset) = n.checked_sub(&One::one()).and_then(|n| Self::is_ending(n)) {
				if !Winning::<T>::contains_key(&offset) {
					Winning::<T>::insert(offset,
						offset.checked_sub(&One::one())
							.and_then(Winning::<T>::get)
							.unwrap_or_default()
					);
				}
			}

			// Check to see if an auction just ended.
			if let Some((winning_ranges, auction_lease_period_index)) = Self::check_auction_end(n) {
				// Auction is ended now. We have the winning ranges and the lease period index which
				// acts as the offset. Handle it.
				Self::manage_auction_end(
					auction_lease_period_index,
					winning_ranges,
				);
			}

			// TODO: weight
			0
		}

		/// Create a new auction.
		///
		/// This can only happen when there isn't already an auction in progress and may only be
		/// called by the root origin. Accepts the `duration` of this auction and the
		/// `lease_period_index` of the initial lease period of the four that are to be auctioned.
		#[weight = (100_000_000, DispatchClass::Operational)]
		pub fn new_auction(origin,
			#[compact] duration: T::BlockNumber,
			#[compact] lease_period_index: LeasePeriodOf<T>
		) {
			T::InitiateOrigin::ensure_origin(origin)?;

			ensure!(!Self::is_in_progress(), Error::<T>::AuctionInProgress);
			ensure!(lease_period_index >= T::Leaser::lease_period_index(), Error::<T>::LeasePeriodInPast);

			// Bump the counter.
			let n = AuctionCounter::mutate(|n| { *n += 1; *n });

			// Set the information.
			let ending = frame_system::Module::<T>::block_number().saturating_add(duration);
			AuctionInfo::<T>::put((lease_period_index, ending));

			Self::deposit_event(RawEvent::AuctionStarted(n, lease_period_index, ending))
		}

		/// Make a new bid from an account (including a parachain account) for deploying a new
		/// parachain.
		///
		/// Multiple simultaneous bids from the same bidder are allowed only as long as all active
		/// bids overlap each other (i.e. are mutually exclusive). Bids cannot be redacted.
		///
		/// - `sub` is the sub-bidder ID, allowing for multiple competing bids to be made by (and
		/// funded by) the same account.
		/// - `auction_index` is the index of the auction to bid on. Should just be the present
		/// value of `AuctionCounter`.
		/// - `first_slot` is the first lease period index of the range to bid on. This is the
		/// absolute lease period index value, not an auction-specific offset.
		/// - `last_slot` is the last lease period index of the range to bid on. This is the
		/// absolute lease period index value, not an auction-specific offset.
		/// - `amount` is the amount to bid to be held as deposit for the parachain should the
		/// bid win. This amount is held throughout the range.
		#[weight = 500_000_000]
		pub fn bid(origin,
			#[compact] para: ParaId,
			#[compact] auction_index: AuctionIndex,
			#[compact] first_slot: LeasePeriodOf<T>,
			#[compact] last_slot: LeasePeriodOf<T>,
			#[compact] amount: BalanceOf<T>
		) {
			let who = ensure_signed(origin)?;
			Self::handle_bid(who, para, auction_index, first_slot, last_slot, amount)?;
		}
	}
}

impl<T: Config> Auctioneer for Module<T> {
	type AccountId = T::AccountId;
	type BlockNumber = T::BlockNumber;
	type LeasePeriod = T::BlockNumber;
	type Currency = CurrencyOf<T>;

	fn is_ending(now: Self::BlockNumber) -> Option<Self::BlockNumber> {
		if let Some((_, early_end)) = AuctionInfo::<T>::get() {
			if let Some(after_early_end) = now.checked_sub(&early_end) {
				if after_early_end < T::EndingPeriod::get() {
					return Some(after_early_end)
				}
			}
		}
		None
	}

	fn place_bid(
		bidder: T::AccountId,
		para: ParaId,
		first_slot: LeasePeriodOf<T>,
		last_slot: LeasePeriodOf<T>,
		amount: BalanceOf<T>,
	) -> DispatchResult {
		Self::handle_bid(bidder, para, AuctionCounter::get(), first_slot, last_slot, amount)
	}
}

impl<T: Config> Module<T> {
	/// True if an auction is in progress.
	pub fn is_in_progress() -> bool {
		AuctionInfo::<T>::exists()
	}

	/// Actually place a bid in the current auction.
	///
	/// - `bidder`: The account that will be funding this bid.
	/// - `auction_index`: The auction index of the bid. For this to succeed, must equal
	/// the current value of `AuctionCounter`.
	/// - `first_slot`: The first lease period index of the range to be bid on.
	/// - `last_slot`: The last lease period index of the range to be bid on (inclusive).
	/// - `amount`: The total amount to be the bid for deposit over the range.
	pub fn handle_bid(
		bidder: T::AccountId,
		para: ParaId,
		auction_index: u32,
		first_slot: LeasePeriodOf<T>,
		last_slot: LeasePeriodOf<T>,
		amount: BalanceOf<T>,
	) -> DispatchResult {
		// Bidding on latest auction.
		ensure!(auction_index == AuctionCounter::get(), Error::<T>::NotCurrentAuction);
		// Assume it's actually an auction (this should never fail because of above).
		let (first_lease_period, _) = AuctionInfo::<T>::get().ok_or(Error::<T>::NotAuction)?;

		// Our range.
		let range = SlotRange::new_bounded(first_lease_period, first_slot, last_slot)?;
		// Range as an array index.
		let range_index = range as u8 as usize;
		// The offset into the auction ending set.
		let offset = Self::is_ending(frame_system::Module::<T>::block_number()).unwrap_or_default();
		// The current winning ranges.
		let mut current_winning = Winning::<T>::get(offset)
			.or_else(|| offset.checked_sub(&One::one()).and_then(Winning::<T>::get))
			.unwrap_or_default();
		// If this bid beat the previous winner of our range.
		if current_winning[range_index].as_ref().map_or(true, |last| amount > last.2) {
			// This must overlap with all existing ranges that we're winning on or it's invalid.
			ensure!(current_winning.iter()
				.enumerate()
				.all(|(i, x)| x.as_ref().map_or(true, |(w, _, _)|
					w != &bidder || range.intersects(i.try_into()
						.expect("array has SLOT_RANGE_COUNT items; index never reaches that value; qed")
					)
				)),
				Error::<T>::NonIntersectingRange,
			);

			// Ok; we are the new winner of this range - reserve the additional amount and record.

			// Get the amount already held on deposit if this is a renewal bid (i.e. there's
			// an existing lease on the same para by the same leaser).
			let existing_lease_deposit = T::Leaser::deposit_held(para, &bidder);
			let reserve_required = amount.saturating_sub(existing_lease_deposit);

			// Get the amount already reserved in any prior and still active bids by us.
			let bidder_para = (bidder.clone(), para);
			let already_reserved = ReservedAmounts::<T>::get(&bidder_para).unwrap_or_default();

			// If these don't already cover the bid...
			if let Some(additional) = reserve_required.checked_sub(&already_reserved) {
				// ...then reserve some more funds from their account, failing if there's not
				// enough funds.
				CurrencyOf::<T>::reserve(&bidder, additional)?;
				// ...and record the amount reserved.
				ReservedAmounts::<T>::insert(&bidder_para, reserve_required);

				Self::deposit_event(RawEvent::Reserved(
					bidder.clone(),
					additional,
					reserve_required,
				));
			}

			// Return any funds reserved for the previous winner if they no longer have any active
			// bids.
			let mut outgoing_winner = Some((bidder, para, amount));
			swap(&mut current_winning[range_index], &mut outgoing_winner);
			if let Some((who, para, _amount)) = outgoing_winner {
				if current_winning.iter()
					.filter_map(Option::as_ref)
					.all(|&(ref other, other_para, _)| other != &who || other_para != para)
				{
					// Previous bidder is no longer winning any ranges: unreserve their funds.
					if let Some(amount) = ReservedAmounts::<T>::take(&bidder_para) {
						// It really should be reserved; there's not much we can do here on fail.
						let _ = CurrencyOf::<T>::unreserve(&who, amount);

						Self::deposit_event(RawEvent::Unreserved(who, amount));
					}
				}
			}
			// Update the range winner.
			Winning::<T>::insert(offset, &current_winning);
		}
		Ok(())
	}

	/// Some when the auction's end is known (with the end block number). None if it is unknown.
	/// If `Some` then the block number must be at most the previous block and at least the
	/// previous block minus `T::EndingPeriod::get()`.
	///
	/// This mutates the state, cleaning up `AuctionInfo` and `Winning` in the case of an auction
	/// ending. An immediately subsequent call with the same argument will always return `None`.
	fn check_auction_end(now: T::BlockNumber) -> Option<(WinningData<T>, LeasePeriodOf<T>)> {
		if let Some((lease_period_index, early_end)) = AuctionInfo::<T>::get() {
			let ending_period = T::EndingPeriod::get();
			if early_end + ending_period == now {
				// Just ended!
				let offset = T::BlockNumber::decode(&mut T::Randomness::random_seed().as_ref())
					.expect("secure hashes always bigger than block numbers; qed") % ending_period;
				let res = Winning::<T>::get(offset).unwrap_or_default();
				let mut i = T::BlockNumber::zero();
				while i < ending_period {
					Winning::<T>::remove(i);
					i += One::one();
				}
				AuctionInfo::<T>::kill();
				return Some((res, lease_period_index))
			}
		}
		None
	}

	/// Auction just ended. We have the current lease period, the auction's lease period (which
	/// is guaranteed to be at least the current period) and the bidders that were winning each
	/// range at the time of the auction's close.
	fn manage_auction_end(
		auction_lease_period_index: LeasePeriodOf<T>,
		winning_ranges: WinningData<T>,
	) {
		Self::deposit_event(RawEvent::AuctionClosed(AuctionCounter::get()));

		// First, unreserve all amounts that were reserved for the bids. We will later re-reserve the
		// amounts from the bidders that ended up being assigned the slot so there's no need to
		// special-case them here.
		for ((bidder, para), amount) in ReservedAmounts::<T>::iter() {
			ReservedAmounts::<T>::take((bidder.clone(), para));
			CurrencyOf::<T>::unreserve(&bidder, amount);
		}

		// Next, calculate the winning combination of slots and thus the final winners of the
		// auction.
		let winners = Self::calculate_winners(winning_ranges);

		// Go through those winners and re-reserve their bid, updating our table of deposits
		// accordingly.
		for (leaser, para, amount, range) in winners.into_iter() {
			let begin_offset = LeasePeriodOf::<T>::from(range.as_pair().0 as u32);
			let period_begin = auction_lease_period_index + begin_offset;
			let period_count = LeasePeriodOf::<T>::from(range.len() as u32);

			match T::Leaser::lease_out(para, &leaser, amount, period_begin, period_count) {
				Err(LeaseError::ReserveFailed) | Err(LeaseError::AlreadyEnded) => {
					// Should never happen since we just unreserved this amount (and our offset is from the
					// present period). But if it does, there's not much we can do.
				}
				Err(LeaseError::AlreadyLeased) => {
					// The leaser attempted to get a second lease on the same para ID, possibly griefing us. Let's
					// keep the amount reserved and let governance sort it out.
					if CurrencyOf::<T>::reserve(&leaser, amount).is_ok() {
						Self::deposit_event(RawEvent::ReserveConfiscated(para, leaser, amount));
					}
				}
				Ok(()) => {}, // Nothing to report.
			}
		}
	}

	/// Calculate the final winners from the winning slots.
	///
	/// This is a simple dynamic programming algorithm designed by Al, the original code is at:
	/// https://github.com/w3f/consensus/blob/master/NPoS/auctiondynamicthing.py
	fn calculate_winners(
		mut winning: WinningData<T>
	) -> WinnersData<T> {
		let winning_ranges = {
			let mut best_winners_ending_at:
				[(Vec<SlotRange>, BalanceOf<T>); 4] = Default::default();
			let best_bid = |range: SlotRange| {
				winning[range as u8 as usize].as_ref()
					.map(|(_, _, amount)| *amount * (range.len() as u32).into())
			};
			for i in 0..4 {
				let r = SlotRange::new_bounded(0, 0, i as u32).expect("`i < 4`; qed");
				if let Some(bid) = best_bid(r) {
					best_winners_ending_at[i] = (vec![r], bid);
				}
				for j in 0..i {
					let r = SlotRange::new_bounded(0, j as u32 + 1, i as u32)
						.expect("`i < 4`; `j < i`; `j + 1 < 4`; qed");
					if let Some(mut bid) = best_bid(r) {
						bid += best_winners_ending_at[j].1;
						if bid > best_winners_ending_at[i].1 {
							let mut new_winners = best_winners_ending_at[j].0.clone();
							new_winners.push(r);
							best_winners_ending_at[i] = (new_winners, bid);
						}
					} else {
						if best_winners_ending_at[j].1 > best_winners_ending_at[i].1 {
							best_winners_ending_at[i] = best_winners_ending_at[j].clone();
						}
					}
				}
			}
			let [_, _, _, (winning_ranges, _)] = best_winners_ending_at;
			winning_ranges
		};

		winning_ranges.into_iter().map(|range| {
			let mut final_winner = Default::default();
			swap(&mut final_winner, winning[range as u8 as usize].as_mut()
				.expect("none values are filtered out in previous logic; qed"));
			let (bidder, para, amount) = final_winner;
			(bidder, para, amount, range)
		}).collect::<Vec<_>>()
	}
}

/// tests for this module
#[cfg(test)]
mod tests {
	use super::*;
	use std::{collections::BTreeMap, cell::RefCell};

	use sp_core::H256;
	use sp_runtime::traits::{BlakeTwo256, IdentityLookup};
	use frame_support::{
		impl_outer_origin, parameter_types, ord_parameter_types, assert_ok, assert_noop, assert_storage_noop,
		traits::{OnInitialize, OnFinalize}, dispatch::DispatchError::BadOrigin,
	};
	use frame_system::{EnsureSignedBy, EnsureOneOf, EnsureRoot};
	use pallet_balances;
	use primitives::v1::{BlockNumber, Header, Id as ParaId};

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	// For testing the module, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of modules we want to use.
	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
	}
	impl frame_system::Config for Test {
		type BaseCallFilter = ();
		type BlockWeights = ();
		type BlockLength = ();
		type DbWeight = ();
		type Origin = Origin;
		type Call = ();
		type Index = u64;
		type BlockNumber = BlockNumber;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = ();
		type AccountData = pallet_balances::AccountData<u64>;
		type OnNewAccount = ();
		type OnKilledAccount = ();
		type SystemWeightInfo = ();
		type SS58Prefix = ();
	}

	parameter_types! {
		pub const ExistentialDeposit: u64 = 1;
	}

	impl pallet_balances::Config for Test {
		type Balance = u64;
		type DustRemoval = ();
		type Event = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type WeightInfo = ();
		type MaxLocks = ();
	}

	#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Debug)]
	pub struct LeaseData {
		leaser: u64,
		amount: u64,
	}

	thread_local! {
		pub static LEASES:
			RefCell<BTreeMap<(ParaId, BlockNumber), LeaseData>> = RefCell::new(BTreeMap::new());
	}

	fn leases() -> Vec<((ParaId, BlockNumber), LeaseData)> {
		LEASES.with(|p| (&*p.borrow()).clone().into_iter().collect::<Vec<_>>())
	}

	pub struct TestLeaser;
	impl Leaser for TestLeaser {
		type AccountId = u64;
		type LeasePeriod = BlockNumber;
		type Currency = Balances;

		fn lease_out(
			para: ParaId,
			leaser: &Self::AccountId,
			amount: <Self::Currency as Currency<Self::AccountId>>::Balance,
			period_begin: Self::LeasePeriod,
			period_count: Self::LeasePeriod,
		) -> Result<(), LeaseError> {
			LEASES.with(|l| {
				let mut leases = l.borrow_mut();
				if period_begin < Self::lease_period_index() {
					return Err(LeaseError::AlreadyEnded)
				}
				for period in period_begin..(period_begin + period_count) {
					if leases.contains_key(&(para, period)) {
						return Err(LeaseError::AlreadyLeased)
					}
					leases.insert((para, period), LeaseData { leaser: leaser.clone(), amount });
				}
				Ok(())
			})
		}

		fn deposit_held(para: ParaId, leaser: &Self::AccountId) -> <Self::Currency as Currency<Self::AccountId>>::Balance {
			leases().iter()
				.filter_map(|((id, _period), data)|
					if id == &para && &data.leaser == leaser { Some(data.amount) } else { None }
				)
				.max()
				.unwrap_or_default()
		}

		fn lease_period() -> Self::LeasePeriod {
			10
		}

		fn lease_period_index() -> Self::LeasePeriod {
			(System::block_number() / Self::lease_period()).into()
		}
	}

	parameter_types!{
		pub const EndingPeriod: BlockNumber = 3;
	}

	ord_parameter_types!{
		pub const Six: u64 = 6;
	}

	type RootOrSix = EnsureOneOf<
		u64,
		EnsureRoot<u64>,
		EnsureSignedBy<Six, u64>,
	>;

	impl Config for Test {
		type Event = ();
		type Leaser = TestLeaser;
		type EndingPeriod = EndingPeriod;
		type Randomness = RandomnessCollectiveFlip;
		type InitiateOrigin = RootOrSix;
	}

	type System = frame_system::Module<Test>;
	type Balances = pallet_balances::Module<Test>;
	type RandomnessCollectiveFlip = pallet_randomness_collective_flip::Module<Test>;
	type Auctions = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mock up.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		pallet_balances::GenesisConfig::<Test>{
			balances: vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50), (6, 60)],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	fn run_to_block(n: BlockNumber) {
		while System::block_number() < n {
			Auctions::on_finalize(System::block_number());
			Balances::on_finalize(System::block_number());
			System::on_finalize(System::block_number());
			System::set_block_number(System::block_number() + 1);
			System::on_initialize(System::block_number());
			Balances::on_initialize(System::block_number());
			Auctions::on_initialize(System::block_number());
		}
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(AuctionCounter::get(), 0);
			assert_eq!(TestLeaser::deposit_held(0u32.into(), &1), 0);
			assert_eq!(Auctions::is_in_progress(), false);
			assert_eq!(Auctions::is_ending(System::block_number()), None);

			run_to_block(10);

			assert_eq!(AuctionCounter::get(), 0);
			assert_eq!(TestLeaser::deposit_held(0u32.into(), &1), 0);
			assert_eq!(Auctions::is_in_progress(), false);
			assert_eq!(Auctions::is_ending(System::block_number()), None);
		});
	}

	#[test]
	fn can_start_auction() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_noop!(Auctions::new_auction(Origin::signed(1), 5, 1), BadOrigin);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));

			assert_eq!(AuctionCounter::get(), 1);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), None);
		});
	}

	#[test]
	fn bidding_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 1, 4, 5));

			assert_eq!(Balances::reserved_balance(1), 5);
			assert_eq!(Balances::free_balance(1), 5);
			assert_eq!(
				Auctions::winning(0).unwrap()[SlotRange::ZeroThree as u8 as usize],
				Some((1, 0.into(), 5))
			);
		});
	}

	#[test]
	fn under_bidding_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));

			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 1, 4, 5));

			assert_storage_noop!(
				{assert_ok!(Auctions::bid(Origin::signed(2), 0.into(), 1, 1, 4, 1));}
			);
		});
	}

	#[test]
	fn over_bidding_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 1, 4, 5));
			assert_ok!(Auctions::bid(Origin::signed(2), 0.into(), 1, 1, 4, 6));

			assert_eq!(Balances::reserved_balance(1), 0);
			assert_eq!(Balances::free_balance(1), 10);
			assert_eq!(Balances::reserved_balance(2), 6);
			assert_eq!(Balances::free_balance(2), 14);
			assert_eq!(
				Auctions::winning(0).unwrap()[SlotRange::ZeroThree as u8 as usize],
				Some((2, 0.into(), 6))
			);
		});
	}

	#[test]
	fn auction_proceeds_correctly() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));

			assert_eq!(AuctionCounter::get(), 1);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), None);

			run_to_block(2);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), None);

			run_to_block(3);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), None);

			run_to_block(4);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), None);

			run_to_block(5);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), None);

			run_to_block(6);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), Some(0));

			run_to_block(7);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), Some(1));

			run_to_block(8);
			assert_eq!(Auctions::is_in_progress(), true);
			assert_eq!(Auctions::is_ending(System::block_number()), Some(2));

			run_to_block(9);
			assert_eq!(Auctions::is_in_progress(), false);
			assert_eq!(Auctions::is_ending(System::block_number()), None);
		});
	}

	#[test]
	fn can_win_auction() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 1, 4, 1));
			assert_eq!(Balances::reserved_balance(1), 1);
			assert_eq!(Balances::free_balance(1), 9);
			run_to_block(9);

			assert_eq!(leases(), vec![
				((0.into(), 1), LeaseData { leaser: 1, amount: 1 }),
				((0.into(), 2), LeaseData { leaser: 1, amount: 1 }),
				((0.into(), 3), LeaseData { leaser: 1, amount: 1 }),
				((0.into(), 4), LeaseData { leaser: 1, amount: 1 }),
			]);
			assert_eq!(TestLeaser::deposit_held(0.into(), &1), 1);
		});
	}

	#[test]
	fn can_win_incomplete_auction() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 4, 4, 5));
			run_to_block(9);

			assert_eq!(leases(), vec![
				((0.into(), 4), LeaseData { leaser: 1, amount: 5 }),
			]);
			assert_eq!(TestLeaser::deposit_held(0.into(), &1), 5);
		});
	}

	#[test]
	fn should_choose_best_combination() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 1, 1, 1));
			assert_ok!(Auctions::bid(Origin::signed(2), 0.into(), 1, 2, 3, 4));
			assert_ok!(Auctions::bid(Origin::signed(3), 0.into(), 1, 4, 4, 2));
			assert_ok!(Auctions::bid(Origin::signed(1), 1.into(), 1, 1, 4, 2));
			run_to_block(9);

			assert_eq!(leases(), vec![
				((0.into(), 1), LeaseData { leaser: 1, amount: 1 }),
				((0.into(), 2), LeaseData { leaser: 2, amount: 4 }),
				((0.into(), 3), LeaseData { leaser: 2, amount: 4 }),
				((0.into(), 4), LeaseData { leaser: 3, amount: 2 }),
			]);
			assert_eq!(TestLeaser::deposit_held(0.into(), &1), 1);
			assert_eq!(TestLeaser::deposit_held(1.into(), &1), 0);
			assert_eq!(TestLeaser::deposit_held(0.into(), &2), 4);
			assert_eq!(TestLeaser::deposit_held(0.into(), &3), 2);
		});
	}

	#[test]
	fn independent_bids_should_fail() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 1, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 1, 2, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 2, 4, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 2, 2, 1));
			assert_noop!(
				Auctions::bid(Origin::signed(1), 0.into(), 1, 3, 3, 1),
				Error::<Test>::NonIntersectingRange
			);
		});
	}

	#[test]
	fn deposit_credit_should_work() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 1, 1, 5));
			assert_eq!(Balances::reserved_balance(1), 5);
			run_to_block(10);

			assert_eq!(leases(), vec![
				((0.into(), 1), LeaseData { leaser: 1, amount: 5 }),
			]);
			assert_eq!(TestLeaser::deposit_held(0.into(), &1), 5);

			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 2));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 2, 2, 2, 6));
			// Only 1 reserved since we have a deposit credit of 5.
			assert_eq!(Balances::reserved_balance(1), 1);
			run_to_block(20);

			assert_eq!(leases(), vec![
				((0.into(), 1), LeaseData { leaser: 1, amount: 5 }),
				((0.into(), 2), LeaseData { leaser: 1, amount: 6 }),
			]);
			assert_eq!(TestLeaser::deposit_held(0.into(), &1), 6);
		});
	}

	#[test]
	fn deposit_credit_on_alt_para_should_not_count() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));
			assert_ok!(Auctions::bid(Origin::signed(1), 0.into(), 1, 1, 1, 5));
			assert_eq!(Balances::reserved_balance(1), 5);
			run_to_block(10);

			assert_eq!(leases(), vec![
				((0.into(), 1), LeaseData { leaser: 1, amount: 5 }),
			]);
			assert_eq!(TestLeaser::deposit_held(0.into(), &1), 5);

			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 2));
			assert_ok!(Auctions::bid(Origin::signed(1), 1.into(), 2, 2, 2, 6));
			// 6 reserved since we are bidding on a new para; only works because we don't
			assert_eq!(Balances::reserved_balance(1), 6);
			run_to_block(20);

			assert_eq!(leases(), vec![
				((0.into(), 1), LeaseData { leaser: 1, amount: 5 }),
				((1.into(), 2), LeaseData { leaser: 1, amount: 6 }),
			]);
			assert_eq!(TestLeaser::deposit_held(0.into(), &1), 5);
			assert_eq!(TestLeaser::deposit_held(1.into(), &1), 6);
		});
	}

	#[test]
	fn multiple_bids_work_pre_ending() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));

			for i in 1..6u64 {
				run_to_block(i as _);
				assert_ok!(Auctions::bid(Origin::signed(i), 0.into(), 1, 1, 4, i));
				for j in 1..6 {
					assert_eq!(Balances::reserved_balance(j), if j == i { j } else { 0 });
					assert_eq!(Balances::free_balance(j), if j == i { j * 9 } else { j * 10 });
				}
			}

			run_to_block(9);
			assert_eq!(leases(), vec![
				((0.into(), 1), LeaseData { leaser: 5, amount: 5 }),
				((0.into(), 2), LeaseData { leaser: 5, amount: 5 }),
				((0.into(), 3), LeaseData { leaser: 5, amount: 5 }),
				((0.into(), 4), LeaseData { leaser: 5, amount: 5 }),
			]);
		});
	}

	#[test]
	fn multiple_bids_work_post_ending() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Auctions::new_auction(Origin::signed(6), 5, 1));

			for i in 1..6u64 {
				run_to_block((i + 3) as _);
				assert_ok!(Auctions::bid(Origin::signed(i), 0.into(), 1, 1, 4, i));
				for j in 1..6 {
					assert_eq!(Balances::reserved_balance(j), if j == i { j } else { 0 });
					assert_eq!(Balances::free_balance(j), if j == i { j * 9 } else { j * 10 });
				}
			}

			run_to_block(9);
			assert_eq!(leases(), vec![
				((0.into(), 1), LeaseData { leaser: 3, amount: 3 }),
				((0.into(), 2), LeaseData { leaser: 3, amount: 3 }),
				((0.into(), 3), LeaseData { leaser: 3, amount: 3 }),
				((0.into(), 4), LeaseData { leaser: 3, amount: 3 }),
			]);
		});
	}

	#[test]
	fn incomplete_calculate_winners_works() {
		let winning = [
			None,
			None,
			None,
			None,
			None,
			None,
			None,
			None,
			None,
			Some((1, 0.into(), 1)),
		];
		let winners = vec![
			(1, 0.into(), 1, SlotRange::ThreeThree)
		];

		assert_eq!(Auctions::calculate_winners(winning), winners);
	}

	#[test]
	fn first_incomplete_calculate_winners_works() {
		let winning = [
			Some((1, 0.into(), 1)),
			None,
			None,
			None,
			None,
			None,
			None,
			None,
			None,
			None,
		];
		let winners = vec![
			(1, 0.into(), 1, SlotRange::ZeroZero)
		];

		assert_eq!(Auctions::calculate_winners(winning), winners);
	}

	#[test]
	fn calculate_winners_works() {
		let mut winning = [
			/*0..0*/
			Some((2, 0.into(), 2)),
			/*0..1*/
			None,
			/*0..2*/
			None,
			/*0..3*/
			Some((1, 100.into(), 1)),
			/*1..1*/
			Some((3, 1.into(), 1)),
			/*1..2*/
			None,
			/*1..3*/
			None,
			/*2..2*/
			Some((1, 2.into(), 53)),
			/*2..3*/
			None,
			/*3..3*/
			Some((5, 3.into(), 1)),
		];
		let winners = vec![
			(2, 0.into(), 2, SlotRange::ZeroZero),
			(3, 1.into(), 1, SlotRange::OneOne),
			(1, 2.into(), 53, SlotRange::TwoTwo),
			(5, 3.into(), 1, SlotRange::ThreeThree),
		];
		assert_eq!(Auctions::calculate_winners(winning.clone()), winners);

		winning[SlotRange::ZeroOne as u8 as usize] = Some((4, 10.into(), 3));
		let winners = vec![
			(4, 10.into(), 3, SlotRange::ZeroOne),
			(1, 2.into(), 53, SlotRange::TwoTwo),
			(5, 3.into(), 1, SlotRange::ThreeThree),
		];
		assert_eq!(Auctions::calculate_winners(winning.clone()), winners);

		winning[SlotRange::ZeroThree as u8 as usize] = Some((1, 100.into(), 100));
		let winners = vec![
			(1, 100.into(), 100, SlotRange::ZeroThree),
		];
		assert_eq!(Auctions::calculate_winners(winning.clone()), winners);
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking {
	use super::{*, Module as Crowdloan};
	use crate::slots::Module as Slots;
	use frame_system::RawOrigin;
	use frame_support::{
		assert_ok,
		traits::OnInitialize,
	};
	use sp_runtime::traits::Bounded;
	use sp_std::prelude::*;

	use frame_benchmarking::{benchmarks, whitelisted_caller, account, whitelist_account};

	fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
		let events = frame_system::Module::<T>::events();
		let system_event: <T as frame_system::Config>::Event = generic_event.into();
		// compare to the last event record
		let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
		assert_eq!(event, &system_event);
	}

	benchmarks! {
		new_auction {
			let duration = T::BlockNumber::max_value();
			let lease_period_index = LeasePeriodOf::<T>::max_value();
			let origin = T::InitiateOrigin::successful_origin();
		}: _(RawOrigin::Root, duration, lease_period_index)
		verify {
			assert_last_event::<T>(RawEvent::AuctionStarted(
				AuctionCounter::get(),
				lease_period_index,
				duration,
			).into());
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use crate::auctions::tests::{new_test_ext, Test};

		#[test]
		fn test_benchmarks() {
			new_test_ext().execute_with(|| {
				assert_ok!(test_benchmark_new_auction::<Test>());
			});
		}
	}
}
