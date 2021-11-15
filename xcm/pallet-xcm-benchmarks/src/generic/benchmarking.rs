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
use crate::{new_executor, worst_case_holding, XcmCallOf};
use codec::Encode;
use frame_benchmarking::{benchmarks, BenchmarkError};
use frame_support::dispatch::GetDispatchInfo;
use sp_std::vec;
use xcm::{
	latest::{prelude::*, MultiAssets},
	DoubleEncoded,
};

const MAX_ASSETS: u32 = 100;

benchmarks! {
	query_holding {
		let holding = worst_case_holding();

		let mut executor = new_executor::<T>(Default::default());
		executor.holding = holding;

		let instruction = Instruction::<XcmCallOf<T>>::QueryHolding {
			query_id: Default::default(),
			dest: T::valid_destination()?,
			assets: Wild(WildMultiAsset::All), // TODO is worst case filter?
			max_response_weight: u64::MAX,
		};

		let xcm = Xcm(vec![instruction]);

	} : {
		executor.execute(xcm)?;
	} verify {
		// The assert above is enough to validate this is completed.
		// todo maybe XCM sender peek
	}

	// This benchmark does not use any additional orders or instructions. This should be managed
	// by the `deep` and `shallow` implementation.
	buy_execution {
		let holding = worst_case_holding();

		let mut executor = new_executor::<T>(Default::default());
		executor.holding = holding;

		let fee_asset = Concrete(Here.into());

		let instruction = Instruction::<XcmCallOf<T>>::BuyExecution {
			fees: (fee_asset, 100_000_000).into(), // should be something inside of holding
			weight_limit: WeightLimit::Unlimited,
		};

		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {

	}

	// Worst case scenario for this benchmark is a large number of assets to
	// filter through the reserve.
	reserve_asset_deposited {
		let mut executor = new_executor::<T>(Default::default());
		// TODO real max assets
		let assets = (0..MAX_ASSETS).map(|i| MultiAsset {
			id: Abstract(i.encode()),
			fun: Fungible(i as u128),

		}).collect::<vec::Vec<_>>();
		let multiassets: MultiAssets = assets.into();

		let instruction = Instruction::ReserveAssetDeposited(multiassets.clone());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm).map_err(|_| BenchmarkError::Skip)?;
	} verify {
		assert_eq!(executor.holding, multiassets.into());
	}

	query_response {
		let mut executor = new_executor::<T>(Default::default());
		let (query_id, response) = T::worst_case_response();
		let max_weight = u64::MAX;
		let instruction = Instruction::QueryResponse { query_id, response, max_weight };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm)?;
	} verify {
		// The assert above is enough to show this XCM succeeded
	}

	// We don't care about the call itself, since that is accounted for in the weight parameter
	// and included in the final weight calculation. So this is just the overhead of submitting
	// a noop call.
	transact {
		let origin = T::transact_origin().ok_or(BenchmarkError::Skip)?;
		let mut executor = new_executor::<T>(origin);
		let noop_call: <T as Config>::Call = frame_system::Call::remark_with_event {
			remark: Default::default()
		}.into();
		let double_encoded_noop_call: DoubleEncoded<_> = noop_call.encode().into();

		let instruction = Instruction::Transact {
			origin_type: OriginKind::SovereignAccount,
			require_weight_at_most: noop_call.get_dispatch_info().weight,
			call: double_encoded_noop_call,
		};
		let xcm = Xcm(vec![instruction]);

		let num_events = frame_system::Pallet::<T>::events().len();
	}: {
		executor.execute(xcm)?;
	} verify {
		// TODO make better assertion?
		let num_events2 = frame_system::Pallet::<T>::events().len();
		assert_eq!(num_events + 1, num_events2);
	}

	// hrmp_new_channel_open_request {
	// 	let mut executor = new_executor::<T>(Default::default());
	// 	let instruction = Instruction::HrmpNewChannelOpenRequest {
	// 		sender: 1,
	// 		max_message_size: 100,
	// 		max_capacity: 100
	// 	};

	// 	let xcm = Xcm(vec![instruction]);
	// }: {
	// 	executor.execute(xcm)?;
	// } verify {
	// 	// TODO: query pallet
	// }

	// hrmp_channel_accepted {
	// 	let mut executor = new_executor::<T>(Default::default());
	// 	// TODO open channel request first.

	// 	let instruction = Instruction::HrmpChannelAccepted {
	// 		recipient: 1,
	// 	};
	// 	let xcm = Xcm(vec![instruction]);
	// }: {
	// 	executor.execute(xcm)?;
	// } verify {
	// 	// TODO: query pallet
	// }

	// hrmp_channel_closing {
	// 	let (sender_account, sender_location) = account_and_location::<T>(1);
	// 	// TODO open channel first.

	// 	let instruction = Instruction::HrmpChannelClosing {
	// 		initiator: 1,
	// 		sender: 1,
	// 		recipient: 2,
	// 	};
	// 	let xcm = Xcm(vec![instruction]);
	// }: {
	// 	execute_xcm_override_error::<T>(sender_location, Default::default(), xcm)?;
	// } verify {
	// 	// TODO: query pallet
	// }

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
		let mut executor = new_executor::<T>(Default::default());
		let appendix = Xcm(vec![]);
		let instruction = Instruction::<XcmCallOf<T>>::SetAppendix(appendix);
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {
		assert_eq!(executor.appendix, Xcm(vec![]));
	}

	clear_error {
		let mut executor = new_executor::<T>(Default::default());
		executor.error = Some((5u32, XcmError::Overflow));
		let instruction = Instruction::<XcmCallOf<T>>::ClearError;
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {
		assert!(executor.error.is_none())
	}

	descend_origin {
		let mut executor = new_executor::<T>(Default::default());
		let who = X2(OnlyChild, OnlyChild);
		let instruction = Instruction::DescendOrigin(who.clone());
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {
		assert_eq!(
			executor.origin,
			Some(MultiLocation {
				parents: 0,
				interior: who,
			}),
		);
	}

	clear_origin {
		let mut executor = new_executor::<T>(Default::default());
		let instruction = Instruction::ClearOrigin;
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {
		assert_eq!(executor.origin, None);
	}

	report_error {
		let mut executor = new_executor::<T>(Default::default());
		executor.error = Some((0u32, XcmError::Unimplemented));
		let query_id = Default::default();
		let dest = T::valid_destination().map_err(|_| BenchmarkError::Skip)?;
		let max_response_weight = Default::default();

		let instruction = Instruction::ReportError { query_id, dest, max_response_weight };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.execute(xcm)?;
	} verify {
		// the execution succeeding is all we need to verify this xcm was successful
	}

	claim_asset {
		let (origin, ticket, assets) = T::claimable_asset()
			.ok_or(BenchmarkError::Skip)?;

		// We place some items into the asset trap to claim.
		let mut executor = new_executor::<T>(origin.clone());
		executor.holding = assets.clone().into();
		executor.execute(Xcm(vec![]))?;
		executor.post_execute(0);
		// Assets should be in the trap now.

		let mut executor = new_executor::<T>(origin);
		let instruction = Instruction::ClaimAsset { assets: assets.clone(), ticket };
		let xcm = Xcm(vec![instruction]);
	} :{
		executor.execute(xcm)?;
	} verify {
		assert!(executor.holding.ensure_contains(&assets).is_ok());
	}

	trap {
		let mut executor = new_executor::<T>(Default::default());
		let instruction = Instruction::Trap(10);
		let xcm = Xcm(vec![instruction]);
	} : {
		match executor.execute(xcm) {
			Err(error) if error.xcm_error == XcmError::Trap(10) => {
				// This is the success condition
			},
			_ => return Err(BenchmarkError::Stop("xcm trap did not return the expected error"))
		};
	} verify {
		// Verification is done above.
	}

	subscribe_version {
		use xcm_executor::traits::VersionChangeNotifier;
		let origin = T::transact_origin().ok_or(BenchmarkError::Skip)?;
		let query_id = Default::default();
		let max_response_weight = Default::default();
		let mut executor = new_executor::<T>(origin.clone());
		let instruction = Instruction::SubscribeVersion { query_id, max_response_weight };
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {
		assert!(<T::XcmConfig as xcm_executor::Config>::SubscriptionService::is_subscribed(&origin));
	}

	unsubscribe_version {
		use xcm_executor::traits::VersionChangeNotifier;
		// First we need to subscribe to notifications.
		let origin = T::transact_origin().ok_or(BenchmarkError::Skip)?;
		let query_id = Default::default();
		let max_response_weight = Default::default();
		let mut executor = new_executor::<T>(origin.clone());
		let instruction = Instruction::SubscribeVersion { query_id, max_response_weight };
		let xcm = Xcm(vec![instruction]);
		executor.execute(xcm)?;
		assert!(<T::XcmConfig as xcm_executor::Config>::SubscriptionService::is_subscribed(&origin));

		let mut executor = new_executor::<T>(origin.clone());
		let instruction = Instruction::UnsubscribeVersion;
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.execute(xcm)?;
	} verify {
		assert!(!<T::XcmConfig as xcm_executor::Config>::SubscriptionService::is_subscribed(&origin));
	}

	initiate_reserve_withdraw {
		let assets = MultiAssetFilter::Wild(All);
		let reserve = T::valid_destination().map_err(|_| BenchmarkError::Skip)?;
		let mut executor = new_executor::<T>(Default::default());
		let instruction = Instruction::InitiateReserveWithdraw { assets, reserve, xcm: Xcm(vec![]) };
		let xcm = Xcm(vec![instruction]);
	}:{
		executor.execute(xcm)?;
	} verify {
		// The execute completing successfully is as good as we can check.
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::generic::mock::new_test_ext(),
		crate::generic::mock::Test
	);

}
