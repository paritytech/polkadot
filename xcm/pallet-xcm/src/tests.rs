use crate::mock::*;
use xcm::v0::{Xcm, Junction};
use xcm::opaque::v0::prelude::*;
use MultiLocation::*;
use frame_support::assert_ok;
use polkadot_parachain::primitives::{Id as ParaId, AccountIdConversion};


#[test]
fn reserve_transfer_assets_works() {
	test_ext().execute_with(|| {
		let amount = 5_000_000;
		let dest_weight = 2_000_000;
		assert_ok!(XcmPallet::reserve_transfer_assets(
			Origin::signed(ALICE),
			Parachain(PARA_ID).into(),
			Junction::AccountId32 { network: NetworkId::Kusama, id: ALICE.into() }.into(),
			vec![ConcreteFungible { id: Null, amount }],
			dest_weight
		));

		assert_eq!(Balances::free_balance(ALICE), INITIAL_BALANCE - amount);
		let para_acc: AccountId = ParaId::from(PARA_ID).into_account();
		assert_eq!(Balances::free_balance(para_acc), INITIAL_BALANCE + amount);
		assert_eq!(
			crate::mock::sent_xcm(),
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
					DepositAsset { assets: vec![ All ], dest: Junction::AccountId32 { network: NetworkId::Kusama, id: ALICE.into() }.into() },
				]
			})]
		);
	});
}