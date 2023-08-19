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

//! Tests for making sure `PayOverXcm::pay` generates the correct message and sends it to the
//! correct destination

use super::{mock::*, *};
use frame_support::{assert_ok, traits::tokens::Pay};

/// Type representing both a location and an asset that is held at that location.
/// The id of the held asset is relative to the location where it is being held.
#[derive(Encode, Decode, Clone, Copy, PartialEq, Eq)]
pub struct AssetKind {
	destination: MultiLocation,
	asset_id: AssetId,
}

pub struct LocatableAssetKindConverter;
impl sp_runtime::traits::Convert<AssetKind, LocatableAssetId> for LocatableAssetKindConverter {
	fn convert(value: AssetKind) -> LocatableAssetId {
		LocatableAssetId { asset_id: value.asset_id, location: value.destination }
	}
}

parameter_types! {
	pub SenderAccount: AccountId = AccountId::new([3u8; 32]);
	pub InteriorAccount: InteriorMultiLocation = AccountId32 { id: SenderAccount::get().into(), network: None }.into();
	pub InteriorBody: InteriorMultiLocation = Plurality { id: BodyId::Treasury, part: BodyPart::Voice }.into();
	pub Timeout: BlockNumber = 5; // 5 blocks
}

/// Scenario:
/// Account #3 on the local chain, parachain 42, controls an amount of funds on parachain 2.
/// [`PayOverXcm::pay`] creates the correct message for account #3 to pay another account, account
/// #5, on parachain 2, remotely, in its native token.
#[test]
fn pay_over_xcm_works() {
	let recipient = AccountId::new([5u8; 32]);
	let asset_kind =
		AssetKind { destination: (Parent, Parachain(2)).into(), asset_id: Here.into() };
	let amount = 10 * UNITS;

	new_test_ext().execute_with(|| {
		// Check starting balance
		assert_eq!(mock::Assets::balance(0, &recipient), 0);

		assert_ok!(PayOverXcm::<
			InteriorAccount,
			TestMessageSender,
			TestQueryHandler<TestConfig, BlockNumber>,
			Timeout,
			AccountId,
			AssetKind,
			LocatableAssetKindConverter,
			AliasesIntoAccountId32<AnyNetwork, AccountId>,
		>::pay(&recipient, asset_kind, amount));

		let expected_message = Xcm(vec![
			DescendOrigin(AccountId32 { id: SenderAccount::get().into(), network: None }.into()),
			UnpaidExecution { weight_limit: Unlimited, check_origin: None },
			SetAppendix(Xcm(vec![ReportError(QueryResponseInfo {
				destination: (Parent, Parachain(42)).into(),
				query_id: 1,
				max_weight: Weight::zero(),
			})])),
			TransferAsset {
				assets: (Here, amount).into(),
				beneficiary: AccountId32 { id: recipient.clone().into(), network: None }.into(),
			},
		]);
		let expected_hash = fake_message_hash(&expected_message);

		assert_eq!(
			sent_xcm(),
			vec![((Parent, Parachain(2)).into(), expected_message, expected_hash)]
		);

		let (_, message, hash) = sent_xcm()[0].clone();
		let message =
			Xcm::<<XcmConfig as xcm_executor::Config>::RuntimeCall>::from(message.clone());

		// Execute message in parachain 2 with parachain 42's origin
		let origin = (Parent, Parachain(42));
		XcmExecutor::<XcmConfig>::execute_xcm(origin, message, hash, Weight::MAX);
		assert_eq!(mock::Assets::balance(0, &recipient), amount);
	});
}

/// Scenario:
/// A pluralistic body, a Treasury, on the local chain, parachain 42, controls an amount of funds
/// on parachain 2. [`PayOverXcm::pay`] creates the correct message for the treasury to pay
/// another account, account #7, on parachain 2, remotely, in the relay's token.
#[test]
fn pay_over_xcm_governance_body() {
	let recipient = AccountId::new([7u8; 32]);
	let asset_kind =
		AssetKind { destination: (Parent, Parachain(2)).into(), asset_id: Parent.into() };
	let amount = 10 * UNITS;

	let relay_asset_index = 1;

	new_test_ext().execute_with(|| {
		// Check starting balance
		assert_eq!(mock::Assets::balance(relay_asset_index, &recipient), 0);

		assert_ok!(PayOverXcm::<
			InteriorBody,
			TestMessageSender,
			TestQueryHandler<TestConfig, BlockNumber>,
			Timeout,
			AccountId,
			AssetKind,
			LocatableAssetKindConverter,
			AliasesIntoAccountId32<AnyNetwork, AccountId>,
		>::pay(&recipient, asset_kind, amount));

		let expected_message = Xcm(vec![
			DescendOrigin(Plurality { id: BodyId::Treasury, part: BodyPart::Voice }.into()),
			UnpaidExecution { weight_limit: Unlimited, check_origin: None },
			SetAppendix(Xcm(vec![ReportError(QueryResponseInfo {
				destination: (Parent, Parachain(42)).into(),
				query_id: 1,
				max_weight: Weight::zero(),
			})])),
			TransferAsset {
				assets: (Parent, amount).into(),
				beneficiary: AccountId32 { id: recipient.clone().into(), network: None }.into(),
			},
		]);
		let expected_hash = fake_message_hash(&expected_message);
		assert_eq!(
			sent_xcm(),
			vec![((Parent, Parachain(2)).into(), expected_message, expected_hash)]
		);

		let (_, message, hash) = sent_xcm()[0].clone();
		let message =
			Xcm::<<XcmConfig as xcm_executor::Config>::RuntimeCall>::from(message.clone());

		// Execute message in parachain 2 with parachain 42's origin
		let origin = (Parent, Parachain(42));
		XcmExecutor::<XcmConfig>::execute_xcm(origin, message, hash, Weight::MAX);
		assert_eq!(mock::Assets::balance(relay_asset_index, &recipient), amount);
	});
}
