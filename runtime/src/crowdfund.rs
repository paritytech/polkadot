// Copyright 2017-2019 Parity Technologies (UK) Ltd.
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

//! # Parachain Crowdfunding module
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

use srml_support::{
	decl_module, decl_storage, decl_event, storage::child, ensure,
	traits::{Currency, Get, OnUnbalanced, WithdrawReason, ExistenceRequirement}
};
use system::ensure_signed;
use sr_primitives::{ModuleId, weights::SimpleDispatchInfo,
	traits::{AccountIdConversion, Hash, Saturating, Zero, CheckedAdd}
};
use crate::slots;
use codec::{Encode, Decode};
use rstd::vec::Vec;
use substrate_primitives::storage::well_known_keys::CHILD_STORAGE_KEY_PREFIX;
use primitives::parachain::Id as ParaId;

const MODULE_ID: ModuleId = ModuleId(*b"py/cfund");

pub type BalanceOf<T> =
	<<T as slots::Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;
#[allow(dead_code)]
pub type NegativeImbalanceOf<T> =
	<<T as slots::Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::NegativeImbalance;

pub trait Trait: slots::Trait {
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;

	/// The amount to be held on deposit by the owner of a crowdfund.
	type SubmissionDeposit: Get<BalanceOf<Self>>;

	/// The minimum amount that may be contributed into a crowdfund. Should almost certainly be at
	/// least ExistentialDeposit.
	type MinContribution: Get<BalanceOf<Self>>;

	/// The period of time (in blocks) after an unsuccessful crowdfund ending when
	/// contributors are able to withdraw their funds. After this period, their funds are lost.
	type RetirementPeriod: Get<Self::BlockNumber>;

	/// What to do with funds that were not withdrawn.
	type OrphanedFunds: OnUnbalanced<NegativeImbalanceOf<Self>>;
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
	/// is the code hash, second is the initial head data.
	deploy_data: Option<(Hash, Vec<u8>)>,
}

decl_storage! {
	trait Store for Module<T: Trait> as Example {
		/// Info on all of the funds.
		Funds get(funds):
			map FundIndex => Option<FundInfo<T::AccountId, BalanceOf<T>, T::Hash, T::BlockNumber>>;

		/// The total number of funds that have so far been allocated.
		FundCount get(fund_count): FundIndex;

		/// The funds that have had additional contributions during the last block. This is used
		/// in order to determine which funds should submit new or updated bids.
		NewRaise get(new_raise): Vec<FundIndex>;

		/// The number of auctions that have entered into their ending period so far.
		EndingsCount get(endings_count): slots::AuctionIndex;
	}
}

decl_event! {
	pub enum Event<T> where
		<T as system::Trait>::AccountId,
		Balance = BalanceOf<T>,
	{
		Created(FundIndex),
		Contributed(AccountId, FundIndex, Balance),
		Withdrew(AccountId, FundIndex, Balance),
		Retiring(FundIndex),
		Dissolved(FundIndex),
		DeployDataFixed(FundIndex),
		Onboarded(FundIndex, ParaId),
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		fn deposit_event() = default;

		/// Create a new crowdfunding campaign for a parachain slot deposit for the current auction.
		#[weight = SimpleDispatchInfo::FixedNormal(100_000)]
		fn create(origin,
			#[compact] cap: BalanceOf<T>,
			#[compact] first_slot: T::BlockNumber,
			#[compact] last_slot: T::BlockNumber,
			#[compact] end: T::BlockNumber
		) {
			let owner = ensure_signed(origin)?;

			ensure!(first_slot < last_slot, "last slot must be greater than first slot");
			ensure!(last_slot <= first_slot + 3.into(), "last slot cannot be more then 3 more than first slot");
			ensure!(end > <system::Module<T>>::block_number(), "end must be in the future");

			let deposit = T::SubmissionDeposit::get();
			let imb = T::Currency::withdraw(
				&owner,
				deposit,
				WithdrawReason::Transfer,
				ExistenceRequirement::AllowDeath,
			)?;

			let index = FundCount::get();
			let next_index = index.checked_add(1).ok_or("overflow when adding fund")?;
			FundCount::put(next_index);

			// No fees are paid here if we need to create this account; that's why we don't just
			// use the stock `transfer`.
			T::Currency::resolve_creating(&Self::fund_account_id(index), imb);

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
		/// slot is
		fn contribute(origin, #[compact] index: FundIndex, #[compact] value: BalanceOf<T>) {
			let who = ensure_signed(origin)?;

			ensure!(value >= T::MinContribution::get(), "contribution too small");
			let mut fund = Self::funds(index).ok_or("invalid fund index")?;
			fund.raised  = fund.raised.checked_add(&value).ok_or("overflow when adding new funds")?;
			ensure!(fund.raised <= fund.cap, "contributions exceed cap");

			// Make sure crowdfund has not ended
			let now = <system::Module<T>>::block_number();
			ensure!(fund.end > now, "contribution period ended");

			T::Currency::transfer(&who, &Self::fund_account_id(index), value)?;

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
						NewRaise::mutate(|v| v.push(index));
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
						NewRaise::mutate(|v| v.push(index));
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
		fn fix_deploy_data(origin,
			#[compact] index: FundIndex,
			code_hash: T::Hash,
			initial_head_data: Vec<u8>
		) {
			let who = ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or("invalid fund index")?;
			ensure!(fund.owner == who, "origin must be fund owner");
			ensure!(fund.deploy_data.is_none(), "deploy data already set");

			fund.deploy_data = Some((code_hash, initial_head_data));

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::DeployDataFixed(index));
		}

		/// Complete onboarding process for a winning parachain fund. This can be called once by
		/// any origin once a fund wins a slot and the fund has set its deploy data (using
		/// `fix_deploy_data`).
		///
		/// - `index` is the fund index that `origin` owns and whose deploy data will be set.
		/// - `para_id` is the parachain index that this fund won.
		fn onboard(origin,
			#[compact] index: FundIndex,
			#[compact] para_id: ParaId
		) {
			let _ = ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or("invalid fund index")?;
			let (code_hash, initial_head_data) = fund.clone().deploy_data.ok_or("deploy data not fixed")?;
			ensure!(fund.parachain.is_none(), "fund already onboarded");
			fund.parachain = Some(para_id);

			let fund_origin = system::RawOrigin::Signed(Self::fund_account_id(index)).into();
			<slots::Module<T>>::fix_deploy_data(fund_origin, index, para_id, code_hash, initial_head_data)?;

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::Onboarded(index, para_id));
		}

		/// Note that a successful fund has lost its parachain slot, and place it into retirement.
		fn begin_retirement(origin, #[compact] index: FundIndex) {
			let _ = ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or("invalid fund index")?;
			let parachain_id = fund.parachain.take().ok_or("fund has no parachain")?;
			// No deposit information implies the parachain was off-boarded
			ensure!(<slots::Module<T>>::deposits(parachain_id).len() == 0, "parachain still has deposit");
			let account = Self::fund_account_id(index);
			// Funds should be returned at the end of off-boarding
			ensure!(T::Currency::free_balance(&account) >= fund.raised, "funds not yet returned");

			// This fund just ended. Withdrawal period begins.
			let now = <system::Module<T>>::block_number();
			fund.end = now;

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::Retiring(index));
		}

		/// Withdraw full balance of a contributor to an unsuccessful or off-boarded fund.
		fn withdraw(origin, #[compact] index: FundIndex) {
			let who = ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or("invalid fund index")?;
			ensure!(fund.parachain.is_none(), "fund has not retired");
			let now = <system::Module<T>>::block_number();

			// `fund.end` can represent the end of a failed crowdsale or the beginning of retirement
			ensure!(now >= fund.end, "fund has not ended");

			let balance = Self::contribution_get(index, &who);
			ensure!(balance > Zero::zero(), "no contributions stored");

			// Avoid using transfer to ensure we don't pay any fees.
			let _ = T::Currency::resolve_into_existing(&who, T::Currency::withdraw(
				&Self::fund_account_id(index),
				balance,
				WithdrawReason::Transfer,
				ExistenceRequirement::AllowDeath
			)?);

			Self::contribution_kill(index, &who);
			fund.raised = fund.raised.saturating_sub(balance);

			<Funds<T>>::insert(index, &fund);

			Self::deposit_event(RawEvent::Withdrew(who, index, balance));
		}

		/// Remove a fund after either: it was unsuccessful and it timed out; or it was successful
		/// but it has been retired from its parachain slot. This places any deposits that were not
		/// withdrawn into the treasury.
		fn dissolve(origin, #[compact] index: FundIndex) {
			let _ = ensure_signed(origin)?;

			let fund = Self::funds(index).ok_or("invalid fund index")?;
			ensure!(fund.parachain.is_none(), "cannot dissolve fund with active parachain");
			let now = <system::Module<T>>::block_number();
			ensure!(now >= fund.end + T::RetirementPeriod::get(), "retirement period not over");

			let account = Self::fund_account_id(index);

			// Avoid using transfer to ensure we don't pay any fees.
			let _ = T::Currency::resolve_into_existing(&fund.owner, T::Currency::withdraw(
				&account,
				fund.deposit,
				WithdrawReason::Transfer,
				ExistenceRequirement::AllowDeath
			)?);

			T::OrphanedFunds::on_unbalanced(T::Currency::withdraw(
				&account,
				fund.raised,
				WithdrawReason::Transfer,
				ExistenceRequirement::AllowDeath
			)?);

			Self::crowdfund_kill(index);
			<Funds<T>>::remove(index);

			Self::deposit_event(RawEvent::Dissolved(index));
		}

		fn on_finalize(n: T::BlockNumber) {
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

					// Care needs to be taken by the crowdfund creator that this function will succeed given
					// the crowdfunding configuration. We do some checks ahead of time in crowdfund `create`.
					let _ = <slots::Module<T>>::handle_bid(
						bidder,
						auction_index,
						fund.first_slot,
						fund.last_slot,
						fund.raised,
					);
				}
			}
		}
	}
}

impl<T: Trait> Module<T> {
	/// The account ID of the fund pot.
	///
	/// This actually does computation. If you need to keep using it, then make sure you cache the
	/// value and only call this once.
	pub fn fund_account_id(index: FundIndex) -> T::AccountId {
		MODULE_ID.into_sub_account(index)
	}

	pub fn id_from_index(index: FundIndex) -> Vec<u8> {
		let mut buf = Vec::new();
		buf.extend_from_slice(b"crowdfund");
		buf.extend_from_slice(&index.to_le_bytes()[..]);

		CHILD_STORAGE_KEY_PREFIX.into_iter()
			.chain(b"default:")
			.chain(T::Hashing::hash(&buf[..]).as_ref().into_iter())
			.cloned()
			.collect()
	}

	pub fn contribution_put(index: FundIndex, who: &T::AccountId, balance: &BalanceOf<T>) {
		let id = Self::id_from_index(index);
		who.using_encoded(|b| child::put(id.as_ref(), b, balance));
	}

	pub fn contribution_get(index: FundIndex, who: &T::AccountId) -> BalanceOf<T> {
		let id = Self::id_from_index(index);
		who.using_encoded(|b| child::get_or_default::<BalanceOf<T>>(id.as_ref(), b))
	}

	pub fn contribution_kill(index: FundIndex, who: &T::AccountId) {
		let id = Self::id_from_index(index);
		who.using_encoded(|b| child::kill(id.as_ref(), b));
	}

	pub fn crowdfund_kill(index: FundIndex) {
		let id = Self::id_from_index(index);
		child::kill_storage(id.as_ref());
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::{collections::HashMap, cell::RefCell};
	use srml_support::{impl_outer_origin, assert_ok, assert_noop, parameter_types};
	use sr_io::with_externalities;
	use substrate_primitives::{H256, Blake2Hasher};
	use primitives::parachain::Info as ParaInfo;
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are requried.
	use sr_primitives::{
		Perbill, Permill, testing::Header,
		traits::{BlakeTwo256, OnInitialize, OnFinalize, IdentityLookup, ConvertInto},
	};
	use crate::registrar::Registrar;

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
		type WeightMultiplierUpdate = ();
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
	}
	parameter_types! {
		pub const ExistentialDeposit: u64 = 0;
		// We want to make sure these fees are non zero, so we can check
		// that our module correctly avoids these fees :)
		pub const TransferFee: u64 = 10;
		pub const CreationFee: u64 = 10;
		pub const TransactionBaseFee: u64 = 0;
		pub const TransactionByteFee: u64 = 0;
	}
	impl balances::Trait for Test {
		type Balance = u64;
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

	parameter_types! {
		pub const ProposalBond: Permill = Permill::from_percent(5);
		pub const ProposalBondMinimum: u64 = 1;
		pub const SpendPeriod: u64 = 2;
		pub const Burn: Permill = Permill::from_percent(50);
	}
	impl treasury::Trait for Test {
		type Currency = balances::Module<Test>;
		type ApproveOrigin = system::EnsureRoot<u64>;
		type RejectOrigin = system::EnsureRoot<u64>;
		type Event = ();
		type MintedForSpending = ();
		type ProposalRejection = ();
		type ProposalBond = ProposalBond;
		type ProposalBondMinimum = ProposalBondMinimum;
		type SpendPeriod = SpendPeriod;
		type Burn = Burn;
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
				if p.borrow().contains_key(&id.into_inner()) {
					panic!("ID already exists")
				}
				p.borrow_mut().insert(id.into_inner(), (code, initial_head_data));
				Ok(())
			})
		}
		fn deregister_para(id: ParaId) -> Result<(), &'static str> {
			PARACHAINS.with(|p| {
				if !p.borrow().contains_key(&id.into_inner()) {
					panic!("ID doesn't exist")
				}
				p.borrow_mut().remove(&id.into_inner());
				Ok(())
			})
		}
	}

	parameter_types!{
		pub const LeasePeriod: u64 = 10;
		pub const EndingPeriod: u64 = 3;
	}
	impl slots::Trait for Test {
		type Event = ();
		type Currency = Balances;
		type Parachains = TestParachains;
		type LeasePeriod = LeasePeriod;
		type EndingPeriod = EndingPeriod;
	}
	parameter_types! {
		pub const SubmissionDeposit: u64 = 1;
		pub const MinContribution: u64 = 10;
		pub const RetirementPeriod: u64 = 5;
	}
	impl Trait for Test {
		type Event = ();
		type SubmissionDeposit = SubmissionDeposit;
		type MinContribution = MinContribution;
		type RetirementPeriod = RetirementPeriod;
		type OrphanedFunds = Treasury;
	}

	type System = system::Module<Test>;
	type Balances = balances::Module<Test>;
	type Slots = slots::Module<Test>;
	type Treasury = treasury::Module<Test>;
	type Crowdfund = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> sr_io::TestExternalities<Blake2Hasher> {
		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
		balances::GenesisConfig::<Test>{
			balances: vec![(1, 1000), (2, 2000), (3, 3000), (4, 4000)],
			vesting: vec![],
		}.assimilate_storage(&mut t).unwrap();
		t.into()
	}

	fn run_to_block(n: u64) {
		while System::block_number() < n {
			Crowdfund::on_finalize(System::block_number());
			Treasury::on_finalize(System::block_number());
			Slots::on_finalize(System::block_number());
			Balances::on_finalize(System::block_number());
			System::on_finalize(System::block_number());
			System::set_block_number(System::block_number() + 1);
			System::on_initialize(System::block_number());
			Balances::on_initialize(System::block_number());
			Slots::on_initialize(System::block_number());
			Treasury::on_finalize(System::block_number());
			Crowdfund::on_initialize(System::block_number());
		}
	}

	#[test]
	fn basic_setup_works() {
		with_externalities(&mut new_test_ext(), || {
			assert_eq!(System::block_number(), 1);
			assert_eq!(Crowdfund::fund_count(), 0);
			assert_eq!(Crowdfund::funds(0), None);
			let empty: Vec<FundIndex> = Vec::new();
			assert_eq!(Crowdfund::new_raise(), empty);
			assert_eq!(Crowdfund::contribution_get(0, &1), 0);
			assert_eq!(Crowdfund::endings_count(), 0);
		});
	}

	#[test]
	fn create_works() {
		with_externalities(&mut new_test_ext(), || {
			// Now try to create a crowdfund campaign
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Crowdfund::fund_count(), 1);
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
			assert_eq!(Crowdfund::funds(0), Some(fund_info));
			// User has deposit removed from their free balance
			assert_eq!(Balances::free_balance(1), 999);
			// Deposit is placed in crowdfund free balance
			assert_eq!(Balances::free_balance(Crowdfund::fund_account_id(0)), 1);
			// No new raise until first contribution
			let empty: Vec<FundIndex> = Vec::new();
			assert_eq!(Crowdfund::new_raise(), empty);
		});
	}

	#[test]
	fn create_handles_basic_errors() {
		with_externalities(&mut new_test_ext(), || {
			// Cannot create a crowdfund with bad slots
			assert_noop!(Crowdfund::create(Origin::signed(1), 1000, 4, 1, 9), "last slot must be greater than first slot");
			assert_noop!(Crowdfund::create(Origin::signed(1), 1000, 1, 5, 9), "last slot cannot be more then 3 more than first slot");

			// Cannot create a crowdfund without some deposit funds
			assert_noop!(Crowdfund::create(Origin::signed(1337), 1000, 1, 3, 9), "too few free funds in account");
		});
	}

	#[test]
	fn contribute_works() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);
			assert_eq!(Balances::free_balance(Crowdfund::fund_account_id(0)), 1);

			// No contributions yet
			assert_eq!(Crowdfund::contribution_get(0, &1), 0);

			// User 1 contributes to their own crowdfund
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 0, 49));
			// User 1 has spent some funds to do this, transfer fees **are** taken
			assert_eq!(Balances::free_balance(1), 940);
			// Contributions are stored in the trie
			assert_eq!(Crowdfund::contribution_get(0, &1), 49);
			// Contributions appear in free balance of crowdfund
			assert_eq!(Balances::free_balance(Crowdfund::fund_account_id(0)), 50);
			// Crowdfund is added to NewRaise
			assert_eq!(Crowdfund::new_raise(), vec![0]);

			let fund = Crowdfund::funds(0).unwrap();

			// Last contribution time recorded
			assert_eq!(fund.last_contribution, LastContribution::PreEnding(0));
			assert_eq!(fund.raised, 49);
		});
	}

	#[test]
	fn contribute_handles_basic_errors() {
		with_externalities(&mut new_test_ext(), || {
			// Cannot contribute to non-existing fund
			assert_noop!(Crowdfund::contribute(Origin::signed(1), 0, 49), "invalid fund index");
			// Cannot contribute below minimum contribution
			assert_noop!(Crowdfund::contribute(Origin::signed(1), 0, 9), "contribution too small");

			// Set up a crowdfund
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 0, 101));

			// Cannot contribute past the limit
			assert_noop!(Crowdfund::contribute(Origin::signed(2), 0, 900), "contributions exceed cap");

			// Move past end date
			run_to_block(10);

			// Cannot contribute to ended fund
			assert_noop!(Crowdfund::contribute(Origin::signed(1), 0, 49), "contribution period ended");
		});
	}

	#[test]
	fn fix_deploy_data_works() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Add deploy data
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));

			let fund = Crowdfund::funds(0).unwrap();

			// Confirm deploy data is stored correctly
			assert_eq!(fund.deploy_data, Some((<Test as system::Trait>::Hash::default(), vec![0])));
		});
	}

	#[test]
	fn fix_deploy_data_handles_basic_errors() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Cannot set deploy data by non-owner
			assert_noop!(Crowdfund::fix_deploy_data(
				Origin::signed(2),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]),
				"origin must be fund owner"
			);

			// Cannot set deploy data to an invalid index
			assert_noop!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				1,
				<Test as system::Trait>::Hash::default(),
				vec![0]),
				"invalid fund index"
			);

			// Cannot set deploy data after it already has been set
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));

			assert_noop!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![1]),
				"deploy data already set"
			);
		});
	}

	#[test]
	fn onboard_works() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Add deploy data
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));

			// Fund crowdfund
			assert_ok!(Crowdfund::contribute(Origin::signed(2), 0, 1000));

			run_to_block(10);

			// Endings count incremented
			assert_eq!(Crowdfund::endings_count(), 1);

			// Onboard crowdfund
			assert_ok!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()));

			let fund = Crowdfund::funds(0).unwrap();
			// Crowdfund is now assigned a parachain id
			assert_eq!(fund.parachain, Some(0.into()));
			// This parachain is managed by Slots
			assert_eq!(Slots::managed_ids(), vec![0.into()]);
		});
	}

	#[test]
	fn onboard_handles_basic_errors() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Fund crowdfund
			assert_ok!(Crowdfund::contribute(Origin::signed(2), 0, 1000));

			run_to_block(10);

			// Cannot onboard invalid fund index
			assert_noop!(Crowdfund::onboard(Origin::signed(1), 1, 0.into()), "invalid fund index");
			// Cannot onboard crowdfund without deploy data
			assert_noop!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()), "deploy data not fixed");

			// Add deploy data
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));

			// Cannot onboard fund with incorrect parachain id
			assert_noop!(Crowdfund::onboard(Origin::signed(1), 0, 1.into()), "parachain id not in onboarding");

			// Onboard crowdfund
			assert_ok!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()));

			// Cannot onboard fund again
			assert_noop!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()), "fund already onboarded");
		});
	}

	#[test]
	fn begin_retirement_works() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Add deploy data
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));

			// Fund crowdfund
			assert_ok!(Crowdfund::contribute(Origin::signed(2), 0, 1000));

			run_to_block(10);

			// Onboard crowdfund
			assert_ok!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()));
			// Fund is assigned a parachain id
			let fund = Crowdfund::funds(0).unwrap();
			assert_eq!(fund.parachain, Some(0.into()));

			// Off-boarding is set to the crowdfund account
			assert_eq!(Slots::offboarding(ParaId::from(0)), Crowdfund::fund_account_id(0));

			run_to_block(50);

			// Retire crowdfund to remove parachain id
			assert_ok!(Crowdfund::begin_retirement(Origin::signed(1), 0));

			// Fund should no longer have parachain id
			let fund = Crowdfund::funds(0).unwrap();
			assert_eq!(fund.parachain, None);

		});
	}

	#[test]
	fn begin_retirement_handles_basic_errors() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			assert_eq!(Balances::free_balance(1), 999);

			// Add deploy data
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));

			// Fund crowdfund
			assert_ok!(Crowdfund::contribute(Origin::signed(2), 0, 1000));

			run_to_block(10);

			// Cannot retire fund that is not onboarded
			assert_noop!(Crowdfund::begin_retirement(Origin::signed(1), 0), "fund has no parachain");

			// Onboard crowdfund
			assert_ok!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()));
			// Fund is assigned a parachain id
			let fund = Crowdfund::funds(0).unwrap();
			assert_eq!(fund.parachain, Some(0.into()));

			// Cannot retire fund whose deposit has not been returned
			assert_noop!(Crowdfund::begin_retirement(Origin::signed(1), 0), "parachain still has deposit");

			run_to_block(50);

			// Cannot retire invalid fund index
			assert_noop!(Crowdfund::begin_retirement(Origin::signed(1), 1), "invalid fund index");

			// Cannot retire twice
			assert_ok!(Crowdfund::begin_retirement(Origin::signed(1), 0));
			assert_noop!(Crowdfund::begin_retirement(Origin::signed(1), 0), "fund has no parachain");
		});
	}

	#[test]
	fn withdraw_works() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			// Transfer fee is taken here
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 0, 100));
			assert_ok!(Crowdfund::contribute(Origin::signed(2), 0, 200));
			assert_ok!(Crowdfund::contribute(Origin::signed(3), 0, 300));

			// Skip all the way to the end
			run_to_block(50);

			// User can withdraw their full balance without fees
			assert_ok!(Crowdfund::withdraw(Origin::signed(1), 0));
			assert_eq!(Balances::free_balance(1), 989);

			assert_ok!(Crowdfund::withdraw(Origin::signed(2), 0));
			assert_eq!(Balances::free_balance(2), 1990);

			assert_ok!(Crowdfund::withdraw(Origin::signed(3), 0));
			assert_eq!(Balances::free_balance(3), 2990);
		});
	}

	#[test]
	fn withdraw_handles_basic_errors() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			// Transfer fee is taken here
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 0, 49));
			assert_eq!(Balances::free_balance(1), 940);

			run_to_block(5);

			// Cannot withdraw before fund ends
			assert_noop!(Crowdfund::withdraw(Origin::signed(1), 0), "fund has not ended");

			run_to_block(10);

			// Cannot withdraw if they did not contribute
			assert_noop!(Crowdfund::withdraw(Origin::signed(2), 0), "no contributions stored");
			// Cannot withdraw from a non-existent fund
			assert_noop!(Crowdfund::withdraw(Origin::signed(1), 1), "invalid fund index");
		});
	}

	#[test]
	fn dissolve_works() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			// Transfer fee is taken here
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 0, 100));
			assert_ok!(Crowdfund::contribute(Origin::signed(2), 0, 200));
			assert_ok!(Crowdfund::contribute(Origin::signed(3), 0, 300));

			// Skip all the way to the end
			run_to_block(50);

			// Check current funds (contributions + deposit)
			assert_eq!(Balances::free_balance(Crowdfund::fund_account_id(0)), 601);

			// Dissolve the crowdfund
			assert_ok!(Crowdfund::dissolve(Origin::signed(1), 0));

			// Fund account is emptied
			assert_eq!(Balances::free_balance(Crowdfund::fund_account_id(0)), 0);
			// Deposit is returned
			assert_eq!(Balances::free_balance(1), 890);
			// Treasury account is filled
			assert_eq!(Balances::free_balance(Treasury::account_id()), 600);

			// Storage trie is removed
			assert_eq!(Crowdfund::contribution_get(0,&0), 0);
			// Fund storage is removed
			assert_eq!(Crowdfund::funds(0), None);

		});
	}

	#[test]
	fn dissolve_handles_basic_errors() {
		with_externalities(&mut new_test_ext(), || {
			// Set up a crowdfund
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			// Transfer fee is taken here
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 0, 100));
			assert_ok!(Crowdfund::contribute(Origin::signed(2), 0, 200));
			assert_ok!(Crowdfund::contribute(Origin::signed(3), 0, 300));

			// Cannot dissolve an invalid fund index
			assert_noop!(Crowdfund::dissolve(Origin::signed(1), 1), "invalid fund index");
			// Cannot dissolve a fund in progress
			assert_noop!(Crowdfund::dissolve(Origin::signed(1), 0), "retirement period not over");

			run_to_block(10);

			// Onboard fund
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));
			assert_ok!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()));

			// Cannot dissolve an active fund
			assert_noop!(Crowdfund::dissolve(Origin::signed(1), 0), "cannot dissolve fund with active parachain");
		});
	}

	#[test]
	fn fund_before_auction_works() {
		with_externalities(&mut new_test_ext(), || {
			// Create a crowdfund before an auction is created
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 9));
			// Users can already contribute
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 0, 49));
			// Fund added to NewRaise
			assert_eq!(Crowdfund::new_raise(), vec![0]);

			// Some blocks later...
			run_to_block(2);
			// Create an auction
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			// Add deploy data
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));
			// Move to the end of auction...
			run_to_block(12);

			// Endings count incremented
			assert_eq!(Crowdfund::endings_count(), 1);

			// Onboard crowdfund
			assert_ok!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()));

			let fund = Crowdfund::funds(0).unwrap();
			// Crowdfund is now assigned a parachain id
			assert_eq!(fund.parachain, Some(0.into()));
			// This parachain is managed by Slots
			assert_eq!(Slots::managed_ids(), vec![0.into()]);
		});
	}

	#[test]
	fn fund_across_multiple_auctions_works() {
		with_externalities(&mut new_test_ext(), || {
			// Create an auction
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			// Create two competing crowdfunds, with end dates across multiple auctions
			// Each crowdfund is competing for the same slots, so only one can win
			assert_ok!(Crowdfund::create(Origin::signed(1), 1000, 1, 4, 30));
			assert_ok!(Crowdfund::create(Origin::signed(2), 1000, 1, 4, 30));

			// Contribute to all, but more money to 0, less to 1
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 0, 300));
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 1, 200));

			// Add deploy data to all
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(1),
				0,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));
			assert_ok!(Crowdfund::fix_deploy_data(
				Origin::signed(2),
				1,
				<Test as system::Trait>::Hash::default(),
				vec![0]
			));

			// End the current auction, fund 0 wins!
			run_to_block(10);
			assert_eq!(Crowdfund::endings_count(), 1);
			// Onboard crowdfund
			assert_ok!(Crowdfund::onboard(Origin::signed(1), 0, 0.into()));
			let fund = Crowdfund::funds(0).unwrap();
			// Crowdfund is now assigned a parachain id
			assert_eq!(fund.parachain, Some(0.into()));
			// This parachain is managed by Slots
			assert_eq!(Slots::managed_ids(), vec![0.into()]);

			// Create a second auction
			assert_ok!(Slots::new_auction(Origin::ROOT, 5, 1));
			// Contribute to existing funds add to NewRaise
			assert_ok!(Crowdfund::contribute(Origin::signed(1), 1, 10));

			// End the current auction, fund 1 wins!
			run_to_block(20);
			assert_eq!(Crowdfund::endings_count(), 2);
			// Onboard crowdfund
			assert_ok!(Crowdfund::onboard(Origin::signed(2), 1, 1.into()));
			let fund = Crowdfund::funds(1).unwrap();
			// Crowdfund is now assigned a parachain id
			assert_eq!(fund.parachain, Some(1.into()));
			// This parachain is managed by Slots
			assert_eq!(Slots::managed_ids(), vec![0.into(), 1.into()]);
		});
	}
}
