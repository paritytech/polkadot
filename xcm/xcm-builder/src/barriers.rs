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
use xcm::latest::{Instruction::*, Junction, Junctions, MultiLocation, WeightLimit::*, Xcm};
use xcm_executor::traits::{OnResponse, ShouldExecute};

/// Execution barrier that just takes `max_weight` from `weight_credit`.
pub struct TakeWeightCredit;
impl ShouldExecute for TakeWeightCredit {
	fn should_execute<Call>(
		_origin: &Option<MultiLocation>,
		_top_level: bool,
		_message: &mut Xcm<Call>,
		max_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		*weight_credit = weight_credit.checked_sub(max_weight).ok_or(())?;
		Ok(())
	}
}

/// Allows execution from `origin` if it is contained in `T` (i.e. `T::Contains(origin)`) taking
/// payments into account.
pub struct AllowTopLevelPaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowTopLevelPaidExecutionFrom<T> {
	fn should_execute<Call>(
		origin: &Option<MultiLocation>,
		top_level: bool,
		message: &mut Xcm<Call>,
		max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		let origin = origin.as_ref().ok_or(())?;
		ensure!(T::contains(origin), ());
		ensure!(top_level, ());
		let mut iter = message.0.iter_mut();
		let i = iter.next().ok_or(())?;
		match i {
			ReceiveTeleportedAsset { .. } | WithdrawAsset { .. } | ReserveAssetDeposited { .. } =>
				(),
			_ => return Err(()),
		}
		let mut i = iter.next().ok_or(())?;
		if let ClearOrigin = i {
			i = iter.next().ok_or(())?;
		}
		match i {
			BuyExecution { weight_limit: Limited(ref mut weight), .. } if *weight >= max_weight => {
				*weight = max_weight;
				Ok(())
			},
			BuyExecution { ref mut weight_limit, .. } if weight_limit == &Unlimited => {
				*weight_limit = Limited(max_weight);
				Ok(())
			},
			_ => Err(()),
		}
	}
}

/// Allows execution from any origin that is contained in `T` (i.e. `T::Contains(origin)`) without any payments.
/// Use only for executions from trusted origin groups.
pub struct AllowUnpaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowUnpaidExecutionFrom<T> {
	fn should_execute<Call>(
		origin: &Option<MultiLocation>,
		_top_level: bool,
		_message: &mut Xcm<Call>,
		_max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		let origin = origin.as_ref().ok_or(())?;
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
		origin: &Option<MultiLocation>,
		_top_level: bool,
		message: &mut Xcm<Call>,
		_max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		let origin = origin.as_ref().ok_or(())?;
		match message.0.first() {
			Some(QueryResponse { query_id, .. })
				if ResponseHandler::expecting_response(origin, *query_id) =>
				Ok(()),
			_ => Err(()),
		}
	}
}
