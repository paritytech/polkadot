// Copyright 2020 Parity Technologies (UK) Ltd.
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
//! Based on https://w3f-research.readthedocs.io/en/latest/polkadot/Token%20Economics.html
//! which doesn't currently mention availability bitfields. As such, we don't reward them
//! for the time being, although we will build schemes to do so in the future.

use primitives::v1::ValidatorIndex;
use pallet_staking::SessionInterface;

/// The amount of era points given by backing a candidate that is included.
pub const BACKING_POINTS: u32 = 20;

/// Rewards validators for participating in parachains with era points in pallet-staking.
pub struct RewardValidatorsWithEraPoints<C>(sp_std::marker::PhantomData<C>);

fn reward_by_indices<C, I>(points: u32, indices: I) where
	C: pallet_staking::Config,
	I: IntoIterator<Item = ValidatorIndex>
{
	// Fetch the validators from the _session_ because sessions are offset from eras
	// and we are rewarding for behavior in current session.
	let validators = C::SessionInterface::validators();
	let rewards = indices.into_iter()
		.filter_map(|i| validators.get(i as usize).map(|v| v.clone()))
		.map(|v| (v, points));

	<pallet_staking::Module<C>>::reward_by_ids(rewards);
}

impl<C> crate::inclusion::RewardValidators for RewardValidatorsWithEraPoints<C>
	where C: pallet_staking::Config
{
	fn reward_backing(validators: impl IntoIterator<Item=ValidatorIndex>) {
		reward_by_indices::<C, _>(BACKING_POINTS, validators);
	}

	fn reward_bitfields(_validators: impl IntoIterator<Item=ValidatorIndex>) { }
}
