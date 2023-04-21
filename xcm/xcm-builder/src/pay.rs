// Copyright Parity Technologies (UK) Ltd.
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

//! `PayOverXcm` struct for paying through XCM and getting the status back

use frame_support::traits::{
	tokens::{Pay, PaymentStatus},
	Get,
};
use polkadot_primitives::AccountId;
use sp_std::{marker::PhantomData, vec};
use xcm::prelude::*;
use xcm_executor::traits::{FinishedQuery, QueryResponseStatus, XcmQueryHandler};

/// Implementation of the `frame_support_traits::tokens::Pay` trait, to allow
/// for generic payments of a given `AssetKind` and `Balance` from an implied origin, to a
/// beneficiary via XCM, relying on the XCM `TransferAsset` instruction.
///
/// `PayOverXcm::pay` is asynchronous, and returns a `QueryId` which can then be used in
/// `check_payment` to check the status of the XCM transaction.
///
pub struct PayOverXcm<
	DestinationChain,
	SenderAccount,
	Router,
	Querier,
	Timeout,
	Beneficiary,
	AssetKind,
>(
	PhantomData<(
		DestinationChain,
		SenderAccount,
		Router,
		Querier,
		Timeout,
		Beneficiary,
		AssetKind,
	)>,
);
impl<
		DestinationChain: Get<MultiLocation>,
		SenderAccount: Get<AccountId>,
		Router: SendXcm,
		Querier: XcmQueryHandler,
		Timeout: Get<Querier::BlockNumber>,
		Beneficiary: Into<[u8; 32]> + Clone,
		AssetKind: Into<AssetId>,
	> Pay
	for PayOverXcm<DestinationChain, SenderAccount, Router, Querier, Timeout, Beneficiary, AssetKind>
{
	type Beneficiary = Beneficiary;
	type AssetKind = AssetKind;
	type Balance = u128;
	type Id = Querier::QueryId;
	type Error = xcm::latest::Error;

	fn pay(
		who: &Self::Beneficiary,
		asset_kind: Self::AssetKind,
		amount: Self::Balance,
	) -> Result<Self::Id, Self::Error> {
		let destination_chain = DestinationChain::get();
		let sender_account = SenderAccount::get();
		let mut message = Xcm(vec![
			UnpaidExecution { weight_limit: Unlimited, check_origin: None },
			DescendOrigin(
				Junction::AccountId32 { network: None, id: sender_account.into() }.into(),
			),
			TransferAsset {
				beneficiary: AccountId32 { network: None, id: who.clone().into() }.into(),
				assets: vec![MultiAsset {
					id: asset_kind.into(),
					fun: Fungibility::Fungible(amount),
				}]
				.into(),
			},
		]);
		let id = Querier::report_outcome(&mut message, destination_chain, Timeout::get())
			.map_err(|_| Self::Error::LocationNotInvertible)?;
		let (ticket, _) = Router::validate(&mut Some(destination_chain), &mut Some(message))?;
		Router::deliver(ticket)?;
		Ok(id)
	}

	fn check_payment(id: Self::Id) -> PaymentStatus {
		match Querier::take_response(id) {
			QueryResponseStatus::Finished(FinishedQuery::Response { response, at: _ }) =>
				match response {
					Response::ExecutionResult(Some(_)) => PaymentStatus::Failure,
					Response::ExecutionResult(None) => PaymentStatus::Success,
					_ => PaymentStatus::Unknown,
				},
			QueryResponseStatus::Pending => PaymentStatus::InProgress,
			QueryResponseStatus::NotFound |
			QueryResponseStatus::UnexpectedVersion |
			QueryResponseStatus::Finished(FinishedQuery::VersionNotification { .. }) =>
				PaymentStatus::Unknown,
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn ensure_successful(_: &Self::Beneficiary, _: Self::Balance) {}

	#[cfg(feature = "runtime-benchmarks")]
	fn ensure_concluded(id: Self::Id) {
		Querier::expect_response(id);
	}
}
