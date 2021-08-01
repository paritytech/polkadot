use crate::mock::*;
use xcm::v0::{Xcm, Junction};
use xcm::opaque::v0::prelude::*;
use MultiLocation::*;
use frame_support::{assert_ok, traits::Currency};
use polkadot_parachain::primitives::{Id as ParaId, AccountIdConversion};

#[test]
fn send_works() {
    new_test_ext().execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 2 * BaseXcmWeight::get();
        let dest: MultiLocation = Junction::AccountId32 { network: AnyNetwork::get(), id: ALICE.into() }.into();
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
        let message =  Xcm::ReserveAssetDeposit {
            assets: vec![ConcreteFungible { id: Parent.into(), amount }],
            effects: vec![
                buy_execution(weight),
                DepositAsset { assets: vec![ All ], dest: dest.clone() },
            ]
        };
        assert_ok!(XcmPallet::send(Origin::signed(ALICE), RelayLocation::get(), message.clone()));
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE);
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Sent(dest, RelayLocation::get(), message)));
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
            RelayLocation::get(), // TODO: other location
            vec![ConcreteFungible { id: Null, amount }],
            weight,
        ));
        assert_eq!(Balances::total_balance(&ALICE), INITIAL_BALANCE - amount);
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(2000u64))));
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
			vec![ConcreteFungible { id: Null, amount }],
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
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(1000u64))));
	});
}

#[test]
fn execute_works() {
    new_test_ext().execute_with(|| {
		let amount = 10 * ExistentialDeposit::get();
		let weight = 3 * BaseXcmWeight::get();
        let dest: MultiLocation = Junction::AccountId32 { network: NetworkId::Any, id: ALICE.into() }.into();
        assert_ok!(XcmPallet::execute(Origin::signed(ALICE), Box::new(Xcm::WithdrawAsset {
            assets: vec![ConcreteFungible { id: Null, amount }],
            effects: vec![
                buy_execution(weight),
                DepositAsset { assets: vec![ All ], dest },
            ]
        }), 2_000_000));
        assert_eq!(last_event(), Event::XcmPallet(crate::Event::Attempted(Outcome::Complete(3000))));
    });
}