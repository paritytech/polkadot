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
use crate::{new_executor, XcmCallOf};
use codec::Encode;
use frame_benchmarking::{benchmarks, BenchmarkError};
use frame_support::dispatch::GetDispatchInfo;
use sp_std::vec;
use xcm::{latest::prelude::*, DoubleEncoded};

benchmarks! {
	query_holding {
		let holding = T::worst_case_holding();

		let mut executor = new_executor::<T>(Default::default());
		executor.holding = holding.clone().into();

		let instruction = Instruction::<XcmCallOf<T>>::QueryHolding {
			query_id: Default::default(),
			dest: T::valid_destination()?,
			// Worst case is looking through all holdings for every asset explicitly.
			assets: Definite(holding),
			max_response_weight: u64::MAX,
		};

		let xcm = Xcm(vec![instruction]);

	} : {
		executor.execute(xcm)?;
	} verify {
		// The completion of execution above is enough to validate this is completed.
	}

	// This benchmark does not use any additional orders or instructions. This should be managed
	// by the `deep` and `shallow` implementation.
	buy_execution {
		let holding = T::worst_case_holding().into();

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
		let origin = T::transact_origin()?;
		let mut executor = new_executor::<T>(origin);
		let noop_call: <T as Config>::RuntimeCall = frame_system::Call::remark_with_event {
			remark: Default::default()
		}.into();
		let double_encoded_noop_call: DoubleEncoded<_> = noop_call.encode().into();

		let instruction = Instruction::Transact {
			origin_type: OriginKind::SovereignAccount,
			require_weight_at_most: noop_call.get_dispatch_info().weight.ref_time(),
			call: double_encoded_noop_call,
		};
		let xcm = Xcm(vec![instruction]);

		let num_events = frame_system::Pallet::<T>::events().len();
	}: {
		executor.execute(xcm)?;
	} verify {
		// TODO make better assertion? #4426
		let num_events2 = frame_system::Pallet::<T>::events().len();
		assert_eq!(num_events + 1, num_events2);
	}

	refund_surplus {
		let holding = T::worst_case_holding().into();
		let mut executor = new_executor::<T>(Default::default());
		executor.holding = holding;
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
		use xcm_executor::traits::DropAssets;

		let (origin, ticket, assets) = T::claimable_asset()?;

		// We place some items into the asset trap to claim.
		<T::XcmConfig as xcm_executor::Config>::AssetTrap::drop_assets(
			&origin,
			assets.clone().into(),
		);

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
		// In order to access result in the verification below, it needs to be defined here.
		let mut _result = Ok(());
	} : {
		_result = executor.execute(xcm);
	} verify {
		match _result {
			Err(error) if error.xcm_error == XcmError::Trap(10) => {
				// This is the success condition
			},
			_ => Err("xcm trap did not return the expected error")?
		};
	}

	subscribe_version {
		use xcm_executor::traits::VersionChangeNotifier;
		let origin = T::subscribe_origin()?;
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
		let origin = T::transact_origin()?;
		let query_id = Default::default();
		let max_response_weight = Default::default();
		<T::XcmConfig as xcm_executor::Config>::SubscriptionService::start(
			&origin,
			query_id,
			max_response_weight
		).map_err(|_| "Could not start subscription")?;
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
		let holding = T::worst_case_holding();
		let assets_filter = MultiAssetFilter::Definite(holding.clone());
		let reserve = T::valid_destination().map_err(|_| BenchmarkError::Skip)?;
		let mut executor = new_executor::<T>(Default::default());
		executor.holding = holding.into();
		let instruction = Instruction::InitiateReserveWithdraw { assets: assets_filter, reserve, xcm: Xcm(vec![]) };
		let xcm = Xcm(vec![instruction]);
	}:{
		executor.execute(xcm)?;
	} verify {
		// The execute completing successfully is as good as we can check.
		// TODO: Potentially add new trait to XcmSender to detect a queued outgoing message. #4426
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::generic::mock::new_test_ext(),
		crate::generic::mock::Test
	);

}
