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
use sp_runtime::traits::AccountIdConversion;
use xcm_simulator::{decl_test_network, decl_test_parachain, decl_test_relay_chain};

pub const ALICE: sp_runtime::AccountId32 = sp_runtime::AccountId32::new([0u8; 32]);
pub const INITIAL_BALANCE: u128 = 1_000_000_000;

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

pub fn para_account_id(id: u32) -> relay_chain::AccountId {
	ParaId::from(id).into_account()
}

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
		balances: vec![(ALICE, INITIAL_BALANCE), (para_account_id(1), INITIAL_BALANCE)],
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
	use xcm::latest::prelude::*;
	use xcm_simulator::TestExt;

	// Helper function for forming buy execution message
	fn buy_execution<C>(fees: impl Into<MultiAsset>, debt: Weight) -> Order<C> {
		Order::BuyExecution {
			fees: fees.into(),
			weight: 0,
			debt,
			halt_on_error: false,
			orders: vec![],
			instructions: vec![],
		}
	}

	#[test]
	fn dmp() {
		MockNet::reset();

		let remark = parachain::Call::System(
			frame_system::Call::<parachain::Runtime>::remark_with_event(vec![1, 2, 3]),
		);
		Relay::execute_with(|| {
			assert_ok!(RelayChainPalletXcm::send_xcm(
				Here.into(),
				Parachain(1).into(),
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
				Here.into(),
				Parent.into(),
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
				Here.into(),
				MultiLocation::new(1, X1(Parachain(2))),
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

		let withdraw_amount = 123;
		let max_weight_for_execution = 3;

		Relay::execute_with(|| {
			assert_ok!(RelayChainPalletXcm::reserve_transfer_assets(
				relay_chain::Origin::signed(ALICE),
				Box::new(X1(Parachain(1)).into()),
				Box::new(X1(AccountId32 { network: Any, id: ALICE.into() }).into()),
				(Here, withdraw_amount).into(),
				0,
				max_weight_for_execution,
			));
			assert_eq!(
				parachain::Balances::free_balance(&para_account_id(1)),
				INITIAL_BALANCE + withdraw_amount
			);
		});

		ParaA::execute_with(|| {
			// Check message received
			let expected_message = (
				X1(Parent),
				ReserveAssetDeposit {
					assets: vec![ConcreteFungible { id: X1(Parent), amount: withdraw_amount }],
					effects: vec![
						buy_execution(max_weight_for_execution),
						Order::DepositAsset {
							assets: All.into(),
							max_assets: 1,
							beneficiary: X1(Junction::AccountId32 {
								network: NetworkId::Any,
								id: ALICE.into(),
							}).into(),
						},
					],
				},
			);
			assert_eq!(parachain::MsgQueue::received_dmp(), vec![expected_message]);
			// Check message execution with full amount received
			assert_eq!(
				pallet_balances::Pallet::<parachain::Runtime>::free_balance(&ALICE),
				INITIAL_BALANCE + withdraw_amount
			);
		});
	}

	/// Scenario:
	/// A parachain transfers funds on the relay chain to another parachain account.
	///
	/// Asserts that the parachain accounts are updated as expected.
	#[test]
	fn withdraw_and_deposit() {
		MockNet::reset();

		let send_amount = 10;
		let weight_for_execution = 3 * relay_chain::BaseXcmWeight::get();

		ParaA::execute_with(|| {
			let message = WithdrawAsset {
				assets: (Here, send_amount).into(),
				effects: vec![
					buy_execution((Here, send_amount), weight_for_execution),
					Order::DepositAsset {
						assets: All.into(),
						max_assets: 1,
						beneficiary: Parachain(2).into(),
					},
				],
			};
			// Send withdraw and deposit
			assert_ok!(ParachainPalletXcm::send_xcm(Here.into(), Parent.into(), message.clone()));
		});

		Relay::execute_with(|| {
			assert_eq!(
				relay_chain::Balances::free_balance(para_account_id(1)),
				INITIAL_BALANCE - send_amount
			);
			assert_eq!(relay_chain::Balances::free_balance(para_account_id(2)), send_amount);
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

		let send_amount = 10;
		let weight_for_execution = 3 * relay_chain::BaseXcmWeight::get();
		let query_id_set = 1234;

		// Send a message which fully succeeds on the relay chain
		ParaA::execute_with(|| {
			let message = WithdrawAsset {
				assets: (Here, send_amount).into(),
				effects: vec![
					buy_execution((Here, send_amount), weight_for_execution),
					Order::DepositAsset {
						assets: All.into(),
						max_assets: 1,
						beneficiary: Parachain(2).into(),
					},
					Order::QueryHolding {
						query_id: query_id_set,
						dest: Parachain(1).into(),
						assets: All.into(),
					},
				],
			};
			// Send withdraw and deposit with query holding
			assert_ok!(ParachainPalletXcm::send_xcm(Here.into(), Parent.into(), message.clone(),));
		});

		// Check that transfer was executed
		Relay::execute_with(|| {
			// Withdraw executed
			assert_eq!(
				relay_chain::Balances::free_balance(para_account_id(1)),
				INITIAL_BALANCE - send_amount
			);
			// Deposit executed
			assert_eq!(relay_chain::Balances::free_balance(para_account_id(2)), send_amount);
		});

		// Check that QueryResponse message was received
		ParaA::execute_with(|| {
			assert_eq!(
				parachain::MsgQueue::received_dmp(),
				vec![(
					Parent.into(),
					QueryResponse {
						query_id: query_id_set,
						response: Response::Assets(MultiAssets::new())
					}
				)]
			);
		});
	}
}
