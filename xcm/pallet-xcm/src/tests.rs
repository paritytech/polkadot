use crate::mock::*;
use xcm::v0::{Xcm, Junction};
use xcm::opaque::v0::prelude::*;
use frame_support::{assert_ok, assert_noop, traits::Currency};
use polkadot_parachain::primitives::{Id as ParaId, AccountIdConversion};

#[test]
fn send_reserve_asset_deposit_works() {
    new_test_ext().execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 2 * BaseXcmWeight::get();
        let sender: MultiLocation = Junction::AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
        let message =  Xcm::ReserveAssetDeposit {
            assets: vec![ConcreteFungible { id: Parent.into(), amount }],
            effects: vec![
                buy_execution(weight),
                DepositAsset { assets: vec![ All ], dest: sender.clone() },
            ]
        };
        assert_ok!(XcmPallet::send(Origin::signed(ALICE), RelayLocation::get(), message.clone()));
        assert_eq!(
			sent_xcm(),
            vec![(
                MultiLocation::Null,
                RelayedFrom { who: sender.clone(), message: Box::new(message.clone()) }
            )]
		);
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Sent(sender, RelayLocation::get(), message)));
    });
}

#[test]
fn send_fails_when_xcm_router_blocks() {
    new_test_ext().execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 2 * BaseXcmWeight::get();
        let sender: MultiLocation = Junction::AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
        let message =  Xcm::ReserveAssetDeposit {
            assets: vec![ConcreteFungible { id: Parent.into(), amount }],
            effects: vec![
                buy_execution(weight),
                DepositAsset { assets: vec![ All ], dest: sender.clone() },
            ]
        };
        assert_noop!(
            XcmPallet::send(Origin::signed(ALICE), X3(Junction::Parent, Junction::Parent, Junction::Parent), message.clone()),
            crate::Error::<Test>::SendFailure
        );
    });
}

#[test]
fn teleport_assets_works() {
    new_test_ext().execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 2 * BaseXcmWeight::get();
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
        assert_ok!(XcmPallet::teleport_assets(
            Origin::signed(ALICE), 
            RelayLocation::get(),
            RelayLocation::get(),
            vec![ConcreteFungible { id: MultiLocation::Null, amount }],
            weight,
        ));
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - amount);
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight))));
    });
}

#[test]
fn reserve_transfer_assets_works() {
	new_test_ext().execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = BaseXcmWeight::get();
        let dest: MultiLocation = Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::reserve_transfer_assets(
			Origin::signed(ALICE),
			Parachain(PARA_ID).into(),
			dest.clone(),
			vec![ConcreteFungible { id: MultiLocation::Null, amount }],
			weight
		));
        // Alice spent amount
		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - amount);
        // Destination account (parachain account) has amount
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + amount);
		assert_eq!(
			sent_xcm(),
			vec![(Parachain(PARA_ID).into(), Xcm::ReserveAssetDeposit {
				assets: vec![ConcreteFungible { id: Parent.into(), amount }],
				effects: vec![
					buy_execution(weight),
					DepositAsset { assets: vec![ All ], dest },
				]
			})]
		);
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight))));
	});
}

#[test]
fn execute_works() {
    new_test_ext().execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 3 * BaseXcmWeight::get();
        let dest: MultiLocation = Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
        assert_ok!(XcmPallet::execute(Origin::signed(ALICE), Box::new(Xcm::WithdrawAsset {
            assets: vec![ConcreteFungible { id: MultiLocation::Null, amount }],
            effects: vec![
                buy_execution(weight),
                DepositAsset { assets: vec![ All ], dest },
            ]
        }), weight));
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(weight))));
    });
}