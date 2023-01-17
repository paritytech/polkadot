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

use frame_support::{
	ensure,
	traits::{Contains, Get},
};
use polkadot_parachain::primitives::IsSystem;
use sp_std::{marker::PhantomData, result::Result};
use xcm::latest::{
	Instruction::{self, *},
	InteriorMultiLocation, Junction, Junctions,
	Junctions::X1,
	MultiLocation, Weight,
	WeightLimit::*,
};
use xcm_executor::traits::{OnResponse, ShouldExecute};

/// Execution barrier that just takes `max_weight` from `weight_credit`.
///
/// Useful to allow XCM execution by local chain users via extrinsics.
/// E.g. `pallet_xcm::reserve_asset_transfer` to transfer a reserve asset
/// out of the local chain to another one.
pub struct TakeWeightCredit;
impl ShouldExecute for TakeWeightCredit {
	fn should_execute<RuntimeCall>(
		_origin: &MultiLocation,
		_instructions: &mut [Instruction<RuntimeCall>],
		max_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		log::trace!(
			target: "xcm::barriers",
			"TakeWeightCredit origin: {:?}, instructions: {:?}, max_weight: {:?}, weight_credit: {:?}",
			_origin, _instructions, max_weight, weight_credit,
		);
		*weight_credit = weight_credit.checked_sub(&max_weight).ok_or(())?;
		Ok(())
	}
}

/// Allows execution from `origin` if it is contained in `T` (i.e. `T::Contains(origin)`) taking
/// payments into account.
///
/// Only allows for `TeleportAsset`, `WithdrawAsset`, `ClaimAsset` and `ReserveAssetDeposit` XCMs
/// because they are the only ones that place assets in the Holding Register to pay for execution.
pub struct AllowTopLevelPaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowTopLevelPaidExecutionFrom<T> {
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<RuntimeCall>],
		max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		log::trace!(
			target: "xcm::barriers",
			"AllowTopLevelPaidExecutionFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, weight_credit: {:?}",
			origin, instructions, max_weight, _weight_credit,
		);

		ensure!(T::contains(origin), ());
		// We will read up to 5 instructions. This allows up to 3 `ClearOrigin` instructions. We
		// allow for more than one since anything beyond the first is a no-op and it's conceivable
		// that composition of operations might result in more than one being appended.
		let mut iter = instructions.iter_mut().take(5);
		let i = iter.next().ok_or(())?;
		match i {
			ReceiveTeleportedAsset(..) |
			WithdrawAsset(..) |
			ReserveAssetDeposited(..) |
			ClaimAsset { .. } => (),
			_ => return Err(()),
		}
		let mut i = iter.next().ok_or(())?;
		while let ClearOrigin = i {
			i = iter.next().ok_or(())?;
		}
		match i {
			BuyExecution { weight_limit: Limited(ref mut weight), .. }
				if weight.all_gte(max_weight) =>
			{
				*weight = weight.max(max_weight);
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

/// A derivative barrier, which scans the first `MaxPrefixes` instructions for origin-alterers and
/// then evaluates `should_execute` of the `InnerBarrier` based on the remaining instructions and
/// the newly computed origin.
///
/// This effectively allows for the possibility of distinguishing an origin which is acting as a
/// router for its derivative locations (or as a bridge for a remote location) and an origin which
/// is actually trying to send a message for itself. In the former case, the message will be
/// prefixed with origin-mutating instructions.
///
/// Any barriers which should be interpreted based on the computed origin rather than the original
/// message origin should be subject to this. This is the case for most barriers since the
/// effective origin is generally more important than the routing origin. Any other barriers, and
/// especially those which should be interpreted only the routing origin should not be subject to
/// this.
///
/// E.g.
/// ```nocompile
/// type MyBarrier = (
/// 	TakeWeightCredit,
/// 	AllowTopLevelPaidExecutionFrom<DirectCustomerLocations>,
/// 	WithComputedOrigin<(
/// 		AllowTopLevelPaidExecutionFrom<DerivativeCustomerLocations>,
/// 		AllowUnpaidExecutionFrom<ParentLocation>,
/// 		AllowSubscriptionsFrom<AllowedSubscribers>,
/// 		AllowKnownQueryResponses<TheResponseHandler>,
/// 	)>,
/// );
/// ```
///
/// In the above example, `AllowUnpaidExecutionFrom` appears once underneath
/// `WithComputedOrigin`. This is in order to distinguish between messages which are notionally
/// from a derivative location of `ParentLocation` but that just happened to be sent via
/// `ParentLocaction` rather than messages that were sent by the parent.
///
/// Similarly `AllowTopLevelPaidExecutionFrom` appears twice: once inside of `WithComputedOrigin`
/// where we provide the list of origins which are derivative origins, and then secondly outside
/// of `WithComputedOrigin` where we provide the list of locations which are direct origins. It's
/// reasonable for these lists to be merged into one and that used both inside and out.
///
/// Finally, we see `AllowSubscriptionsFrom` and `AllowKnownQueryResponses` are both inside of
/// `WithComputedOrigin`. This means that if a message begins with origin-mutating instructions,
/// then it must be the finally computed origin which we accept subscriptions or expect a query
/// response from. For example, even if an origin appeared in the `AllowedSubscribers` list, we
/// would ignore this rule if it began with origin mutators and they changed the origin to something
/// which was not on the list.
pub struct WithComputedOrigin<InnerBarrier, LocalUniversal, MaxPrefixes>(
	PhantomData<(InnerBarrier, LocalUniversal, MaxPrefixes)>,
);
impl<
		InnerBarrier: ShouldExecute,
		LocalUniversal: Get<InteriorMultiLocation>,
		MaxPrefixes: Get<u32>,
	> ShouldExecute for WithComputedOrigin<InnerBarrier, LocalUniversal, MaxPrefixes>
{
	fn should_execute<Call>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<Call>],
		max_weight: Weight,
		weight_credit: &mut Weight,
	) -> Result<(), ()> {
		log::trace!(
			target: "xcm::barriers",
			"WithComputedOrigin origin: {:?}, instructions: {:?}, max_weight: {:?}, weight_credit: {:?}",
			origin, instructions, max_weight, weight_credit,
		);
		let mut actual_origin = *origin;
		let mut skipped = 0;
		// NOTE: We do not check the validity of `UniversalOrigin` here, meaning that a malicious
		// origin could place a `UniversalOrigin` in order to spoof some location which gets free
		// execution. This technical could get it past the barrier condition, but the execution
		// would instantly fail since the first instruction would cause an error with the
		// invalid UniversalOrigin.
		while skipped < MaxPrefixes::get() as usize {
			match instructions.get(skipped) {
				Some(UniversalOrigin(new_global)) => {
					// Note the origin is *relative to local consensus*! So we need to escape local
					// consensus with the `parents` before diving in into the `universal_location`.
					actual_origin = X1(*new_global).relative_to(&LocalUniversal::get());
				},
				Some(DescendOrigin(j)) => {
					actual_origin.append_with(*j).map_err(|_| ())?;
				},
				_ => break,
			}
			skipped += 1;
		}
		InnerBarrier::should_execute(
			&actual_origin,
			&mut instructions[skipped..],
			max_weight,
			weight_credit,
		)
	}
}

/// Allows execution from any origin that is contained in `T` (i.e. `T::Contains(origin)`).
///
/// Use only for executions from completely trusted origins, from which no unpermissioned messages
/// can be sent.
pub struct AllowUnpaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowUnpaidExecutionFrom<T> {
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<RuntimeCall>],
		_max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		log::trace!(
			target: "xcm::barriers",
			"AllowUnpaidExecutionFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, weight_credit: {:?}",
			origin, instructions, _max_weight, _weight_credit,
		);
		ensure!(T::contains(origin), ());
		Ok(())
	}
}

/// Allows execution from any origin that is contained in `T` (i.e. `T::Contains(origin)`) if the
/// message begins with the instruction `UnpaidExecution`.
///
/// Use only for executions from trusted origin groups.
pub struct AllowExplicitUnpaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowExplicitUnpaidExecutionFrom<T> {
	fn should_execute<Call>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<Call>],
		max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		log::trace!(
			target: "xcm::barriers",
			"AllowExplicitUnpaidExecutionFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, weight_credit: {:?}",
			origin, instructions, max_weight, _weight_credit,
		);
		ensure!(T::contains(origin), ());
		match instructions.first() {
			Some(UnpaidExecution { weight_limit: Limited(m), .. }) if m.all_gte(max_weight) =>
				Ok(()),
			Some(UnpaidExecution { weight_limit: Unlimited, .. }) => Ok(()),
			_ => Err(()),
		}
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
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<RuntimeCall>],
		_max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		log::trace!(
			target: "xcm::barriers",
			"AllowKnownQueryResponses origin: {:?}, instructions: {:?}, max_weight: {:?}, weight_credit: {:?}",
			origin, instructions, _max_weight, _weight_credit,
		);
		ensure!(instructions.len() == 1, ());
		match instructions.first() {
			Some(QueryResponse { query_id, querier, .. })
				if ResponseHandler::expecting_response(origin, *query_id, querier.as_ref()) =>
				Ok(()),
			_ => Err(()),
		}
	}
}

/// Allows execution from `origin` if it is just a straight `SubscribeVersion` or
/// `UnsubscribeVersion` instruction.
pub struct AllowSubscriptionsFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowSubscriptionsFrom<T> {
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<RuntimeCall>],
		_max_weight: Weight,
		_weight_credit: &mut Weight,
	) -> Result<(), ()> {
		log::trace!(
			target: "xcm::barriers",
			"AllowSubscriptionsFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, weight_credit: {:?}",
			origin, instructions, _max_weight, _weight_credit,
		);
		ensure!(T::contains(origin), ());
		match instructions {
			&mut [SubscribeVersion { .. } | UnsubscribeVersion] => Ok(()),
			_ => Err(()),
		}
	}
}
