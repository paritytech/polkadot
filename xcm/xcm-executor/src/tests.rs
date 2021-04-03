// Copyright 2020 Parity Technologies (UK) Ltd.
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
use super::mock::*;
use MultiAsset::*;
use xcm::v0::Order;

#[test]
fn basic_setup_works() {
	add_reserve(X1(Parent), AllConcreteFungible { id: X1(Parent) });
	assert!(<TestConfig as Config>::IsReserve::filter_asset_location(
		&ConcreteFungible { id: X1(Parent), amount: 100 },
		&X1(Parent),
	));
}

#[test]
fn basic_execution_should_work() {
	add_reserve(X1(Parent), AllConcreteFungible { id: X1(Parent) });
	let r = XcmExecutor::<TestConfig>::execute_xcm(
		X1(Parent),
		Xcm::ReserveAssetDeposit {
			assets: vec![ ConcreteFungible { id: X1(Parent), amount: 100 } ],
			effects: vec![ Order::DepositAsset { assets: vec![ All ], dest: Null } ],
		},
		50,
	);
	assert_eq!(r, Ok(20));
	assert_eq!(assets(Null), vec![ ConcreteFungible { id: X1(Parent), amount: 100 } ]);
}
