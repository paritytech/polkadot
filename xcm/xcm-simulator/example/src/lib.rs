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

mod parachain;
mod relay_chain;

use sp_runtime::AccountId32;
use xcm_simulator::{decl_test_network, decl_test_parachain, decl_test_relay_chain};
use polkadot_parachain::primitives::Id as ParaId;
use sp_runtime::traits::AccountIdConversion;

pub const ALICE: AccountId32 = AccountId32::new([0u8; 32]);

decl_test_parachain! {
	pub struct ParaA {
		Runtime = parachain::Runtime,
		XcmpMessageHandler = parachain::MsgQueue,
		DmpMessageHandler = parachain::MsgQueue,
		new_ext = para_ext(1),
	}
}

decl_test_parachain! {
	pub struct ParaB {
		Runtime = parachain::Runtime,
		XcmpMessageHandler = parachain::MsgQueue,
		DmpMessageHandler = parachain::MsgQueue,
		new_ext = para_ext(2),
	}
}

decl_test_relay_chain! {
	pub struct Relay {
		Runtime = relay_chain::Runtime,
		XcmConfig = relay_chain::XcmConfig,
		new_ext = relay_ext(),
	}
}

decl_test_network! {
	pub struct MockNet {
		relay_chain = Relay,
		parachains = vec![
			(1, ParaA),
			(2, ParaB),
		],
	}
}

pub const INITIAL_BALANCE: u128 = 1_000_000_000;

pub fn para_ext(para_id: u32) -> sp_io::TestExternalities {
	use parachain::{MsgQueue, Runtime, System};

	let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();

	pallet_balances::GenesisConfig::<Runtime> { balances: vec![(ALICE, INITIAL_BALANCE)] }
		.assimilate_storage(&mut t)
		.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| {
		System::set_block_number(1);
		MsgQueue::set_para_id(para_id.into());
	});
	ext
}

pub fn relay_ext() -> sp_io::TestExternalities {
	use relay_chain::{Runtime, System};
	let para_account_a: relay_chain::AccountId = ParaId::from(1).into_account();

	let mut t = frame_system::GenesisConfig::default().build_storage::<Runtime>().unwrap();
	pallet_balances::GenesisConfig::<Runtime> { balances: vec![(ALICE, INITIAL_BALANCE), (para_account_a, INITIAL_BALANCE)] }
		.assimilate_storage(&mut t)
		.unwrap();

	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

pub type RelayChainPalletXcm = pallet_xcm::Pallet<relay_chain::Runtime>;
pub type ParachainPalletXcm = pallet_xcm::Pallet<parachain::Runtime>;

#[cfg(test)]
mod tests {
	use super::*;

	use codec::Encode;
	use frame_support::{assert_ok, weights::Weight};
	use xcm::v0::{
		Junction::{self, Parachain, Parent},
		MultiAsset::*,
		MultiLocation::*,
		NetworkId, OriginKind,
		Xcm::*,
		Order,
	};
	use xcm_simulator::TestExt;

	#[test]
	fn dmp() {
		MockNet::reset();

		let remark = parachain::Call::System(
			frame_system::Call::<parachain::Runtime>::remark_with_event(vec![1, 2, 3]),
		);
		Relay::execute_with(|| {
			assert_ok!(RelayChainPalletXcm::send_xcm(
				Null,
				X1(Parachain(1)),
				Transact {
					origin_type: OriginKind::SovereignAccount,
					require_weight_at_most: INITIAL_BALANCE as u64,
					call: remark.encode().into(),
				},
			));
		});

		ParaA::execute_with(|| {
			use parachain::{Event, System};
			assert!(System::events()
				.iter()
				.any(|r| matches!(r.event, Event::System(frame_system::Event::Remarked(_, _)))));
		});
	}

	#[test]
	fn ump() {
		MockNet::reset();

		let remark = relay_chain::Call::System(
			frame_system::Call::<relay_chain::Runtime>::remark_with_event(vec![1, 2, 3]),
		);
		ParaA::execute_with(|| {
			assert_ok!(ParachainPalletXcm::send_xcm(
				Null,
				X1(Parent),
				Transact {
					origin_type: OriginKind::SovereignAccount,
					require_weight_at_most: INITIAL_BALANCE as u64,
					call: remark.encode().into(),
				},
			));
		});

		Relay::execute_with(|| {
			use relay_chain::{Event, System};
			assert!(System::events()
				.iter()
				.any(|r| matches!(r.event, Event::System(frame_system::Event::Remarked(_, _)))));
		});
	}

	#[test]
	fn xcmp() {
		MockNet::reset();

		let remark = parachain::Call::System(
			frame_system::Call::<parachain::Runtime>::remark_with_event(vec![1, 2, 3]),
		);
		ParaA::execute_with(|| {
			assert_ok!(ParachainPalletXcm::send_xcm(
				Null,
				X2(Parent, Parachain(2)),
				Transact {
					origin_type: OriginKind::SovereignAccount,
					require_weight_at_most: INITIAL_BALANCE as u64,
					call: remark.encode().into(),
				},
			));
		});

		ParaB::execute_with(|| {
			use parachain::{Event, System};
			assert!(System::events()
				.iter()
				.any(|r| matches!(r.event, Event::System(frame_system::Event::Remarked(_, _)))));
		});
	}

	#[test]
	fn reserve_transfer() {
		MockNet::reset();

		Relay::execute_with(|| {
			assert_ok!(RelayChainPalletXcm::reserve_transfer_assets(
				relay_chain::Origin::signed(ALICE),
				X1(Parachain(1)),
				X1(Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }),
				vec![ConcreteFungible { id: Null, amount: 123 }],
				123,
			));
		});

		ParaA::execute_with(|| {
			// free execution, full amount received
			assert_eq!(
				pallet_balances::Pallet::<parachain::Runtime>::free_balance(&ALICE),
				INITIAL_BALANCE + 123
			);
		});
	}

	// Helper function for forming buy execution message
	fn buy_execution<C>(debt: Weight) -> Order<C> {
		use xcm::opaque::v0::prelude::*;
		Order::BuyExecution { fees: All, weight: 0, debt, halt_on_error: false, xcm: vec![] }
	}

	fn last_relay_chain_event() -> relay_chain::Event {
		relay_chain::System::events().pop().expect("Event expected").event
	}

	fn last_parachain_event() -> parachain::Event {
		parachain::System::events().pop().expect("Event expected").event
	}

	/// Scenario:
	/// A parachain transfers funds on the relaychain to another parachain's account.
	///
	/// Asserts that the parachain accounts are updated as expected.
	#[test]
	fn withdraw_and_deposit() {
		use xcm::opaque::v0::prelude::*;
		use sp_runtime::traits::AccountIdConversion;
		use polkadot_parachain::primitives::Id as ParaId;
		MockNet::reset();

		ParaA::execute_with(|| {
			let amount = 10;
			let weight = 3 * relay_chain::BaseXcmWeight::get();
			let message = WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: Parachain(2).into(),
					},
				],
			};
			// Send withdraw and deposit
			assert_ok!(ParachainPalletXcm::send_xcm(
				Null,
				X1(Parent),
				message.clone(),
			));
		});
		
		Relay::execute_with(|| {
			let amount = 10;
			let para_account_one: relay_chain::AccountId = ParaId::from(1).into_account();
			assert_eq!(relay_chain::Balances::free_balance(para_account_one), INITIAL_BALANCE - amount);
			let para_account: relay_chain::AccountId = ParaId::from(2).into_account();
			assert_eq!(relay_chain::Balances::free_balance(para_account), 10);
		});
	}
}
