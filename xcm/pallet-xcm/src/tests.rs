use crate::mock::*;
use xcm::v0::{Xcm, Junction};
use xcm::opaque::v0::prelude::*;
use MultiLocation::*;
use frame_support::{assert_ok, assert_noop, traits::Currency};
use polkadot_parachain::primitives::{Id as ParaId, AccountIdConversion};

#[test]
fn send_fails_with_bad_origin() {
    new_test_ext().execute_with(|| {
		let amount = 5_000_000;
		let dest_weight = 2_000_000;
        assert_noop!(XcmPallet::send(Origin::none(), MultiLocation::Null, Xcm::ReserveAssetDeposit {
            assets: vec![ConcreteFungible { id: Parent.into(), amount }],
            effects: vec![
                BuyExecution {
                    fees: All,
                    weight: 0,
                    debt: dest_weight,
                    halt_on_error: false,
                    xcm: vec![],
                },
                DepositAsset { assets: vec![ All ], dest: Junction::AccountId32 { network: NetworkId::Kusama, id: ALICE.into() }.into() },
            ]
        }), frame_support::error::BadOrigin);
    });
}

#[test]
fn send_works() {
    new_test_ext().execute_with(|| {
		let amount = 5_000_000;
		let dest_weight = 2_000_000;
        // TODO: test sending simpler message?
        // test sending all messages
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
        let dest: MultiLocation = Junction::AccountId32 { network: NetworkId::Kusama, id: ALICE.into() }.into();
        let message =  Xcm::ReserveAssetDeposit {
            assets: vec![ConcreteFungible { id: Parent.into(), amount }],
            effects: vec![
                BuyExecution {
                    fees: All,
                    weight: 0,
                    debt: dest_weight,
                    halt_on_error: false,
                    xcm: vec![],
                },
                DepositAsset { assets: vec![ All ], dest: dest.clone() },
            ]
        };
        assert_ok!(XcmPallet::send(Origin::signed(ALICE), MultiLocation::Null, message.clone()));
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
        // why is dest in the `origin_location` spot?
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Sent(dest, MultiLocation::Null, message)));
    });
}

#[test]
fn teleport_assets_works() {
    new_test_ext().execute_with(|| {
		let amount = 5_000_000;
		let dest_weight = 2_000_000;
        // TODO: test sending simpler message?
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
        assert_ok!(XcmPallet::teleport_assets(
            Origin::signed(ALICE), 
            MultiLocation::Null,
            MultiLocation::Null,
            vec![ConcreteFungible { id: Null, amount }],
            dest_weight,
        ));
        let fee = 5000000;
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - fee);
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(2000u64))));
    });
}

#[test]
fn reserve_transfer_assets_works() {
	new_test_ext().execute_with(|| {
		let amount = 5_000_000;
		let dest_weight = 2_000_000;
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
		assert_ok!(XcmPallet::reserve_transfer_assets(
			Origin::signed(ALICE),
			Parachain(PARA_ID).into(),
			Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into(),
			vec![ConcreteFungible { id: Null, amount }],
			dest_weight
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
					BuyExecution {
						fees: All,
						weight: 0,
						debt: dest_weight,
						halt_on_error: false,
						xcm: vec![],
					},
					DepositAsset { assets: vec![ All ], dest: Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into() },
				]
			})]
		);
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(1000u64))));
	});
}

#[test]
fn execute_works() {
    new_test_ext().execute_with(|| {
		let amount = 5_000_000;
		let dest_weight = 2_000_000;
        // TODO: test sending simpler message?
        assert_ok!(XcmPallet::execute(Origin::signed(ALICE), Box::new(Xcm::WithdrawAsset {
            assets: vec![ConcreteFungible { id: Null, amount }],
            effects: vec![
                BuyExecution {
                    fees: All,
                    weight: 0,
                    debt: dest_weight,
                    halt_on_error: false,
                    xcm: vec![],
                },
                DepositAsset { assets: vec![ All ], dest: Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into() },
            ]
        }), 2_000_000));
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(3000))));
    });
}