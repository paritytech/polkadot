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

use polkadot_parachain::primitives::Id as ParaId;
use sp_runtime::{traits::AccountIdConversion, AccountId32};
use xcm_simulator::{decl_test_network, decl_test_parachain, decl_test_relay_chain};

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
	pallet_balances::GenesisConfig::<Runtime> {
		balances: vec![(ALICE, INITIAL_BALANCE), (para_account_a, INITIAL_BALANCE)],
	}
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
		prelude::OnlyChild,
		Junction::{self, Parachain, Parent},
		MultiAsset::*,
		MultiLocation::*,
		NetworkId, Order, OriginKind,
		Xcm::*,
	};
	use xcm_simulator::TestExt;

	// Helper function for forming buy execution message
	fn buy_execution<C>(debt: Weight) -> Order<C> {
		use xcm::opaque::v0::prelude::*;
		Order::BuyExecution { fees: All, weight: 0, debt, halt_on_error: false, xcm: vec![] }
	}

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

	/// Scenario:
	/// A user Alice sends funds from the relaychain to a parachain.
	///
	/// Asserts that the correct XCM is sent and the balances are set as expected.
	#[test]
	fn reserve_transfer_assets() {
		MockNet::reset();

		Relay::execute_with(|| {
			assert_ok!(RelayChainPalletXcm::reserve_transfer_assets(
				relay_chain::Origin::signed(ALICE),
				X1(Parachain(1)),
				X1(Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }),
				vec![ConcreteFungible { id: Null, amount: 123 }],
				123,
			));
			let para_account_a = ParaId::from(1).into_account();
			assert_eq!(parachain::Balances::free_balance(&para_account_a), INITIAL_BALANCE + 123);
		});

		ParaA::execute_with(|| {
			use xcm::opaque::v0::NetworkId;
			use xcm_simulator::{BuyExecution, DepositAsset};
			// check message received
			let expected_message = (
				X1(Parent),
				ReserveAssetDeposit {
					assets: vec![ConcreteFungible { id: X1(Parent), amount: 123 }],
					effects: vec![
						BuyExecution {
							fees: All,
							weight: 0,
							debt: 123,
							halt_on_error: false,
							xcm: vec![],
						},
						DepositAsset {
							assets: vec![All],
							dest: X1(Junction::AccountId32 {
								network: NetworkId::Any,
								id: ALICE.into(),
							}),
						},
					],
				},
			);
			assert_eq!(parachain::MsgQueue::received_dmp(), vec![expected_message]);
			// check message execution with full amount received
			assert_eq!(
				pallet_balances::Pallet::<parachain::Runtime>::free_balance(&ALICE),
				INITIAL_BALANCE + 123
			);
		});
	}

	/// Scenario:
	/// A parachain transfers funds on the relaychain to another parachain's account.
	///
	/// Asserts that the parachain accounts are updated as expected.
	#[test]
	fn withdraw_and_deposit() {
		MockNet::reset();

		ParaA::execute_with(|| {
			let amount = 10;
			let weight = 3 * relay_chain::BaseXcmWeight::get();
			let message = WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset { assets: vec![All], dest: Parachain(2).into() },
				],
			};
			// Send withdraw and deposit
			assert_ok!(ParachainPalletXcm::send_xcm(Null, X1(Parent), message.clone(),));
		});

		Relay::execute_with(|| {
			let amount = 10;
			let para_account_a: relay_chain::AccountId = ParaId::from(1).into_account();
			assert_eq!(
				relay_chain::Balances::free_balance(para_account_a),
				INITIAL_BALANCE - amount
			);
			let para_account_b: relay_chain::AccountId = ParaId::from(2).into_account();
			assert_eq!(relay_chain::Balances::free_balance(para_account_b), 10);
		});
	}

	/// Scenario:
	/// A parachain wants to be notified that a transfer worked correctly.
	/// It sends a `QueryHolding` after the deposit to get notified on success.
	///
	/// Asserts that the balances are updated correctly and the expected XCM is sent.
	#[test]
	fn query_holding() {
		MockNet::reset();

		// First send a message which fails on the relay chain
		ParaA::execute_with(|| {
			let amount = 10;
			let weight = 3 * relay_chain::BaseXcmWeight::get();
			//let para_account_b: relay_chain::AccountId = ParaId::from(2).into_account();
			let query_id = 1234;
			let message = WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: OnlyChild.into(), // invalid destination
					},
					// is not triggered because the deposit fails
					Order::QueryHolding { query_id, dest: Parachain(2).into(), assets: vec![All] },
				],
			};
			// Send withdraw and deposit with query holding
			assert_ok!(ParachainPalletXcm::send_xcm(Null, X1(Parent), message.clone(),));
		});

		// Check that no transfer was executed and no response message was sent
		Relay::execute_with(|| {
			let amount = 10;
			let para_account_a: relay_chain::AccountId = ParaId::from(1).into_account();
			// withdraw did execute
			assert_eq!(
				relay_chain::Balances::free_balance(para_account_a),
				INITIAL_BALANCE - amount
			);
			// but deposit did not execute
			let para_account_b: relay_chain::AccountId = ParaId::from(2).into_account();
			assert_eq!(relay_chain::Balances::free_balance(para_account_b), 0);
		});

		// Now send a message which fully succeeds on the relay chain
		ParaA::execute_with(|| {
			let amount = 10;
			let weight = 3 * relay_chain::BaseXcmWeight::get();
			let query_id = 1234;
			let message = WithdrawAsset {
				assets: vec![ConcreteFungible { id: Null, amount }],
				effects: vec![
					buy_execution(weight),
					Order::DepositAsset {
						assets: vec![All],
						dest: Parachain(2).into(), // valid destination
					},
					Order::QueryHolding { query_id, dest: Parachain(2).into(), assets: vec![All] },
				],
			};
			// Send withdraw and deposit with query holding
			assert_ok!(ParachainPalletXcm::send_xcm(Null, X1(Parent), message.clone(),));
		});

		// Check that transfer was executed
		Relay::execute_with(|| {
			let spent = 20;
			let para_account_a: relay_chain::AccountId = ParaId::from(1).into_account();
			// withdraw did execute
			assert_eq!(
				relay_chain::Balances::free_balance(para_account_a),
				INITIAL_BALANCE - spent
			);
			// and deposit did execute
			let para_account_b: relay_chain::AccountId = ParaId::from(2).into_account();
			assert_eq!(relay_chain::Balances::free_balance(para_account_b), 10);
		});

		// Check that QueryResponse message was received
		ParaB::execute_with(|| {
			use xcm::opaque::v0::Response::Assets;
			assert_eq!(
				parachain::MsgQueue::received_dmp(),
				vec![(X1(Parent), QueryResponse { query_id: 1234, response: Assets(vec![]) })]
			);
		});
	}
}
