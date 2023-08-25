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

//! Various implementations for `ShouldExecute`.

use crate::{CreateMatcher, MatchXcm};
use frame_support::{
	ensure,
	traits::{Contains, Get, ProcessMessageError},
};
use polkadot_parachain::primitives::IsSystem;
use sp_std::{cell::Cell, marker::PhantomData, ops::ControlFlow, result::Result};
use xcm::prelude::*;
use xcm_executor::traits::{CheckSuspension, OnResponse, Properties, ShouldExecute};

/// Execution barrier that just takes `max_weight` from `properties.weight_credit`.
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
		properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		log::trace!(
			target: "xcm::barriers",
			"TakeWeightCredit origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
			_origin, _instructions, max_weight, properties,
		);
		properties.weight_credit = properties
			.weight_credit
			.checked_sub(&max_weight)
			.ok_or(ProcessMessageError::Overweight(max_weight))?;
		Ok(())
	}
}

const MAX_ASSETS_FOR_BUY_EXECUTION: usize = 1;

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
		_properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		log::trace!(
			target: "xcm::barriers",
			"AllowTopLevelPaidExecutionFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
			origin, instructions, max_weight, _properties,
		);

		ensure!(T::contains(origin), ProcessMessageError::Unsupported);
		// We will read up to 5 instructions. This allows up to 3 `ClearOrigin` instructions. We
		// allow for more than one since anything beyond the first is a no-op and it's conceivable
		// that composition of operations might result in more than one being appended.
		let end = instructions.len().min(5);
		instructions[..end]
			.matcher()
			.match_next_inst(|inst| match inst {
				ReceiveTeleportedAsset(..) | ReserveAssetDeposited(..) => Ok(()),
				WithdrawAsset(ref assets) if assets.len() <= MAX_ASSETS_FOR_BUY_EXECUTION => Ok(()),
				ClaimAsset { ref assets, .. } if assets.len() <= MAX_ASSETS_FOR_BUY_EXECUTION =>
					Ok(()),
				_ => Err(ProcessMessageError::BadFormat),
			})?
			.skip_inst_while(|inst| matches!(inst, ClearOrigin))?
			.match_next_inst(|inst| match inst {
				BuyExecution { weight_limit: Limited(ref mut weight), .. }
					if weight.all_gte(max_weight) =>
				{
					*weight = max_weight;
					Ok(())
				},
				BuyExecution { ref mut weight_limit, .. } if weight_limit == &Unlimited => {
					*weight_limit = Limited(max_weight);
					Ok(())
				},
				_ => Err(ProcessMessageError::Overweight(max_weight)),
			})?;
		Ok(())
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
		properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		log::trace!(
			target: "xcm::barriers",
			"WithComputedOrigin origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
			origin, instructions, max_weight, properties,
		);
		let mut actual_origin = *origin;
		let skipped = Cell::new(0usize);
		// NOTE: We do not check the validity of `UniversalOrigin` here, meaning that a malicious
		// origin could place a `UniversalOrigin` in order to spoof some location which gets free
		// execution. This technical could get it past the barrier condition, but the execution
		// would instantly fail since the first instruction would cause an error with the
		// invalid UniversalOrigin.
		instructions.matcher().match_next_inst_while(
			|_| skipped.get() < MaxPrefixes::get() as usize,
			|inst| {
				match inst {
					UniversalOrigin(new_global) => {
						// Note the origin is *relative to local consensus*! So we need to escape
						// local consensus with the `parents` before diving in into the
						// `universal_location`.
						actual_origin = X1(*new_global).relative_to(&LocalUniversal::get());
					},
					DescendOrigin(j) => {
						let Ok(_) = actual_origin.append_with(*j) else {
							return Err(ProcessMessageError::Unsupported)
						};
					},
					_ => return Ok(ControlFlow::Break(())),
				};
				skipped.set(skipped.get() + 1);
				Ok(ControlFlow::Continue(()))
			},
		)?;
		InnerBarrier::should_execute(
			&actual_origin,
			&mut instructions[skipped.get()..],
			max_weight,
			properties,
		)
	}
}

/// Sets the message ID to `t` using a `SetTopic(t)` in the last position if present.
///
/// Note that the message ID does not necessarily have to be unique; it is the
/// sender's responsibility to ensure uniqueness.
///
/// Requires some inner barrier to pass on the rest of the message.
pub struct TrailingSetTopicAsId<InnerBarrier>(PhantomData<InnerBarrier>);
impl<InnerBarrier: ShouldExecute> ShouldExecute for TrailingSetTopicAsId<InnerBarrier> {
	fn should_execute<Call>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<Call>],
		max_weight: Weight,
		properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		log::trace!(
			target: "xcm::barriers",
			"TrailingSetTopicAsId origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
			origin, instructions, max_weight, properties,
		);
		let until = if let Some(SetTopic(t)) = instructions.last() {
			properties.message_id = Some(*t);
			instructions.len() - 1
		} else {
			instructions.len()
		};
		InnerBarrier::should_execute(&origin, &mut instructions[..until], max_weight, properties)
	}
}

/// Barrier condition that allows for a `SuspensionChecker` that controls whether or not the XCM
/// executor will be suspended from executing the given XCM.
pub struct RespectSuspension<Inner, SuspensionChecker>(PhantomData<(Inner, SuspensionChecker)>);
impl<Inner, SuspensionChecker> ShouldExecute for RespectSuspension<Inner, SuspensionChecker>
where
	Inner: ShouldExecute,
	SuspensionChecker: CheckSuspension,
{
	fn should_execute<Call>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<Call>],
		max_weight: Weight,
		properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		if SuspensionChecker::is_suspended(origin, instructions, max_weight, properties) {
			Err(ProcessMessageError::Yield)
		} else {
			Inner::should_execute(origin, instructions, max_weight, properties)
		}
	}
}

/// Allows execution from any origin that is contained in `T` (i.e. `T::Contains(origin)`).
///
/// Use only for executions from completely trusted origins, from which no permissionless messages
/// can be sent.
pub struct AllowUnpaidExecutionFrom<T>(PhantomData<T>);
impl<T: Contains<MultiLocation>> ShouldExecute for AllowUnpaidExecutionFrom<T> {
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<RuntimeCall>],
		_max_weight: Weight,
		_properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		log::trace!(
			target: "xcm::barriers",
			"AllowUnpaidExecutionFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
			origin, instructions, _max_weight, _properties,
		);
		ensure!(T::contains(origin), ProcessMessageError::Unsupported);
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
		_properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		log::trace!(
			target: "xcm::barriers",
			"AllowExplicitUnpaidExecutionFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
			origin, instructions, max_weight, _properties,
		);
		ensure!(T::contains(origin), ProcessMessageError::Unsupported);
		instructions.matcher().match_next_inst(|inst| match inst {
			UnpaidExecution { weight_limit: Limited(m), .. } if m.all_gte(max_weight) => Ok(()),
			UnpaidExecution { weight_limit: Unlimited, .. } => Ok(()),
			_ => Err(ProcessMessageError::Overweight(max_weight)),
		})?;
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
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		instructions: &mut [Instruction<RuntimeCall>],
		_max_weight: Weight,
		_properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		log::trace!(
			target: "xcm::barriers",
			"AllowKnownQueryResponses origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
			origin, instructions, _max_weight, _properties,
		);
		instructions
			.matcher()
			.assert_remaining_insts(1)?
			.match_next_inst(|inst| match inst {
				QueryResponse { query_id, querier, .. }
					if ResponseHandler::expecting_response(origin, *query_id, querier.as_ref()) =>
					Ok(()),
				_ => Err(ProcessMessageError::BadFormat),
			})?;
		Ok(())
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
		_properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		log::trace!(
			target: "xcm::barriers",
			"AllowSubscriptionsFrom origin: {:?}, instructions: {:?}, max_weight: {:?}, properties: {:?}",
			origin, instructions, _max_weight, _properties,
		);
		ensure!(T::contains(origin), ProcessMessageError::Unsupported);
		instructions
			.matcher()
			.assert_remaining_insts(1)?
			.match_next_inst(|inst| match inst {
				SubscribeVersion { .. } | UnsubscribeVersion => Ok(()),
				_ => Err(ProcessMessageError::BadFormat),
			})?;
		Ok(())
	}
}

/// Deny executing the XCM if it matches any of the Deny filter regardless of anything else.
/// If it passes the Deny, and matches one of the Allow cases then it is let through.
pub struct DenyThenTry<Deny, Allow>(PhantomData<Deny>, PhantomData<Allow>)
where
	Deny: ShouldExecute,
	Allow: ShouldExecute;

impl<Deny, Allow> ShouldExecute for DenyThenTry<Deny, Allow>
where
	Deny: ShouldExecute,
	Allow: ShouldExecute,
{
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		message: &mut [Instruction<RuntimeCall>],
		max_weight: Weight,
		properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		Deny::should_execute(origin, message, max_weight, properties)?;
		Allow::should_execute(origin, message, max_weight, properties)
	}
}

// See issue <https://github.com/paritytech/polkadot/issues/5233>
pub struct DenyReserveTransferToRelayChain;
impl ShouldExecute for DenyReserveTransferToRelayChain {
	fn should_execute<RuntimeCall>(
		origin: &MultiLocation,
		message: &mut [Instruction<RuntimeCall>],
		_max_weight: Weight,
		_properties: &mut Properties,
	) -> Result<(), ProcessMessageError> {
		message.matcher().match_next_inst_while(
			|_| true,
			|inst| match inst {
				InitiateReserveWithdraw {
					reserve: MultiLocation { parents: 1, interior: Here },
					..
				} |
				DepositReserveAsset {
					dest: MultiLocation { parents: 1, interior: Here }, ..
				} |
				TransferReserveAsset {
					dest: MultiLocation { parents: 1, interior: Here }, ..
				} => {
					Err(ProcessMessageError::Unsupported) // Deny
				},

				// An unexpected reserve transfer has arrived from the Relay Chain. Generally,
				// `IsReserve` should not allow this, but we just log it here.
				ReserveAssetDeposited { .. }
					if matches!(origin, MultiLocation { parents: 1, interior: Here }) =>
				{
					log::warn!(
						target: "xcm::barrier",
						"Unexpected ReserveAssetDeposited from the Relay Chain",
					);
					Ok(ControlFlow::Continue(()))
				},

				_ => Ok(ControlFlow::Continue(())),
			},
		)?;

		// Permit everything else
		Ok(())
	}
}
