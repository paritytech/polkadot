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

#[test]
fn fixed_rate_of_fungible_should_work() {
	parameter_types! {
		pub static WeightPrice: (AssetId, u128, u128) =
			(Here.into(), WEIGHT_REF_TIME_PER_SECOND.into(), WEIGHT_PROOF_SIZE_PER_MB.into());
	}

	let mut trader = FixedRateOfFungible::<WeightPrice, ()>::new();
	let ctx = XcmContext { origin: None, message_id: XcmHash::default(), topic: None };

	// supplies 100 unit of asset, 80 still remains after purchasing weight
	assert_eq!(
		trader.buy_weight(
			Weight::from_parts(10, 10),
			fungible_multi_asset(Here.into(), 100).into(),
			&ctx,
		),
		Ok(fungible_multi_asset(Here.into(), 80).into()),
	);
	// should have nothing left, as 5 + 5 = 10, and we supplied 10 units of asset.
	assert_eq!(
		trader.buy_weight(
			Weight::from_parts(5, 5),
			fungible_multi_asset(Here.into(), 10).into(),
			&ctx,
		),
		Ok(vec![].into()),
	);
	// should have 5 left, as there are no proof size components
	assert_eq!(
		trader.buy_weight(
			Weight::from_parts(5, 0),
			fungible_multi_asset(Here.into(), 10).into(),
			&ctx,
		),
		Ok(fungible_multi_asset(Here.into(), 5).into()),
	);
	// not enough to purchase the combined weights
	assert_err!(
		trader.buy_weight(
			Weight::from_parts(5, 5),
			fungible_multi_asset(Here.into(), 5).into(),
			&ctx,
		),
		XcmError::TooExpensive,
	);
}

#[test]
fn errors_should_return_unused_weight() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![Here.into()]);
	// We own 1000 of our tokens.
	add_asset(Here, (Here, 11u128));
	let mut message = Xcm(vec![
		// First xfer results in an error on the last message only
		TransferAsset {
			assets: (Here, 1u128).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
		// Second xfer results in error third message and after
		TransferAsset {
			assets: (Here, 2u128).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
		// Third xfer results in error second message and after
		TransferAsset {
			assets: (Here, 4u128).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
	]);
	// Weight limit of 70 is needed.
	let limit = <TestConfig as Config>::Weigher::weight(&mut message).unwrap();
	assert_eq!(limit, Weight::from_parts(30, 30));

	let hash = fake_message_hash(&message);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), hash, limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(30, 30)));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 7u128).into()]);
	assert_eq!(asset_list(Here), vec![(Here, 4u128).into()]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), hash, limit);
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(30, 30), XcmError::NotWithdrawable));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 10u128).into()]);
	assert_eq!(asset_list(Here), vec![(Here, 1u128).into()]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), hash, limit);
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(20, 20), XcmError::NotWithdrawable));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 11u128).into()]);
	assert_eq!(asset_list(Here), vec![]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message, hash, limit);
	assert_eq!(r, Outcome::Incomplete(Weight::from_parts(10, 10), XcmError::NotWithdrawable));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 11u128).into()]);
	assert_eq!(asset_list(Here), vec![]);
	assert_eq!(sent_xcm(), vec![]);
}

#[test]
fn weight_bounds_should_respect_instructions_limit() {
	MaxInstructions::set(3);
	let mut message = Xcm(vec![ClearOrigin; 4]);
	// 4 instructions are too many.
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Err(()));

	let mut message =
		Xcm(vec![SetErrorHandler(Xcm(vec![ClearOrigin])), SetAppendix(Xcm(vec![ClearOrigin]))]);
	// 4 instructions are too many, even when hidden within 2.
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Err(()));

	let mut message =
		Xcm(vec![SetErrorHandler(Xcm(vec![SetErrorHandler(Xcm(vec![SetErrorHandler(Xcm(
			vec![ClearOrigin],
		))]))]))]);
	// 4 instructions are too many, even when it's just one that's 3 levels deep.
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Err(()));

	let mut message =
		Xcm(vec![SetErrorHandler(Xcm(vec![SetErrorHandler(Xcm(vec![ClearOrigin]))]))]);
	// 3 instructions are OK.
	assert_eq!(
		<TestConfig as Config>::Weigher::weight(&mut message),
		Ok(Weight::from_parts(30, 30))
	);
}

#[test]
fn weight_trader_tuple_should_work() {
	let para_1: MultiLocation = Parachain(1).into();
	let para_2: MultiLocation = Parachain(2).into();

	parameter_types! {
		pub static HereWeightPrice: (AssetId, u128, u128) =
			(Here.into(), WEIGHT_REF_TIME_PER_SECOND.into(), WEIGHT_PROOF_SIZE_PER_MB.into());
		pub static Para1WeightPrice: (AssetId, u128, u128) =
			(Parachain(1).into(), WEIGHT_REF_TIME_PER_SECOND.into(), WEIGHT_PROOF_SIZE_PER_MB.into());
	}

	type Traders = (
		// trader one
		FixedRateOfFungible<HereWeightPrice, ()>,
		// trader two
		FixedRateOfFungible<Para1WeightPrice, ()>,
	);

	let mut traders = Traders::new();
	let ctx = XcmContext { origin: None, message_id: XcmHash::default(), topic: None };

	// trader one buys weight
	assert_eq!(
		traders.buy_weight(
			Weight::from_parts(5, 5),
			fungible_multi_asset(Here.into(), 10).into(),
			&ctx
		),
		Ok(vec![].into()),
	);
	// trader one refunds
	assert_eq!(
		traders.refund_weight(Weight::from_parts(2, 2), &ctx),
		Some(fungible_multi_asset(Here.into(), 4))
	);

	let mut traders = Traders::new();
	// trader one failed; trader two buys weight
	assert_eq!(
		traders.buy_weight(Weight::from_parts(5, 5), fungible_multi_asset(para_1, 10).into(), &ctx),
		Ok(vec![].into()),
	);
	// trader two refunds
	assert_eq!(
		traders.refund_weight(Weight::from_parts(2, 2), &ctx),
		Some(fungible_multi_asset(para_1, 4))
	);

	let mut traders = Traders::new();
	// all traders fails
	assert_err!(
		traders.buy_weight(Weight::from_parts(5, 5), fungible_multi_asset(para_2, 10).into(), &ctx),
		XcmError::TooExpensive,
	);
	// and no refund
	assert_eq!(traders.refund_weight(Weight::from_parts(2, 2), &ctx), None);
}
