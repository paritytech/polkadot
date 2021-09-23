// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Cumulus.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Cumulus.  If not, see <http://www.gnu.org/licenses/>.

//! Cross-Consensus Message format data structures.

use core::result;
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

use super::*;

#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, Debug, TypeInfo)]
pub enum Error {
	Undefined,
	/// An arithmetic overflow happened.
	Overflow,
	/// The operation is intentionally unsupported.
	Unimplemented,
	UnhandledXcmVersion,
	/// The implementation does not handle a given XCM.
	UnhandledXcmMessage,
	/// The implementation does not handle an effect present in an XCM.
	UnhandledEffect,
	EscalationOfPrivilege,
	UntrustedReserveLocation,
	UntrustedTeleportLocation,
	DestinationBufferOverflow,
	MultiLocationFull,
	MultiLocationNotInvertible,
	FailedToDecode,
	BadOrigin,
	ExceedsMaxMessageSize,
	/// An asset transaction (like withdraw or deposit) failed.
	/// See implementers of the `TransactAsset` trait for sources.
	/// Causes can include type conversion failures between id or balance types.
	FailedToTransactAsset(#[codec(skip)] &'static str),
	/// Execution of the XCM would potentially result in a greater weight used than the pre-specified
	/// weight limit. The amount that is potentially required is the parameter.
	WeightLimitReached(Weight),
	/// An asset wildcard was passed where it was not expected (e.g. as the asset to withdraw in a
	/// `WithdrawAsset` XCM).
	Wildcard,
	/// The case where an XCM message has specified a optional weight limit and the weight required for
	/// processing is too great.
	///
	/// Used by:
	/// - `Transact`
	TooMuchWeightRequired,
	/// The fees specified by the XCM message were not found in the holding register.
	///
	/// Used by:
	/// - `BuyExecution`
	NotHoldingFees,
	/// The weight of an XCM message is not computable ahead of execution. This generally means at least part
	/// of the message is invalid, which could be due to it containing overly nested structures or an invalid
	/// nested data segment (e.g. for the call in `Transact`).
	WeightNotComputable,
	/// The XCM did not pass the barrier condition for execution. The barrier condition differs on different
	/// chains and in different circumstances, but generally it means that the conditions surrounding the message
	/// were not such that the chain considers the message worth spending time executing. Since most chains
	/// lift the barrier to execution on appropriate payment, presentation of an NFT voucher, or based on the
	/// message origin, it means that none of those were the case.
	Barrier,
	/// Indicates that it is not possible for a location to have an asset be withdrawn or transferred from its
	/// ownership. This probably means it doesn't own (enough of) it, but may also indicate that it is under a
	/// lock, hold, freeze or is otherwise unavailable.
	NotWithdrawable,
	/// Indicates that the consensus system cannot deposit an asset under the ownership of a particular location.
	LocationCannotHold,
	/// The assets given to purchase weight is are insufficient for the weight desired.
	TooExpensive,
	/// The given asset is not handled.
	AssetNotFound,
	/// The given message cannot be translated into a format that the destination can be expected to interpret.
	DestinationUnsupported,
	/// `execute_xcm` has been called too many times recursively.
	RecursionLimitReached,
	/// Destination is routable, but there is some issue with the transport mechanism.
	///
	/// A human-readable explanation of the specific issue is provided.
	Transport(#[codec(skip)] &'static str),
	/// Destination is known to be unroutable.
	Unroutable,
	/// The weight required was not specified when it should have been.
	UnknownWeightRequired,
	/// An error was intentionally forced. A code is included.
	Trap(u64),
	/// The given claim could not be recognized/found.
	UnknownClaim,
	/// The location given was invalid for some reason specific to the operation at hand.
	InvalidLocation,
}

impl From<()> for Error {
	fn from(_: ()) -> Self {
		Self::Undefined
	}
}

impl From<SendError> for Error {
	fn from(e: SendError) -> Self {
		match e {
			SendError::CannotReachDestination(..) | SendError::Unroutable => Error::Unroutable,
			SendError::Transport(s) => Error::Transport(s),
			SendError::DestinationUnsupported => Error::DestinationUnsupported,
			SendError::ExceedsMaxMessageSize => Error::ExceedsMaxMessageSize,
		}
	}
}

pub type Result = result::Result<(), Error>;

/// Local weight type; execution time in picoseconds.
pub type Weight = u64;

/// Outcome of an XCM execution.
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, TypeInfo)]
pub enum Outcome {
	/// Execution completed successfully; given weight was used.
	Complete(Weight),
	/// Execution started, but did not complete successfully due to the given error; given weight was used.
	Incomplete(Weight, Error),
	/// Execution did not start due to the given error.
	Error(Error),
}

impl Outcome {
	pub fn ensure_complete(self) -> Result {
		match self {
			Outcome::Complete(_) => Ok(()),
			Outcome::Incomplete(_, e) => Err(e),
			Outcome::Error(e) => Err(e),
		}
	}
	pub fn ensure_execution(self) -> result::Result<Weight, Error> {
		match self {
			Outcome::Complete(w) => Ok(w),
			Outcome::Incomplete(w, _) => Ok(w),
			Outcome::Error(e) => Err(e),
		}
	}
	/// How much weight was used by the XCM execution attempt.
	pub fn weight_used(&self) -> Weight {
		match self {
			Outcome::Complete(w) => *w,
			Outcome::Incomplete(w, _) => *w,
			Outcome::Error(_) => 0,
		}
	}
}

/// Type of XCM message executor.
pub trait ExecuteXcm<Call> {
	/// Execute some XCM `message` from `origin` using no more than `weight_limit` weight. The weight limit is
	/// a basic hard-limit and the implementation may place further restrictions or requirements on weight and
	/// other aspects.
	fn execute_xcm(origin: MultiLocation, message: Xcm<Call>, weight_limit: Weight) -> Outcome {
		log::debug!(
			target: "xcm::execute_xcm",
			"origin: {:?}, message: {:?}, weight_limit: {:?}",
			origin,
			message,
			weight_limit,
		);
		Self::execute_xcm_in_credit(origin, message, weight_limit, 0)
	}

	/// Execute some XCM `message` from `origin` using no more than `weight_limit` weight.
	///
	/// Some amount of `weight_credit` may be provided which, depending on the implementation, may allow
	/// execution without associated payment.
	fn execute_xcm_in_credit(
		origin: MultiLocation,
		message: Xcm<Call>,
		weight_limit: Weight,
		weight_credit: Weight,
	) -> Outcome;
}

impl<C> ExecuteXcm<C> for () {
	fn execute_xcm_in_credit(
		_origin: MultiLocation,
		_message: Xcm<C>,
		_weight_limit: Weight,
		_weight_credit: Weight,
	) -> Outcome {
		Outcome::Error(Error::Unimplemented)
	}
}

/// Error result value when attempting to send an XCM message.
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, scale_info::TypeInfo)]
pub enum SendError {
	/// The message and destination combination was not recognized as being reachable.
	///
	/// This is not considered fatal: if there are alternative transport routes available, then
	/// they may be attempted. For this reason, the destination and message are contained.
	CannotReachDestination(MultiLocation, Xcm<()>),
	/// Destination is routable, but there is some issue with the transport mechanism. This is
	/// considered fatal.
	/// A human-readable explanation of the specific issue is provided.
	Transport(#[codec(skip)] &'static str),
	/// Destination is known to be unroutable. This is considered fatal.
	Unroutable,
	/// The given message cannot be translated into a format that the destination can be expected
	/// to interpret.
	DestinationUnsupported,
	/// Message could not be sent due to its size exceeding the maximum allowed by the transport
	/// layer.
	ExceedsMaxMessageSize,
}

/// Result value when attempting to send an XCM message.
pub type SendResult = result::Result<(), SendError>;

/// Utility for sending an XCM message.
///
/// These can be amalgamated in tuples to form sophisticated routing systems. In tuple format, each router might return
/// `CannotReachDestination` to pass the execution to the next sender item. Note that each `CannotReachDestination`
/// might alter the destination and the XCM message for to the next router.
///
///
/// # Example
/// ```rust
/// # use xcm::v2::prelude::*;
/// # use parity_scale_codec::Encode;
///
/// /// A sender that only passes the message through and does nothing.
/// struct Sender1;
/// impl SendXcm for Sender1 {
///     fn send_xcm(destination: MultiLocation, message: Xcm<()>) -> SendResult {
///         return Err(SendError::CannotReachDestination(destination, message))
///     }
/// }
///
/// /// A sender that accepts a message that has an X2 junction, otherwise stops the routing.
/// struct Sender2;
/// impl SendXcm for Sender2 {
///     fn send_xcm(destination: MultiLocation, message: Xcm<()>) -> SendResult {
///         if let MultiLocation { parents: 0, interior: X2(j1, j2) } = destination {
///             Ok(())
///         } else {
///             Err(SendError::Unroutable)
///         }
///     }
/// }
///
/// /// A sender that accepts a message from a parent, passing through otherwise.
/// struct Sender3;
/// impl SendXcm for Sender3 {
///     fn send_xcm(destination: MultiLocation, message: Xcm<()>) -> SendResult {
///         match destination {
///             MultiLocation { parents: 1, interior: Here } => Ok(()),
///             _ => Err(SendError::CannotReachDestination(destination, message)),
///         }
///     }
/// }
///
/// // A call to send via XCM. We don't really care about this.
/// # fn main() {
/// let call: Vec<u8> = ().encode();
/// let message = Xcm(vec![Instruction::Transact {
///     origin_type: OriginKind::Superuser,
///     require_weight_at_most: 0,
///     call: call.into(),
/// }]);
/// let destination = MultiLocation::parent();
///
/// assert!(
///     // Sender2 will block this.
///     <(Sender1, Sender2, Sender3) as SendXcm>::send_xcm(destination.clone(), message.clone())
///         .is_err()
/// );
///
/// assert!(
///     // Sender3 will catch this.
///     <(Sender1, Sender3) as SendXcm>::send_xcm(destination.clone(), message.clone())
///         .is_ok()
/// );
/// # }
/// ```
pub trait SendXcm {
	/// Send an XCM `message` to a given `destination`.
	///
	/// If it is not a destination which can be reached with this type but possibly could by others, then it *MUST*
	/// return `CannotReachDestination`. Any other error will cause the tuple implementation to exit early without
	/// trying other type fields.
	fn send_xcm(destination: MultiLocation, message: Xcm<()>) -> SendResult;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl SendXcm for Tuple {
	fn send_xcm(destination: MultiLocation, message: Xcm<()>) -> SendResult {
		for_tuples!( #(
			// we shadow `destination` and `message` in each expansion for the next one.
			let (destination, message) = match Tuple::send_xcm(destination, message) {
				Err(SendError::CannotReachDestination(d, m)) => (d, m),
				o @ _ => return o,
			};
		)* );
		Err(SendError::CannotReachDestination(destination, message))
	}
}

/// The info needed to weight an XCM.
// TODO: Automate Generation
pub trait XcmWeightInfo<Call> {
	fn withdraw_asset(assets: &MultiAssets) -> Weight;
	fn reserve_asset_deposited(assets: &MultiAssets) -> Weight;
	fn receive_teleported_asset(assets: &MultiAssets) -> Weight;
	fn query_response(query_id: &u64, response: &Response, max_weight: &u64) -> Weight;
	fn transfer_asset(assets: &MultiAssets, beneficiary: &MultiLocation) -> Weight;
	fn transfer_reserve_asset(assets: &MultiAssets, dest: &MultiLocation, xcm: &Xcm<()>) -> Weight;
	fn transact(
		origin_type: &OriginKind,
		require_weight_at_most: &u64,
		call: &DoubleEncoded<Call>,
	) -> Weight;
	fn hrmp_new_channel_open_request(
		sender: &u32,
		max_message_size: &u32,
		max_capacity: &u32,
	) -> Weight;
	fn hrmp_channel_accepted(recipient: &u32) -> Weight;
	fn hrmp_channel_closing(initiator: &u32, sender: &u32, recipient: &u32) -> Weight;
	fn clear_origin() -> Weight;
	fn descend_origin(who: &InteriorMultiLocation) -> Weight;
	fn report_error(query_id: &QueryId, dest: &MultiLocation, max_response_weight: &u64) -> Weight;
	fn relayed_from(who: &Junctions, message: &alloc::boxed::Box<Xcm<Call>>) -> Weight;
	fn deposit_asset(
		assets: &MultiAssetFilter,
		max_assets: &u32,
		beneficiary: &MultiLocation,
	) -> Weight;
	fn deposit_reserve_asset(
		assets: &MultiAssetFilter,
		max_assets: &u32,
		dest: &MultiLocation,
		xcm: &Xcm<()>,
	) -> Weight;
	fn exchange_asset(give: &MultiAssetFilter, receive: &MultiAssets) -> Weight;
	fn initiate_reserve_withdraw(
		assets: &MultiAssetFilter,
		reserve: &MultiLocation,
		xcm: &Xcm<()>,
	) -> Weight;
	fn initiate_teleport(assets: &MultiAssetFilter, dest: &MultiLocation, xcm: &Xcm<()>) -> Weight;
	fn query_holding(
		query_id: &u64,
		dest: &MultiLocation,
		assets: &MultiAssetFilter,
		max_response_weight: &u64,
	) -> Weight;
	fn buy_execution(fees: &MultiAsset, weight_limit: &WeightLimit) -> Weight;
	fn refund_surplus() -> Weight;
	fn set_error_handler(xcm: &Xcm<Call>) -> Weight;
	fn set_appendix(xcm: &Xcm<Call>) -> Weight;
	fn clear_error() -> Weight;
	fn claim_asset(assets: &MultiAssets, ticket: &MultiLocation) -> Weight;
	fn trap(code: &u64) -> Weight;
	fn subscribe_version(query_id: &QueryId, max_response_weight: &u64) -> Weight;
	fn unsubscribe_version() -> Weight;
}
