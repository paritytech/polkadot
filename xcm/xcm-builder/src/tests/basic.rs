// Copyright 2020 Parity Technologies query_id: (), max_response_weight: ()  query_id: (), max_response_weight: ()  (UK) Ltd.
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
	assert!(<TestConfig as Config>::IsReserve::filter_asset_location(
		&(Parent, 100).into(),
		&Parent.into(),
	));

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
		ReserveAssetDeposited((Parent, 100).into()),
		BuyExecution { fees: (Parent, 1).into(), weight_limit: Limited(30) },
		DepositAsset { assets: AllCounted(1).into(), beneficiary: Here.into() },
	]);
	assert_eq!(<TestConfig as Config>::Weigher::weight(&mut message), Ok(30));
}
