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

mod karura_like;
mod kusama_like;
mod statemine_like;

use sp_runtime::AccountId32;
use xcm_simulator::{decl_test_network, decl_test_parachain, decl_test_relay_chain};
use polkadot_parachain::primitives::Id as ParaId;
use sp_runtime::traits::AccountIdConversion;

pub const ALICE: AccountId32 = AccountId32::new([42u8; 32]);
pub const INITIAL_BALANCE: u128 = 1_000_000_000;

pub const KARURA_ID: u32 = 2000;
pub const STATEMINE_ID: u32 = 1000;

decl_test_parachain! {
	pub struct KaruraLike {
		Runtime = karura_like::Runtime,
		XcmpMessageHandler = karura_like::MsgQueue,
		DmpMessageHandler = karura_like::MsgQueue,
		new_ext = karura_like_ext(KARURA_ID),
	}
}

decl_test_relay_chain! {
	pub struct KusamaLike {
		Runtime = kusama_like::Runtime,
		XcmConfig = kusama_like::XcmConfig,
		new_ext = kusama_like_ext(),
	}
}

decl_test_parachain! {
	pub struct StatemineLike {
		Runtime = statemine_like::Runtime,
		XcmpMessageHandler = statemine_like::MsgQueue,
		DmpMessageHandler = statemine_like::MsgQueue,
		new_ext = statemine_like_ext(STATEMINE_ID),
	}
}

decl_test_network! {
	pub struct MockNet {
		relay_chain = KusamaLike,
		parachains = vec![
			(STATEMINE_ID, StatemineLike),
			(KARURA_ID, KaruraLike),
		],
	}
}

pub fn karura_like_ext(para_id: u32) -> sp_io::TestExternalities {
	use karura_like::{MsgQueue, Runtime, System};

	let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();

	let balances = vec![
		(ALICE, INITIAL_BALANCE)
	];

	pallet_balances::GenesisConfig::<Runtime> { balances }
		.assimilate_storage(&mut t)
		.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| {
		System::set_block_number(1);
		MsgQueue::set_para_id(para_id.into());
	});
	ext
}

pub fn kusama_like_ext() -> sp_io::TestExternalities {
	use kusama_like::{AccountId, Runtime, System};
	let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();

	let karura_acc: AccountId = ParaId::from(KARURA_ID).into_account();
	let statemine_acc: AccountId = ParaId::from(STATEMINE_ID).into_account();

	let balances = vec![
		(ALICE, INITIAL_BALANCE),
		(statemine_acc, INITIAL_BALANCE),
		(karura_acc, INITIAL_BALANCE),
	];

	pallet_balances::GenesisConfig::<Runtime> {
		balances,
	}
	.assimilate_storage(&mut t)
	.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

pub fn statemine_like_ext(para_id: u32) -> sp_io::TestExternalities {
	use statemine_like::{MsgQueue, Runtime, System};

	let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();

	let balances = vec![
		(ALICE, INITIAL_BALANCE)
	];

	pallet_balances::GenesisConfig::<Runtime> { balances }
		.assimilate_storage(&mut t)
		.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| {
		System::set_block_number(1);
		MsgQueue::set_para_id(para_id.into());
	});
	ext
}

pub type KaruraPalletXcm = pallet_xcm::Pallet<karura_like::Runtime>;
pub type KusamaPalletXcm = pallet_xcm::Pallet<kusama_like::Runtime>;
pub type StateminePalletXcm = pallet_xcm::Pallet<statemine_like::Runtime>;

#[cfg(test)]
mod tests {
	use super::*;

	use frame_support::{assert_ok, weights::Weight};
	use xcm::v0::{
		Junction::{Parachain, Parent},
		MultiAsset::*,
		MultiLocation::*,
		Order,
	};
	use xcm::opaque::v0::prelude::*;
	use xcm_simulator::TestExt;

	// Construct a `BuyExecution` order.
	fn buy_execution<C>(debt: Weight) -> Order<C> {
		use xcm::opaque::v0::prelude::*;
		Order::BuyExecution {
			fees: All,
			weight: 0,
			debt,
			halt_on_error: false,
			xcm: vec![],
		}
	}

	/// Scenario:
	/// A parachain wants to move KSM from Kusama to Statemine.
	/// It withdraws funds and then teleports them to the destination.
	///
	/// Asserts that the balances are updated accordingly and the correct XCM is sent.
	#[test]
	fn teleport_to_statemine_works() {
		let amount =  10 * kusama_like::ExistentialDeposit::get();

		KaruraLike::execute_with(|| {
			let weight = 3 * kusama_like::BaseXcmWeight::get();
			let message = Xcm::WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::InitiateTeleport {
						assets: vec![All],
						dest: Parachain(STATEMINE_ID).into(),
						effects: vec![
							buy_execution(weight),
							Order::DepositAsset { assets: vec![All], dest: X2(Parent, Parachain(KARURA_ID)) },
						],
					},
				],
			};
			assert_ok!(KaruraPalletXcm::send_xcm(
				Null,
				X1(Parent),
				message.clone(),
			));
		});

		KusamaLike::execute_with(|| {
			let karura_acc: kusama_like::AccountId = ParaId::from(KARURA_ID).into_account();
			assert_eq!(kusama_like::Balances::free_balance(karura_acc), INITIAL_BALANCE - amount);
		});

		StatemineLike::execute_with(|| {
			use polkadot_parachain::primitives::Sibling;
			use xcm_builder::SiblingParachainConvertsVia;
			use xcm_executor::traits::Convert;
			let karura_acc = SiblingParachainConvertsVia::<Sibling, statemine_like::AccountId>::convert(
				X2(Parent, Parachain(KARURA_ID))).unwrap();
			assert_eq!(statemine_like::Balances::free_balance(karura_acc), amount);
		});
	}
}
