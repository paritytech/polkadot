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
fn basic_setup_works() {
	add_reserve(Parent.into(), Wild((Parent, WildFungible).into()));
	assert!(
		<TestConfig as Config>::IsReserve::contains(&(Parent, 100u128).into(), &Parent.into(),)
	);

	assert_eq!(to_account(Parachain(1)), Ok(1001));
	assert_eq!(to_account(Parachain(50)), Ok(1050));
	assert_eq!(to_account((Parent, Parachain(1))), Ok(2001));
	assert_eq!(to_account((Parent, Parachain(50))), Ok(2050));
	assert_eq!(
		to_account(MultiLocation::new(0, X1(AccountIndex64 { index: 1, network: None }))),
		Ok(1),
	);
	assert_eq!(
		to_account(MultiLocation::new(0, X1(AccountIndex64 { index: 42, network: None }))),
		Ok(42),
	);
	assert_eq!(to_account(Here), Ok(3000));
}

#[test]
fn weigher_should_work() {
	let mut message = Xcm(vec![
		ReserveAssetDeposited((Parent, 100u128).into()),
		BuyExecution {
			fees: (Parent, 1u128).into(),
			weight_limit: Limited(Weight::from_parts(30, 30)),
		},
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);
	assert_eq!(
		<TestConfig as Config>::Weigher::weight(&mut message),
		Ok(Weight::from_parts(30, 30))
	);
}

#[test]
fn code_registers_should_work() {
	// we'll let them have message execution for free.
	AllowUnpaidFrom::set(vec![Here.into()]);
	// We own 1000 of our tokens.
	add_asset(Here, (Here, 21u128));
	let mut message = Xcm(vec![
		// Set our error handler - this will fire only on the second message, when there's an error
		SetErrorHandler(Xcm(vec![
			TransferAsset {
				assets: (Here, 2u128).into(),
				beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
			},
			// It was handled fine.
			ClearError,
		])),
		// Set the appendix - this will always fire.
		SetAppendix(Xcm(vec![TransferAsset {
			assets: (Here, 4u128).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		}])),
		// First xfer always works ok
		TransferAsset {
			assets: (Here, 1u128).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
		// Second xfer results in error on the second message - our error handler will fire.
		TransferAsset {
			assets: (Here, 8u128).into(),
			beneficiary: X1(AccountIndex64 { index: 3, network: None }).into(),
		},
	]);
	// Weight limit of 70 is needed.
	let limit = <TestConfig as Config>::Weigher::weight(&mut message).unwrap();
	assert_eq!(limit, Weight::from_parts(70, 70));

	let hash = fake_message_hash(&message);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message.clone(), hash, limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(50, 50))); // We don't pay the 20 weight for the error handler.
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 13u128).into()]);
	assert_eq!(asset_list(Here), vec![(Here, 8u128).into()]);
	assert_eq!(sent_xcm(), vec![]);

	let r = XcmExecutor::<TestConfig>::execute_xcm(Here, message, hash, limit);
	assert_eq!(r, Outcome::Complete(Weight::from_parts(70, 70))); // We pay the full weight here.
	assert_eq!(asset_list(AccountIndex64 { index: 3, network: None }), vec![(Here, 20u128).into()]);
	assert_eq!(asset_list(Here), vec![(Here, 1u128).into()]);
	assert_eq!(sent_xcm(), vec![]);
}
