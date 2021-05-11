// Copyright 2021 Parity Technologies (UK) Ltd.
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
// along with Polkadot. If not, see <http://www.gnu.org/licenses/>.

//! Tests for the Kusama Runtime Configuration

use crate::*;

#[test]
fn compute_inflation_should_give_sensible_results() {
	assert_eq!(pallet_staking_reward_fn::compute_inflation(
		Perquintill::from_percent(75),
		Perquintill::from_percent(75),
		Perquintill::from_percent(5),
	), Perquintill::one());
	assert_eq!(pallet_staking_reward_fn::compute_inflation(
		Perquintill::from_percent(50),
		Perquintill::from_percent(75),
		Perquintill::from_percent(5),
	), Perquintill::from_rational(2u64, 3u64));
	assert_eq!(pallet_staking_reward_fn::compute_inflation(
		Perquintill::from_percent(80),
		Perquintill::from_percent(75),
		Perquintill::from_percent(5),
	), Perquintill::from_rational(1u64, 2u64));
}

#[test]
fn era_payout_should_give_sensible_results() {
	assert_eq!(era_payout(
		75,
		100,
		Perquintill::from_percent(10),
		Perquintill::one(),
		0,
	), (10, 0));
	assert_eq!(era_payout(
		80,
		100,
		Perquintill::from_percent(10),
		Perquintill::one(),
		0,
	), (6, 4));
}
