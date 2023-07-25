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

use super::*;
use crate::{new_executor, XcmCallOf};
use codec::Encode;
use frame_benchmarking::{benchmarks, BenchmarkError};
use frame_support::dispatch::GetDispatchInfo;
use sp_std::vec;
use xcm::{
	latest::{prelude::*, MaxDispatchErrorLen, MaybeErrorCode, Weight},
	DoubleEncoded,
};
use xcm_executor::{ExecutorError, FeesMode};

benchmarks! {
	report_holding {
		let holding = T::worst_case_holding(0);

		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding.clone().into());

		let instruction = Instruction::<XcmCallOf<T>>::ReportHolding {
			response_info: QueryResponseInfo {
				destination: T::valid_destination()?,
				query_id: Default::default(),
				max_weight: Weight::MAX,
			},
			// Worst case is looking through all holdings for every asset explicitly.
			assets: Definite(holding),
		};

		let xcm = Xcm(vec![instruction]);

	} : {
		executor.bench_process(xcm)?;
	} verify {
		// The completion of execution above is enough to validate this is completed.
	}

	// This benchmark does not use any additional orders or instructions. This should be managed
	// by the `deep` and `shallow` implementation.
	buy_execution {
		let holding = T::worst_case_holding(0).into();

		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding);

		let fee_asset = Concrete(Here.into());

		let instruction = Instruction::<XcmCallOf<T>>::BuyExecution {
			fees: (fee_asset, 100_000_000u128).into(), // should be something inside of holding
			weight_limit: WeightLimit::Unlimited,
		};

		let xcm = Xcm(vec![instruction]);
	} : {
		executor.bench_process(xcm)?;
	} verify {

	}

	query_response {
		let mut executor = new_executor::<T>(Default::default());
		let (query_id, response) = T::worst_case_response();
		let max_weight = Weight::MAX;
		let querier: Option<MultiLocation> = Some(Here.into());
		let instruction = Instruction::QueryResponse { query_id, response, max_weight, querier };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// The assert above is enough to show this XCM succeeded
	}

	// We don't care about the call itself, since that is accounted for in the weight parameter
	// and included in the final weight calculation. So this is just the overhead of submitting
	// a noop call.
	transact {
		let (origin, noop_call) = T::transact_origin_and_runtime_call()?;
		let mut executor = new_executor::<T>(origin);
		let double_encoded_noop_call: DoubleEncoded<_> = noop_call.encode().into();

		let instruction = Instruction::Transact {
			origin_kind: OriginKind::SovereignAccount,
			require_weight_at_most: noop_call.get_dispatch_info().weight,
			call: double_encoded_noop_call,
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// TODO Make the assertion configurable?
	}

	refund_surplus {
		let holding = T::worst_case_holding(0).into();
		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding);
		executor.set_total_surplus(Weight::from_parts(1337, 1337));
		executor.set_total_refunded(Weight::zero());

		let instruction = Instruction::<XcmCallOf<T>>::RefundSurplus;
		let xcm = Xcm(vec![instruction]);
	} : {
		let result = executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.total_surplus(), &Weight::from_parts(1337, 1337));
		assert_eq!(executor.total_refunded(), &Weight::from_parts(1337, 1337));
	}

	set_error_handler {
		let mut executor = new_executor::<T>(Default::default());
		let instruction = Instruction::<XcmCallOf<T>>::SetErrorHandler(Xcm(vec![]));
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.error_handler(), &Xcm(vec![]));
	}

	set_appendix {
		let mut executor = new_executor::<T>(Default::default());
		let appendix = Xcm(vec![]);
		let instruction = Instruction::<XcmCallOf<T>>::SetAppendix(appendix);
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.appendix(), &Xcm(vec![]));
	}

	clear_error {
		let mut executor = new_executor::<T>(Default::default());
		executor.set_error(Some((5u32, XcmError::Overflow)));
		let instruction = Instruction::<XcmCallOf<T>>::ClearError;
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.bench_process(xcm)?;
	} verify {
		assert!(executor.error().is_none())
	}

	descend_origin {
		let mut executor = new_executor::<T>(Default::default());
		let who = X2(OnlyChild, OnlyChild);
		let instruction = Instruction::DescendOrigin(who.clone());
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(
			executor.origin(),
			&Some(MultiLocation {
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
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.origin(), &None);
	}

	report_error {
		let mut executor = new_executor::<T>(Default::default());
		executor.set_error(Some((0u32, XcmError::Unimplemented)));
		let query_id = Default::default();
		let destination = T::valid_destination().map_err(|_| BenchmarkError::Skip)?;
		let max_weight = Default::default();

		let instruction = Instruction::ReportError(QueryResponseInfo {
			query_id, destination, max_weight
		});
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
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
			&XcmContext {
				origin: Some(origin.clone()),
				message_id: [0; 32],
				topic: None,
			},
		);

		// Assets should be in the trap now.

		let mut executor = new_executor::<T>(origin);
		let instruction = Instruction::ClaimAsset { assets: assets.clone(), ticket };
		let xcm = Xcm(vec![instruction]);
	} :{
		executor.bench_process(xcm)?;
	} verify {
		assert!(executor.holding().ensure_contains(&assets).is_ok());
	}

	trap {
		let mut executor = new_executor::<T>(Default::default());
		let instruction = Instruction::Trap(10);
		let xcm = Xcm(vec![instruction]);
		// In order to access result in the verification below, it needs to be defined here.
		let mut _result = Ok(());
	} : {
		_result = executor.bench_process(xcm);
	} verify {
		assert!(matches!(_result, Err(ExecutorError {
			xcm_error: XcmError::Trap(10),
			..
		})));
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
		executor.bench_process(xcm)?;
	} verify {
		assert!(<T::XcmConfig as xcm_executor::Config>::SubscriptionService::is_subscribed(&origin));
	}

	unsubscribe_version {
		use xcm_executor::traits::VersionChangeNotifier;
		// First we need to subscribe to notifications.
		let (origin, _) = T::transact_origin_and_runtime_call()?;
		let query_id = Default::default();
		let max_response_weight = Default::default();
		<T::XcmConfig as xcm_executor::Config>::SubscriptionService::start(
			&origin,
			query_id,
			max_response_weight,
			&XcmContext {
				origin: Some(origin.clone()),
				message_id: [0; 32],
				topic: None,
			},
		).map_err(|_| "Could not start subscription")?;
		assert!(<T::XcmConfig as xcm_executor::Config>::SubscriptionService::is_subscribed(&origin));

		let mut executor = new_executor::<T>(origin.clone());
		let instruction = Instruction::UnsubscribeVersion;
		let xcm = Xcm(vec![instruction]);
	} : {
		executor.bench_process(xcm)?;
	} verify {
		assert!(!<T::XcmConfig as xcm_executor::Config>::SubscriptionService::is_subscribed(&origin));
	}

	burn_asset {
		let holding = T::worst_case_holding(0);
		let assets = holding.clone();

		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding.into());

		let instruction = Instruction::BurnAsset(assets.into());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert!(executor.holding().is_empty());
	}

	expect_asset {
		let holding = T::worst_case_holding(0);
		let assets = holding.clone();

		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(holding.into());

		let instruction = Instruction::ExpectAsset(assets.into());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// `execute` completing successfully is as good as we can check.
	}

	expect_origin {
		let expected_origin = Parent.into();
		let mut executor = new_executor::<T>(Default::default());

		let instruction = Instruction::ExpectOrigin(Some(expected_origin));
		let xcm = Xcm(vec![instruction]);
		let mut _result = Ok(());
	}: {
		_result = executor.bench_process(xcm);
	} verify {
		assert!(matches!(_result, Err(ExecutorError {
			xcm_error: XcmError::ExpectationFalse,
			..
		})));
	}

	expect_error {
		let mut executor = new_executor::<T>(Default::default());
		executor.set_error(Some((3u32, XcmError::Overflow)));

		let instruction = Instruction::ExpectError(None);
		let xcm = Xcm(vec![instruction]);
		let mut _result = Ok(());
	}: {
		_result = executor.bench_process(xcm);
	} verify {
		assert!(matches!(_result, Err(ExecutorError {
			xcm_error: XcmError::ExpectationFalse,
			..
		})));
	}

	expect_transact_status {
		let mut executor = new_executor::<T>(Default::default());
		let worst_error = || -> MaybeErrorCode {
			vec![0; MaxDispatchErrorLen::get() as usize].into()
		};
		executor.set_transact_status(worst_error());

		let instruction = Instruction::ExpectTransactStatus(worst_error());
		let xcm = Xcm(vec![instruction]);
		let mut _result = Ok(());
	}: {
		_result = executor.bench_process(xcm);
	} verify {
		assert!(matches!(_result, Ok(..)));
	}

	query_pallet {
		let query_id = Default::default();
		let destination = T::valid_destination().map_err(|_| BenchmarkError::Skip)?;
		let max_weight = Default::default();
		let mut executor = new_executor::<T>(Default::default());

		let instruction = Instruction::QueryPallet {
			module_name: b"frame_system".to_vec(),
			response_info: QueryResponseInfo { destination, query_id, max_weight },
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// TODO: Potentially add new trait to XcmSender to detect a queued outgoing message. #4426
	}

	expect_pallet {
		let mut executor = new_executor::<T>(Default::default());

		let instruction = Instruction::ExpectPallet {
			index: 0,
			name: b"System".to_vec(),
			module_name: b"frame_system".to_vec(),
			crate_major: 4,
			min_crate_minor: 0,
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// the execution succeeding is all we need to verify this xcm was successful
	}

	report_transact_status {
		let query_id = Default::default();
		let destination = T::valid_destination().map_err(|_| BenchmarkError::Skip)?;
		let max_weight = Default::default();

		let mut executor = new_executor::<T>(Default::default());
		executor.set_transact_status(b"MyError".to_vec().into());

		let instruction = Instruction::ReportTransactStatus(QueryResponseInfo {
			query_id,
			destination,
			max_weight,
		});
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// TODO: Potentially add new trait to XcmSender to detect a queued outgoing message. #4426
	}

	clear_transact_status {
		let mut executor = new_executor::<T>(Default::default());
		executor.set_transact_status(b"MyError".to_vec().into());

		let instruction = Instruction::ClearTransactStatus;
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.transact_status(), &MaybeErrorCode::Success);
	}

	set_topic {
		let mut executor = new_executor::<T>(Default::default());

		let instruction = Instruction::SetTopic([1; 32]);
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.topic(), &Some([1; 32]));
	}

	clear_topic {
		let mut executor = new_executor::<T>(Default::default());
		executor.set_topic(Some([2; 32]));

		let instruction = Instruction::ClearTopic;
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.topic(), &None);
	}

	exchange_asset {
		let (give, want) = T::worst_case_asset_exchange().map_err(|_| BenchmarkError::Skip)?;
		let assets = give.clone();

		let mut executor = new_executor::<T>(Default::default());
		executor.set_holding(give.into());
		let instruction = Instruction::ExchangeAsset {
			give: assets.into(),
			want: want.clone(),
			maximal: true,
		};
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.holding(), &want.into());
	}

	universal_origin {
		let (origin, alias) = T::universal_alias().map_err(|_| BenchmarkError::Skip)?;

		let mut executor = new_executor::<T>(origin);

		let instruction = Instruction::UniversalOrigin(alias.clone());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		use frame_support::traits::Get;
		let universal_location = <T::XcmConfig as xcm_executor::Config>::UniversalLocation::get();
		assert_eq!(executor.origin(), &Some(X1(alias).relative_to(&universal_location)));
	}

	export_message {
		let x in 1 .. 1000;
		// The `inner_xcm` influences `ExportMessage` total weight based on
		// `inner_xcm.encoded_size()`, so for this benchmark use smallest encoded instruction
		// to approximate weight per "unit" of encoded size; then actual weight can be estimated
		// to be `inner_xcm.encoded_size() * benchmarked_unit`.
		// Use `ClearOrigin` as the small encoded instruction.
		let inner_xcm = Xcm(vec![ClearOrigin; x as usize]);
		// Get `origin`, `network` and `destination` from configured runtime.
		let (origin, network, destination) = T::export_message_origin_and_destination()?;
		let mut executor = new_executor::<T>(origin);
		let xcm = Xcm(vec![ExportMessage {
			network, destination, xcm: inner_xcm,
		}]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// The execute completing successfully is as good as we can check.
		// TODO: Potentially add new trait to XcmSender to detect a queued outgoing message. #4426
	}

	set_fees_mode {
		let mut executor = new_executor::<T>(Default::default());
		executor.set_fees_mode(FeesMode { jit_withdraw: false });

		let instruction = Instruction::SetFeesMode { jit_withdraw: true };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.fees_mode(), &FeesMode { jit_withdraw: true });
	}

	lock_asset {
		let (unlocker, owner, asset) = T::unlockable_asset()?;

		let mut executor = new_executor::<T>(owner);
		executor.set_holding(asset.clone().into());

		let instruction = Instruction::LockAsset { asset, unlocker };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// TODO: Potentially add new trait to XcmSender to detect a queued outgoing message. #4426
	}

	unlock_asset {
		use xcm_executor::traits::{AssetLock, Enact};

		let (unlocker, owner, asset) = T::unlockable_asset()?;

		let mut executor = new_executor::<T>(unlocker.clone());

		// We first place the asset in lock first...
		<T::XcmConfig as xcm_executor::Config>::AssetLocker::prepare_lock(
			unlocker,
			asset.clone(),
			owner.clone(),
		)
		.map_err(|_| BenchmarkError::Skip)?
		.enact()
		.map_err(|_| BenchmarkError::Skip)?;

		// ... then unlock them with the UnlockAsset instruction.
		let instruction = Instruction::UnlockAsset { asset, target: owner };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {

	}

	note_unlockable {
		use xcm_executor::traits::{AssetLock, Enact};

		let (unlocker, owner, asset) = T::unlockable_asset()?;

		let mut executor = new_executor::<T>(unlocker.clone());

		// We first place the asset in lock first...
		<T::XcmConfig as xcm_executor::Config>::AssetLocker::prepare_lock(
			unlocker,
			asset.clone(),
			owner.clone(),
		)
		.map_err(|_| BenchmarkError::Skip)?
		.enact()
		.map_err(|_| BenchmarkError::Skip)?;

		// ... then note them as unlockable with the NoteUnlockable instruction.
		let instruction = Instruction::NoteUnlockable { asset, owner };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {

	}

	request_unlock {
		use xcm_executor::traits::{AssetLock, Enact};

		let (locker, owner, asset) = T::unlockable_asset()?;

		// We first place the asset in lock first...
		<T::XcmConfig as xcm_executor::Config>::AssetLocker::prepare_lock(
			locker.clone(),
			asset.clone(),
			owner.clone(),
		)
		.map_err(|_| BenchmarkError::Skip)?
		.enact()
		.map_err(|_| BenchmarkError::Skip)?;

		// ... then request for an unlock with the RequestUnlock instruction.
		let mut executor = new_executor::<T>(owner);
		let instruction = Instruction::RequestUnlock { asset, locker };
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		// TODO: Potentially add new trait to XcmSender to detect a queued outgoing message. #4426
	}

	unpaid_execution {
		let mut executor = new_executor::<T>(Default::default());
		executor.set_origin(Some(Here.into()));

		let instruction = Instruction::<XcmCallOf<T>>::UnpaidExecution {
			weight_limit: WeightLimit::Unlimited,
			check_origin: Some(Here.into()),
		};

		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	}

	alias_origin {
		let (origin, target) = T::alias_origin().map_err(|_| BenchmarkError::Skip)?;

		let mut executor = new_executor::<T>(origin);

		let instruction = Instruction::AliasOrigin(target.clone());
		let xcm = Xcm(vec![instruction]);
	}: {
		executor.bench_process(xcm)?;
	} verify {
		assert_eq!(executor.origin(), &Some(target));
	}

	impl_benchmark_test_suite!(
		Pallet,
		crate::generic::mock::new_test_ext(),
		crate::generic::mock::Test
	);
}
