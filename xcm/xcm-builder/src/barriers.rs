// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Various implementations for `ShouldExecute`.

use frame_support::{ensure, traits::Contains, weights::Weight};
use polkadot_parachain::primitives::IsSystem;
use sp_std::{marker::PhantomData, result::Result};
use xcm::latest::{Junction, Junctions, MultiLocation, Order, Xcm};
use xcm_executor::traits::{OnResponse, ShouldExecute};

/// Execution barrier that just takes `shallow_weight` from `weight_credit`.
///
/// Useful to allow XCM execution by local chain users via extrinsics.
/// E.g. `pallet_xcm::reserve_asset_transfer` to transfer a reserve asset
/// out of the local chain to another one.
pub struct TakeWeightCredit;
impl ShouldExecute for TakeWeightCredit {
	fn should_execute<Call>(
		_origin: &MultiLocation,
		_top_level: bool,
		_message: &Xcm<Call>,
		shallow_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		*weight_credit = weight_credit.checked_sub(shallow_weight).ok_or(())?;
		Ok(())
	}
}

/// Allows execution from `origin` if it is contained in `T` (i.e. `T::Contains(origin)`) taking payments into
/// account.
///
/// Only allows for `TeleportAsset`, `WithdrawAsset` and `ReserveAssetDeposit` XCMs because they are the only ones
/// that place assets in the Holding Register to pay for execution.
pub struct AllowTopLevelPaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowTopLevelPaidExecutionFrom<T> {
	fn should_execute<Call>(
		origin: &MultiLocation,
		top_level: bool,
		message: &Xcm<Call>,
		shallow_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		ensure!(T::contains(origin), ());
		ensure!(top_level, ());
		match message {
			Xcm::ReceiveTeleportedAsset { effects, .. } |
			Xcm::WithdrawAsset { effects, .. } |
			Xcm::ReserveAssetDeposited { effects, .. }
				if matches!(
					effects.first(),
					Some(Order::BuyExecution { debt, ..}) if *debt >= shallow_weight
				) =>
				Ok(()),
			_ => Err(()),
		}
	}
}

/// Allows execution from any origin that is contained in `T` (i.e. `T::Contains(origin)`) without any payments.
/// Use only for executions from trusted origin groups.
pub struct AllowUnpaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowUnpaidExecutionFrom<T> {
	fn should_execute<Call>(
		origin: &MultiLocation,
		_top_level: bool,
		_message: &Xcm<Call>,
		_shallow_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		ensure!(T::contains(origin), ());
		Ok(())
	}
}

/// Allows a message only if it is from a system-level child parachain.
pub struct IsChildSystemParachain<ParaId>(PhantomData<ParaId>);
impl<ParaId: IsSystem + From<u32>> Contains<MultiLocation> for IsChildSystemParachain<ParaId> {
	fn contains(l: &MultiLocation) -> bool {
		matches!(
			l.interior(),
			Junctions::X1(Junction::Parachain(id))
				if ParaId::from(*id).is_system() && l.parent_count() == 0,
		)
	}
}

/// Allows only messages if the generic `ResponseHandler` expects them via `expecting_response`.
pub struct AllowKnownQueryResponses<ResponseHandler>(PhantomData<ResponseHandler>);
impl<ResponseHandler: OnResponse> ShouldExecute for AllowKnownQueryResponses<ResponseHandler> {
	fn should_execute<Call>(
		origin: &MultiLocation,
		_top_level: bool,
		message: &Xcm<Call>,
		_shallow_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		match message {
			Xcm::QueryResponse { query_id, .. }
				if ResponseHandler::expecting_response(origin, *query_id) =>
				Ok(()),
			_ => Err(()),
		}
	}
}

/// Allows execution from `origin` if it is just a straight `SubscribeVerison` or
/// `UnsubscribeVersion` instruction.
pub struct AllowSubscriptionsFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowSubscriptionsFrom<T> {
	fn should_execute<Call>(
		origin: &MultiLocation,
		top_level: bool,
		message: &Xcm<Call>,
		_max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		ensure!(T::contains(origin), ());
		ensure!(top_level, ());
		match message {
			Xcm::SubscribeVersion { .. } | Xcm::UnsubscribeVersion => Ok(()),
			_ => Err(()),
		}
	}
}
