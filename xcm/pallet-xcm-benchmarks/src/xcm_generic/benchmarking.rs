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
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

use super::*;
use crate::{
	account_and_location, account_id_junction, execute_order, execute_xcm, worst_case_holding,
	XcmCallOf,
};
use codec::Encode;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite};
use frame_support::{assert_ok, dispatch::GetDispatchInfo, pallet_prelude::Get};
use sp_std::prelude::*;
use xcm::{latest::prelude::*, DoubleEncoded};

benchmarks! {
	order_noop {
		let order = Order::<XcmCallOf<T>>::Noop;
		let origin: MultiLocation = account_id_junction::<T>(1).into();
		let holding = worst_case_holding();
	}: {
		assert_ok!(execute_order::<T>(origin, holding, order));
	}

	order_query_holding {
		let origin: MultiLocation = account_id_junction::<T>(1).into();
		let holding = worst_case_holding();

		let order = Order::<XcmCallOf<T>>::QueryHolding {
			query_id: Default::default(),
			dest: T::ValidDestination::get(),
			assets: Wild(WildMultiAsset::All), // TODO is worst case filter?
		};

	} : {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {
		// The assert above is enough to validate this is completed.
		// todo maybe XCM sender peek
	}

	// This benchmark does not use any additional orders or instructions. This should be managed
	// by the `deep` and `shallow` implementation.
	order_buy_execution {
		let origin: MultiLocation = account_id_junction::<T>(1).into();
		let holding = worst_case_holding();

		let fee_asset = Concrete(Here.into());

		let order = Order::<XcmCallOf<T>>::BuyExecution {
			fees: (fee_asset, 100_000_000).into(), // should be something inside of holding
			weight: 0, // TODO think about sensible numbers
			debt: 0, // TODO think about sensible numbers
			halt_on_error: false,
			orders: Default::default(), // no orders
			instructions: Default::default(), // no instructions
		};
	} : {
		assert_ok!(execute_order::<T>(origin, holding, order));
	} verify {

	}

	// Worst case scenario for this benchmark is a large number of assets to
	// filter through the reserve.
	xcm_reserve_asset_deposited {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let assets: MultiAssets = (Concrete(Here.into()), 100).into();// TODO worst case

		// no effects to isolate this benchmark
		let effects: Vec<Order<_>> = Default::default();
		let xcm = Xcm::ReserveAssetDeposited { assets, effects };
	}: {
		assert_ok!(execute_xcm::<T>(sender_location, xcm).ensure_complete());
	} verify {
		// The assert above is enough to show this XCM succeeded
	}

	xcm_query_response {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let (query_id, response) = T::worst_case_response();
		let xcm = Xcm::QueryResponse { query_id, response };
	}: {
		assert_ok!(execute_xcm::<T>(sender_location, xcm).ensure_complete());
	} verify {
		// The assert above is enough to show this XCM succeeded
	}

	// We don't care about the call itself, since that is accounted for in the weight parameter
	// and included in the final weight calculation. So this is just the overhead of submitting
	// a noop call.
	xcm_transact {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let noop_call: <T as Config>::Call = frame_system::Call::remark_with_event(Default::default()).into();
		let double_encoded_noop_call: DoubleEncoded<_> = noop_call.encode().into();

		let xcm = Xcm::Transact {
			origin_type: OriginKind::SovereignAccount,
			require_weight_at_most: noop_call.get_dispatch_info().weight,
			call: double_encoded_noop_call,
		};

		let num_events = frame_system::Pallet::<T>::events().len();
	}: {
		assert_ok!(execute_xcm::<T>(sender_location, xcm).ensure_complete());
	} verify {
		// TODO make better assertion?
		let num_events2 = frame_system::Pallet::<T>::events().len();
		assert_eq!(num_events + 1, num_events2);
	}

	xcm_hrmp_new_channel_open_request {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		// Inputs here should not matter to weight.
		// let xcm = Xcm::HrmpNewChannelOpenRequest {
		// 	sender: Default::default(),
		// 	max_message_size: Default::default(),
		// 	max_capacity: Default::default(),
		// };
	}: {
		// assert_ok!(execute_xcm::<T>(sender_location, xcm).ensure_complete());
		// currently unhandled
	} verify {

	}
	xcm_hrmp_channel_accepted {}: {
				// currently unhandled
	} verify {}
	xcm_hrmp_channel_closing {}: {
				// currently unhandled
	} verify {}

	// We want to measure this benchmark with a noop XCM to isolate specifically
	// the overhead of `RelayedFrom`. The weight of the inner XCM should be accounted
	// for when calculating the total weight.
	xcm_relayed_from {
		let (sender_account, sender_location) = account_and_location::<T>(1);

		 // when these two inputs are empty, we basically noop
		let noop_xcm = Xcm::ReserveAssetDeposited {
			assets: Vec::<MultiAsset>::new().into(),
			effects: Default::default(),
		};

		let xcm = Xcm::RelayedFrom {
			who: Here.into(),
			message: sp_std::boxed::Box::new(noop_xcm),
		};
	}: {
		assert_ok!(execute_xcm::<T>(sender_location, xcm).ensure_complete());
	} verify {
		// the assert above verifies the XCM completed successfully.
	}

}

impl_benchmark_test_suite!(
	Pallet,
	crate::xcm_generic::mock::new_test_ext(),
	crate::xcm_generic::mock::Test
);
