// Copyright (C) Parity Technologies (UK) Ltd.
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

//! An implementation of the `RewardValidators` trait used by `inclusion` that employs
//! `pallet-staking` to compute the rewards.
//!
//! Based on <https://research.web3.foundation/en/latest/polkadot/overview/2-token-economics.html>
//! which doesn't currently mention availability bitfields. As such, we don't reward them
//! for the time being, although we will build schemes to do so in the future.

use crate::{session_info, shared};
use frame_support::traits::{Defensive, ValidatorSet};
use primitives::{SessionIndex, ValidatorIndex};
use sp_std::collections::btree_set::BTreeSet;

/// The amount of era points given by backing a candidate that is included.
pub const BACKING_POINTS: u32 = 20;
/// The amount of era points given by dispute voting on a candidate.
pub const DISPUTE_STATEMENT_POINTS: u32 = 20;

/// Rewards validators for participating in parachains with era points in pallet-staking.
pub struct RewardValidatorsWithEraPoints<C>(sp_std::marker::PhantomData<C>);

impl<C> RewardValidatorsWithEraPoints<C>
where
	C: pallet_staking::Config + session_info::Config,
	C::ValidatorSet: ValidatorSet<C::AccountId, ValidatorId = C::AccountId>,
{
	/// Reward validators in session with points, but only if they are in the active set.
	fn reward_only_active(
		session_index: SessionIndex,
		indices: impl IntoIterator<Item = ValidatorIndex>,
		points: u32,
	) {
		let validators = session_info::Pallet::<C>::account_keys(&session_index);
		let validators = match validators
			.defensive_proof("account_keys are present for dispute_period sessions")
		{
			Some(validators) => validators,
			None => return,
		};
		// limit rewards to the active validator set
		let active_set: BTreeSet<_> = C::ValidatorSet::validators().into_iter().collect();

		let rewards = indices
			.into_iter()
			.filter_map(|i| validators.get(i.0 as usize).cloned())
			.filter(|v| active_set.contains(v))
			.map(|v| (v, points));

		<pallet_staking::Pallet<C>>::reward_by_ids(rewards);
	}
}

impl<C> crate::inclusion::RewardValidators for RewardValidatorsWithEraPoints<C>
where
	C: pallet_staking::Config + shared::Config + session_info::Config,
	C::ValidatorSet: ValidatorSet<C::AccountId, ValidatorId = C::AccountId>,
{
	fn reward_backing(indices: impl IntoIterator<Item = ValidatorIndex>) {
		let session_index = shared::Pallet::<C>::session_index();
		Self::reward_only_active(session_index, indices, BACKING_POINTS);
	}

	fn reward_bitfields(_validators: impl IntoIterator<Item = ValidatorIndex>) {}
}

impl<C> crate::disputes::RewardValidators for RewardValidatorsWithEraPoints<C>
where
	C: pallet_staking::Config + session_info::Config,
	C::ValidatorSet: ValidatorSet<C::AccountId, ValidatorId = C::AccountId>,
{
	fn reward_dispute_statement(
		session: SessionIndex,
		validators: impl IntoIterator<Item = ValidatorIndex>,
	) {
		Self::reward_only_active(session, validators, DISPUTE_STATEMENT_POINTS);
	}
}
