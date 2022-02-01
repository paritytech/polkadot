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


/// This migration converts crowdloans to use a crowdloan index rather than the parachain id as a
/// unique identifier. This makes it easier to swap two crowdloans between parachains.
pub fn crowdloan_index_migration<T: Config>() {
	// First migrate `NextTrieIndex` counter to `NextFundIndex`.
	generate_storage_alias!(Crowdloan, NextTrieIndex => Value<u32>);

	let next_index = NextTrieIndex::take();
	NextFundIndex::put(next_index);

	// Migrate all funds from `paraid -> fund` to `index -> fund`

}


/// Information on a funding effort for a pre-existing parachain. We assume that the parachain ID
/// is known as it's used for the key of the storage item for which this is the value (`Funds`).
#[derive(Encode, Decode, Clone, PartialEq, Eq, RuntimeDebug, TypeInfo)]
#[codec(dumb_trait_bound)]
pub struct OldFundInfo<AccountId, Balance, BlockNumber, LeasePeriod> {
	/// The owning account who placed the deposit.
	pub depositor: AccountId,
	/// An optional verifier. If exists, contributions must be signed by verifier.
	pub verifier: Option<MultiSigner>,
	/// The amount of deposit placed.
	pub deposit: Balance,
	/// The total amount raised.
	pub raised: Balance,
	/// Block number after which the funding must have succeeded. If not successful at this number
	/// then everyone may withdraw their funds.
	pub end: BlockNumber,
	/// A hard-cap on the amount that may be contributed.
	pub cap: Balance,
	/// The most recent block that this had a contribution. Determines if we make a bid or not.
	/// If this is `Never`, this fund has never received a contribution.
	/// If this is `PreEnding(n)`, this fund received a contribution sometime in auction
	/// number `n` before the ending period.
	/// If this is `Ending(n)`, this fund received a contribution during the current ending period,
	/// where `n` is how far into the ending period the contribution was made.
	pub last_contribution: LastContribution<BlockNumber>,
	/// First lease period in range to bid on; it's actually a `LeasePeriod`, but that's the same type
	/// as `BlockNumber`.
	pub first_period: LeasePeriod,
	/// Last lease period in range to bid on; it's actually a `LeasePeriod`, but that's the same type
	/// as `BlockNumber`.
	pub last_period: LeasePeriod,
	/// Unique index used for this crowdfund.
	pub fund_index: FundIndex,
}

fn old_fund_account_id(index: ParaId) -> T::AccountId {
	T::PalletId::get().into_sub_account(index)
}
