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

//! On demand assigner pallet benchmarking.

#![cfg(feature = "runtime-benchmarks")]

use super::{Pallet, *};
use crate::{
	configuration::{HostConfiguration, Pallet as ConfigurationPallet},
	paras::{Pallet as ParasPallet, ParaGenesisArgs, ParaKind, ParachainsCache},
	shared::Pallet as ParasShared,
};

use frame_benchmarking::v2::*;
use frame_system::RawOrigin;
use sp_runtime::traits::Bounded;

use primitives::{
	HeadData, Id as ParaId, SessionIndex, ValidationCode, ON_DEMAND_DEFAULT_QUEUE_MAX_SIZE,
};

// Constants for the benchmarking
const SESSION_INDEX: SessionIndex = 1;

// Initialize a parathread for benchmarking.
pub fn init_parathread<T>(para_id: ParaId)
where
	T: Config + crate::paras::Config + crate::shared::Config,
{
	ParasShared::<T>::set_session_index(SESSION_INDEX);
	let mut config = HostConfiguration::default();
	config.on_demand_cores = 1;
	ConfigurationPallet::<T>::force_set_active_config(config);
	let mut parachains = ParachainsCache::new();
	ParasPallet::<T>::initialize_para_now(
		&mut parachains,
		para_id,
		&ParaGenesisArgs {
			para_kind: ParaKind::Parathread,
			genesis_head: HeadData(vec![1, 2, 3, 4]),
			validation_code: ValidationCode(vec![1, 2, 3, 4]),
		},
	);
}

#[benchmarks]
mod benchmarks {
	/// We want to fill the queue to the maximum, so exactly one more item fits.
	const MAX_FILL_BENCH: u32 = ON_DEMAND_DEFAULT_QUEUE_MAX_SIZE.saturating_sub(1);

	use super::*;
	#[benchmark]
	fn place_order_keep_alive(s: Linear<1, MAX_FILL_BENCH>) {
		// Setup
		let caller = whitelisted_caller();
		let para_id = ParaId::from(111u32);
		init_parathread::<T>(para_id);
		T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		let assignment = Assignment::new(para_id);

		for _ in 0..s {
			Pallet::<T>::add_on_demand_assignment(assignment.clone(), QueuePushDirection::Back)
				.unwrap();
		}

		#[extrinsic_call]
		_(RawOrigin::Signed(caller.into()), BalanceOf::<T>::max_value(), para_id)
	}

	#[benchmark]
	fn place_order_allow_death(s: Linear<1, MAX_FILL_BENCH>) {
		// Setup
		let caller = whitelisted_caller();
		let para_id = ParaId::from(111u32);
		init_parathread::<T>(para_id);
		T::Currency::make_free_balance_be(&caller, BalanceOf::<T>::max_value());
		let assignment = Assignment::new(para_id);

		for _ in 0..s {
			Pallet::<T>::add_on_demand_assignment(assignment.clone(), QueuePushDirection::Back)
				.unwrap();
		}

		#[extrinsic_call]
		_(RawOrigin::Signed(caller.into()), BalanceOf::<T>::max_value(), para_id)
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::mock::new_test_ext(
			crate::assigner_on_demand::mock_helpers::GenesisConfigBuilder::default().build()
		),
		crate::mock::Test
	);
}
