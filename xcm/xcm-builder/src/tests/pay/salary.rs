// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Tests for the integration between `PayOverXcm` and the salary pallet

use super::{mock::*, *};

use frame_support::{
	assert_ok,
	traits::{
		fungibles::{Create, Mutate},
		tokens::GetSalary,
		RankedMembers,
	},
};
use sp_runtime::{traits::ConvertToValue, BuildStorage, DispatchResult};

parameter_types! {
	pub Interior: InteriorMultiLocation = Plurality { id: BodyId::Treasury, part: BodyPart::Voice }.into();
	pub Timeout: BlockNumber = 5;
	pub AssetHub: MultiLocation = (Parent, Parachain(1)).into();
	pub AssetIdGeneralIndex: u128 = 100;
	pub AssetHubAssetId: AssetId = (PalletInstance(1), GeneralIndex(AssetIdGeneralIndex::get())).into();
	pub LocatableAsset: LocatableAssetId = LocatableAssetId { asset_id: AssetHubAssetId::get(), location: AssetHub::get() };
}

type SalaryPayOverXcm = PayOverXcm<
	Interior,
	TestMessageSender,
	TestQueryHandler<TestConfig, BlockNumber>,
	Timeout,
	AccountId,
	(),
	ConvertToValue<LocatableAsset>,
	AliasesIntoAccountId32<AnyNetwork, AccountId>,
>;

type Rank = u128;

thread_local! {
	pub static CLUB: RefCell<BTreeMap<AccountId, Rank>> = RefCell::new(BTreeMap::new());
}

pub struct TestClub;
impl RankedMembers for TestClub {
	type AccountId = AccountId;
	type Rank = Rank;

	fn min_rank() -> Self::Rank {
		0
	}
	fn rank_of(who: &Self::AccountId) -> Option<Self::Rank> {
		CLUB.with(|club| club.borrow().get(who).cloned())
	}
	fn induct(who: &Self::AccountId) -> DispatchResult {
		CLUB.with(|club| club.borrow_mut().insert(who.clone(), 0));
		Ok(())
	}
	fn promote(who: &Self::AccountId) -> DispatchResult {
		CLUB.with(|club| {
			club.borrow_mut().entry(who.clone()).and_modify(|rank| *rank += 1);
		});
		Ok(())
	}
	fn demote(who: &Self::AccountId) -> DispatchResult {
		CLUB.with(|club| match club.borrow().get(who) {
			None => Err(sp_runtime::DispatchError::Unavailable),
			Some(&0) => {
				club.borrow_mut().remove(&who);
				Ok(())
			},
			Some(_) => {
				club.borrow_mut().entry(who.clone()).and_modify(|rank| *rank += 1);
				Ok(())
			},
		})
	}
}

fn set_rank(who: AccountId, rank: u128) {
	CLUB.with(|club| club.borrow_mut().insert(who, rank));
}

parameter_types! {
	pub const RegistrationPeriod: BlockNumber = 2;
	pub const PayoutPeriod: BlockNumber = 2;
	pub const FixedSalaryAmount: Balance = 10_000;
	pub static Budget: Balance = FixedSalaryAmount::get();
}

pub struct FixedSalary;
impl GetSalary<Rank, AccountId, Balance> for FixedSalary {
	fn get_salary(_rank: Rank, _who: &AccountId) -> Balance {
		FixedSalaryAmount::get()
	}
}

impl pallet_salary::Config for Test {
	type WeightInfo = ();
	type RuntimeEvent = RuntimeEvent;
	type Paymaster = SalaryPayOverXcm;
	type Members = TestClub;
	type Salary = FixedSalary;
	type RegistrationPeriod = RegistrationPeriod;
	type PayoutPeriod = PayoutPeriod;
	type Budget = Budget;
}

fn new_test_ext() -> sp_io::TestExternalities {
	let t = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	let mut ext = sp_io::TestExternalities::new(t);
	ext.execute_with(|| System::set_block_number(1));
	ext
}

fn next_block() {
	System::set_block_number(System::block_number() + 1);
}

fn run_to(block_number: BlockNumber) {
	while System::block_number() < block_number {
		next_block();
	}
}

/// Scenario:
/// The salary pallet is used to pay a member over XCM.
/// The correct XCM message is generated and when executed in the remote chain,
/// the member receives the salary.
#[test]
fn salary_pay_over_xcm_works() {
	let admin_account_id = AccountId::new([0u8; 32]);
	let account_id = AccountId::new([1u8; 32]);
	let asset_id: u128 = 100;

	new_test_ext().execute_with(|| {
		// Create the asset and give enought to the sender
		assert_ok!(<mock::Assets as Create<_>>::create(
			asset_id,
			admin_account_id.clone(),
			true,
			FixedSalaryAmount::get() / 2,
		));
		assert_ok!(<mock::Assets as Mutate<_>>::mint_into(
			asset_id,
			&TreasuryAccountId::get(),
			FixedSalaryAmount::get() * 2
		));

		// Set the recipient as a member of a ranked collective
		set_rank(account_id.clone(), 1);

		// Check starting balance
		assert_eq!(mock::Assets::balance(AssetIdGeneralIndex::get(), &account_id), 0);

		// Use salary pallet to call `PayOverXcm::pay`
		assert_ok!(Salary::init(RuntimeOrigin::signed(account_id.clone())));
		run_to(5);
		assert_ok!(Salary::induct(RuntimeOrigin::signed(account_id.clone())));
		assert_ok!(Salary::bump(RuntimeOrigin::signed(account_id.clone())));
		assert_ok!(Salary::register(RuntimeOrigin::signed(account_id.clone())));
		run_to(7);
		assert_ok!(Salary::payout(RuntimeOrigin::signed(account_id.clone())));

		// Get message from mock transport layer
		let (_, message, hash) = sent_xcm()[0].clone();
		// Change type from `Xcm<()>` to `Xcm<RuntimeCall>` to be able to execute later
		let message =
			Xcm::<<XcmConfig as xcm_executor::Config>::RuntimeCall>::from(message.clone());

		let expected_message: Xcm<RuntimeCall> = Xcm::<RuntimeCall>(vec![
			DescendOrigin(Plurality { id: BodyId::Treasury, part: BodyPart::Voice }.into()),
			UnpaidExecution { weight_limit: Unlimited, check_origin: None },
			SetAppendix(Xcm(vec![ReportError(QueryResponseInfo {
				destination: (Parent, Parachain(42)).into(),
				query_id: 1,
				max_weight: Weight::zero(),
			})])),
			TransferAsset {
				assets: (AssetHubAssetId::get(), FixedSalaryAmount::get()).into(),
				beneficiary: AccountId32 { id: account_id.clone().into(), network: None }.into(),
			},
		]);
		assert_eq!(message, expected_message);

		// Execute message as the asset hub
		XcmExecutor::<XcmConfig>::execute_xcm((Parent, Parachain(42)), message, hash, Weight::MAX);

		// Recipient receives the payment
		assert_eq!(
			mock::Assets::balance(AssetIdGeneralIndex::get(), &account_id),
			FixedSalaryAmount::get()
		);
	});
}
