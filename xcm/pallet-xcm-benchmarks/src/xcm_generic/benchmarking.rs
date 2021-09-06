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
	account_and_location, account_id_junction, execute_xcm, new_executor, worst_case_holding,
	XcmCallOf,
};
use codec::Encode;
use frame_benchmarking::{benchmarks, impl_benchmark_test_suite, BenchmarkError, BenchmarkResult};
use frame_support::{dispatch::GetDispatchInfo, pallet_prelude::Get};
use sp_std::vec;
use xcm::{latest::prelude::*, DoubleEncoded};

benchmarks! {
	query_holding {
		let origin: MultiLocation = account_id_junction::<T>(1).into();
		let holding = worst_case_holding();

		let instruction = Instruction::<XcmCallOf<T>>::QueryHolding {
			query_id: Default::default(),
			dest: T::valid_destination()?,
			assets: Wild(WildMultiAsset::All), // TODO is worst case filter?
			max_response_weight: u64::MAX,
		};

		let xcm = Xcm(vec![instruction]);

	} : {
		execute_xcm::<T>(origin, holding, xcm)
			.map_err(|_|
				BenchmarkError::Override(
					BenchmarkResult::from_weight(T::BlockWeights::get().max_block)
				)
			)?;
	} verify {
		// The assert above is enough to validate this is completed.
		// todo maybe XCM sender peek
	}

	// This benchmark does not use any additional orders or instructions. This should be managed
	// by the `deep` and `shallow` implementation.
	buy_execution {
		let origin: MultiLocation = account_id_junction::<T>(1).into();
		let holding = worst_case_holding();

		let fee_asset = Concrete(Here.into());

		let instruction = Instruction::<XcmCallOf<T>>::BuyExecution {
			fees: (fee_asset, 100_000_000).into(), // should be something inside of holding
			weight_limit: WeightLimit::Unlimited,
		};

		let xcm = Xcm(vec![instruction]);
	} : {
		execute_xcm::<T>(origin, holding, xcm)?;
	} verify {

	}

	// Worst case scenario for this benchmark is a large number of assets to
	// filter through the reserve.
	reserve_asset_deposited {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let assets: MultiAssets = (Concrete(Here.into()), 100).into();// TODO worst case

		let instruction = Instruction::ReserveAssetDeposited(assets);
		let xcm = Xcm(vec![instruction]);
	}: {
		execute_xcm::<T>(sender_location, Default::default(), xcm)?;
	} verify {
		// The assert above is enough to show this XCM succeeded
	}

	query_response {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let (query_id, response) = T::worst_case_response();
		let max_weight = u64::MAX;
		let instruction = Instruction::QueryResponse { query_id, response, max_weight };
		let xcm = Xcm(vec![instruction]);
	}: {
		execute_xcm::<T>(sender_location, Default::default(), xcm)?;
	} verify {
		// The assert above is enough to show this XCM succeeded
	}

	// We don't care about the call itself, since that is accounted for in the weight parameter
	// and included in the final weight calculation. So this is just the overhead of submitting
	// a noop call.
	transact {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		let noop_call: <T as Config>::Call = frame_system::Call::remark_with_event(Default::default()).into();
		let double_encoded_noop_call: DoubleEncoded<_> = noop_call.encode().into();

		let instruction = Instruction::Transact {
			origin_type: OriginKind::SovereignAccount,
			require_weight_at_most: noop_call.get_dispatch_info().weight,
			call: double_encoded_noop_call,
		};
		let xcm = Xcm(vec![instruction]);

		let num_events = frame_system::Pallet::<T>::events().len();
	}: {
		execute_xcm::<T>(sender_location, Default::default(), xcm)?;
	} verify {
		// TODO make better assertion?
		let num_events2 = frame_system::Pallet::<T>::events().len();
		assert_eq!(num_events + 1, num_events2);
	}

	hrmp_new_channel_open_request {
		let (sender_account, sender_location) = account_and_location::<T>(1);
		// Inputs here should not matter to weight.
		// let instruction = Instruction::HrmpNewChannelOpenRequest {
		// 	sender: Default::default(),
		// 	max_message_size: Default::default(),
		// 	max_capacity: Default::default(),
		// };
	}: {
		// assert_ok!(execute_xcm::<T>(sender_location, Default::default(), xcm).ensure_complete());
		// currently unhandled
	} verify {

	}
	hrmp_channel_accepted {}: {
				// currently unhandled
	} verify {}
	hrmp_channel_closing {}: {
				// currently unhandled
	} verify {}

	refund_surplus {
		let mut executor = new_executor::<T>(Default::default());
		executor.total_surplus = 1337;
		executor.total_refunded = 0;

		let instruction = Instruction::<XcmCallOf<T>>::RefundSurplus;
		let xcm = Xcm(vec![instruction]);
	} : {
		let result = executor.execute(xcm)?;
	} verify {
		assert_eq!(executor.total_surplus, 1337);
		assert_eq!(executor.total_refunded, 1337);
	}

	set_error_handler {
		let mut executor = new_executor::<T>(Default::default());
		let instruction = Instruction::<XcmCallOf<T>>::SetErrorHandler(Xcm(vec![]));
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {
		assert_eq!(executor.error_handler, Xcm(vec![]));
	}

	set_appendix {
	} : {

	} verify {}

	clear_error {
		let mut executor = new_executor::<T>(Default::default());
		executor.error = Some((5u32, XcmError::Undefined));
		let instruction = Instruction::<XcmCallOf<T>>::ClearError;
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {
		assert!(executor.error.is_none())
	}

	claim_asset {} : {} verify {}

	trap {} : {} verify {}

	subscribe_version {} : {} verify {}

	unsubscribe_version {} : {} verify {}

	clear_origin {} : {} verify {}

	descend_origin {} : {} verify {}

}

impl_benchmark_test_suite!(
	Pallet,
	crate::xcm_generic::mock::new_test_ext(),
	crate::xcm_generic::mock::Test
);
