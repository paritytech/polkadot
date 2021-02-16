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

//! # Parachain Crowdloaning module
//!
//! The point of this module is to allow parachain projects to offer the ability to help fund a
//! deposit for the parachain. When the parachain is retired, the funds may be returned.
//!
//! Contributing funds is permissionless. Each fund has a child-trie which stores all
//! contributors account IDs together with the amount they contributed; the root of this can then be
//! used by the parachain to allow contributors to prove that they made some particular contribution
//! to the project (e.g. to be rewarded through some token or badge). The trie is retained for later
//! (efficient) redistribution back to the contributors.
//!
//! Contributions must be of at least `MinContribution` (to account for the resources taken in
//! tracking contributions), and may never tally greater than the fund's `cap`, set and fixed at the
//! time of creation. The `create` call may be used to create a new fund. In order to do this, then
//! a deposit must be paid of the amount `SubmissionDeposit`. Substantial resources are taken on
//! the main trie in tracking a fund and this accounts for that.
//!
//! Funds may be set up during an auction period; their closing time is fixed at creation (as a
//! block number) and if the fund is not successful by the closing time, then it will become *retired*.
//! Funds may span multiple auctions, and even auctions that sell differing periods. However, for a
//! fund to be active in bidding for an auction, it *must* have had *at least one bid* since the end
//! of the last auction. Until a fund takes a further bid following the end of an auction, then it
//! will be inactive.
//!
//! Contributors may get a refund of their contributions from retired funds. After a period (`RetirementPeriod`)
//! the fund may be dissolved entirely. At this point any non-refunded contributions are considered
//! `orphaned` and are disposed of through the `OrphanedFunds` handler (which may e.g. place them
//! into the treasury).
//!
//! Funds may accept contributions at any point before their success or retirement. When a parachain
//! slot auction enters its ending period, then parachains will each place a bid; the bid will be
//! raised once per block if the parachain had additional funds contributed since the last bid.
//!
//! Funds may set their deploy data (the code hash and head data of their parachain) at any point.
//! It may only be done once and once set cannot be changed. Good procedure would be to set them
//! ahead of receiving any contributions in order that contributors may verify that their parachain
//! contains all expected functionality. However, this is not enforced and deploy data may happen
//! at any point, even after a slot has been successfully won or, indeed, never.
//!
//! Funds that are successful winners of a slot may have their slot claimed through the `onboard`
//! call. This may only be done once and must be after the deploy data has been fixed. Successful
//! funds remain tracked (in the `Funds` storage item and the associated child trie) as long as
//! the parachain remains active. Once it does not, it is up to the parachain to ensure that the
//! funds are returned to this module's fund sub-account in order that they be redistributed back to
//! contributors. *Retirement* may be initiated by any account (using the `begin_retirement` call)
//! once the parachain is removed from the its slot.
//!
//! @WARNING: For funds to be returned, it is imperative that this module's account is provided as
//! the offboarding account for the slot. In the case that a parachain supplemented these funds in
//! order to win a later auction, then it is the parachain's duty to ensure that the right amount of
//! funds ultimately end up in module's fund sub-account.

use frame_support::{
	decl_module, decl_storage, decl_event, decl_error, ensure,
	storage::child,
	traits::{
		Currency, Get, OnUnbalanced, ExistenceRequirement::AllowDeath
	},
};
use frame_system::ensure_signed;
use sp_runtime::{ModuleId, DispatchResult,
	traits::{AccountIdConversion, Hash, Saturating, Zero, CheckedAdd, Bounded}
};
use crate::slots;
use parity_scale_codec::{Encode, Decode};
use sp_std::vec::Vec;
use primitives::v1::{Id as ParaId, HeadData};

pub type BalanceOf<T> =
	<<T as slots::Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;
#[allow(dead_code)]
pub type NegativeImbalanceOf<T> =
	<<T as slots::Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

pub trait Config: slots::Config {
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;

	/// ModuleID for the crowdloan module. An appropriate value could be ```ModuleId(*b"py/cfund")```
	type ModuleId: Get<ModuleId>;

	/// The amount to be held on deposit by the owner of a crowdloan.
	type SubmissionDeposit: Get<BalanceOf<Self>>;

	/// The minimum amount that may be contributed into a crowdloan. Should almost certainly be at
	/// least ExistentialDeposit.
	type MinContribution: Get<BalanceOf<Self>>;

	/// The period of time (in blocks) after an unsuccessful crowdloan ending when
	/// contributors are able to withdraw their funds. After this period, their funds are lost.
	type RetirementPeriod: Get<Self::BlockNumber>;

	/// What to do with funds that were not withdrawn.
	type OrphanedFunds: OnUnbalanced<NegativeImbalanceOf<Self>>;

	/// Max number of storage keys to remove per extrinsic call.
	type RemoveKeysLimit: Get<u32>;
}

/// Simple index for identifying a fund.
pub type FundIndex = u32;

#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum LastContribution<BlockNumber> {
	Never,
	PreEnding(slots::AuctionIndex),
	Ending(BlockNumber),
}

#[derive(Encode, Decode, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Debug))]
struct DeployData<Hash> {
	code_hash: Hash,
	code_size: u32,
	initial_head_data: HeadData,
}

#[derive(Encode, Decode, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "std", derive(Debug))]
#[codec(dumb_trait_bound)]
pub struct FundInfo<AccountId, Balance, Hash, BlockNumber> {
	/// The parachain that this fund has funded, if there is one. As long as this is `Some`, then
	/// the funds may not be withdrawn and the fund cannot be dissolved.
	parachain: Option<ParaId>,
	/// The owning account who placed the deposit.
	owner: AccountId,
	/// The amount of deposit placed.
	deposit: Balance,
	/// The total amount raised.
	raised: Balance,
	/// Block number after which the funding must have succeeded. If not successful at this number
	/// then everyone may withdraw their funds.
	end: BlockNumber,
	/// A hard-cap on the amount that may be contributed.
	cap: Balance,
	/// The most recent block that this had a contribution. Determines if we make a bid or not.
	/// If this is `Never`, this fund has never received a contribution.
	/// If this is `PreEnding(n)`, this fund received a contribution sometime in auction
	/// number `n` before the ending period.
	/// If this is `Ending(n)`, this fund received a contribution during the current ending period,
	/// where `n` is how far into the ending period the contribution was made.
	last_contribution: LastContribution<BlockNumber>,
	/// First slot in range to bid on; it's actually a LeasePeriod, but that's the same type as
	/// BlockNumber.
	first_slot: BlockNumber,
	/// Last slot in range to bid on; it's actually a LeasePeriod, but that's the same type as
	/// BlockNumber.
	last_slot: BlockNumber,
	/// The deployment data associated with this fund, if any. Once set it may not be reset. First
	/// is the code hash, second is the code size, third is the initial head data.
	deploy_data: Option<DeployData<Hash>>,
}

decl_storage! {
	trait Store for Module<T: Config> as Crowdloan {
		/// Info on all of the funds.
		Funds get(fn funds):
			map hasher(twox_64_concat) FundIndex
			=> Option<FundInfo<T::AccountId, BalanceOf<T>, T::Hash, T::BlockNumber>>;

		/// The total number of funds that have so far been allocated.
		FundCount get(fn fund_count): FundIndex;

		/// The funds that have had additional contributions during the last block. This is used
		/// in order to determine which funds should submit new or updated bids.
		NewRaise get(fn new_raise): Vec<FundIndex>;

		/// The number of auctions that have entered into their ending period so far.
		EndingsCount get(fn endings_count): slots::AuctionIndex;
	}
}

decl_event! {
	pub enum Event<T> where
		<T as frame_system::Config>::AccountId,
		Balance = BalanceOf<T>,
	{
		/// Create a new crowdloaning campaign. [fund_index]
		Created(FundIndex),
		/// Contributed to a crowd sale. [who, fund_index, amount]
		Contributed(AccountId, FundIndex, Balance),
		/// Withdrew full balance of a contributor. [who, fund_index, amount]
		Withdrew(AccountId, FundIndex, Balance),
		/// Fund is placed into retirement. [fund_index]
		Retiring(FundIndex),
		/// Fund is partially dissolved, i.e. there are some left over child
		/// keys that still need to be killed. [fund_index]
		PartiallyDissolved(FundIndex),
		/// Fund is dissolved. [fund_index]
		Dissolved(FundIndex),
		/// The deploy data of the funded parachain is setted. [fund_index]
		DeployDataFixed(FundIndex),
		/// Onboarding process for a winning parachain fund is completed. [find_index, parachain_id]
		Onboarded(FundIndex, ParaId),
		/// The result of trying to submit a new bid to the Slots pallet.
		HandleBidResult(FundIndex, DispatchResult),
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// Last slot must be greater than first slot.
		LastSlotBeforeFirstSlot,
		/// The last slot cannot be more then 3 slots after the first slot.
		LastSlotTooFarInFuture,
		/// The campaign ends before the current block number. The end must be in the future.
		CannotEndInPast,
		/// There was an overflow.
		Overflow,
		/// The contribution was below the minimum, `MinContribution`.
		ContributionTooSmall,
		/// Invalid fund index.
		InvalidFundIndex,
		/// Contributions exceed maximum amount.
		CapExceeded,
		/// The contribution period has already ended.
		ContributionPeriodOver,
		/// The origin of this call is invalid.
		InvalidOrigin,
		/// Deployment data for a fund can only be set once. The deployment data for this fund
		/// already exists.
		ExistingDeployData,
		/// Deployment data has not been set for this fund.
		UnsetDeployData,
		/// This fund has already been onboarded.
		AlreadyOnboard,
		/// This crowdloan does not correspond to a parachain.
		NotParachain,
		/// This parachain still has its deposit. Implies that it has already been offboarded.
		ParaHasDeposit,
		/// Funds have not yet been returned.
		FundsNotReturned,
		/// Fund has not yet retired.
		FundNotRetired,
		/// The crowdloan has not yet ended.
		FundNotEnded,
		/// There are no contributions stored in this crowdloan.
		NoContributions,
		/// This crowdloan has an active parachain and cannot be dissolved.
		HasActiveParachain,
		/// The retirement period has not ended.
		InRetirementPeriod,
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: T::Origin {
		type Error = Error<T>;

		const ModuleId: ModuleId = T::ModuleId::get();

		fn deposit_event() = default;

		/// Create a new crowdloaning campaign for a parachain slot deposit for the current auction.
		#[weight = 100_000_000]
		fn create(origin,
			#[compact] cap: BalanceOf<T>,
			#[compact] first_slot: T::BlockNumber,
			#[compact] last_slot: T::BlockNumber,
			#[compact] end: T::BlockNumber
		) {
			let owner = ensure_signed(origin)?;

			ensure!(first_slot < last_slot, Error::<T>::LastSlotBeforeFirstSlot);
			ensure!(last_slot <= first_slot + 3u32.into(), Error::<T>::LastSlotTooFarInFuture);
			ensure!(end > <frame_system::Module<T>>::block_number(), Error::<T>::CannotEndInPast);

			let index = FundCount::get();
			let next_index = index.checked_add(1).ok_or(Error::<T>::Overflow)?;

			let deposit = T::SubmissionDeposit::get();
			T::Currency::transfer(&owner, &Self::fund_account_id(index), deposit, AllowDeath)?;
			FundCount::put(next_index);

			<Funds<T>>::insert(index, FundInfo {
				parachain: None,
				owner,
				deposit,
				raised: Zero::zero(),
				end,
				cap,
				last_contribution: LastContribution::Never,
				first_slot,
				last_slot,
				deploy_data: None,
			});

			Self::deposit_event(RawEvent::Created(index));
		}

		/// Contribute to a crowd sale. This will transfer some balance over to fund a parachain
		/// slot. It will be withdrawable in two instances: the parachain becomes retired; or the
		/// slot is unable to be purchased and the timeout expires.
		#[weight = 0]
		fn contribute(origin, #[compact] index: FundIndex, #[compact] value: BalanceOf<T>) {
			let who = ensure_signed(origin)?;

			ensure!(value >= T::MinContribution::get(), Error::<T>::ContributionTooSmall);
			let mut fund = Self::funds(index).ok_or(Error::<T>::InvalidFundIndex)?;
			fund.raised  = fund.raised.checked_add(&value).ok_or(Error::<T>::Overflow)?;
			ensure!(fund.raised <= fund.cap, Error::<T>::CapExceeded);

			// Make sure crowdloan has not ended
			let now = <frame_system::Module<T>>::block_number();
			ensure!(fund.end > now, Error::<T>::ContributionPeriodOver);

			T::Currency::transfer(&who, &Self::fund_account_id(index), value, AllowDeath)?;

			let balance = Self::contribution_get(index, &who);
			let balance = balance.saturating_add(value);
			Self::contribution_put(index, &who, &balance);

			if <slots::Module<T>>::is_ending(now).is_some() {
				match fund.last_contribution {
					// In ending period; must ensure that we are in NewRaise.
					LastContribution::Ending(n) if n == now => {
						// do nothing - already in NewRaise
					}
					_ => {
						NewRaise::append(index);
						fund.last_contribution = LastContribution::Ending(now);
					}
				}
			} else {
				let endings_count = Self::endings_count();
				match fund.last_contribution {
					LastContribution::PreEnding(a) if a == endings_count => {
						// Not in ending period and no auctions have ended ending since our
						// previous bid which was also not in an ending period.
						// `NewRaise` will contain our ID still: Do nothing.
					}
					_ => {
						// Not in ending period; but an auction has been ending since our previous
						// bid, or we never had one to begin with. Add bid.
						NewRaise::append(index);
						fund.last_contribution = LastContribution::PreEnding(endings_count);
					}
				}
			}

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::Contributed(who, index, value));
		}

		/// Set the deploy data of the funded parachain if not already set. Once set, this cannot
		/// be changed again.
		///
		/// - `origin` must be the fund owner.
		/// - `index` is the fund index that `origin` owns and whose deploy data will be set.
		/// - `code_hash` is the hash of the parachain's Wasm validation function.
		/// - `initial_head_data` is the parachain's initial head data.
		#[weight = 0]
		fn fix_deploy_data(origin,
			#[compact] index: FundIndex,
			code_hash: T::Hash,
			code_size: u32,
			initial_head_data: HeadData,
		) {
			let who = ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or(Error::<T>::InvalidFundIndex)?;
			ensure!(fund.owner == who, Error::<T>::InvalidOrigin); // must be fund owner
			ensure!(fund.deploy_data.is_none(), Error::<T>::ExistingDeployData);

			fund.deploy_data = Some(DeployData { code_hash, code_size, initial_head_data });

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::DeployDataFixed(index));
		}

		/// Complete onboarding process for a winning parachain fund. This can be called once by
		/// any origin once a fund wins a slot and the fund has set its deploy data (using
		/// `fix_deploy_data`).
		///
		/// - `index` is the fund index that `origin` owns and whose deploy data will be set.
		/// - `para_id` is the parachain index that this fund won.
		#[weight = 0]
		fn onboard(origin,
			#[compact] index: FundIndex,
			#[compact] para_id: ParaId
		) {
			ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or(Error::<T>::InvalidFundIndex)?;
			let DeployData { code_hash, code_size, initial_head_data }
				= fund.clone().deploy_data.ok_or(Error::<T>::UnsetDeployData)?;
			ensure!(fund.parachain.is_none(), Error::<T>::AlreadyOnboard);
			fund.parachain = Some(para_id);

			let fund_origin = frame_system::RawOrigin::Signed(Self::fund_account_id(index)).into();
			<slots::Module<T>>::fix_deploy_data(
				fund_origin,
				index,
				para_id,
				code_hash,
				code_size,
				initial_head_data,
			)?;

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::Onboarded(index, para_id));
		}

		/// Note that a successful fund has lost its parachain slot, and place it into retirement.
		#[weight = 0]
		fn begin_retirement(origin, #[compact] index: FundIndex) {
			ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or(Error::<T>::InvalidFundIndex)?;
			let parachain_id = fund.parachain.take().ok_or(Error::<T>::NotParachain)?;
			// No deposit information implies the parachain was off-boarded
			ensure!(<slots::Module<T>>::deposits(parachain_id).len() == 0, Error::<T>::ParaHasDeposit);
			let account = Self::fund_account_id(index);
			// Funds should be returned at the end of off-boarding
			ensure!(T::Currency::free_balance(&account) >= fund.raised, Error::<T>::FundsNotReturned);

			// This fund just ended. Withdrawal period begins.
			let now = <frame_system::Module<T>>::block_number();
			fund.end = now;

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::Retiring(index));
		}

		/// Withdraw full balance of a contributor to an unsuccessful or off-boarded fund.
		#[weight = 0]
		fn withdraw(origin, who: T::AccountId, #[compact] index: FundIndex) {
			ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or(Error::<T>::InvalidFundIndex)?;
			ensure!(fund.parachain.is_none(), Error::<T>::FundNotRetired);
			let now = <frame_system::Module<T>>::block_number();

			// `fund.end` can represent the end of a failed crowdsale or the beginning of retirement
			ensure!(now >= fund.end, Error::<T>::FundNotEnded);

			let balance = Self::contribution_get(index, &who);
			ensure!(balance > Zero::zero(), Error::<T>::NoContributions);

			// Avoid using transfer to ensure we don't pay any fees.
			let fund_account = Self::fund_account_id(index);
			T::Currency::transfer(&fund_account, &who, balance, AllowDeath)?;

			Self::contribution_kill(index, &who);
			fund.raised = fund.raised.saturating_sub(balance);

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::Withdrew(who, index, balance));
		}

		/// Remove a fund after either: it was unsuccessful and it timed out; or it was successful
		/// but it has been retired from its parachain slot. This places any deposits that were not
		/// withdrawn into the treasury.
		#[weight = 0]
		fn dissolve(origin, #[compact] index: FundIndex) {
			ensure_signed(origin)?;

			let fund = Self::funds(index).ok_or(Error::<T>::InvalidFundIndex)?;
			ensure!(fund.parachain.is_none(), Error::<T>::HasActiveParachain);
			let now = <frame_system::Module<T>>::block_number();
			ensure!(
				now >= fund.end.saturating_add(T::RetirementPeriod::get()),
				Error::<T>::InRetirementPeriod
			);

			// Try killing the crowdloan child trie
			match Self::crowdloan_kill(index) {
				child::KillOutcome::AllRemoved => {
					let account = Self::fund_account_id(index);
					T::Currency::transfer(&account, &fund.owner, fund.deposit, AllowDeath)?;

					// Remove all other balance from the account into orphaned funds.
					let (imbalance, _) = T::Currency::slash(&account, BalanceOf::<T>::max_value());
					T::OrphanedFunds::on_unbalanced(imbalance);

					<Funds<T>>::remove(index);

					Self::deposit_event(RawEvent::Dissolved(index));
				},
				child::KillOutcome::SomeRemaining => {
					Self::deposit_event(RawEvent::PartiallyDissolved(index));
				}
			}
		}

		fn on_initialize(n: T::BlockNumber) -> frame_support::weights::Weight {
			if let Some(n) = <slots::Module<T>>::is_ending(n) {
				let auction_index = <slots::Module<T>>::auction_counter();
				if n.is_zero() {
					// first block of ending period.
					EndingsCount::mutate(|c| *c += 1);
				}
				for (fund, index) in NewRaise::take().into_iter().filter_map(|i| Self::funds(i).map(|f| (f, i))) {
					let bidder = slots::Bidder::New(slots::NewBidder {
						who: Self::fund_account_id(index),
						/// FundIndex and slots::SubId happen to be the same type (u32). If this
						/// ever changes, then some sort of conversion will be needed here.
						sub: index,
					});

					// Care needs to be taken by the crowdloan creator that this function will succeed given
					// the crowdloaning configuration. We do some checks ahead of time in crowdloan `create`.
					let result = <slots::Module<T>>::handle_bid(
						bidder,
						auction_index,
						fund.first_slot,
						fund.last_slot,
						fund.raised,
					);

					Self::deposit_event(RawEvent::HandleBidResult(index, result));
				}
			}

			0
		}
	}
}

impl<T: Config> Module<T> {
	/// The account ID of the fund pot.
	///
	/// This actually does computation. If you need to keep using it, then make sure you cache the
	/// value and only call this once.
	pub fn fund_account_id(index: FundIndex) -> T::AccountId {
		T::ModuleId::get().into_sub_account(index)
	}

	pub fn id_from_index(index: FundIndex) -> child::ChildInfo {
		let mut buf = Vec::new();
		buf.extend_from_slice(b"crowdloan");
		buf.extend_from_slice(&index.to_le_bytes()[..]);
		child::ChildInfo::new_default(T::Hashing::hash(&buf[..]).as_ref())
	}

	pub fn contribution_put(index: FundIndex, who: &T::AccountId, balance: &BalanceOf<T>) {
		who.using_encoded(|b| child::put(&Self::id_from_index(index), b, balance));
	}

	pub fn contribution_get(index: FundIndex, who: &T::AccountId) -> BalanceOf<T> {
		who.using_encoded(|b| child::get_or_default::<BalanceOf<T>>(
			&Self::id_from_index(index),
			b,
		))
	}

	pub fn contribution_kill(index: FundIndex, who: &T::AccountId) {
		who.using_encoded(|b| child::kill(&Self::id_from_index(index), b));
	}

	pub fn crowdloan_kill(index: FundIndex) -> child::KillOutcome {
		child::kill_storage(&Self::id_from_index(index), Some(T::RemoveKeysLimit::get()))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::{collections::HashMap, cell::RefCell};
	use frame_support::{
		assert_ok, assert_noop, parameter_types,
		traits::{OnInitialize, OnFinalize},
	};
	use sp_core::H256;
	use primitives::v1::{Id as ParaId, ValidationCode};
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are requried.
	use sp_runtime::{
		Permill, testing::Header,
		traits::{BlakeTwo256, IdentityLookup},
	};
	use crate::slots::{self, Registrar};
	use crate::crowdloan;

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
			Treasury: pallet_treasury::{Module, Call, Storage, Config, Event<T>},
			Slots: slots::{Module, Call, Storage, Event<T>},
			Crowdloan: crowdloan::{Module, Call, Storage, Event<T>},
	 		RandomnessCollectiveFlip: pallet_randomness_collective_flip::{Module, Call, Storage},
		}
	);

	parameter_types! {
		pub const BlockHashCount: u32 = 250;
	}

	impl frame_system::Config for Test {
		type BaseCallFilter = ();
		type BlockWeights = ();
		type BlockLength = ();
		type DbWeight = ();
		type Origin = Origin;
		type Call = Call;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = Event;
		type BlockHashCount = BlockHashCount;
		type Version = ();
		type PalletInfo = PalletInfo;
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
		type Event = Event;
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type AccountStore = System;
		type MaxLocks = ();
		type WeightInfo = ();
	}

	parameter_types! {
		pub const ProposalBond: Permill = Permill::from_percent(5);
		pub const ProposalBondMinimum: u64 = 1;
		pub const SpendPeriod: u64 = 2;
		pub const Burn: Permill = Permill::from_percent(50);
		pub const TreasuryModuleId: ModuleId = ModuleId(*b"py/trsry");
	}
	impl pallet_treasury::Config for Test {
		type Currency = pallet_balances::Module<Test>;
		type ApproveOrigin = frame_system::EnsureRoot<u64>;
		type RejectOrigin = frame_system::EnsureRoot<u64>;
		type Event = Event;
		type OnSlash = ();
		type ProposalBond = ProposalBond;
		type ProposalBondMinimum = ProposalBondMinimum;
		type SpendPeriod = SpendPeriod;
		type Burn = Burn;
		type BurnDestination = ();
		type ModuleId = TreasuryModuleId;
		type SpendFunds = ();
		type WeightInfo = ();
	}

	thread_local! {
		pub static PARACHAIN_COUNT: RefCell<u32> = RefCell::new(0);
		pub static PARACHAINS:
			RefCell<HashMap<u32, (ValidationCode, HeadData)>> = RefCell::new(HashMap::new());
	}

	const MAX_CODE_SIZE: u32 = 100;
	const MAX_HEAD_DATA_SIZE: u32 = 10;

	pub struct TestParachains;
	impl Registrar<u64> for TestParachains {
		fn new_id() -> ParaId {
			PARACHAIN_COUNT.with(|p| {
				*p.borrow_mut() += 1;
				(*p.borrow() - 1).into()
			})
		}

		fn head_data_size_allowed(head_data_size: u32) -> bool {
			head_data_size <= MAX_HEAD_DATA_SIZE
		}

		fn code_size_allowed(code_size: u32) -> bool {
			code_size <= MAX_CODE_SIZE
		}

		fn register_para(
			id: ParaId,
			_parachain: bool,
			code: ValidationCode,
			initial_head_data: HeadData,
		) -> DispatchResult {
			PARACHAINS.with(|p| {
				if p.borrow().contains_key(&id.into()) {
					panic!("ID already exists")
				}
				p.borrow_mut().insert(id.into(), (code, initial_head_data));
				Ok(())
			})
		}

		fn deregister_para(id: ParaId) -> DispatchResult {
			PARACHAINS.with(|p| {
				if !p.borrow().contains_key(&id.into()) {
					panic!("ID doesn't exist")
				}
				p.borrow_mut().remove(&id.into());
				Ok(())
			})
		}
	}

	parameter_types!{
		pub const LeasePeriod: u64 = 10;
		pub const EndingPeriod: u64 = 3;
	}
	impl slots::Config for Test {
		type Event = Event;
		type Currency = Balances;
		type Parachains = TestParachains;
		type LeasePeriod = LeasePeriod;
		type EndingPeriod = EndingPeriod;
		type Randomness = RandomnessCollectiveFlip;
	}
	parameter_types! {
		pub const SubmissionDeposit: u64 = 1;
		pub const MinContribution: u64 = 10;
		pub const RetirementPeriod: u64 = 5;
		pub const CrowdloanModuleId: ModuleId = ModuleId(*b"py/cfund");
		pub const RemoveKeysLimit: u32 = 10;
	}
	impl Config for Test {
		type Event = Event;
		type SubmissionDeposit = SubmissionDeposit;
		type MinContribution = MinContribution;
		type RetirementPeriod = RetirementPeriod;
		type OrphanedFunds = Treasury;
		type ModuleId = CrowdloanModuleId;
		type RemoveKeysLimit = RemoveKeysLimit;
	}

	use pallet_balances::Error as BalancesError;
	use slots::Error as SlotsError;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		pallet_balances::GenesisConfig::<Test>{
			balances: vec![(1, 1000), (2, 2000), (3, 3000), (4, 4000)],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	fn run_to_block(n: u64) {
		while System::block_number() < n {
			Crowdloan::on_finalize(System::block_number());
			Treasury::on_finalize(System::block_number());
			Slots::on_finalize(System::block_number());
			Balances::on_finalize(System::block_number());
			System::on_finalize(System::block_number());
			System::set_block_number(System::block_number() + 1);
			System::on_initialize(System::block_number());
			Balances::on_initialize(System::block_number());
			Slots::on_initialize(System::block_number());
			Treasury::on_initialize(System::block_number());
			Crowdloan::on_initialize(System::block_number());
		}
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(System::block_number(), 0);
			assert_eq!(Crowdloan::fund_count(), 0);
			assert_eq!(Crowdloan::funds(0), None);
			let empty: Vec<FundIndex> = Vec::new();
			assert_eq!(Crowdloan::new_raise(), empty);
			assert_eq!(Crowdloan::contribution_get(0, &1), 0);
			assert_eq!(Crowdloan::endings_count(), 0);
		});
	}

	#[test]
	fn create_works() {
		new_test_ext().execute_with(|| {
			// Now try to create a crowdloan campaign
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Crowdloan::fund_count(), 1);
			// This is what the initial `fund_info` should look like
			let fund_info = FundInfo {
				parachain: None,
				owner: 1,
				deposit: 1,
				raised: 0,
				// 5 blocks length + 3 block ending period + 1 starting block
				end: 9,
				cap: 1000,
				last_contribution: LastContribution::Never,
				first_slot: 1,
				last_slot: 4,
				deploy_data: None,
			};
			assert_eq!(Crowdloan::funds(0), Some(fund_info));
			// User has deposit removed from their free balance
			assert_eq!(Balances::free_balance(1), 999);
			// Deposit is placed in crowdloan free balance
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(0)), 1);
			// No new raise until first contribution
			let empty: Vec<FundIndex> = Vec::new();
			assert_eq!(Crowdloan::new_raise(), empty);
		});
	}

	#[test]
	fn create_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Cannot create a crowdloan with bad slots
			assert_noop!(
				Crowdloan::create(Origin::signed(1), 1000, 4, 1, 9),
				Error::<Test>::LastSlotBeforeFirstSlot
			);
			assert_noop!(
				Crowdloan::create(Origin::signed(1), 1000, 1, 5, 9),
				Error::<Test>::LastSlotTooFarInFuture
			);

			// Cannot create a crowdloan without some deposit funds
			assert_noop!(
				Crowdloan::create(Origin::signed(1337), 1000, 1, 3, 9),
				BalancesError::<Test, _>::InsufficientBalance
			);
		});
	}

	#[test]
	fn contribute_works() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(0)), 1);

			// No contributions yet
			assert_eq!(Crowdloan::contribution_get(0, &1), 0);

			// User 1 contributes to their own crowdloan
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 0, 49));
			// User 1 has spent some funds to do this, transfer fees **are** taken
			assert_eq!(Balances::free_balance(1), 950);
			// Contributions are stored in the trie
			assert_eq!(Crowdloan::contribution_get(0, &1), 49);
			// Contributions appear in free balance of crowdloan
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(0)), 50);
			// Crowdloan is added to NewRaise
			assert_eq!(Crowdloan::new_raise(), vec![0]);

			let fund = Crowdloan::funds(0).unwrap();

			// Last contribution time recorded
			assert_eq!(fund.last_contribution, LastContribution::PreEnding(0));
			assert_eq!(fund.raised, 49);
		});
	}

	#[test]
	fn contribute_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Cannot contribute to non-existing fund
			assert_noop!(Crowdloan::contribute(Origin::signed(1), 0, 49), Error::<Test>::InvalidFundIndex);
			// Cannot contribute below minimum contribution
			assert_noop!(Crowdloan::contribute(Origin::signed(1), 0, 9), Error::<Test>::ContributionTooSmall);

			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 0, 101));

			// Cannot contribute past the limit
			assert_noop!(Crowdloan::contribute(Origin::signed(2), 0, 900), Error::<Test>::CapExceeded);

			// Move past end date
			run_to_block(10);

			// Cannot contribute to ended fund
			assert_noop!(Crowdloan::contribute(Origin::signed(1), 0, 49), Error::<Test>::ContributionPeriodOver);
		});
	}

	#[test]
	fn fix_deploy_data_works() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Add deploy data
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into()
			));

			let fund = Crowdloan::funds(0).unwrap();

			// Confirm deploy data is stored correctly
			assert_eq!(
				fund.deploy_data,
				Some(DeployData {
					code_hash: <Test as frame_system::Config>::Hash::default(),
					code_size: 0,
					initial_head_data: vec![0].into(),
				}),
			);
		});
	}

	#[test]
	fn fix_deploy_data_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Cannot set deploy data by non-owner
			assert_noop!(Crowdloan::fix_deploy_data(
				Origin::signed(2),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into()),
				Error::<Test>::InvalidOrigin
			);

			// Cannot set deploy data to an invalid index
			assert_noop!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				1,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into()),
				Error::<Test>::InvalidFundIndex
			);

			// Cannot set deploy data after it already has been set
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));

			assert_noop!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![1].into()),
				Error::<Test>::ExistingDeployData
			);
		});
	}

	#[test]
	fn onboard_works() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Add deploy data
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));

			// Fund crowdloan
			assert_ok!(Crowdloan::contribute(Origin::signed(2), 0, 1000));

			run_to_block(10);

			// Endings count incremented
			assert_eq!(Crowdloan::endings_count(), 1);

			// Onboard crowdloan
			assert_ok!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()));

			let fund = Crowdloan::funds(0).unwrap();
			// Crowdloan is now assigned a parachain id
			assert_eq!(fund.parachain, Some(0.into()));
			// This parachain is managed by Slots
			assert_eq!(Slots::managed_ids(), vec![0.into()]);
		});
	}

	#[test]
	fn onboard_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Fund crowdloan
			assert_ok!(Crowdloan::contribute(Origin::signed(2), 0, 1000));

			run_to_block(10);

			// Cannot onboard invalid fund index
			assert_noop!(Crowdloan::onboard(Origin::signed(1), 1, 0.into()), Error::<Test>::InvalidFundIndex);
			// Cannot onboard crowdloan without deploy data
			assert_noop!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()), Error::<Test>::UnsetDeployData);

			// Add deploy data
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));

			// Cannot onboard fund with incorrect parachain id
			assert_noop!(Crowdloan::onboard(Origin::signed(1), 0, 1.into()), SlotsError::<Test>::ParaNotOnboarding);

			// Onboard crowdloan
			assert_ok!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()));

			// Cannot onboard fund again
			assert_noop!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()), Error::<Test>::AlreadyOnboard);
		});
	}

	#[test]
	fn begin_retirement_works() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Add deploy data
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));

			// Fund crowdloan
			assert_ok!(Crowdloan::contribute(Origin::signed(2), 0, 1000));

			run_to_block(10);

			// Onboard crowdloan
			assert_ok!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()));
			// Fund is assigned a parachain id
			let fund = Crowdloan::funds(0).unwrap();
			assert_eq!(fund.parachain, Some(0.into()));

			// Off-boarding is set to the crowdloan account
			assert_eq!(Slots::offboarding(ParaId::from(0)), Crowdloan::fund_account_id(0));

			run_to_block(50);

			// Retire crowdloan to remove parachain id
			assert_ok!(Crowdloan::begin_retirement(Origin::signed(1), 0));

			// Fund should no longer have parachain id
			let fund = Crowdloan::funds(0).unwrap();
			assert_eq!(fund.parachain, None);

		});
	}

	#[test]
	fn begin_retirement_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Add deploy data
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));

			// Fund crowdloan
			assert_ok!(Crowdloan::contribute(Origin::signed(2), 0, 1000));

			run_to_block(10);

			// Cannot retire fund that is not onboarded
			assert_noop!(Crowdloan::begin_retirement(Origin::signed(1), 0), Error::<Test>::NotParachain);

			// Onboard crowdloan
			assert_ok!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()));
			// Fund is assigned a parachain id
			let fund = Crowdloan::funds(0).unwrap();
			assert_eq!(fund.parachain, Some(0.into()));

			// Cannot retire fund whose deposit has not been returned
			assert_noop!(Crowdloan::begin_retirement(Origin::signed(1), 0), Error::<Test>::ParaHasDeposit);

			run_to_block(50);

			// Cannot retire invalid fund index
			assert_noop!(Crowdloan::begin_retirement(Origin::signed(1), 1), Error::<Test>::InvalidFundIndex);

			// Cannot retire twice
			assert_ok!(Crowdloan::begin_retirement(Origin::signed(1), 0));
			assert_noop!(Crowdloan::begin_retirement(Origin::signed(1), 0), Error::<Test>::NotParachain);
		});
	}

	#[test]
	fn withdraw_works() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			// Transfer fee is taken here
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 0, 100));
			assert_ok!(Crowdloan::contribute(Origin::signed(2), 0, 200));
			assert_ok!(Crowdloan::contribute(Origin::signed(3), 0, 300));

			// Skip all the way to the end
			run_to_block(50);

			// Anyone can trigger withdraw of a user's balance without fees
			assert_ok!(Crowdloan::withdraw(Origin::signed(1337), 1, 0));
			assert_eq!(Balances::free_balance(1), 999);

			assert_ok!(Crowdloan::withdraw(Origin::signed(1337), 2, 0));
			assert_eq!(Balances::free_balance(2), 2000);

			assert_ok!(Crowdloan::withdraw(Origin::signed(1337), 3, 0));
			assert_eq!(Balances::free_balance(3), 3000);
		});
	}

	#[test]
	fn withdraw_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			// Transfer fee is taken here
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 0, 49));
			assert_eq!(Balances::free_balance(1), 950);

			run_to_block(5);

			// Cannot withdraw before fund ends
			assert_noop!(Crowdloan::withdraw(Origin::signed(1337), 1, 0), Error::<Test>::FundNotEnded);

			run_to_block(10);

			// Cannot withdraw if they did not contribute
			assert_noop!(Crowdloan::withdraw(Origin::signed(1337), 2, 0), Error::<Test>::NoContributions);
			// Cannot withdraw from a non-existent fund
			assert_noop!(Crowdloan::withdraw(Origin::signed(1337), 2, 1), Error::<Test>::InvalidFundIndex);
		});
	}

	#[test]
	fn dissolve_works() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			// Transfer fee is taken here
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 0, 100));
			assert_ok!(Crowdloan::contribute(Origin::signed(2), 0, 200));
			assert_ok!(Crowdloan::contribute(Origin::signed(3), 0, 300));

			// Skip all the way to the end
			run_to_block(50);

			// Check initiator's balance.
			assert_eq!(Balances::free_balance(1), 899);
			// Check current funds (contributions + deposit)
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(0)), 601);

			// Dissolve the crowdloan
			assert_ok!(Crowdloan::dissolve(Origin::signed(1), 0));

			// Fund account is emptied
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(0)), 0);
			// Deposit is returned
			assert_eq!(Balances::free_balance(1), 900);
			// Treasury account is filled
			assert_eq!(Balances::free_balance(Treasury::account_id()), 600);

			// Storage trie is removed
			assert_eq!(Crowdloan::contribution_get(0,&0), 0);
			// Fund storage is removed
			assert_eq!(Crowdloan::funds(0), None);

		});
	}

	#[test]
	fn partial_dissolve_works() {
		let mut ext = new_test_ext();
		ext.execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 100_000, 1, 4, 9));

			// Add lots of contributors, beyond what we can delete in one go.
			for i in 0 .. 30 {
				Balances::make_free_balance_be(&i, 300);
				assert_ok!(Crowdloan::contribute(Origin::signed(i), 0, 100));
				assert_eq!(Crowdloan::contribution_get(0, &i), 100);
			}

			// Skip all the way to the end
			run_to_block(50);

			// Check current funds (contributions + deposit)
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(0)), 100 * 30 + 1);
		});

		ext.commit_all().unwrap();
		ext.execute_with(|| {
			// Partially dissolve the crowdloan
			assert_ok!(Crowdloan::dissolve(Origin::signed(1), 0));
			for i in 0 .. 10 {
				assert_eq!(Crowdloan::contribution_get(0, &i), 0);
			}
			for i in 10 .. 30 {
				assert_eq!(Crowdloan::contribution_get(0, &i), 100);
			}
		});

		ext.commit_all().unwrap();
		ext.execute_with(|| {
			// Partially dissolve the crowdloan, again
			assert_ok!(Crowdloan::dissolve(Origin::signed(1), 0));
			for i in 0 .. 20 {
				assert_eq!(Crowdloan::contribution_get(0, &i), 0);
			}
			for i in 20 .. 30 {
				assert_eq!(Crowdloan::contribution_get(0, &i), 100);
			}
		});

		ext.commit_all().unwrap();
		ext.execute_with(|| {
			// Fully dissolve the crowdloan
			assert_ok!(Crowdloan::dissolve(Origin::signed(1), 0));
			for i in 0 .. 30 {
				assert_eq!(Crowdloan::contribution_get(0, &i), 0);
			}

			// Fund account is emptied
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(0)), 0);
			// Deposit is returned
			assert_eq!(Balances::free_balance(1), 201);
			// Treasury account is filled
			assert_eq!(Balances::free_balance(Treasury::account_id()), 100 * 30);
			// Fund storage is removed
			assert_eq!(Crowdloan::funds(0), None);
		});
	}

	#[test]
	fn dissolve_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Set up a crowdloan
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			// Transfer fee is taken here
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 0, 100));
			assert_ok!(Crowdloan::contribute(Origin::signed(2), 0, 200));
			assert_ok!(Crowdloan::contribute(Origin::signed(3), 0, 300));

			// Cannot dissolve an invalid fund index
			assert_noop!(Crowdloan::dissolve(Origin::signed(1), 1), Error::<Test>::InvalidFundIndex);
			// Cannot dissolve a fund in progress
			assert_noop!(Crowdloan::dissolve(Origin::signed(1), 0), Error::<Test>::InRetirementPeriod);

			run_to_block(10);

			// Onboard fund
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));
			assert_ok!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()));

			// Cannot dissolve an active fund
			assert_noop!(Crowdloan::dissolve(Origin::signed(1), 0), Error::<Test>::HasActiveParachain);
		});
	}

	#[test]
	fn fund_before_auction_works() {
		new_test_ext().execute_with(|| {
			// Create a crowdloan before an auction is created
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 9));
			// Users can already contribute
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 0, 49));
			// Fund added to NewRaise
			assert_eq!(Crowdloan::new_raise(), vec![0]);

			// Some blocks later...
			run_to_block(2);
			// Create an auction
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			// Add deploy data
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));
			// Move to the end of auction...
			run_to_block(12);

			// Endings count incremented
			assert_eq!(Crowdloan::endings_count(), 1);

			// Onboard crowdloan
			assert_ok!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()));

			let fund = Crowdloan::funds(0).unwrap();
			// Crowdloan is now assigned a parachain id
			assert_eq!(fund.parachain, Some(0.into()));
			// This parachain is managed by Slots
			assert_eq!(Slots::managed_ids(), vec![0.into()]);
		});
	}

	#[test]
	fn fund_across_multiple_auctions_works() {
		new_test_ext().execute_with(|| {
			// Create an auction
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			// Create two competing crowdloans, with end dates across multiple auctions
			// Each crowdloan is competing for the same slots, so only one can win
			assert_ok!(Crowdloan::create(Origin::signed(1), 1000, 1, 4, 30));
			assert_ok!(Crowdloan::create(Origin::signed(2), 1000, 1, 4, 30));

			// Contribute to all, but more money to 0, less to 1
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 0, 300));
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 1, 200));

			// Add deploy data to all
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));
			assert_ok!(Crowdloan::fix_deploy_data(
				Origin::signed(2),
				1,
				<Test as frame_system::Config>::Hash::default(),
				0,
				vec![0].into(),
			));

			// End the current auction, fund 0 wins!
			run_to_block(10);
			assert_eq!(Crowdloan::endings_count(), 1);
			// Onboard crowdloan
			assert_ok!(Crowdloan::onboard(Origin::signed(1), 0, 0.into()));
			let fund = Crowdloan::funds(0).unwrap();
			// Crowdloan is now assigned a parachain id
			assert_eq!(fund.parachain, Some(0.into()));
			// This parachain is managed by Slots
			assert_eq!(Slots::managed_ids(), vec![0.into()]);

			// Create a second auction
			assert_ok!(Slots::new_auction(Origin::root(), 5, 1));
			// Contribute to existing funds add to NewRaise
			assert_ok!(Crowdloan::contribute(Origin::signed(1), 1, 10));

			// End the current auction, fund 1 wins!
			run_to_block(20);
			assert_eq!(Crowdloan::endings_count(), 2);
			// Onboard crowdloan
			assert_ok!(Crowdloan::onboard(Origin::signed(2), 1, 1.into()));
			let fund = Crowdloan::funds(1).unwrap();
			// Crowdloan is now assigned a parachain id
			assert_eq!(fund.parachain, Some(1.into()));
			// This parachain is managed by Slots
			assert_eq!(Slots::managed_ids(), vec![0.into(), 1.into()]);
		});
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

	// TODO: replace with T::Parachains::MAX_CODE_SIZE
	const MAX_CODE_SIZE: u32 = 10;
	const MAX_HEAD_DATA_SIZE: u32 = 10;

	fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
		let events = frame_system::Module::<T>::events();
		let system_event: <T as frame_system::Config>::Event = generic_event.into();
		// compare to the last event record
		let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
		assert_eq!(event, &system_event);
	}

	fn create_fund<T: Config>(end: T::BlockNumber) -> FundIndex {
		let cap = BalanceOf::<T>::max_value();
		let lease_period_index = end / T::LeasePeriod::get();
		let first_slot = lease_period_index;
		let last_slot = lease_period_index + 3u32.into();

		let caller = account("fund_creator", 0, 0);
		T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());

		assert_ok!(Crowdloan::<T>::create(RawOrigin::Signed(caller).into(), cap, first_slot, last_slot, end));
		FundCount::get() - 1
	}

	fn contribute_fund<T: Config>(who: &T::AccountId, index: FundIndex) {
		T::Currency::make_free_balance_be(&who, BalanceOf::<T>::max_value());
		let value = T::MinContribution::get();
		assert_ok!(Crowdloan::<T>::contribute(RawOrigin::Signed(who.clone()).into(), index, value));
	}

	fn worst_validation_code<T: Config>() -> Vec<u8> {
		// TODO: replace with T::Parachains::MAX_CODE_SIZE
		let mut validation_code = vec![0u8; MAX_CODE_SIZE as usize];
		// Replace first bytes of code with "WASM_MAGIC" to pass validation test.
		let _ = validation_code.splice(
			..crate::WASM_MAGIC.len(),
			crate::WASM_MAGIC.iter().cloned(),
		).collect::<Vec<_>>();
		validation_code
	}

	fn worst_deploy_data<T: Config>() -> DeployData<T::Hash> {
		let validation_code = worst_validation_code::<T>();
		let code = primitives::v1::ValidationCode(validation_code);
		// TODO: replace with T::Parachains::MAX_HEAD_DATA_SIZE
		let head_data = HeadData(vec![0u8; MAX_HEAD_DATA_SIZE as usize]);

		DeployData {
			code_hash: T::Hashing::hash(&code.0),
			// TODO: replace with T::Parachains::MAX_CODE_SIZE
			code_size: MAX_CODE_SIZE,
			initial_head_data: head_data,
		}
	}

	fn setup_onboarding<T: Config>(
		fund_index: FundIndex,
		para_id: ParaId,
		end_block: T::BlockNumber,
	) -> DispatchResult {
		// Matches fund creator in `create_fund`
		let fund_creator = account("fund_creator", 0, 0);
		let DeployData { code_hash, code_size, initial_head_data } = worst_deploy_data::<T>();
		Crowdloan::<T>::fix_deploy_data(
			RawOrigin::Signed(fund_creator).into(),
			fund_index,
			code_hash,
			code_size,
			initial_head_data
		)?;

		let lease_period_index = end_block / T::LeasePeriod::get();
		Slots::<T>::new_auction(RawOrigin::Root.into(), end_block, lease_period_index)?;
		let contributor: T::AccountId = account("contributor", 0, 0);
		contribute_fund::<T>(&contributor, fund_index);

		// TODO: Probably should use on_initialize
		//Slots::<T>::on_initialize(end_block + T::EndingPeriod::get());
		let onboarding_data = (lease_period_index, crate::slots::IncomingParachain::Unset(
			crate::slots::NewBidder {
				who: Crowdloan::<T>::fund_account_id(fund_index),
				sub: Default::default(),
			}
		));
		crate::slots::Onboarding::<T>::insert(para_id, onboarding_data);
		Ok(())
	}

	benchmarks! {
		create {
			let cap = BalanceOf::<T>::max_value();
			let first_slot = 0u32.into();
			let last_slot = 3u32.into();
			let end = T::BlockNumber::max_value();

			let caller: T::AccountId = whitelisted_caller();

			T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		}: _(RawOrigin::Signed(caller), cap, first_slot, last_slot, end)
		verify {
			assert_last_event::<T>(RawEvent::Created(FundCount::get() - 1).into())
		}

		// Contribute has two arms: PreEnding and Ending, but both are equal complexity.
		contribute {
			let fund_index = create_fund::<T>(100u32.into());
			let caller: T::AccountId = whitelisted_caller();
			let contribution = T::MinContribution::get();
			T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		}: _(RawOrigin::Signed(caller.clone()), fund_index, contribution)
		verify {
			// NewRaise is appended to, so we don't need to fill it up for worst case scenario.
			assert!(!NewRaise::get().is_empty());
			assert_last_event::<T>(RawEvent::Contributed(caller, fund_index, contribution).into());
		}

		fix_deploy_data {
			let fund_index = create_fund::<T>(100u32.into());
			// Matches fund creator in `create_fund`
			let caller = account("fund_creator", 0, 0);

			let DeployData { code_hash, code_size, initial_head_data } = worst_deploy_data::<T>();

			whitelist_account!(caller);
		}: _(RawOrigin::Signed(caller), fund_index, code_hash, code_size, initial_head_data)
		verify {
			assert_last_event::<T>(RawEvent::DeployDataFixed(fund_index).into());
		}

		onboard {
			let end_block: T::BlockNumber = 100u32.into();
			let fund_index = create_fund::<T>(end_block);
			let para_id = Default::default();

			setup_onboarding::<T>(fund_index, para_id, end_block)?;

			let caller = whitelisted_caller();
		}: _(RawOrigin::Signed(caller), fund_index, para_id)
		verify {
			assert_last_event::<T>(RawEvent::Onboarded(fund_index, para_id).into());
		}

		begin_retirement {
			let end_block: T::BlockNumber = 100u32.into();
			let fund_index = create_fund::<T>(end_block);
			let para_id = Default::default();

			setup_onboarding::<T>(fund_index, para_id, end_block)?;

			let caller: T::AccountId = whitelisted_caller();
			Crowdloan::<T>::onboard(RawOrigin::Signed(caller.clone()).into(), fund_index, para_id)?;

			// Remove deposits to look like it is off-boarded
			crate::slots::Deposits::<T>::remove(para_id);
		}: _(RawOrigin::Signed(caller), fund_index)
		verify {
			assert_last_event::<T>(RawEvent::Retiring(fund_index).into());
		}

		withdraw {
			let fund_index = create_fund::<T>(100u32.into());
			let caller: T::AccountId = whitelisted_caller();
			let contributor = account("contributor", 0, 0);
			contribute_fund::<T>(&contributor, fund_index);
			frame_system::Module::<T>::set_block_number(200u32.into());
		}: _(RawOrigin::Signed(caller), contributor.clone(), fund_index)
		verify {
			assert_last_event::<T>(RawEvent::Withdrew(contributor, fund_index, T::MinContribution::get()).into());
		}

		// Worst case: Dissolve removes `RemoveKeysLimit` keys, and then finishes up the dissolution of the fund.
		dissolve {
			let fund_index = create_fund::<T>(100u32.into());

			// Dissolve will remove at most `RemoveKeysLimit` at once.
			for i in 0 .. T::RemoveKeysLimit::get() {
				contribute_fund::<T>(&account("contributor", i, 0), fund_index);
			}

			let caller: T::AccountId = whitelisted_caller();
			frame_system::Module::<T>::set_block_number(T::RetirementPeriod::get().saturating_add(200u32.into()));
		}: _(RawOrigin::Signed(caller.clone()), fund_index)
		verify {
			assert_last_event::<T>(RawEvent::Dissolved(fund_index).into());
		}

		// Worst case scenario: N funds are all in the `NewRaise` list, we are
		// in the beginning of the ending period, and each fund outbids the next
		// over the same slot.
		on_initialize {
			// We test the complexity over different number of new raise
			let n in 2 .. 100;
			let end_block: T::BlockNumber = 100u32.into();

			for i in 0 .. n {
				let fund_index = create_fund::<T>(end_block);
				let contributor: T::AccountId = account("contributor", i, 0);
				let contribution = T::MinContribution::get() * (i + 1).into();
				T::Currency::make_free_balance_be(&contributor, BalanceOf::<T>::max_value());
				Crowdloan::<T>::contribute(RawOrigin::Signed(contributor).into(), fund_index, contribution)?;
			}

			let lease_period_index = end_block / T::LeasePeriod::get();
			Slots::<T>::new_auction(RawOrigin::Root.into(), end_block, lease_period_index)?;

			assert_eq!(<slots::Module<T>>::is_ending(end_block), Some(0u32.into()));
			assert_eq!(NewRaise::get().len(), n as usize);
			let old_endings_count = EndingsCount::get();
		}: {
			Crowdloan::<T>::on_initialize(end_block);
		} verify {
			assert_eq!(EndingsCount::get(), old_endings_count + 1);
			assert_last_event::<T>(RawEvent::HandleBidResult((n - 1).into(), Ok(())).into());
		}
	}

	#[cfg(test)]
	mod tests {
		use super::*;
		use crate::crowdloan::tests::{new_test_ext, Test};

		#[test]
		fn test_benchmarks() {
			new_test_ext().execute_with(|| {
				assert_ok!(test_benchmark_create::<Test>());
				assert_ok!(test_benchmark_contribute::<Test>());
				assert_ok!(test_benchmark_fix_deploy_data::<Test>());
				assert_ok!(test_benchmark_onboard::<Test>());
				assert_ok!(test_benchmark_begin_retirement::<Test>());
				assert_ok!(test_benchmark_withdraw::<Test>());
				assert_ok!(test_benchmark_dissolve::<Test>());
				assert_ok!(test_benchmark_on_initialize::<Test>());
			});
		}
	}
}
