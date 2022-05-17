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
//! Based on <https://research.web3.foundation/en/latest/polkadot/overview/2-token-economics.html>
//! which doesn't currently mention availability bitfields. As such, we don't reward them
//! for the time being, although we will build schemes to do so in the future.

use crate::{session_info, shared};
use frame_support::traits::ValidatorSet;
use primitives::v2::ValidatorIndex;

/// The amount of era points given by backing a candidate that is included.
pub const BACKING_POINTS: u32 = 20;

/// Rewards validators for participating in parachains with era points in pallet-staking.
pub struct RewardValidatorsWithEraPoints<C>(sp_std::marker::PhantomData<C>);

impl<C> crate::inclusion::RewardValidators for RewardValidatorsWithEraPoints<C>
where
	C: pallet_staking::Config + shared::Config + session_info::Config,
	C::ValidatorSet: ValidatorSet<C::AccountId, ValidatorId = C::AccountId>,
{
	fn reward_backing(indices: impl IntoIterator<Item = ValidatorIndex>) {
		// Fetch the validators from the _session_ because sessions are offset from eras
		// and we are rewarding for behavior in current session.
		let session_index = shared::Pallet::<C>::session_index();
		let validators = session_info::Pallet::<C>::account_keys(&session_index);
		let validators = match validators {
			Some(validators) => validators,
			None => {
				// Account keys are missing for the current session.
				// This might happen only for the first session after
				// `AccountKeys` were introduced via runtime upgrade.
				return
			},
		};

		let rewards = indices
			.into_iter()
			.filter_map(|i| validators.get(i.0 as usize).cloned())
			.map(|v| (v, BACKING_POINTS));

		<pallet_staking::Pallet<C>>::reward_by_ids(rewards);
	}

	fn reward_bitfields(_validators: impl IntoIterator<Item = ValidatorIndex>) {}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		configuration::HostConfiguration,
		mock::{new_test_ext, MockGenesisConfig, ParasShared, Test},
	};
	use keyring::Sr25519Keyring;
	use primitives::v2::{TypeVec, ValidatorId};

	#[test]
	fn rewards_based_on_indirection() {
		let validators = vec![
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		];

		fn validator_pubkeys(val_ids: &[Sr25519Keyring]) -> Vec<ValidatorId> {
			val_ids.iter().map(|v| v.public().into()).collect()
		}

		new_test_ext(MockGenesisConfig::default()).execute_with(|| {
			let mut config = HostConfiguration::default();
			config.max_validators = None;

			let pubkeys = TypeVec::from(validator_pubkeys(&validators));

			let shuffled_pubkeys =
				ParasShared::initializer_on_new_session(1, [1; 32], &config, pubkeys);

			assert_eq!(
				shuffled_pubkeys,
				TypeVec::from(validator_pubkeys(&[
					Sr25519Keyring::Ferdie,
					Sr25519Keyring::Bob,
					Sr25519Keyring::Charlie,
					Sr25519Keyring::Dave,
					Sr25519Keyring::Alice,
				]))
			);

			assert_eq!(
				ParasShared::active_validator_indices(),
				vec![
					ValidatorIndex(4),
					ValidatorIndex(1),
					ValidatorIndex(2),
					ValidatorIndex(3),
					ValidatorIndex(0),
				]
			);

			assert_eq!(
				validators_to_reward::<Test, _, _>(
					&validators,
					vec![ValidatorIndex(0), ValidatorIndex(1), ValidatorIndex(2)],
				)
				.into_iter()
				.copied()
				.collect::<Vec<_>>(),
				vec![Sr25519Keyring::Ferdie, Sr25519Keyring::Bob, Sr25519Keyring::Charlie],
			);
		})
	}
}

