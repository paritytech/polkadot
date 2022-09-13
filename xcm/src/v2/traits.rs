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
	// Errors that happen due to instructions being executed. These alone are defined in the
	// XCM specification.
	/// An arithmetic overflow happened.
	#[codec(index = 0)]
	Overflow,
	/// The instruction is intentionally unsupported.
	#[codec(index = 1)]
	Unimplemented,
	/// Origin Register does not contain a value value for a reserve transfer notification.
	#[codec(index = 2)]
	UntrustedReserveLocation,
	/// Origin Register does not contain a value value for a teleport notification.
	#[codec(index = 3)]
	UntrustedTeleportLocation,
	/// `MultiLocation` value too large to descend further.
	#[codec(index = 4)]
	MultiLocationFull,
	/// `MultiLocation` value ascend more parents than known ancestors of local location.
	#[codec(index = 5)]
	MultiLocationNotInvertible,
	/// The Origin Register does not contain a valid value for instruction.
	#[codec(index = 6)]
	BadOrigin,
	/// The location parameter is not a valid value for the instruction.
	#[codec(index = 7)]
	InvalidLocation,
	/// The given asset is not handled.
	#[codec(index = 8)]
	AssetNotFound,
	/// An asset transaction (like withdraw or deposit) failed (typically due to type conversions).
	#[codec(index = 9)]
	FailedToTransactAsset(#[codec(skip)] &'static str),
	/// An asset cannot be withdrawn, potentially due to lack of ownership, availability or rights.
	#[codec(index = 10)]
	NotWithdrawable,
	/// An asset cannot be deposited under the ownership of a particular location.
	#[codec(index = 11)]
	LocationCannotHold,
	/// Attempt to send a message greater than the maximum supported by the transport protocol.
	#[codec(index = 12)]
	ExceedsMaxMessageSize,
	/// The given message cannot be translated into a format supported by the destination.
	#[codec(index = 13)]
	DestinationUnsupported,
	/// Destination is routable, but there is some issue with the transport mechanism.
	#[codec(index = 14)]
	Transport(#[codec(skip)] &'static str),
	/// Destination is known to be unroutable.
	#[codec(index = 15)]
	Unroutable,
	/// Used by `ClaimAsset` when the given claim could not be recognized/found.
	#[codec(index = 16)]
	UnknownClaim,
	/// Used by `Transact` when the functor cannot be decoded.
	#[codec(index = 17)]
	FailedToDecode,
	/// Used by `Transact` to indicate that the given weight limit could be breached by the functor.
	#[codec(index = 18)]
	MaxWeightInvalid,
	/// Used by `BuyExecution` when the Holding Register does not contain payable fees.
	#[codec(index = 19)]
	NotHoldingFees,
	/// Used by `BuyExecution` when the fees declared to purchase weight are insufficient.
	#[codec(index = 20)]
	TooExpensive,
	/// Used by the `Trap` instruction to force an error intentionally. Its code is included.
	#[codec(index = 21)]
	Trap(u64),

	// Errors that happen prior to instructions being executed. These fall outside of the XCM spec.
	/// XCM version not able to be handled.
	UnhandledXcmVersion,
	/// Execution of the XCM would potentially result in a greater weight used than weight limit.
	WeightLimitReached(Weight),
	/// The XCM did not pass the barrier condition for execution.
	///
	/// The barrier condition differs on different chains and in different circumstances, but
	/// generally it means that the conditions surrounding the message were not such that the chain
	/// considers the message worth spending time executing. Since most chains lift the barrier to
	/// execution on appropriate payment, presentation of an NFT voucher, or based on the message
	/// origin, it means that none of those were the case.
	Barrier,
	/// The weight of an XCM message is not computable ahead of execution.
	WeightNotComputable,
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
pub trait ExecuteXcm<RuntimeCall> {
	/// Execute some XCM `message` from `origin` using no more than `weight_limit` weight. The weight limit is
	/// a basic hard-limit and the implementation may place further restrictions or requirements on weight and
	/// other aspects.
	fn execute_xcm(
		origin: impl Into<MultiLocation>,
		message: Xcm<RuntimeCall>,
		weight_limit: Weight,
	) -> Outcome {
		let origin = origin.into();
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
		origin: impl Into<MultiLocation>,
		message: Xcm<RuntimeCall>,
		weight_limit: Weight,
		weight_credit: Weight,
	) -> Outcome;
}

impl<C> ExecuteXcm<C> for () {
	fn execute_xcm_in_credit(
		_origin: impl Into<MultiLocation>,
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
///     fn send_xcm(destination: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult {
///         return Err(SendError::CannotReachDestination(destination.into(), message))
///     }
/// }
///
/// /// A sender that accepts a message that has an X2 junction, otherwise stops the routing.
/// struct Sender2;
/// impl SendXcm for Sender2 {
///     fn send_xcm(destination: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult {
///         if let MultiLocation { parents: 0, interior: X2(j1, j2) } = destination.into() {
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
///     fn send_xcm(destination: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult {
///         let destination = destination.into();
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
///
/// assert!(
///     // Sender2 will block this.
///     <(Sender1, Sender2, Sender3) as SendXcm>::send_xcm(Parent, message.clone())
///         .is_err()
/// );
///
/// assert!(
///     // Sender3 will catch this.
///     <(Sender1, Sender3) as SendXcm>::send_xcm(Parent, message.clone())
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
	fn send_xcm(destination: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl SendXcm for Tuple {
	fn send_xcm(destination: impl Into<MultiLocation>, message: Xcm<()>) -> SendResult {
		for_tuples!( #(
			// we shadow `destination` and `message` in each expansion for the next one.
			let (destination, message) = match Tuple::send_xcm(destination, message) {
				Err(SendError::CannotReachDestination(d, m)) => (d, m),
				o @ _ => return o,
			};
		)* );
		Err(SendError::CannotReachDestination(destination.into(), message))
	}
}
