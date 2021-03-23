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
		Currency, ReservableCurrency, Get, OnUnbalanced, ExistenceRequirement::AllowDeath
	},
	pallet_prelude::{Weight, DispatchResultWithPostInfo},
};
use frame_system::ensure_signed;
use sp_runtime::{
	ModuleId, DispatchResult, RuntimeDebug, MultiSignature, MultiSigner,
	traits::{
		AccountIdConversion, Hash, Saturating, Zero, CheckedAdd, Bounded, Verify, IdentifyAccount,
	},
};
use crate::traits::{Registrar, Auctioneer};
use parity_scale_codec::{Encode, Decode};
use sp_std::vec::Vec;
use primitives::v1::Id as ParaId;

type CurrencyOf<T> = <<T as Config>::Auctioneer as Auctioneer>::Currency;
type LeasePeriodOf<T> = <<T as Config>::Auctioneer as Auctioneer>::LeasePeriod;
type BalanceOf<T> = <CurrencyOf<T> as Currency<<T as frame_system::Config>::AccountId>>::Balance;

#[allow(dead_code)]
type NegativeImbalanceOf<T> = <CurrencyOf<T> as Currency<<T as frame_system::Config>::AccountId>>::NegativeImbalance;

type TrieIndex = u32;

pub trait WeightInfo {
	fn create() -> Weight;
	fn contribute() -> Weight;
	fn withdraw() -> Weight;
	fn dissolve(k: u32, ) -> Weight;
	fn on_initialize(n: u32, ) -> Weight;
}

pub struct TestWeightInfo;
impl WeightInfo for TestWeightInfo {
	fn create() -> Weight { 0 }
	fn contribute() -> Weight { 0 }
	fn withdraw() -> Weight { 0 }
	fn dissolve(_k: u32, ) -> Weight { 0 }
	fn on_initialize(_n: u32, ) -> Weight { 0 }
}

pub trait Config: frame_system::Config {
	type Event: From<Event<Self>> + Into<<Self as frame_system::Config>::Event>;

	/// ModuleID for the crowdloan module. An appropriate value could be ```ModuleId(*b"py/cfund")```
	type ModuleId: Get<ModuleId>;

	/// The amount to be held on deposit by the depositor of a crowdloan.
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

	/// The parachain registrar type. We jus use this to ensure that only the manager of a para is able to
	/// start a crowdloan for its slot.
	type Registrar: Registrar<AccountId=Self::AccountId>;

	/// The type representing the auctioning system.
	type Auctioneer: Auctioneer<
		AccountId=Self::AccountId,
		BlockNumber=Self::BlockNumber,
		LeasePeriod=Self::BlockNumber,
	>;

	/// Weight Information for the Extrinsics in the Pallet
	type WeightInfo: WeightInfo;
}

#[derive(Encode, Decode, Copy, Clone, PartialEq, Eq, RuntimeDebug)]
pub enum LastContribution<BlockNumber> {
	Never,
	PreEnding(u32),
	Ending(BlockNumber),
}

/// Information on a funding effort for a pre-existing parachain. We assume that the parachain ID
/// is known as it's used for the key of the storage item for which this is the value (`Funds`).
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug)]
#[codec(dumb_trait_bound)]
pub struct FundInfo<AccountId, Balance, BlockNumber, LeasePeriod> {
	/// True if the fund is being retired. This can only be set once and only when the current
	/// lease period is greater than the `last_slot`.
	retiring: bool,
	/// The owning account who placed the deposit.
	depositor: AccountId,
	/// An optional verifier. If exists, contributions must be signed by verifier.
	verifier: Option<MultiSigner>,
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
	first_slot: LeasePeriod,
	/// Last slot in range to bid on; it's actually a LeasePeriod, but that's the same type as
	/// BlockNumber.
	last_slot: LeasePeriod,
	/// Index used for the child trie of this fund
	trie_index: TrieIndex,
}

decl_storage! {
	trait Store for Module<T: Config> as Crowdloan {
		/// Info on all of the funds.
		Funds get(fn funds):
			map hasher(twox_64_concat) ParaId
			=> Option<FundInfo<T::AccountId, BalanceOf<T>, T::BlockNumber, LeasePeriodOf<T>>>;

		/// The funds that have had additional contributions during the last block. This is used
		/// in order to determine which funds should submit new or updated bids.
		NewRaise get(fn new_raise): Vec<ParaId>;

		/// The number of auctions that have entered into their ending period so far.
		EndingsCount get(fn endings_count): u32;

		/// Tracker for the next available trie index
		NextTrieIndex get(fn next_trie_index): u32;
	}
}

decl_event! {
	pub enum Event<T> where
		<T as frame_system::Config>::AccountId,
		Balance = BalanceOf<T>,
	{
		/// Create a new crowdloaning campaign. [fund_index]
		Created(ParaId),
		/// Contributed to a crowd sale. [who, fund_index, amount]
		Contributed(AccountId, ParaId, Balance),
		/// Withdrew full balance of a contributor. [who, fund_index, amount]
		Withdrew(AccountId, ParaId, Balance),
		/// Fund is placed into retirement. [fund_index]
		Retiring(ParaId),
		/// Fund is partially dissolved, i.e. there are some left over child
		/// keys that still need to be killed. [fund_index]
		PartiallyDissolved(ParaId),
		/// Fund is dissolved. [fund_index]
		Dissolved(ParaId),
		/// The deploy data of the funded parachain is set. [fund_index]
		DeployDataFixed(ParaId),
		/// On-boarding process for a winning parachain fund is completed. [find_index, parachain_id]
		Onboarded(ParaId, ParaId),
		/// The result of trying to submit a new bid to the Slots pallet.
		HandleBidResult(ParaId, DispatchResult),
	}
}

decl_error! {
	pub enum Error for Module<T: Config> {
		/// The first slot needs to at least be less than 3 `max_value`.
		FirstSlotTooFarInFuture,
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
		InvalidParaId,
		/// Contributions exceed maximum amount.
		CapExceeded,
		/// The contribution period has already ended.
		ContributionPeriodOver,
		/// The origin of this call is invalid.
		InvalidOrigin,
		/// This crowdloan does not correspond to a parachain.
		NotParachain,
		/// This parachain lease is still active and retirement cannot yet begin.
		LeaseActive,
		/// This parachain's bid or lease is still active and withdraw cannot yet begin.
		BidOrLeaseActive,
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
		/// The crowdloan is not ready to dissolve. Potentially still has a slot or in retirement period.
		NotReadyToDissolve,
		/// Invalid signature.
		InvalidSignature,
	}
}

decl_module! {
	pub struct Module<T: Config> for enum Call where origin: <T as frame_system::Config>::Origin {
		type Error = Error<T>;

		const ModuleId: ModuleId = T::ModuleId::get();
		const MinContribution: BalanceOf<T> = T::MinContribution::get();
		const RemoveKeysLimit: u32 = T::RemoveKeysLimit::get();
		const RetirementPeriod: T::BlockNumber = T::RetirementPeriod::get();

		fn deposit_event() = default;

		/// Create a new crowdloaning campaign for a parachain slot deposit for the current auction.
		#[weight = T::WeightInfo::create()]
		pub fn create(origin,
			#[compact] index: ParaId,
			#[compact] cap: BalanceOf<T>,
			#[compact] first_slot: LeasePeriodOf<T>,
			#[compact] last_slot: LeasePeriodOf<T>,
			#[compact] end: T::BlockNumber,
			verifier: Option<MultiSigner>,
		) {
			let depositor = ensure_signed(origin)?;

			ensure!(first_slot <= last_slot, Error::<T>::LastSlotBeforeFirstSlot);
			let last_slot_limit = first_slot.checked_add(&3u32.into()).ok_or(Error::<T>::FirstSlotTooFarInFuture)?;
			ensure!(last_slot <= last_slot_limit, Error::<T>::LastSlotTooFarInFuture);
			ensure!(end > <frame_system::Pallet<T>>::block_number(), Error::<T>::CannotEndInPast);

			// There should not be an existing fund.
			ensure!(!Funds::<T>::contains_key(index), Error::<T>::FundNotEnded);

			let manager = T::Registrar::manager_of(index).ok_or(Error::<T>::InvalidParaId)?;
			ensure!(depositor == manager, Error::<T>::InvalidOrigin);

			let trie_index = Self::next_trie_index();
			let new_trie_index = trie_index.checked_add(1).ok_or(Error::<T>::Overflow)?;

			let deposit = T::SubmissionDeposit::get();

			CurrencyOf::<T>::reserve(&depositor, deposit)?;

			Funds::<T>::insert(index, FundInfo {
				retiring: false,
				depositor,
				verifier,
				deposit,
				raised: Zero::zero(),
				end,
				cap,
				last_contribution: LastContribution::Never,
				first_slot,
				last_slot,
				trie_index,
			});

			NextTrieIndex::put(new_trie_index);

			Self::deposit_event(RawEvent::Created(index));
		}

		/// Contribute to a crowd sale. This will transfer some balance over to fund a parachain
		/// slot. It will be withdrawable in two instances: the parachain becomes retired; or the
		/// slot is unable to be purchased and the timeout expires.
		#[weight = T::WeightInfo::contribute()]
		pub fn contribute(origin,
			#[compact] index: ParaId,
			#[compact] value: BalanceOf<T>,
			signature: Option<MultiSignature>
		) {
			let who = ensure_signed(origin)?;

			ensure!(value >= T::MinContribution::get(), Error::<T>::ContributionTooSmall);
			let mut fund = Self::funds(index).ok_or(Error::<T>::InvalidParaId)?;
			fund.raised  = fund.raised.checked_add(&value).ok_or(Error::<T>::Overflow)?;
			ensure!(fund.raised <= fund.cap, Error::<T>::CapExceeded);

			// Make sure crowdloan has not ended
			let now = <frame_system::Pallet<T>>::block_number();
			ensure!(now < fund.end, Error::<T>::ContributionPeriodOver);

			let old_balance = Self::contribution_get(fund.trie_index, &who);

			if let Some(ref verifier) = fund.verifier {
				let signature = signature.ok_or(Error::<T>::InvalidSignature)?;
				let payload = (index, &who, old_balance, value);
				let valid = payload.using_encoded(|encoded| signature.verify(encoded, &verifier.clone().into_account()));
				ensure!(valid, Error::<T>::InvalidSignature);
			}

			CurrencyOf::<T>::transfer(&who, &Self::fund_account_id(index), value, AllowDeath)?;

			let balance = old_balance.saturating_add(value);
			Self::contribution_put(fund.trie_index, &who, &balance);

			if T::Auctioneer::is_ending(now).is_some() {
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

			Funds::<T>::insert(index, &fund);

			Self::deposit_event(RawEvent::Contributed(who, index, value));
		}

		/// Withdraw full balance of a contributor.
		///
		/// Origin must be signed.
		///
		/// The fund must be either in, or ready for, retirement. For a fund to be *in* retirement, then the retirement
		/// flag must be set. For a fund to be ready for retirement, then:
		/// - it must not already be in retirement;
		/// - the amount of raised funds must be bigger than the _free_ balance of the account;
		/// - and either:
		///   - the block number must be at least `end`; or
		///   - the current lease period must be greater than the fund's `last_slot`.
		///
		/// In this case, the fund's retirement flag is set and its `end` is reset to the current block
		/// number.
		///
		/// - `who`: The account whose contribution should be withdrawn.
		/// - `index`: The parachain to whose crowdloan the contribution was made.
		#[weight = T::WeightInfo::withdraw()]
		pub fn withdraw(origin, who: T::AccountId, #[compact] index: ParaId) {
			ensure_signed(origin)?;

			let mut fund = Self::funds(index).ok_or(Error::<T>::InvalidParaId)?;

			// `fund.end` can represent the end of a failed crowdsale or the beginning of retirement
			let now = frame_system::Pallet::<T>::block_number();
			let current_lease_period = T::Auctioneer::lease_period_index();
			ensure!(now >= fund.end || current_lease_period > fund.last_slot, Error::<T>::FundNotEnded);

			let fund_account = Self::fund_account_id(index);
			// free balance must equal amount raised, otherwise a bid or lease must be active.
			ensure!(CurrencyOf::<T>::free_balance(&fund_account) == fund.raised, Error::<T>::BidOrLeaseActive);

			let balance = Self::contribution_get(fund.trie_index, &who);
			ensure!(balance > Zero::zero(), Error::<T>::NoContributions);

			// Avoid using transfer to ensure we don't pay any fees.
			CurrencyOf::<T>::transfer(&fund_account, &who, balance, AllowDeath)?;

			Self::contribution_kill(fund.trie_index, &who);
			fund.raised = fund.raised.saturating_sub(balance);
			if !fund.retiring {
				fund.retiring = true;
				fund.end = now;
			}

			Funds::<T>::insert(index, &fund);

			Self::deposit_event(RawEvent::Withdrew(who, index, balance));
		}

		/// Remove a fund after the retirement period has ended.
		///
		/// This places any deposits that were not withdrawn into the treasury.
		#[weight = T::WeightInfo::dissolve(T::RemoveKeysLimit::get())]
		pub fn dissolve(origin, #[compact] index: ParaId) -> DispatchResultWithPostInfo {
			ensure_signed(origin)?;

			let fund = Self::funds(index).ok_or(Error::<T>::InvalidParaId)?;
			let now = frame_system::Pallet::<T>::block_number();
			let dissolution = fund.end.saturating_add(T::RetirementPeriod::get());
			ensure!((fund.retiring && now >= dissolution) || fund.raised.is_zero(), Error::<T>::NotReadyToDissolve);

			// Try killing the crowdloan child trie
			match Self::crowdloan_kill(fund.trie_index) {
				child::KillChildStorageResult::AllRemoved(num_removed) => {
					CurrencyOf::<T>::unreserve(&fund.depositor, fund.deposit);

					// Remove all other balance from the account into orphaned funds.
					let account = Self::fund_account_id(index);
					let (imbalance, _) = CurrencyOf::<T>::slash(&account, BalanceOf::<T>::max_value());
					T::OrphanedFunds::on_unbalanced(imbalance);

					Funds::<T>::remove(index);

					Self::deposit_event(RawEvent::Dissolved(index));

					Ok(Some(T::WeightInfo::dissolve(num_removed)).into())
				},
				child::KillChildStorageResult::SomeRemaining(num_removed) => {
					Self::deposit_event(RawEvent::PartiallyDissolved(index));
					Ok(Some(T::WeightInfo::dissolve(num_removed)).into())
				}
			}
		}

		fn on_initialize(n: T::BlockNumber) -> frame_support::weights::Weight {
			if let Some(n) = T::Auctioneer::is_ending(n) {
				if n.is_zero() {
					// first block of ending period.
					EndingsCount::mutate(|c| *c += 1);
				}
				let new_raise = NewRaise::take();
				let new_raise_len = new_raise.len() as u32;
				for (fund, para_id) in new_raise.into_iter().filter_map(|i| Self::funds(i).map(|f| (f, i))) {
					// Care needs to be taken by the crowdloan creator that this function will succeed given
					// the crowdloaning configuration. We do some checks ahead of time in crowdloan `create`.
					let result = T::Auctioneer::place_bid(
						Self::fund_account_id(para_id),
						para_id,
						fund.first_slot,
						fund.last_slot,
						fund.raised,
					);

					Self::deposit_event(RawEvent::HandleBidResult(para_id, result));
				}
				T::WeightInfo::on_initialize(new_raise_len)
			} else {
				T::DbWeight::get().reads(1)
			}
		}
	}
}

impl<T: Config> Module<T> {
	/// The account ID of the fund pot.
	///
	/// This actually does computation. If you need to keep using it, then make sure you cache the
	/// value and only call this once.
	pub fn fund_account_id(index: ParaId) -> T::AccountId {
		T::ModuleId::get().into_sub_account(index)
	}

	pub fn id_from_index(index: TrieIndex) -> child::ChildInfo {
		let mut buf = Vec::new();
		buf.extend_from_slice(b"crowdloan");
		buf.extend_from_slice(&index.encode()[..]);
		child::ChildInfo::new_default(T::Hashing::hash(&buf[..]).as_ref())
	}

	pub fn contribution_put(index: TrieIndex, who: &T::AccountId, balance: &BalanceOf<T>) {
		who.using_encoded(|b| child::put(&Self::id_from_index(index), b, balance));
	}

	pub fn contribution_get(index: TrieIndex, who: &T::AccountId) -> BalanceOf<T> {
		who.using_encoded(|b| child::get_or_default::<BalanceOf<T>>(
			&Self::id_from_index(index),
			b,
		))
	}

	pub fn contribution_kill(index: TrieIndex, who: &T::AccountId) {
		who.using_encoded(|b| child::kill(&Self::id_from_index(index), b));
	}

	pub fn crowdloan_kill(index: TrieIndex) -> child::KillChildStorageResult {
		child::kill_storage(&Self::id_from_index(index), Some(T::RemoveKeysLimit::get()))
	}
}

impl<T: Config> crate::traits::OnSwap for Module<T> {
	fn on_swap(one: ParaId, other: ParaId) {
		Funds::<T>::mutate(one, |x|
			Funds::<T>::mutate(other, |y|
				sp_std::mem::swap(x, y)
			)
		)
	}
}

#[cfg(any(feature = "runtime-benchmarks", test))]
mod crypto {
	use sp_core::ed25519;
	use sp_io::crypto::{ed25519_sign, ed25519_generate};
	use sp_std::{
		vec::Vec,
		convert::TryFrom,
	};
	use sp_runtime::{MultiSigner, MultiSignature};

	pub fn create_ed25519_pubkey(seed: Vec<u8>) -> MultiSigner {
		ed25519_generate(0.into(), Some(seed)).into()
	}

	pub fn create_ed25519_signature(payload: &[u8], pubkey: MultiSigner) -> MultiSignature {
		let edpubkey = ed25519::Public::try_from(pubkey).unwrap();
		let edsig = ed25519_sign(0.into(), &edpubkey, payload).unwrap();
		edsig.into()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::{cell::RefCell, sync::Arc};
	use frame_support::{
		assert_ok, assert_noop, parameter_types,
		traits::{OnInitialize, OnFinalize},
	};
	use sp_core::H256;
	use primitives::v1::Id as ParaId;
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are requried.
	use sp_runtime::{
		testing::Header, traits::{BlakeTwo256, IdentityLookup},
	};
	use crate::{
		mock::TestRegistrar,
		traits::OnSwap,
		crowdloan,
	};
	use sp_keystore::{KeystoreExt, testing::KeyStore};

	type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
	type Block = frame_system::mocking::MockBlock<Test>;

	frame_support::construct_runtime!(
		pub enum Test where
			Block = Block,
			NodeBlock = Block,
			UncheckedExtrinsic = UncheckedExtrinsic,
		{
			System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
			Balances: pallet_balances::{Pallet, Call, Storage, Config<T>, Event<T>},
			Crowdloan: crowdloan::{Pallet, Call, Storage, Event<T>},
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

	#[derive(Copy, Clone, Eq, PartialEq, Debug)]
	struct BidPlaced {
		height: u64,
		bidder: u64,
		para: ParaId,
		first_slot: u64,
		last_slot: u64,
		amount: u64
	}
	thread_local! {
		static AUCTION: RefCell<Option<(u64, u64)>> = RefCell::new(None);
		static ENDING_PERIOD: RefCell<u64> = RefCell::new(5);
		static BIDS_PLACED: RefCell<Vec<BidPlaced>> = RefCell::new(Vec::new());
	}
	#[allow(unused)]
	fn set_ending_period(ending_period: u64) {
		ENDING_PERIOD.with(|p| *p.borrow_mut() = ending_period);
	}
	fn auction() -> Option<(u64, u64)> {
		AUCTION.with(|p| p.borrow().clone())
	}
	fn ending_period() -> u64 {
		ENDING_PERIOD.with(|p| p.borrow().clone())
	}
	fn bids() -> Vec<BidPlaced> {
		BIDS_PLACED.with(|p| p.borrow().clone())
	}

	pub struct TestAuctioneer;
	impl Auctioneer for TestAuctioneer {
		type AccountId = u64;
		type BlockNumber = u64;
		type LeasePeriod = u64;
		type Currency = Balances;

		fn new_auction(duration: u64, lease_period_index: u64) -> DispatchResult {
			assert!(lease_period_index >= Self::lease_period_index());

			let ending = System::block_number().saturating_add(duration);
			AUCTION.with(|p| *p.borrow_mut() = Some((lease_period_index, ending)));
			Ok(())
		}

		fn is_ending(now: u64) -> Option<u64> {
			if let Some((_, early_end)) = auction() {
				if let Some(after_early_end) = now.checked_sub(early_end) {
					if after_early_end < ending_period() {
						return Some(after_early_end)
					}
				}
			}
			None
		}

		fn place_bid(
			bidder: u64,
			para: ParaId,
			first_slot: u64,
			last_slot: u64,
			amount: u64
		) -> DispatchResult {
			let height = System::block_number();
			BIDS_PLACED.with(|p| p.borrow_mut().push(BidPlaced { height, bidder, para, first_slot, last_slot, amount }));
			Ok(())
		}

		fn lease_period_index() -> u64 {
			System::block_number() / 20
		}
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
		type OrphanedFunds = ();
		type ModuleId = CrowdloanModuleId;
		type RemoveKeysLimit = RemoveKeysLimit;
		type Registrar = TestRegistrar<Test>;
		type Auctioneer = TestAuctioneer;
		type WeightInfo = crate::crowdloan::TestWeightInfo;
	}

	use pallet_balances::Error as BalancesError;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	pub fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = frame_system::GenesisConfig::default().build_storage::<Test>().unwrap();
		pallet_balances::GenesisConfig::<Test>{
			balances: vec![(1, 1000), (2, 2000), (3, 3000), (4, 4000)],
		}.assimilate_storage(&mut t).unwrap();
		let keystore = KeyStore::new();
		let mut t: sp_io::TestExternalities = t.into();
		t.register_extension(KeystoreExt(Arc::new(keystore)));
		t
	}

	fn new_para() -> ParaId {
		for i in 0.. {
			let para: ParaId = i.into();
			if TestRegistrar::<Test>::is_registered(para) { continue }
			assert_ok!(TestRegistrar::<Test>::register(1, para, Default::default(), Default::default()));
			return para;
		}
		unreachable!()
	}

	fn run_to_block(n: u64) {
		while System::block_number() < n {
			Crowdloan::on_finalize(System::block_number());
			Balances::on_finalize(System::block_number());
			System::on_finalize(System::block_number());
			System::set_block_number(System::block_number() + 1);
			System::on_initialize(System::block_number());
			Balances::on_initialize(System::block_number());
			Crowdloan::on_initialize(System::block_number());
		}
	}

	#[test]
	fn basic_setup_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(System::block_number(), 0);
			assert_eq!(Crowdloan::funds(ParaId::from(0)), None);
			let empty: Vec<ParaId> = Vec::new();
			assert_eq!(Crowdloan::new_raise(), empty);
			assert_eq!(Crowdloan::contribution_get(0u32, &1), 0);
			assert_eq!(Crowdloan::endings_count(), 0);

			assert_ok!(TestAuctioneer::new_auction(5, 0));

			assert_eq!(bids(), vec![]);
			assert_ok!(TestAuctioneer::place_bid(1, 2.into(), 0, 3, 6));
			let b = BidPlaced { height: 0, bidder: 1, para: 2.into(), first_slot: 0, last_slot: 3, amount: 6 };
			assert_eq!(bids(), vec![b]);
			assert_eq!(TestAuctioneer::is_ending(4), None);
			assert_eq!(TestAuctioneer::is_ending(5), Some(0));
			assert_eq!(TestAuctioneer::is_ending(9), Some(4));
			assert_eq!(TestAuctioneer::is_ending(11), None);
		});
	}

	#[test]
	fn create_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();
			// Now try to create a crowdloan campaign
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 4, 9, None));
			// This is what the initial `fund_info` should look like
			let fund_info = FundInfo {
				retiring: false,
				depositor: 1,
				verifier: None,
				deposit: 1,
				raised: 0,
				// 5 blocks length + 3 block ending period + 1 starting block
				end: 9,
				cap: 1000,
				last_contribution: LastContribution::Never,
				first_slot: 1,
				last_slot: 4,
				trie_index: 0,
			};
			assert_eq!(Crowdloan::funds(para), Some(fund_info));
			// User has deposit removed from their free balance
			assert_eq!(Balances::free_balance(1), 999);
			// Deposit is placed in reserved
			assert_eq!(Balances::reserved_balance(1), 1);
			// No new raise until first contribution
			let empty: Vec<ParaId> = Vec::new();
			assert_eq!(Crowdloan::new_raise(), empty);
		});
	}

	#[test]
	fn create_with_verifier_works() {
		new_test_ext().execute_with(|| {
			let pubkey = crypto::create_ed25519_pubkey(b"//verifier".to_vec());
			let para = new_para();
			// Now try to create a crowdloan campaign
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 4, 9, Some(pubkey.clone())));
			// This is what the initial `fund_info` should look like
			let fund_info = FundInfo {
				retiring: false,
				depositor: 1,
				verifier: Some(pubkey),
				deposit: 1,
				raised: 0,
				// 5 blocks length + 3 block ending period + 1 starting block
				end: 9,
				cap: 1000,
				last_contribution: LastContribution::Never,
				first_slot: 1,
				last_slot: 4,
				trie_index: 0,
			};
			assert_eq!(Crowdloan::funds(ParaId::from(0)), Some(fund_info));
			// User has deposit removed from their free balance
			assert_eq!(Balances::free_balance(1), 999);
			// Deposit is placed in reserved
			assert_eq!(Balances::reserved_balance(1), 1);
			// No new raise until first contribution
			let empty: Vec<ParaId> = Vec::new();
			assert_eq!(Crowdloan::new_raise(), empty);
		});
	}

	#[test]
	fn create_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			// Now try to create a crowdloan campaign
			let para = new_para();

			let e = Error::<Test>::InvalidParaId;
			assert_noop!(Crowdloan::create(Origin::signed(1), 1.into(), 1000, 1, 4, 9, None), e);
			// Cannot create a crowdloan with bad slots
			let e = Error::<Test>::LastSlotBeforeFirstSlot;
			assert_noop!(Crowdloan::create(Origin::signed(1), para, 1000, 4, 1, 9, None), e);
			let e = Error::<Test>::LastSlotTooFarInFuture;
			assert_noop!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 5, 9, None), e);

			// Cannot create a crowdloan without some deposit funds
			assert_ok!(TestRegistrar::<Test>::register(1337, ParaId::from(1234), Default::default(), Default::default()));
			let e = BalancesError::<Test, _>::InsufficientBalance;
			assert_noop!(Crowdloan::create(Origin::signed(1337), ParaId::from(1234), 1000, 1, 3, 9, None), e);
		});
	}

	#[test]
	fn contribute_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();

			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 4, 9, None));

			// No contributions yet
			assert_eq!(Crowdloan::contribution_get(u32::from(para), &1), 0);

			// User 1 contributes to their own crowdloan
			assert_ok!(Crowdloan::contribute(Origin::signed(1), para, 49, None));
			// User 1 has spent some funds to do this, transfer fees **are** taken
			assert_eq!(Balances::free_balance(1), 950);
			// Contributions are stored in the trie
			assert_eq!(Crowdloan::contribution_get(u32::from(para), &1), 49);
			// Contributions appear in free balance of crowdloan
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(para)), 49);
			// Crowdloan is added to NewRaise
			assert_eq!(Crowdloan::new_raise(), vec![para]);

			let fund = Crowdloan::funds(para).unwrap();

			// Last contribution time recorded
			assert_eq!(fund.last_contribution, LastContribution::PreEnding(0));
			assert_eq!(fund.raised, 49);
		});
	}

	#[test]
	fn contribute_with_verifier_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();
			let pubkey = crypto::create_ed25519_pubkey(b"//verifier".to_vec());
			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 4, 9, Some(pubkey.clone())));

			// No contributions yet
			assert_eq!(Crowdloan::contribution_get(u32::from(para), &1), 0);

			// Missing signature
			assert_noop!(Crowdloan::contribute(Origin::signed(1), para, 49, None), Error::<Test>::InvalidSignature);

			let payload = (0u32, 1u64, 0u64, 49u64);
			let valid_signature = crypto::create_ed25519_signature(&payload.encode(), pubkey.clone());
			let invalid_signature = MultiSignature::default();

			// Invalid signature
			assert_noop!(Crowdloan::contribute(Origin::signed(1), para, 49, Some(invalid_signature)), Error::<Test>::InvalidSignature);

			// Valid signature wrong parameter
			assert_noop!(Crowdloan::contribute(Origin::signed(1), para, 50, Some(valid_signature.clone())), Error::<Test>::InvalidSignature);
			assert_noop!(Crowdloan::contribute(Origin::signed(2), para, 49, Some(valid_signature.clone())), Error::<Test>::InvalidSignature);

			// Valid signature
			assert_ok!(Crowdloan::contribute(Origin::signed(1), para, 49, Some(valid_signature.clone())));

			// Reuse valid signature
			assert_noop!(Crowdloan::contribute(Origin::signed(1), para, 49, Some(valid_signature)), Error::<Test>::InvalidSignature);

			let payload_2 = (0u32, 1u64, 49u64, 10u64);
			let valid_signature_2 = crypto::create_ed25519_signature(&payload_2.encode(), pubkey);

			// New valid signature
			assert_ok!(Crowdloan::contribute(Origin::signed(1), para, 10, Some(valid_signature_2)));

			// Contributions appear in free balance of crowdloan
			assert_eq!(Balances::free_balance(Crowdloan::fund_account_id(para)), 59);

			// Contribution amount is correct
			let fund = Crowdloan::funds(para).unwrap();
			assert_eq!(fund.raised, 59);
		});
	}

	#[test]
	fn contribute_handles_basic_errors() {
		new_test_ext().execute_with(|| {
			let para = new_para();

			// Cannot contribute to non-existing fund
			assert_noop!(Crowdloan::contribute(Origin::signed(1), para, 49, None), Error::<Test>::InvalidParaId);
			// Cannot contribute below minimum contribution
			assert_noop!(Crowdloan::contribute(Origin::signed(1), para, 9, None), Error::<Test>::ContributionTooSmall);

			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 4, 9, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(1), para, 101, None));

			// Cannot contribute past the limit
			assert_noop!(Crowdloan::contribute(Origin::signed(2), para, 900, None), Error::<Test>::CapExceeded);

			// Move past end date
			run_to_block(10);

			// Cannot contribute to ended fund
			assert_noop!(Crowdloan::contribute(Origin::signed(1), para, 49, None), Error::<Test>::ContributionPeriodOver);
		});
	}

	#[test]
	fn bidding_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();

			let first_slot = 1;
			let last_slot = 4;

			assert_ok!(TestAuctioneer::new_auction(5, 0));

			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, first_slot, last_slot, 9, None));
			let bidder = Crowdloan::fund_account_id(para);

			// Fund crowdloan
			run_to_block(1);
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para, 100, None));
			run_to_block(3);
			assert_ok!(Crowdloan::contribute(Origin::signed(3), para, 150, None));
			run_to_block(5);
			assert_ok!(Crowdloan::contribute(Origin::signed(4), para, 200, None));
			run_to_block(8);
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para, 250, None));
			run_to_block(10);

			assert_eq!(bids(), vec![
				BidPlaced { height: 5, amount: 250, bidder, para, first_slot, last_slot },
				BidPlaced { height: 6, amount: 450, bidder, para, first_slot, last_slot },
				BidPlaced { height: 9, amount: 700, bidder, para, first_slot, last_slot },
			]);

			// Endings count incremented
			assert_eq!(Crowdloan::endings_count(), 1);
		});
	}

	#[test]
	fn withdraw_from_failed_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();

			// Set up two crowdloans
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 1, 9, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para, 100, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(3), para, 50, None));

			run_to_block(10);
			let account_id = Crowdloan::fund_account_id(para);
			// para has no reserved funds, indicating it did ot win the auction.
			assert_eq!(Balances::reserved_balance(&account_id), 0);
			// but there's still the funds in its balance.
			assert_eq!(Balances::free_balance(&account_id), 150);
			assert_eq!(Balances::free_balance(2), 1900);
			assert_eq!(Balances::free_balance(3), 2950);

			assert_ok!(Crowdloan::withdraw(Origin::signed(2), 2, para));
			assert_eq!(Balances::free_balance(&account_id), 50);
			assert_eq!(Balances::free_balance(2), 2000);

			assert_ok!(Crowdloan::withdraw(Origin::signed(2), 3, para));
			assert_eq!(Balances::free_balance(&account_id), 0);
			assert_eq!(Balances::free_balance(3), 3000);
		});
	}

	#[test]
	fn dissolving_failed_without_contributions_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();

			// Set up two crowdloans
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 1, 9, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para, 100, None));
			run_to_block(10);
			assert_noop!(Crowdloan::dissolve(Origin::signed(2), para), Error::<Test>::NotReadyToDissolve);

			assert_ok!(Crowdloan::withdraw(Origin::signed(2), 2, para));
			assert_ok!(Crowdloan::dissolve(Origin::signed(1), para));
			assert_eq!(Balances::free_balance(1), 1000);
		});
	}

	#[test]
	fn dissolving_failed_with_contributions_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();
			let issuance = Balances::total_issuance();

			// Set up two crowdloans
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 1, 9, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para, 100, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(3), para, 50, None));

			run_to_block(10);
			assert_ok!(Crowdloan::withdraw(Origin::signed(2), 2, para));

			run_to_block(14);
			assert_noop!(Crowdloan::dissolve(Origin::signed(1), para), Error::<Test>::NotReadyToDissolve);

			run_to_block(15);
			assert_ok!(Crowdloan::dissolve(Origin::signed(1), para));
			assert_eq!(Balances::free_balance(1), 1000);
			assert_eq!(Balances::total_issuance(), issuance - 50);
		});
	}

	#[test]
	fn withdraw_from_finished_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();
			let account_id = Crowdloan::fund_account_id(para);

			// Set up two crowdloans
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 1, 9, None));

			// Fund crowdloans.
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para, 100, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(3), para, 50, None));
			// simulate the reserving of para's funds. this actually
			// happens in the Slots pallet.
			assert_ok!(Balances::reserve(&account_id, 150));

			run_to_block(19);
			assert_noop!(Crowdloan::withdraw(Origin::signed(2), 2, para), Error::<Test>::BidOrLeaseActive);

			run_to_block(20);
			// simulate the unreserving of para's funds, now that the lease expired. this actually
			// happens in the Slots pallet.
			Balances::unreserve(&account_id, 150);

			// para has no reserved funds, indicating it did ot win the auction.
			assert_eq!(Balances::reserved_balance(&account_id), 0);
			// but there's still the funds in its balance.
			assert_eq!(Balances::free_balance(&account_id), 150);
			assert_eq!(Balances::free_balance(2), 1900);
			assert_eq!(Balances::free_balance(3), 2950);

			assert_ok!(Crowdloan::withdraw(Origin::signed(2), 2, para));
			assert_eq!(Balances::free_balance(&account_id), 50);
			assert_eq!(Balances::free_balance(2), 2000);

			assert_ok!(Crowdloan::withdraw(Origin::signed(2), 3, para));
			assert_eq!(Balances::free_balance(&account_id), 0);
			assert_eq!(Balances::free_balance(3), 3000);
		});
	}

	#[test]
	fn dissolving_finished_without_contributions_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();

			// Set up two crowdloans
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 1, 9, None));
			// Fund crowdloans.
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para, 100, None));
			// during this time the funds get reserved and unreserved.
			run_to_block(20);
			assert_noop!(Crowdloan::dissolve(Origin::signed(2), para), Error::<Test>::NotReadyToDissolve);

			assert_ok!(Crowdloan::withdraw(Origin::signed(2), 2, para));
			assert_ok!(Crowdloan::dissolve(Origin::signed(1), para));
			assert_eq!(Balances::free_balance(1), 1000);
		});
	}

	#[test]
	fn dissolving_finished_with_contributions_works() {
		new_test_ext().execute_with(|| {
			let para = new_para();
			let issuance = Balances::total_issuance();

			// Set up a crowdloan
			assert_ok!(Crowdloan::create(Origin::signed(1), para, 1000, 1, 1, 9, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para, 100, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(3), para, 50, None));
			run_to_block(20);
			assert_ok!(Crowdloan::withdraw(Origin::signed(2), 2, para));

			run_to_block(24);
			assert_noop!(Crowdloan::dissolve(Origin::signed(1), para), Error::<Test>::NotReadyToDissolve);

			run_to_block(25);
			assert_ok!(Crowdloan::dissolve(Origin::signed(1), para));
			assert_eq!(Balances::free_balance(1), 1000);
			assert_eq!(Balances::total_issuance(), issuance - 50);
		});
	}

	#[test]
	fn on_swap_works() {
		new_test_ext().execute_with(|| {
			let para_1 = new_para();
			let para_2 = new_para();

			// Set up crowdloans
			assert_ok!(Crowdloan::create(Origin::signed(1), para_1, 1000, 1, 1, 9, None));
			assert_ok!(Crowdloan::create(Origin::signed(1), para_2, 1000, 1, 1, 9, None));
			// Different contributions
			assert_ok!(Crowdloan::contribute(Origin::signed(2), para_1, 100, None));
			assert_ok!(Crowdloan::contribute(Origin::signed(3), para_2, 50, None));
			// Original state
			assert_eq!(Funds::<Test>::get(para_1).unwrap().raised, 100);
			assert_eq!(Funds::<Test>::get(para_2).unwrap().raised, 50);
			// Swap
			Crowdloan::on_swap(para_1, para_2);
			// Final state
			assert_eq!(Funds::<Test>::get(para_2).unwrap().raised, 100);
			assert_eq!(Funds::<Test>::get(para_1).unwrap().raised, 50);
		});
	}

	#[test]
	fn cannot_create_fund_when_already_active() {
		new_test_ext().execute_with(|| {
			let para_1 = new_para();

			assert_ok!(Crowdloan::create(Origin::signed(1), para_1, 1000, 1, 1, 9, None));
			// Cannot create a fund again
			assert_noop!(
				Crowdloan::create(Origin::signed(1), para_1, 1000, 1, 1, 9, None),
				Error::<Test>::FundNotEnded,
			);

		});
	}
}

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking {
	use super::{*, Module as Crowdloan};
	use frame_system::RawOrigin;
	use frame_support::{
		assert_ok,
		traits::OnInitialize,
	};
	use sp_runtime::traits::{Bounded, CheckedSub};
	use sp_std::prelude::*;

	use frame_benchmarking::{benchmarks, whitelisted_caller, account, impl_benchmark_test_suite};

	fn assert_last_event<T: Config>(generic_event: <T as Config>::Event) {
		let events = frame_system::Pallet::<T>::events();
		let system_event: <T as frame_system::Config>::Event = generic_event.into();
		// compare to the last event record
		let frame_system::EventRecord { event, .. } = &events[events.len() - 1];
		assert_eq!(event, &system_event);
	}

	fn create_fund<T: Config>(id: u32, end: T::BlockNumber) -> ParaId {
		let cap = BalanceOf::<T>::max_value();
		let lease_period_index = T::Auctioneer::lease_period_index();
		let first_slot = lease_period_index;
		let last_slot = lease_period_index + 3u32.into();
		let para_id = id.into();

		let caller = account("fund_creator", id, 0);
		CurrencyOf::<T>::make_free_balance_be(&caller, BalanceOf::<T>::max_value());

		// Assume ed25519 is most complex signature format
		let pubkey = crypto::create_ed25519_pubkey(b"//verifier".to_vec());

		let head_data = T::Registrar::worst_head_data();
		let validation_code = T::Registrar::worst_validation_code();
		assert_ok!(T::Registrar::register(caller.clone(), para_id, head_data, validation_code));

		assert_ok!(Crowdloan::<T>::create(
			RawOrigin::Signed(caller).into(),
			para_id,
			cap,
			first_slot,
			last_slot,
			end,
			Some(pubkey)
		));

		para_id
	}

	fn contribute_fund<T: Config>(who: &T::AccountId, index: ParaId) {
		CurrencyOf::<T>::make_free_balance_be(&who, BalanceOf::<T>::max_value());
		let value = T::MinContribution::get();

		let pubkey = crypto::create_ed25519_pubkey(b"//verifier".to_vec());
		let payload = (index, &who, BalanceOf::<T>::default(), value);
		let sig = crypto::create_ed25519_signature(&payload.encode(), pubkey);

		assert_ok!(Crowdloan::<T>::contribute(RawOrigin::Signed(who.clone()).into(), index, value, Some(sig)));
	}

	benchmarks! {
		create {
			let para_id = ParaId::from(1);
			let cap = BalanceOf::<T>::max_value();
			let first_slot = 0u32.into();
			let last_slot = 3u32.into();
			let end = T::BlockNumber::max_value();

			let caller: T::AccountId = whitelisted_caller();
			let head_data = T::Registrar::worst_head_data();
			let validation_code = T::Registrar::worst_validation_code();

			let verifier = account("verifier", 0, 0);

			CurrencyOf::<T>::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
			T::Registrar::register(caller.clone(), para_id, head_data, validation_code)?;

		}: _(RawOrigin::Signed(caller), para_id, cap, first_slot, last_slot, end, Some(verifier))
		verify {
			assert_last_event::<T>(RawEvent::Created(para_id).into())
		}

		// Contribute has two arms: PreEnding and Ending, but both are equal complexity.
		contribute {
			let fund_index = create_fund::<T>(1, 100u32.into());
			let caller: T::AccountId = whitelisted_caller();
			let contribution = T::MinContribution::get();
			CurrencyOf::<T>::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
			assert!(NewRaise::get().is_empty());

			let pubkey = crypto::create_ed25519_pubkey(b"//verifier".to_vec());
			let payload = (fund_index, &caller, BalanceOf::<T>::default(), contribution);
			let sig = crypto::create_ed25519_signature(&payload.encode(), pubkey);

		}: _(RawOrigin::Signed(caller.clone()), fund_index, contribution, Some(sig))
		verify {
			// NewRaise is appended to, so we don't need to fill it up for worst case scenario.
			assert!(!NewRaise::get().is_empty());
			assert_last_event::<T>(RawEvent::Contributed(caller, fund_index, contribution).into());
		}

		withdraw {
			let fund_index = create_fund::<T>(1, 100u32.into());
			let caller: T::AccountId = whitelisted_caller();
			let contributor = account("contributor", 0, 0);
			contribute_fund::<T>(&contributor, fund_index);
			frame_system::Pallet::<T>::set_block_number(200u32.into());
		}: _(RawOrigin::Signed(caller), contributor.clone(), fund_index)
		verify {
			assert_last_event::<T>(RawEvent::Withdrew(contributor, fund_index, T::MinContribution::get()).into());
		}

		// Worst case: Dissolve removes `RemoveKeysLimit` keys, and then finishes up the dissolution of the fund.
		dissolve {
			let k in 0 .. T::RemoveKeysLimit::get();
			let fund_index = create_fund::<T>(1, 100u32.into());

			// Dissolve will remove at most `RemoveKeysLimit` at once.
			for i in 0 .. k {
				contribute_fund::<T>(&account("contributor", i, 0), fund_index);
			}

			// One extra contributor so we can trigger withdraw
			contribute_fund::<T>(&account("last_contributor", 0, 0), fund_index);

			let caller: T::AccountId = whitelisted_caller();
			frame_system::Pallet::<T>::set_block_number(T::BlockNumber::max_value());
			Crowdloan::<T>::withdraw(
				RawOrigin::Signed(caller.clone()).into(),
				account("last_contributor", 0, 0),
				fund_index,
			)?;
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

			let pubkey = crypto::create_ed25519_pubkey(b"//verifier".to_vec());

			for i in 0 .. n {
				let fund_index = create_fund::<T>(i, end_block);
				let contributor: T::AccountId = account("contributor", i, 0);
				let contribution = T::MinContribution::get() * (i + 1).into();
				let payload = (fund_index, &contributor, BalanceOf::<T>::default(), contribution);
				let sig = crypto::create_ed25519_signature(&payload.encode(), pubkey.clone());

				CurrencyOf::<T>::make_free_balance_be(&contributor, BalanceOf::<T>::max_value());
				Crowdloan::<T>::contribute(RawOrigin::Signed(contributor).into(), fund_index, contribution, Some(sig))?;
			}

			let lease_period_index = T::Auctioneer::lease_period_index();
			let duration = end_block
				.checked_sub(&frame_system::Pallet::<T>::block_number())
				.ok_or("duration of auction less than zero")?;
			T::Auctioneer::new_auction(duration, lease_period_index)?;

			assert_eq!(T::Auctioneer::is_ending(end_block), Some(0u32.into()));
			assert_eq!(NewRaise::get().len(), n as usize);
			let old_endings_count = EndingsCount::get();
		}: {
			Crowdloan::<T>::on_initialize(end_block);
		} verify {
			assert_eq!(EndingsCount::get(), old_endings_count + 1);
			assert_last_event::<T>(RawEvent::HandleBidResult((n - 1).into(), Ok(())).into());
		}
	}

	impl_benchmark_test_suite!(
		Crowdloan,
		crate::integration_tests::new_test_ext(),
		crate::integration_tests::Test,
	);
}
