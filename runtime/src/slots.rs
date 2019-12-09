// Copyright 2019 Parity Technologies (UK) Ltd.
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
//! auctioning mechanism, for locking balance as part of the "payment", and to provide the requisite
//! information for commissioning and decommissioning them.

use rstd::{prelude::*, mem::swap, convert::TryInto};
use sp_runtime::traits::{
	CheckedSub, StaticLookup, Zero, One, CheckedConversion, Hash, AccountIdConversion,
};
use frame_support::weights::SimpleDispatchInfo;
use codec::{Encode, Decode, Codec};
use frame_support::{
	decl_module, decl_storage, decl_event, ensure,
	traits::{Currency, ReservableCurrency, WithdrawReason, ExistenceRequirement, Get, Randomness},
};
use primitives::parachain::{
	SwapAux, PARACHAIN_INFO, Id as ParaId
};
use system::{ensure_signed, ensure_root};
use crate::registrar::{Registrar, swap_ordered_existence};
use crate::slot_range::{SlotRange, SLOT_RANGE_COUNT};

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

/// The module's configuration trait.
pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;

	/// The currency type used for bidding.
	type Currency: ReservableCurrency<Self::AccountId>;

	/// The parachain registrar type.
	type Parachains: Registrar<Self::AccountId>;

	/// The number of blocks over which an auction may be retroactively ended.
	type EndingPeriod: Get<Self::BlockNumber>;

	/// The number of blocks over which a single period lasts.
	type LeasePeriod: Get<Self::BlockNumber>;

	/// Something that provides randomness in the runtime.
	type Randomness: Randomness<Self::Hash>;
}

/// A sub-bidder identifier. Used to distinguish between different logical bidders coming from the
/// same account ID.
pub type SubId = u32;
/// An auction index. We count auctions in this type.
pub type AuctionIndex = u32;

/// A bidder identifier, which is just the combination of an account ID and a sub-bidder ID.
/// This is called `NewBidder` in order to distinguish between bidders that would deploy a *new*
/// parachain and pre-existing parachains bidding to renew themselves.
#[derive(Clone, Eq, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct NewBidder<AccountId> {
	/// The bidder's account ID; this is the account that funds the bid.
	pub who: AccountId,
	/// An additional ID to allow the same account ID (and funding source) to have multiple
	/// logical bidders.
	pub sub: SubId,
}

/// The desired target of a bidder in an auction.
#[derive(Clone, Eq, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Bidder<AccountId> {
	/// An account ID, funds coming from that account.
	New(NewBidder<AccountId>),

	/// An existing parachain, funds coming from the amount locked as part of a previous bid topped
	/// up with funds administered by the parachain.
	Existing(ParaId),
}

impl<AccountId: Clone + Default + Codec> Bidder<AccountId> {
	/// Get the account that will fund this bid.
	fn funding_account(&self) -> AccountId {
		match self {
			Bidder::New(new_bidder) => new_bidder.who.clone(),
			Bidder::Existing(para_id) => para_id.into_account(),
		}
	}
}

/// Information regarding a parachain that will be deployed.
///
/// We store either the bidder that will be able to set the final deployment information or the
/// information itself.
#[derive(Clone, Eq, PartialEq, Encode, Decode)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum IncomingParachain<AccountId, Hash> {
	/// Deploy information not yet set; just the bidder identity.
	Unset(NewBidder<AccountId>),
	/// Deploy information set only by code hash; so we store the code hash and head data.
	Fixed { code_hash: Hash, initial_head_data: Vec<u8> },
	/// Deploy information fully set; so we store the code and head data.
	Deploy { code: Vec<u8>, initial_head_data: Vec<u8> },
}

type LeasePeriodOf<T> = <T as system::Trait>::BlockNumber;
// Winning data type. This encodes the top bidders of each range together with their bid.
type WinningData<T> =
	[Option<(Bidder<<T as system::Trait>::AccountId>, BalanceOf<T>)>; SLOT_RANGE_COUNT];
// Winners data type. This encodes each of the final winners of a parachain auction, the parachain
// index assigned to them, their winning bid and the range that they won.
type WinnersData<T> =
	Vec<(Option<NewBidder<<T as system::Trait>::AccountId>>, ParaId, BalanceOf<T>, SlotRange)>;

// This module's storage items.
decl_storage! {
	trait Store for Module<T: Trait> as Slots {
		/// The number of auctions that have been started so far.
		pub AuctionCounter get(auction_counter): AuctionIndex;

		/// Ordered list of all `ParaId` values that are managed by this module. This includes
		/// chains that are not yet deployed (but have won an auction in the future).
		pub ManagedIds get(managed_ids): Vec<ParaId>;

		/// Various amounts on deposit for each parachain. An entry in `ManagedIds` implies a non-
		/// default entry here.
		///
		/// The actual amount locked on its behalf at any time is the maximum item in this list. The
		/// first item in the list is the amount locked for the current Lease Period. Following
		/// items are for the subsequent lease periods.
		///
		/// The default value (an empty list) implies that the parachain no longer exists (or never
		/// existed) as far as this module is concerned.
		///
		/// If a parachain doesn't exist *yet* but is scheduled to exist in the future, then it
		/// will be left-padded with one or more zeroes to denote the fact that nothing is held on
		/// deposit for the non-existent chain currently, but is held at some point in the future.
		pub Deposits get(deposits): map ParaId => Vec<BalanceOf<T>>;

		/// Information relating to the current auction, if there is one.
		///
		/// The first item in the tuple is the lease period index that the first of the four
		/// contiguous lease periods on auction is for. The second is the block number when the
		/// auction will "begin to end", i.e. the first block of the Ending Period of the auction.
		pub AuctionInfo get(auction_info): Option<(LeasePeriodOf<T>, T::BlockNumber)>;

		/// The winning bids for each of the 10 ranges at each block in the final Ending Period of
		/// the current auction. The map's key is the 0-based index into the Ending Period. The
		/// first block of the ending period is 0; the last is `EndingPeriod - 1`.
		pub Winning get(winning): map T::BlockNumber => Option<WinningData<T>>;

		/// Amounts currently reserved in the accounts of the bidders currently winning
		/// (sub-)ranges.
		pub ReservedAmounts get(reserved_amounts): map Bidder<T::AccountId> => Option<BalanceOf<T>>;

		/// The set of Para IDs that have won and need to be on-boarded at an upcoming lease-period.
		/// This is cleared out on the first block of the lease period.
		pub OnboardQueue get(onboard_queue): map LeasePeriodOf<T> => Vec<ParaId>;

		/// The actual on-boarding information. Only exists when one of the following is true:
		/// - It is before the lease period that the parachain should be on-boarded.
		/// - The full on-boarding information has not yet been provided and the parachain is not
		/// yet due to be off-boarded.
		pub Onboarding get(onboarding): map ParaId =>
			Option<(LeasePeriodOf<T>, IncomingParachain<T::AccountId, T::Hash>)>;

		/// Off-boarding account; currency held on deposit for the parachain gets placed here if the
		/// parachain gets off-boarded; i.e. its lease period is up and it isn't renewed.
		pub Offboarding get(offboarding): map ParaId => T::AccountId;
	}
}

impl<T: Trait> SwapAux for Module<T> {
	fn ensure_can_swap(one: ParaId, other: ParaId) -> Result<(), &'static str> {
		if <Onboarding<T>>::exists(one) || <Onboarding<T>>::exists(other) {
			Err("can't swap an undeployed parachain")?
		}
		Ok(())
	}
	fn on_swap(one: ParaId, other: ParaId) -> Result<(), &'static str> {
		<Offboarding<T>>::swap(one, other);
		<Deposits<T>>::swap(one, other);
		ManagedIds::mutate(|ids| swap_ordered_existence(ids, one, other));
		Ok(())
	}
}

decl_event!(
	pub enum Event<T> where
		AccountId = <T as system::Trait>::AccountId,
		BlockNumber = <T as system::Trait>::BlockNumber,
		LeasePeriod = LeasePeriodOf<T>,
		ParaId = ParaId,
		Balance = BalanceOf<T>,
	{
		/// A new lease period is beginning.
		NewLeasePeriod(LeasePeriod),
		/// An auction started. Provides its index and the block number where it will begin to
		/// close and the first lease period of the quadruplet that is auctioned.
		AuctionStarted(AuctionIndex, LeasePeriod, BlockNumber),
		/// An auction ended. All funds become unreserved.
		AuctionClosed(AuctionIndex),
		/// Someone won the right to deploy a parachain. Balance amount is deducted for deposit.
		WonDeploy(NewBidder<AccountId>, SlotRange, ParaId, Balance),
		/// An existing parachain won the right to continue.
		/// First balance is the extra amount reseved. Second is the total amount reserved.
		WonRenewal(ParaId, SlotRange, Balance, Balance),
		/// Funds were reserved for a winning bid. First balance is the extra amount reserved.
		/// Second is the total.
		Reserved(AccountId, Balance, Balance),
		/// Funds were unreserved since bidder is no longer active.
		Unreserved(AccountId, Balance),
	}
);

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		fn deposit_event() = default;

		fn on_initialize(n: T::BlockNumber) {
			let lease_period = T::LeasePeriod::get();
			let lease_period_index: LeasePeriodOf<T> = (n / lease_period).into();

			// Check to see if an auction just ended.
			if let Some((winning_ranges, auction_lease_period_index)) = Self::check_auction_end(n) {
				// Auction is ended now. We have the winning ranges and the lease period index which
				// acts as the offset. Handle it.
				Self::manage_auction_end(
					lease_period_index,
					auction_lease_period_index,
					winning_ranges,
				);
			}
			// If we're beginning a new lease period then handle that, too.
			if (n % lease_period).is_zero() {
				Self::manage_lease_period_start(lease_period_index);
			}
		}

		fn on_finalize(now: T::BlockNumber) {
			// If the current auction is in it ending period, then ensure that the (sub-)range
			// winner information is duplicated from the previous block in case no bids happened
			// in this block.
			if let Some(offset) = Self::is_ending(now) {
				if !<Winning<T>>::exists(&offset) {
					<Winning<T>>::insert(offset,
						offset.checked_sub(&One::one())
							.and_then(<Winning<T>>::get)
							.unwrap_or_default()
					);
				}
			}
		}

		/// Create a new auction.
		///
		/// This can only happen when there isn't already an auction in progress and may only be
		/// called by the root origin. Accepts the `duration` of this auction and the
		/// `lease_period_index` of the initial lease period of the four that are to be auctioned.
		#[weight = SimpleDispatchInfo::FixedOperational(100_000)]
		pub fn new_auction(origin,
			#[compact] duration: T::BlockNumber,
			#[compact] lease_period_index: LeasePeriodOf<T>
		) {
			ensure_root(origin)?;
			ensure!(!Self::is_in_progress(), "auction already in progress");
			ensure!(lease_period_index >= Self::lease_period_index(), "lease period in past");

			// Bump the counter.
			let n = <AuctionCounter>::mutate(|n| { *n += 1; *n });

			// Set the information.
			let ending = <system::Module<T>>::block_number() + duration;
			<AuctionInfo<T>>::put((lease_period_index, ending));

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
		#[weight = SimpleDispatchInfo::FixedNormal(500_000)]
		pub fn bid(origin,
			#[compact] sub: SubId,
			#[compact] auction_index: AuctionIndex,
			#[compact] first_slot: LeasePeriodOf<T>,
			#[compact] last_slot: LeasePeriodOf<T>,
			#[compact] amount: BalanceOf<T>
		) {
			let who = ensure_signed(origin)?;
			let bidder = Bidder::New(NewBidder{who: who.clone(), sub});
			Self::handle_bid(bidder, auction_index, first_slot, last_slot, amount)?;
		}

		/// Make a new bid from a parachain account for renewing that (pre-existing) parachain.
		///
		/// The origin *must* be a parachain account.
		///
		/// Multiple simultaneous bids from the same bidder are allowed only as long as all active
		/// bids overlap each other (i.e. are mutually exclusive). Bids cannot be redacted.
		///
		/// - `auction_index` is the index of the auction to bid on. Should just be the present
		/// value of `AuctionCounter`.
		/// - `first_slot` is the first lease period index of the range to bid on. This is the
		/// absolute lease period index value, not an auction-specific offset.
		/// - `last_slot` is the last lease period index of the range to bid on. This is the
		/// absolute lease period index value, not an auction-specific offset.
		/// - `amount` is the amount to bid to be held as deposit for the parachain should the
		/// bid win. This amount is held throughout the range.
		#[weight = SimpleDispatchInfo::FixedNormal(500_000)]
		fn bid_renew(origin,
			#[compact] auction_index: AuctionIndex,
			#[compact] first_slot: LeasePeriodOf<T>,
			#[compact] last_slot: LeasePeriodOf<T>,
			#[compact] amount: BalanceOf<T>
		) {
			let who = ensure_signed(origin)?;
			let para_id = <ParaId>::try_from_account(&who)
				.ok_or("account is not a parachain")?;
			let bidder = Bidder::Existing(para_id);
			Self::handle_bid(bidder, auction_index, first_slot, last_slot, amount)?;
		}

		/// Set the off-boarding information for a parachain.
		///
		/// The origin *must* be a parachain account.
		///
		/// - `dest` is the destination account to receive the parachain's deposit.
		#[weight = SimpleDispatchInfo::FixedNormal(1_000_000)]
		pub fn set_offboarding(origin, dest: <T::Lookup as StaticLookup>::Source) {
			let who = ensure_signed(origin)?;
			let dest = T::Lookup::lookup(dest)?;
			let para_id = <ParaId>::try_from_account(&who)
				.ok_or("not a parachain origin")?;
			<Offboarding<T>>::insert(para_id, dest);
		}

		/// Set the deploy information for a successful bid to deploy a new parachain.
		///
		/// - `origin` must be the successful bidder account.
		/// - `sub` is the sub-bidder ID of the bidder.
		/// - `para_id` is the parachain ID allotted to the winning bidder.
		/// - `code_hash` is the hash of the parachain's Wasm validation function.
		/// - `initial_head_data` is the parachain's initial head data.
		#[weight = SimpleDispatchInfo::FixedNormal(500_000)]
		pub fn fix_deploy_data(origin,
			#[compact] sub: SubId,
			#[compact] para_id: ParaId,
			code_hash: T::Hash,
			initial_head_data: Vec<u8>
		) {
			let who = ensure_signed(origin)?;
			let (starts, details) = <Onboarding<T>>::get(&para_id)
				.ok_or("parachain id not in onboarding")?;
			if let IncomingParachain::Unset(ref nb) = details {
				ensure!(nb.who == who && nb.sub == sub, "parachain not registered by origin");
			} else {
				return Err("already registered")
			}
			let item = (starts, IncomingParachain::Fixed{code_hash, initial_head_data});
			<Onboarding<T>>::insert(&para_id, item);
		}

		/// Note a new parachain's code.
		///
		/// This must be called after `fix_deploy_data` and `code` must be the preimage of the
		/// `code_hash` passed there for the same `para_id`.
		///
		/// This may be called before or after the beginning of the parachain's first lease period.
		/// If called before then the parachain will become active at the first block of its
		/// starting lease period. If after, then it will become active immediately after this call.
		///
		/// - `_origin` is irrelevant.
		/// - `para_id` is the parachain ID whose code will be elaborated.
		/// - `code` is the preimage of the registered `code_hash` of `para_id`.
		#[weight = SimpleDispatchInfo::FixedNormal(5_000_000)]
		pub fn elaborate_deploy_data(_origin, #[compact] para_id: ParaId, code: Vec<u8>) {
			let (starts, details) = <Onboarding<T>>::get(&para_id)
				.ok_or("parachain id not in onboarding")?;
			if let IncomingParachain::Fixed{code_hash, initial_head_data} = details {
				ensure!(<T as system::Trait>::Hashing::hash(&code) == code_hash, "code not doesn't correspond to hash");
				if starts > Self::lease_period_index() {
					// Hasn't yet begun. Replace the on-boarding entry with the new information.
					let item = (starts, IncomingParachain::Deploy{code, initial_head_data});
					<Onboarding<T>>::insert(&para_id, item);
				} else {
					// Should have already begun. Remove the on-boarding entry and register the
					// parachain for its immediate start.
					<Onboarding<T>>::remove(&para_id);
					let _ = T::Parachains::
						register_para(para_id, PARACHAIN_INFO, code, initial_head_data);
				}
			} else {
				return Err("deploy data not yet fixed")
			}
		}
	}
}

impl<T: Trait> Module<T> {
	/// Deposit currently held for a particular parachain that we administer.
	fn deposit_held(para_id: &ParaId) -> BalanceOf<T> {
		<Deposits<T>>::get(para_id).into_iter().max().unwrap_or_else(Zero::zero)
	}

	/// True if an auction is in progress.
	pub fn is_in_progress() -> bool {
		<AuctionInfo<T>>::exists()
	}

	/// Returns `Some(n)` if the now block is part of the ending period of an auction, where `n`
	/// represents how far into the ending period this block is. Otherwise, returns `None`.
	pub fn is_ending(now: T::BlockNumber) -> Option<T::BlockNumber> {
		if let Some((_, early_end)) = <AuctionInfo<T>>::get() {
			if let Some(after_early_end) = now.checked_sub(&early_end) {
				if after_early_end < T::EndingPeriod::get() {
					return Some(after_early_end)
				}
			}
		}
		None
	}

	/// Returns the current lease period.
	fn lease_period_index() -> LeasePeriodOf<T> {
		(<system::Module<T>>::block_number() / T::LeasePeriod::get()).into()
	}

	/// Some when the auction's end is known (with the end block number). None if it is unknown.
	/// If `Some` then the block number must be at most the previous block and at least the
	/// previous block minus `T::EndingPeriod::get()`.
	///
	/// This mutates the state, cleaning up `AuctionInfo` and `Winning` in the case of an auction
	/// ending. An immediately subsequent call with the same argument will always return `None`.
	fn check_auction_end(now: T::BlockNumber) -> Option<(WinningData<T>, LeasePeriodOf<T>)> {
		if let Some((lease_period_index, early_end)) = <AuctionInfo<T>>::get() {
			if early_end + T::EndingPeriod::get() == now {
				// Just ended!
				let ending_period = T::EndingPeriod::get();
				let offset = T::BlockNumber::decode(&mut T::Randomness::random_seed().as_ref())
					.expect("secure hashes always bigger than block numbers; qed") % ending_period;
				let res = <Winning<T>>::get(offset).unwrap_or_default();
				let mut i = T::BlockNumber::zero();
				while i < ending_period {
					<Winning<T>>::remove(i);
					i += One::one();
				}
				<AuctionInfo<T>>::kill();
				return Some((res, lease_period_index))
			}
		}
		None
	}

	/// Auction just ended. We have the current lease period, the auction's lease period (which
	/// is guaranteed to be at least the current period) and the bidders that were winning each
	/// range at the time of the auction's close.
	fn manage_auction_end(
		lease_period_index: LeasePeriodOf<T>,
		auction_lease_period_index: LeasePeriodOf<T>,
		winning_ranges: WinningData<T>,
	) {
		// First, unreserve all amounts that were reserved for the bids. We will later deduct the
		// amounts from the bidders that ended up being assigned the slot so there's no need to
		// special-case them here.
		for (bidder, _) in winning_ranges.iter().filter_map(|x| x.as_ref()) {
			if let Some(amount) = <ReservedAmounts<T>>::take(bidder) {
				T::Currency::unreserve(&bidder.funding_account(), amount);
			}
		}

		// Next, calculate the winning combination of slots and thus the final winners of the
		// auction.
		let winners = Self::calculate_winners(winning_ranges, T::Parachains::new_id);

		Self::deposit_event(RawEvent::AuctionClosed(Self::auction_counter()));

		// Go through those winners and deduct their bid, updating our table of deposits
		// accordingly.
		for (maybe_new_deploy, para_id, amount, range) in winners.into_iter() {
			match maybe_new_deploy {
				Some(bidder) => {
					// For new deployments we ensure the full amount is deducted. This should always
					// succeed as we just unreserved the same amount above.
					if T::Currency::withdraw(
						&bidder.who,
						amount,
						WithdrawReason::Fee.into(),
						ExistenceRequirement::AllowDeath
					).is_err() {
						continue;
					}

					// Add para IDs of any chains that will be newly deployed to our set of managed
					// IDs.
					ManagedIds::mutate(|ids|
						if let Err(pos) = ids.binary_search(&para_id) {
							ids.insert(pos, para_id)
						} else {
							// This can't happen as it's a winner being newly
							// deployed and thus the para_id shouldn't already be being managed.
						}
					);
					Self::deposit_event(RawEvent::WonDeploy(bidder.clone(), range, para_id, amount));

					// Add a deployment record so we know to on-board them at the appropriate
					// juncture.
					let begin_offset = <LeasePeriodOf<T>>::from(range.as_pair().0 as u32);
					let begin_lease_period = auction_lease_period_index + begin_offset;
					<OnboardQueue<T>>::mutate(begin_lease_period, |starts| starts.push(para_id));
					// Add a default off-boarding account which matches the original bidder
					<Offboarding<T>>::insert(&para_id, &bidder.who);
					let entry = (begin_lease_period, IncomingParachain::Unset(bidder));
					<Onboarding<T>>::insert(&para_id, entry);
				}
				None => {
					// For renewals, reserve any extra on top of what we already have held
					// on deposit for their chain.
					let extra = if let Some(additional) =
						amount.checked_sub(&Self::deposit_held(&para_id))
					{
						if T::Currency::withdraw(
							&para_id.into_account(),
							additional,
							WithdrawReason::Fee.into(),
							ExistenceRequirement::AllowDeath
						).is_err() {
							continue;
						}
						additional
					} else {
						Default::default()
					};
					Self::deposit_event(RawEvent::WonRenewal(para_id, range, extra, amount));
				}
			}

			// Finally, we update the deposit held so it is `amount` for the new lease period
			// indices that were won in the auction.
			let maybe_offset = auction_lease_period_index
				.checked_sub(&lease_period_index)
				.and_then(|x| x.checked_into::<usize>());
			if let Some(offset) = maybe_offset {
				// Should always succeed; for it to fail it would mean we auctioned a lease period
				// that already ended.

				// The lease period index range (begin, end) that newly belongs to this parachain
				// ID. We need to ensure that it features in `Deposits` to prevent it from being
				// reaped too early (any managed parachain whose `Deposits` set runs low will be
				// removed).
				let pair = range.as_pair();
				let pair = (pair.0 as usize + offset, pair.1 as usize + offset);
				<Deposits<T>>::mutate(para_id, |d| {
					// Left-pad with zeroes as necessary.
					if d.len() < pair.0 {
						d.resize_with(pair.0, Default::default);
					}
					// Then place the deposit values for as long as the chain should exist.
					for i in pair.0 ..= pair.1 {
						if d.len() > i {
							// The chain bought the same lease period twice. Just take the maximum.
							d[i] = d[i].max(amount);
						} else if d.len() == i {
							d.push(amount);
						} else {
							unreachable!("earlier resize means it must be >= i; qed")
						}
					}
				});
			}
		}
	}

	/// A new lease period is beginning. We're at the start of the first block of it.
	///
	/// We need to on-board and off-board parachains as needed. We should also handle reducing/
	/// returning deposits.
	fn manage_lease_period_start(lease_period_index: LeasePeriodOf<T>) {
		Self::deposit_event(RawEvent::NewLeasePeriod(lease_period_index));
		// First, bump off old deposits and decommission any managed chains that are coming
		// to a close.
		ManagedIds::mutate(|ids| {
			let new = ids.drain(..).filter(|id| {
				let mut d = <Deposits<T>>::get(id);
				if !d.is_empty() {
					// ^^ should always be true, since we would have deleted the entry otherwise.

					if d.len() == 1 {
						// Just one entry, which corresponds to the now-ended lease period. Time
						// to decommission this chain.
						if <Onboarding<T>>::take(id).is_none() {
							// Only unregister it if it was actually registered in the first place.
							// If the on-boarding entry still existed, then it was never actually
							// commissioned.
							let _ = T::Parachains::deregister_para(id.clone());
						}
						// Return the full deposit to the off-boarding account.
						T::Currency::deposit_creating(&<Offboarding<T>>::take(id), d[0]);
						// Remove the now-empty deposits set and don't keep the ID around.
						<Deposits<T>>::remove(id);
						false
					} else {
						// The parachain entry is continuing into the next lease period.
						// We need to pop the first deposit entry, which corresponds to the now-
						// ended lease period.
						let outgoing = d[0];
						d.remove(0);
						<Deposits<T>>::insert(id, &d);
						// Then we need to get the new amount that should continue to be held on
						// deposit for the parachain.
						let new_held = d.into_iter().max().unwrap_or_default();
						// If this is less than what we were holding previously, then return it
						// to the parachain itself.
						if let Some(rebate) = outgoing.checked_sub(&new_held) {
							T::Currency::deposit_creating(
								&id.into_account(),
								rebate
							);
						}
						// We keep this entry around until the next lease period.
						true
					}
				} else {
					false
				}
			}).collect::<Vec<_>>();
			*ids = new;
		});

		// Deploy any new chains that are due to be commissioned.
		for para_id in <OnboardQueue<T>>::take(lease_period_index) {
			if let Some((_, IncomingParachain::Deploy{code, initial_head_data}))
				= <Onboarding<T>>::get(&para_id)
			{
				// The chain's deployment data is set; go ahead and register it, and remove the
				// now-redundant on-boarding entry.
				let _ = T::Parachains::
					register_para(para_id.clone(), PARACHAIN_INFO, code, initial_head_data);
				// ^^ not much we can do if it fails for some reason.
				<Onboarding<T>>::remove(para_id)
			}
		}
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
		bidder: Bidder<T::AccountId>,
		auction_index: u32,
		first_slot: LeasePeriodOf<T>,
		last_slot: LeasePeriodOf<T>,
		amount: BalanceOf<T>
	) -> Result<(), &'static str> {
		// Bidding on latest auction.
		ensure!(auction_index == <AuctionCounter>::get(), "not current auction");
		// Assume it's actually an auction (this should never fail because of above).
		let (first_lease_period, _) = <AuctionInfo<T>>::get().ok_or("not an auction")?;

		// Our range.
		let range = SlotRange::new_bounded(first_lease_period, first_slot, last_slot)?;
		// Range as an array index.
		let range_index = range as u8 as usize;
		// The offset into the auction ending set.
		let offset = Self::is_ending(<system::Module<T>>::block_number()).unwrap_or_default();
		// The current winning ranges.
		let mut current_winning = <Winning<T>>::get(offset)
			.or_else(|| offset.checked_sub(&One::one()).and_then(<Winning<T>>::get))
			.unwrap_or_default();
		// If this bid beat the previous winner of our range.
		if current_winning[range_index].as_ref().map_or(true, |last| amount > last.1) {
			// This must overlap with all existing ranges that we're winning on or it's invalid.
			ensure!(current_winning.iter()
				.enumerate()
				.all(|(i, x)| x.as_ref().map_or(true, |(w, _)|
					w != &bidder || range.intersects(i.try_into()
						.expect("array has SLOT_RANGE_COUNT items; index never reaches that value; qed")
					)
				)),
				"bidder winning non-intersecting range"
			);

			// Ok; we are the new winner of this range - reserve the additional amount and record.

			// Get the amount already held on deposit on our behalf if this is a renewal bid from
			// an existing parachain.
			let deposit_held = if let Bidder::Existing(ref bidder_para_id) = bidder {
				Self::deposit_held(bidder_para_id)
			} else {
				Zero::zero()
			};
			// Get the amount already reserved in any prior and still active bids by us.
			let already_reserved =
				<ReservedAmounts<T>>::get(&bidder).unwrap_or_default() + deposit_held;
			// If these don't already cover the bid...
			if let Some(additional) = amount.checked_sub(&already_reserved) {
				// ...then reserve some more funds from their account, failing if there's not
				// enough funds.
				T::Currency::reserve(&bidder.funding_account(), additional)?;
				// ...and record the amount reserved.
				<ReservedAmounts<T>>::insert(&bidder, amount);

				Self::deposit_event(RawEvent::Reserved(
					bidder.funding_account(),
					additional,
					amount
				));
			}

			// Return any funds reserved for the previous winner if they no longer have any active
			// bids.
			let mut outgoing_winner = Some((bidder, amount));
			swap(&mut current_winning[range_index], &mut outgoing_winner);
			if let Some((who, _)) = outgoing_winner {
				if current_winning.iter()
					.filter_map(Option::as_ref)
					.all(|&(ref other, _)| other != &who)
				{
					// Previous bidder is no longer winning any ranges: unreserve their funds.
					if let Some(amount) = <ReservedAmounts<T>>::take(&who) {
						// It really should be reserved; there's not much we can do here on fail.
						let _ = T::Currency::unreserve(&who.funding_account(), amount);

						Self::deposit_event(RawEvent::Unreserved(who.funding_account(), amount));
					}
				}
			}
			// Update the range winner.
			<Winning<T>>::insert(offset, &current_winning);
		}
		Ok(())
	}

	/// Calculate the final winners from the winning slots.
	///
	/// This is a simple dynamic programming algorithm designed by Al, the original code is at:
	/// https://github.com/w3f/consensus/blob/master/NPoS/auctiondynamicthing.py
	fn calculate_winners(
		mut winning: WinningData<T>,
		new_id: impl Fn() -> ParaId
	) -> WinnersData<T> {
		let winning_ranges = {
			let mut best_winners_ending_at:
				[(Vec<SlotRange>, BalanceOf<T>); 4] = Default::default();
			let best_bid = |range: SlotRange| {
				winning[range as u8 as usize].as_ref()
					.map(|(_, amount)| *amount * (range.len() as u32).into())
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

	use sp_core::H256;
	use sp_runtime::{
		Perbill, testing::Header,
		traits::{BlakeTwo256, Hash, IdentityLookup, OnInitialize, OnFinalize},
	};
	use frame_support::{impl_outer_origin, parameter_types, assert_ok, assert_noop};
	use balances;
	use primitives::parachain::{Id as ParaId, Info as ParaInfo};

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
		pub const MaximumBlockWeight: u32 = 4 * 1024 * 1024;
		pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	}
	impl system::Trait for Test {
		type Origin = Origin;
		type Call = ();
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
	}

	parameter_types! {
		pub const ExistentialDeposit: u64 = 0;
		pub const TransferFee: u64 = 0;
		pub const CreationFee: u64 = 0;
	}

	impl balances::Trait for Test {
		type Balance = u64;
		type OnFreeBalanceZero = ();
		type OnNewAccount = ();
		type Event = ();
		type DustRemoval = ();
		type TransferPayment = ();
		type ExistentialDeposit = ExistentialDeposit;
		type TransferFee = TransferFee;
		type CreationFee = CreationFee;
	}

	thread_local! {
		pub static PARACHAIN_COUNT: RefCell<u32> = RefCell::new(0);
		pub static PARACHAINS:
			RefCell<HashMap<u32, (Vec<u8>, Vec<u8>)>> = RefCell::new(HashMap::new());
	}

	pub struct TestParachains;
	impl Registrar<u64> for TestParachains {
		fn new_id() -> ParaId {
			PARACHAIN_COUNT.with(|p| {
				*p.borrow_mut() += 1;
				(*p.borrow() - 1).into()
			})
		}
		fn register_para(
			id: ParaId,
			_info: ParaInfo,
			code: Vec<u8>,
			initial_head_data: Vec<u8>
		) -> Result<(), &'static str> {
			PARACHAINS.with(|p| {
				if p.borrow().contains_key(&id.into()) {
					panic!("ID already exists")
				}
				p.borrow_mut().insert(id.into(), (code, initial_head_data));
				Ok(())
			})
		}
		fn deregister_para(id: ParaId) -> Result<(), &'static str> {
			PARACHAINS.with(|p| {
				if !p.borrow().contains_key(&id.into()) {
					panic!("ID doesn't exist")
				}
				p.borrow_mut().remove(&id.into());
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
		type Randomness = RandomnessCollectiveFlip;
	}

	type System = system::Module<Test>;
	type Balances = balances::Module<Test>;
	type Slots = Module<Test>;
	type RandomnessCollectiveFlip = randomness_collective_flip::Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mock up.
	fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
		balances::GenesisConfig::<Test>{
			balances: vec![(1, 10), (2, 20), (3, 30), (4, 40), (5, 50), (6, 60)],
			vesting: vec![],
		}.assimilate_storage(&mut t).unwrap();
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
		new_test_ext().execute_with(|| {
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
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));

			assert_eq!(Slots::auction_counter(), 1);
			assert_eq!(Slots::is_in_progress(), true);
			assert_eq!(Slots::is_ending(System::block_number()), None);
		});
	}

	#[test]
	fn auction_proceeds_correctly() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));

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
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 1));
			assert_eq!(Balances::reserved_balance(&1), 1);
			assert_eq!(Balances::free_balance(&1), 9);

			run_to_block(9);
			assert_eq!(Slots::onboard_queue(1), vec![0.into()]);
			assert_eq!(Slots::onboarding(ParaId::from(0)),
				Some((1, IncomingParachain::Unset(NewBidder { who: 1, sub: 0 })))
			);
			assert_eq!(Slots::deposit_held(&0.into()), 1);
			assert_eq!(Balances::reserved_balance(&1), 0);
			assert_eq!(Balances::free_balance(&1), 9);
		});
	}

	#[test]
	fn offboarding_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 1));
			assert_eq!(Balances::free_balance(&1), 9);

			run_to_block(9);
			assert_eq!(Slots::deposit_held(&0.into()), 1);
			assert_eq!(Slots::deposits(ParaId::from(0))[0], 0);

			run_to_block(50);
			assert_eq!(Slots::deposit_held(&0.into()), 0);
			assert_eq!(Balances::free_balance(&1), 10);
		});
	}

	#[test]
	fn set_offboarding_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 1));

			run_to_block(9);
			assert_eq!(Slots::deposit_held(&0.into()), 1);
			assert_eq!(Slots::deposits(ParaId::from(0))[0], 0);

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
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 1));

			run_to_block(9);
			let h = BlakeTwo256::hash(&[42u8][..]);
			assert_ok!(Slots::fix_deploy_data(Origin::signed(1), 0, 0.into(), h, vec![69]));
			assert_ok!(Slots::elaborate_deploy_data(Origin::signed(0), 0.into(), vec![42]));

			run_to_block(10);
			with_parachains(|p| {
				assert_eq!(p.len(), 1);
				assert_eq!(p[&0], (vec![42], vec![69]));
			});
		});
	}

	#[test]
	fn late_onboarding_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 1));

			run_to_block(10);
			with_parachains(|p| {
				assert_eq!(p.len(), 0);
			});

			run_to_block(11);
			let h = BlakeTwo256::hash(&[42u8][..]);
			assert_ok!(Slots::fix_deploy_data(Origin::signed(1), 0, 0.into(), h, vec![69]));
			assert_ok!(Slots::elaborate_deploy_data(Origin::signed(0), 0.into(), vec![42]));
			with_parachains(|p| {
				assert_eq!(p.len(), 1);
				assert_eq!(p[&0], (vec![42], vec![69]));
			});
		});
	}

	#[test]
	fn under_bidding_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 4, 5));
			assert_ok!(Slots::bid(Origin::signed(2), 0, 1, 1, 4, 1));
			assert_eq!(Balances::reserved_balance(&2), 0);
			assert_eq!(Balances::free_balance(&2), 20);
			assert_eq!(
				Slots::winning(0).unwrap()[SlotRange::ZeroThree as u8 as usize],
				Some((Bidder::New(NewBidder{who: 1, sub: 0}), 5))
			);
		});
	}

	#[test]
	fn should_choose_best_combination() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 1, 1));
			assert_ok!(Slots::bid(Origin::signed(2), 0, 1, 2, 3, 1));
			assert_ok!(Slots::bid(Origin::signed(3), 0, 1, 4, 4, 2));
			assert_ok!(Slots::bid(Origin::signed(1), 1, 1, 1, 4, 1));
			run_to_block(9);
			assert_eq!(Slots::onboard_queue(1), vec![0.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(0)),
				Some((1, IncomingParachain::Unset(NewBidder { who: 1, sub: 0 })))
			);
			assert_eq!(Slots::onboard_queue(2), vec![1.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(1)),
				Some((2, IncomingParachain::Unset(NewBidder { who: 2, sub: 0 })))
			);
			assert_eq!(Slots::onboard_queue(4), vec![2.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(2)),
				Some((4, IncomingParachain::Unset(NewBidder { who: 3, sub: 0 })))
			);
		});
	}

	#[test]
	fn independent_bids_should_fail() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 1, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 2, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 2, 4, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 2, 2, 1));
			assert_noop!(
				Slots::bid(Origin::signed(1), 0, 1, 3, 3, 1),
				"bidder winning non-intersecting range"
			);
		});
	}

	#[test]
	fn multiple_onboards_offboards_should_work() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 1, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 1, 1));
			assert_ok!(Slots::bid(Origin::signed(2), 0, 1, 2, 3, 1));
			assert_ok!(Slots::bid(Origin::signed(3), 0, 1, 4, 4, 1));

			run_to_block(5);
			assert_ok!(Slots::new_auction(Origin::ROOT, 1, 1));
			assert_ok!(Slots::bid(Origin::signed(4), 1, 2, 1, 2, 1));
			assert_ok!(Slots::bid(Origin::signed(5), 1, 2, 3, 4, 1));

			run_to_block(9);
			assert_eq!(Slots::onboard_queue(1), vec![0.into(), 3.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(0)),
				Some((1, IncomingParachain::Unset(NewBidder { who: 1, sub: 0 })))
			);
			assert_eq!(
				Slots::onboarding(ParaId::from(3)),
				Some((1, IncomingParachain::Unset(NewBidder { who: 4, sub: 1 })))
			);
			assert_eq!(Slots::onboard_queue(2), vec![1.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(1)),
				Some((2, IncomingParachain::Unset(NewBidder { who: 2, sub: 0 })))
			);
			assert_eq!(Slots::onboard_queue(3), vec![4.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(4)),
				Some((3, IncomingParachain::Unset(NewBidder { who: 5, sub: 1 })))
			);
			assert_eq!(Slots::onboard_queue(4), vec![2.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(2)),
				Some((4, IncomingParachain::Unset(NewBidder { who: 3, sub: 0 })))
			);

			for &(para, sub, acc) in &[(0, 0, 1), (1, 0, 2), (2, 0, 3), (3, 1, 4), (4, 1, 5)] {
				let h = BlakeTwo256::hash(&[acc][..]);
				assert_ok!(Slots::fix_deploy_data(Origin::signed(acc as _), sub, para.into(), h, vec![acc]));
				assert_ok!(Slots::elaborate_deploy_data(Origin::signed(0), para.into(), vec![acc]));
			}

			run_to_block(10);
			with_parachains(|p| {
				assert_eq!(p.len(), 2);
				assert_eq!(p[&0], (vec![1], vec![1]));
				assert_eq!(p[&3], (vec![4], vec![4]));
			});
			run_to_block(20);
			with_parachains(|p| {
				assert_eq!(p.len(), 2);
				assert_eq!(p[&1], (vec![2], vec![2]));
				assert_eq!(p[&3], (vec![4], vec![4]));
			});
			run_to_block(30);
			with_parachains(|p| {
				assert_eq!(p.len(), 2);
				assert_eq!(p[&1], (vec![2], vec![2]));
				assert_eq!(p[&4], (vec![5], vec![5]));
			});
			run_to_block(40);
			with_parachains(|p| {
				assert_eq!(p.len(), 2);
				assert_eq!(p[&2], (vec![3], vec![3]));
				assert_eq!(p[&4], (vec![5], vec![5]));
			});
			run_to_block(50);
			with_parachains(|p| {
				assert_eq!(p.len(), 0);
			});
		});
	}

	#[test]
	fn extensions_should_work() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 1, 1));

			run_to_block(9);
			assert_eq!(Slots::onboard_queue(1), vec![0.into()]);

			run_to_block(10);
			let h = BlakeTwo256::hash(&[1u8][..]);
			assert_ok!(Slots::fix_deploy_data(Origin::signed(1), 0, 0.into(), h, vec![1]));
			assert_ok!(Slots::elaborate_deploy_data(Origin::signed(0), 0.into(), vec![1]));

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 2));
			assert_ok!(Slots::bid_renew(Origin::signed(ParaId::from(0).into_account()), 2, 2, 2, 1));

			with_parachains(|p| {
				assert_eq!(p.len(), 1);
				assert_eq!(p[&0], (vec![1], vec![1]));
			});

			run_to_block(20);
			with_parachains(|p| {
				assert_eq!(p.len(), 1);
				assert_eq!(p[&0], (vec![1], vec![1]));
			});
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 2));
			assert_ok!(Balances::transfer(Origin::signed(1), ParaId::from(0).into_account(), 1));
			assert_ok!(Slots::bid_renew(Origin::signed(ParaId::from(0).into_account()), 3, 3, 3, 2));

			run_to_block(30);
			with_parachains(|p| {
				assert_eq!(p.len(), 1);
				assert_eq!(p[&0], (vec![1], vec![1]));
			});

			run_to_block(40);
			with_parachains(|p| {
				assert_eq!(p.len(), 0);
			});
		});
	}

	#[test]
	fn renewal_with_lower_value_should_work() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 1, 1, 5));

			run_to_block(9);
			assert_eq!(Slots::onboard_queue(1), vec![0.into()]);

			run_to_block(10);
			let h = BlakeTwo256::hash(&[1u8][..]);
			assert_ok!(Slots::fix_deploy_data(Origin::signed(1), 0, 0.into(), h, vec![1]));
			assert_ok!(Slots::elaborate_deploy_data(Origin::signed(0), 0.into(), vec![1]));

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 2));
			assert_ok!(Slots::bid_renew(Origin::signed(ParaId::from(0).into_account()), 2, 2, 2, 3));

			run_to_block(20);
			assert_eq!(Balances::free_balance::<u64>(ParaId::from(0).into_account()), 2);

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 2));
			assert_ok!(Slots::bid_renew(Origin::signed(ParaId::from(0).into_account()), 3, 3, 3, 4));

			run_to_block(30);
			assert_eq!(Balances::free_balance::<u64>(ParaId::from(0).into_account()), 1);
		});
	}

	#[test]
	fn can_win_incomplete_auction() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Slots::bid(Origin::signed(1), 0, 1, 4, 4, 5));

			run_to_block(9);
			assert_eq!(Slots::onboard_queue(1), vec![]);
			assert_eq!(Slots::onboard_queue(2), vec![]);
			assert_eq!(Slots::onboard_queue(3), vec![]);
			assert_eq!(Slots::onboard_queue(4), vec![0.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(0)),
				Some((4, IncomingParachain::Unset(NewBidder { who: 1, sub: 0 })))
			);
			assert_eq!(Slots::deposit_held(&0.into()), 5);
		});
	}

	#[test]
	fn multiple_bids_work_pre_ending() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));

			for i in 1..6 {
				run_to_block(i);
				assert_ok!(Slots::bid(Origin::signed(i), 0, 1, 1, 4, i));
				for j in 1..6 {
					assert_eq!(Balances::reserved_balance(&j), if j == i { j } else { 0 });
					assert_eq!(Balances::free_balance(&j), if j == i { j * 9 } else { j * 10 });
				}
			}

			run_to_block(9);
			assert_eq!(Slots::onboard_queue(1), vec![0.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(0)),
				Some((1, IncomingParachain::Unset(NewBidder { who: 5, sub: 0 })))
			);
			assert_eq!(Slots::deposit_held(&0.into()), 5);
			assert_eq!(Balances::reserved_balance(&5), 0);
			assert_eq!(Balances::free_balance(&5), 45);
		});
	}

	#[test]
	fn multiple_bids_work_post_ending() {
		new_test_ext().execute_with(|| {
			run_to_block(1);

			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));

			for i in 1..6 {
				run_to_block(i + 3);
				assert_ok!(Slots::bid(Origin::signed(i), 0, 1, 1, 4, i));
				for j in 1..6 {
					assert_eq!(Balances::reserved_balance(&j), if j == i { j } else { 0 });
					assert_eq!(Balances::free_balance(&j), if j == i { j * 9 } else { j * 10 });
				}
			}

			run_to_block(9);
			assert_eq!(Slots::onboard_queue(1), vec![0.into()]);
			assert_eq!(
				Slots::onboarding(ParaId::from(0)),
				Some((1, IncomingParachain::Unset(NewBidder { who: 3, sub: 0 })))
			);
			assert_eq!(Slots::deposit_held(&0.into()), 3);
			assert_eq!(Balances::reserved_balance(&3), 0);
			assert_eq!(Balances::free_balance(&3), 27);
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
			Some((Bidder::New(NewBidder{who: 1, sub: 0}), 1)),
		];
		let winners = vec![
			(Some(NewBidder{who: 1, sub: 0}), 0.into(), 1, SlotRange::ThreeThree)
		];

		assert_eq!(Slots::calculate_winners(winning, TestParachains::new_id), winners);
	}

	#[test]
	fn first_incomplete_calculate_winners_works() {
		let winning = [
			Some((Bidder::New(NewBidder{who: 1, sub: 0}), 1)),
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
			(Some(NewBidder{who: 1, sub: 0}), 0.into(), 1, SlotRange::ZeroZero)
		];

		assert_eq!(Slots::calculate_winners(winning, TestParachains::new_id), winners);
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
