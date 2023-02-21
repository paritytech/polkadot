// Copyright 2023 Parity Technologies (UK) Ltd.
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

use super::*;
use frame_support::{assert_noop, assert_ok, error::BadOrigin};

use crate::mock::{new_test_ext, MockGenesisConfig, ParathreadAssigner, RuntimeOrigin};

fn default_genesis_config() -> MockGenesisConfig {
	MockGenesisConfig {
		configuration: crate::configuration::GenesisConfig {
			config: crate::configuration::HostConfiguration { ..Default::default() },
		},
		..Default::default()
	}
}

#[derive(Debug)]
pub(super) struct GenesisConfigBuilder {
	//hrmp_channel_max_capacity: u32,
}

impl Default for GenesisConfigBuilder {
	fn default() -> Self {
		Self {
            //hrmp_channel_max_capacity: 2
             }
	}
}

impl GenesisConfigBuilder {
	pub(super) fn build(self) -> crate::mock::MockGenesisConfig {
		let mut genesis = default_genesis_config();
		let config = &mut genesis.configuration.config;
		//config.hrmp_channel_max_capacity = self.hrmp_channel_max_capacity;
		genesis
	}
}

#[test]
fn spot_price_capacity_zero_returns_none() {
	let res = ParathreadAssigner::calculate_spot_price(
		FixedI64::from(i64::MAX),
		0u32,
		u32::MAX,
		Perbill::from_percent(100),
		Perbill::from_percent(1),
	);
	assert_eq!(res, None)
}

#[test]
fn spot_price_queue_size_larger_than_capacity_returns_none() {
	let res = ParathreadAssigner::calculate_spot_price(
		FixedI64::from(i64::MAX),
		1u32,
		2u32,
		Perbill::from_percent(100),
		Perbill::from_percent(1),
	);
	assert_eq!(res, None)
}

#[test]
fn spot_price_calculation_identity() {
	let res = ParathreadAssigner::calculate_spot_price(
		FixedI64::from_float(1.0),
		1000,
		100,
		Perbill::from_percent(10),
		Perbill::from_percent(3),
	);
	assert_eq!(res.unwrap(), FixedI64::from_float(1.0))
}

#[test]
fn spot_price_calculation_u32_max() {
	let res = ParathreadAssigner::calculate_spot_price(
		FixedI64::from_float(1.0),
		u32::MAX,
		u32::MAX,
		Perbill::from_percent(100),
		Perbill::from_percent(3),
	);
	assert_eq!(res.unwrap(), FixedI64::from_float(1.0))
}

#[test]
fn spot_price_calculation_u32_traffic_max() {
	let res = ParathreadAssigner::calculate_spot_price(
		FixedI64::from(i64::MAX),
		u32::MAX,
		u32::MAX,
		Perbill::from_percent(1),
		Perbill::from_percent(1),
	);
	assert_eq!(res, None)
}

#[test]
fn sustained_target_increases_spot_price() {
	let mut traffic = FixedI64::from_u32(1u32);
	for _ in 0..50 {
		traffic = ParathreadAssigner::calculate_spot_price(
			traffic,
			100,
			12,
			Perbill::from_percent(10),
			Perbill::from_percent(100),
		)
		.unwrap()
	}
	assert_eq!(traffic, FixedI64::from_float(2.718103316))
}

#[test]
fn set_base_spot_price_works() {
	new_test_ext(GenesisConfigBuilder::default().build()).execute_with(|| {
		let alice = 1u64;
		let amt = 10_000_000u128;

		// Does not work unsigned
		assert_noop!(
			ParathreadAssigner::set_base_spot_price(RuntimeOrigin::none(), amt),
			BadOrigin
		);

		// Does not work without sudo
		assert_noop!(
			ParathreadAssigner::set_base_spot_price(RuntimeOrigin::signed(alice), amt),
			BadOrigin
		);

		// Works with root
		assert_ok!(ParathreadAssigner::set_base_spot_price(RuntimeOrigin::root(), amt));

		// Base spot price matches amt
		assert_eq!(ParathreadAssigner::get_base_spot_price(), amt);
	});
	//pub fn set_base_spot_price(origin: OriginFor<T>, amount: BalanceOf<T>) -> DispatchResult {
}
