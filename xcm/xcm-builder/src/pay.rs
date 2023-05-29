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

//! `PayOverXcm` struct for paying through XCM and getting the status back.

use frame_support::traits::{
	tokens::{Pay, PaymentStatus},
	Get,
};
use sp_runtime::traits::Convert;
use sp_std::{marker::PhantomData, vec};
use xcm::{opaque::lts::Weight, prelude::*};
use xcm_executor::traits::{QueryHandler, QueryResponseStatus};

/// Implementation of the `frame_support::traits::tokens::Pay` trait, to allow
/// for XCM payments of a given `Balance` of `AssetKind` existing on a `DestinationChain` under
/// ownership of some `Interior` location of the local chain to a particular `Beneficiary`.
///
/// This relies on the XCM `TransferAsset` instruction. A trait `BeneficiaryToLocation` must be
/// provided in order to convert the `Beneficiary` value into a location usable by `TransferAsset`.
///
/// `PayOverXcm::pay` is asynchronous, and returns a `QueryId` which can then be used in
/// `check_payment` to check the status of the XCM transaction.
///
/// See also `PayAccountId32OverXcm` which is similar to this except that `BeneficiaryToLocation`
/// need not be supplied and `Beneficiary` must implement `Into<[u8; 32]>`.
pub struct PayOverXcm<
	DestinationChain,
	Interior,
	Router,
	Querier,
	Timeout,
	Beneficiary,
	AssetKind,
	BeneficiaryToLocation,
>(
	PhantomData<(
		DestinationChain,
		Interior,
		Router,
		Querier,
		Timeout,
		Beneficiary,
		AssetKind,
		BeneficiaryToLocation,
	)>,
);
impl<
		DestinationChain: Get<MultiLocation>,
		Interior: Get<InteriorMultiLocation>,
		Router: SendXcm,
		Querier: QueryHandler,
		Timeout: Get<Querier::BlockNumber>,
		Beneficiary: Clone,
		AssetKind: Into<AssetId>,
		BeneficiaryToLocation: Convert<Beneficiary, MultiLocation>,
	> Pay
	for PayOverXcm<
		DestinationChain,
		Interior,
		Router,
		Querier,
		Timeout,
		Beneficiary,
		AssetKind,
		BeneficiaryToLocation,
	>
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
		let destination = Querier::UniversalLocation::get()
			.invert_target(&destination_chain)
			.map_err(|()| Self::Error::LocationNotInvertible)?;
		let beneficiary = BeneficiaryToLocation::convert(who.clone());

		let query_id = Querier::new_query(destination_chain, Timeout::get(), Interior::get());

		let message = Xcm(vec![
			UnpaidExecution { weight_limit: Unlimited, check_origin: None },
			SetAppendix(Xcm(vec![ReportError(QueryResponseInfo {
				destination,
				query_id,
				max_weight: Weight::zero(),
			})])),
			DescendOrigin(Interior::get()),
			TransferAsset {
				beneficiary,
				assets: vec![MultiAsset {
					id: asset_kind.into(),
					fun: Fungibility::Fungible(amount),
				}]
				.into(),
			},
		]);

		let (ticket, _) = Router::validate(&mut Some(destination_chain), &mut Some(message))?;
		Router::deliver(ticket)?;
		Ok(query_id.into())
	}

	fn check_payment(id: Self::Id) -> PaymentStatus {
		use QueryResponseStatus::*;
		match Querier::take_response(id) {
			Ready { response, .. } => match response {
				Response::ExecutionResult(None) => PaymentStatus::Success,
				Response::ExecutionResult(Some(_)) => PaymentStatus::Failure,
				_ => PaymentStatus::Unknown,
			},
			Pending { .. } => PaymentStatus::InProgress,
			NotFound | UnexpectedVersion => PaymentStatus::Unknown,
		}
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn ensure_successful(_: &Self::Beneficiary, _: Self::Balance) {
		Querier::expect_response(id);
	}

	#[cfg(feature = "runtime-benchmarks")]
	fn ensure_concluded(id: Self::Id) {
		Querier::expect_response(id);
	}
}

/// Specialization of the [PayOverXcm] trait to allow `[u8; 32]`-based `AccountId` values to be
/// paid on a remote chain.
///
/// Implementation of the `frame_support::traits::tokens::Pay` trait, to allow
/// for XCM payments of a given `Balance` of `AssetKind` existing on a `DestinationChain` under
/// ownership of some `Interior` location of the local chain to a particular `Beneficiary`.
///
/// This relies on the XCM `TransferAsset` instruction. `Beneficiary` must implement
/// `Into<[u8; 32]>` (as 32-byte `AccountId`s generally do), and the actual XCM beneficiary will be
/// the location consisting of a single `AccountId32` junction with an appropriate account and no
/// specific network.
///
/// `PayOverXcm::pay` is asynchronous, and returns a `QueryId` which can then be used in
/// `check_payment` to check the status of the XCM transaction.
pub type PayAccountId32OverXcm<
	DestinationChain,
	Interior,
	Router,
	Querier,
	Timeout,
	Beneficiary,
	AssetKind,
> = PayOverXcm<
	DestinationChain,
	Interior,
	Router,
	Querier,
	Timeout,
	Beneficiary,
	AssetKind,
	crate::AliasesIntoAccountId32<(), Beneficiary>,
>;
