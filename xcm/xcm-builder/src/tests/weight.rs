// Copyright 2022 Parity Technologies (UK) Ltd.
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
fn errors_should_return_unused_weight() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![Here.into()]);
	// We own 1000 of our tokens.
	add_asset(Here, (Here, 11));
	let mut message = Xcm(vec![
		// First xfer results in an error on the last message only
		TransferAsset {
			assets: (Here, 1).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
		// Second xfer results in error third message and after
		TransferAsset {
			assets: (Here, 2).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
		// Third xfer results in error second message and after
		TransferAsset {
			assets: (Here, 4).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
	]);
	// Weight limit of 70 is needed.
	let limit = <TestConfig as Config>::Weigher::weight(&mut message).unwrap();
	assert_eq!(limit, 30);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), limit);
	assert_eq!(r, Outcome::Complete(30));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 7).into()]);
	assert_eq!(asset_list(Here), vec![(Here, 4).into()]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), limit);
	assert_eq!(r, Outcome::Incomplete(30, XcmError::NotWithdrawable));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 10).into()]);
	assert_eq!(asset_list(Here), vec![(Here, 1).into()]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), limit);
	assert_eq!(r, Outcome::Incomplete(20, XcmError::NotWithdrawable));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 11).into()]);
	assert_eq!(asset_list(Here), vec![]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message, limit);
	assert_eq!(r, Outcome::Incomplete(10, XcmError::NotWithdrawable));
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 11).into()]);
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
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Ok(30));
}

#[test]
fn weight_trader_tuple_should_work() {
	let para_1: MultiLocation = Parachain(1).into();
	let para_2: MultiLocation = Parachain(2).into();

	parameter_types! {
		pub static HereWeightPrice: (AssetId, u128) = (Here.into(), WEIGHT_PER_SECOND.into());
		pub static Para1WeightPrice: (AssetId, u128) = (Parachain(1).into(), WEIGHT_PER_SECOND.into());
	}

	type Traders = (
		// trader one
		FixedRateOfFungible<HereWeightPrice, ()>,
		// trader two
		FixedRateOfFungible<Para1WeightPrice, ()>,
	);

	let mut traders = Traders::new();
	// trader one buys weight
	assert_eq!(
		traders.buy_weight(5, fungible_multi_asset(Here.into(), 10).into()),
		Ok(fungible_multi_asset(Here.into(), 5).into()),
	);
	// trader one refunds
	assert_eq!(traders.refund_weight(2), Some(fungible_multi_asset(Here.into(), 2)));

	let mut traders = Traders::new();
	// trader one failed; trader two buys weight
	assert_eq!(
		traders.buy_weight(5, fungible_multi_asset(para_1.clone(), 10).into()),
		Ok(fungible_multi_asset(para_1.clone(), 5).into()),
	);
	// trader two refunds
	assert_eq!(traders.refund_weight(2), Some(fungible_multi_asset(para_1, 2)));

	let mut traders = Traders::new();
	// all traders fails
	assert_err!(
		traders.buy_weight(5, fungible_multi_asset(para_2, 10).into()),
		XcmError::TooExpensive,
	);
	// and no refund
	assert_eq!(traders.refund_weight(2), None);
}
