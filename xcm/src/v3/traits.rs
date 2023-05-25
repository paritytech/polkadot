// Copyright (C) Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Cross-Consensus Message format data structures.

use crate::v2::Error as OldError;
use core::result;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub use sp_weights::Weight;

use super::*;

/// Error codes used in XCM. The first errors codes have explicit indices and are part of the XCM
/// format. Those trailing are merely part of the XCM implementation; there is no expectation that
/// they will retain the same index over time.
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
	LocationFull,
	/// `MultiLocation` value ascend more parents than known ancestors of local location.
	#[codec(index = 5)]
	LocationNotInvertible,
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
	/// Used by `ExpectAsset`, `ExpectError` and `ExpectOrigin` when the expectation was not true.
	#[codec(index = 22)]
	ExpectationFalse,
	/// The provided pallet index was not found.
	#[codec(index = 23)]
	PalletNotFound,
	/// The given pallet's name is different to that expected.
	#[codec(index = 24)]
	NameMismatch,
	/// The given pallet's version has an incompatible version to that expected.
	#[codec(index = 25)]
	VersionIncompatible,
	/// The given operation would lead to an overflow of the Holding Register.
	#[codec(index = 26)]
	HoldingWouldOverflow,
	/// The message was unable to be exported.
	#[codec(index = 27)]
	ExportError,
	/// `MultiLocation` value failed to be reanchored.
	#[codec(index = 28)]
	ReanchorFailed,
	/// No deal is possible under the given constraints.
	#[codec(index = 29)]
	NoDeal,
	/// Fees were required which the origin could not pay.
	#[codec(index = 30)]
	FeesNotMet,
	/// Some other error with locking.
	#[codec(index = 31)]
	LockError,
	/// The state was not in a condition where the operation was valid to make.
	#[codec(index = 32)]
	NoPermission,
	/// The universal location of the local consensus is improper.
	#[codec(index = 33)]
	Unanchored,
	/// An asset cannot be deposited, probably because (too much of) it already exists.
	#[codec(index = 34)]
	NotDepositable,

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
	/// Recursion stack limit reached
	ExceedsStackLimit,
}

impl MaxEncodedLen for Error {
	fn max_encoded_len() -> usize {
		// TODO: max_encoded_len doesn't quite work here as it tries to take notice of the fields
		// marked `codec(skip)`. We can hard-code it with the right answer for now.
		1
	}
}

impl TryFrom<OldError> for Error {
	type Error = ();
	fn try_from(old_error: OldError) -> result::Result<Error, ()> {
		use OldError::*;
		Ok(match old_error {
			Overflow => Self::Overflow,
			Unimplemented => Self::Unimplemented,
			UntrustedReserveLocation => Self::UntrustedReserveLocation,
			UntrustedTeleportLocation => Self::UntrustedTeleportLocation,
			MultiLocationFull => Self::LocationFull,
			MultiLocationNotInvertible => Self::LocationNotInvertible,
			BadOrigin => Self::BadOrigin,
			InvalidLocation => Self::InvalidLocation,
			AssetNotFound => Self::AssetNotFound,
			FailedToTransactAsset(s) => Self::FailedToTransactAsset(s),
			NotWithdrawable => Self::NotWithdrawable,
			LocationCannotHold => Self::LocationCannotHold,
			ExceedsMaxMessageSize => Self::ExceedsMaxMessageSize,
			DestinationUnsupported => Self::DestinationUnsupported,
			Transport(s) => Self::Transport(s),
			Unroutable => Self::Unroutable,
			UnknownClaim => Self::UnknownClaim,
			FailedToDecode => Self::FailedToDecode,
			MaxWeightInvalid => Self::MaxWeightInvalid,
			NotHoldingFees => Self::NotHoldingFees,
			TooExpensive => Self::TooExpensive,
			Trap(i) => Self::Trap(i),
			_ => return Err(()),
		})
	}
}

impl From<SendError> for Error {
	fn from(e: SendError) -> Self {
		match e {
			SendError::NotApplicable | SendError::Unroutable | SendError::MissingArgument =>
				Error::Unroutable,
			SendError::Transport(s) => Error::Transport(s),
			SendError::DestinationUnsupported => Error::DestinationUnsupported,
			SendError::ExceedsMaxMessageSize => Error::ExceedsMaxMessageSize,
			SendError::Fees => Error::FeesNotMet,
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
			Outcome::Error(_) => Weight::zero(),
		}
	}
}

pub trait PreparedMessage {
	fn weight_of(&self) -> Weight;
}

/// Type of XCM message executor.
pub trait ExecuteXcm<Call> {
	type Prepared: PreparedMessage;
	fn prepare(message: Xcm<Call>) -> result::Result<Self::Prepared, Xcm<Call>>;
	fn execute(
		origin: impl Into<MultiLocation>,
		pre: Self::Prepared,
		hash: XcmHash,
		weight_credit: Weight,
	) -> Outcome;

	/// Execute some XCM `message` with the message `hash` from `origin` using no more than `weight_limit` weight.
	/// The weight limit is a basic hard-limit and the implementation may place further restrictions or requirements
	/// on weight and other aspects.
	fn execute_xcm(
		origin: impl Into<MultiLocation>,
		message: Xcm<Call>,
		hash: XcmHash,
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
		Self::execute_xcm_in_credit(origin, message, hash, weight_limit, Weight::zero())
	}

	/// Execute some XCM `message` with the message `hash` from `origin` using no more than `weight_limit` weight.
	///
	/// Some amount of `weight_credit` may be provided which, depending on the implementation, may allow
	/// execution without associated payment.
	fn execute_xcm_in_credit(
		origin: impl Into<MultiLocation>,
		message: Xcm<Call>,
		hash: XcmHash,
		weight_limit: Weight,
		weight_credit: Weight,
	) -> Outcome {
		let pre = match Self::prepare(message) {
			Ok(x) => x,
			Err(_) => return Outcome::Error(Error::WeightNotComputable),
		};
		let xcm_weight = pre.weight_of();
		if xcm_weight.any_gt(weight_limit) {
			return Outcome::Error(Error::WeightLimitReached(xcm_weight))
		}
		Self::execute(origin, pre, hash, weight_credit)
	}

	/// Deduct some `fees` to the sovereign account of the given `location` and place them as per
	/// the convention for fees.
	fn charge_fees(location: impl Into<MultiLocation>, fees: MultiAssets) -> Result;
}

pub enum Weightless {}
impl PreparedMessage for Weightless {
	fn weight_of(&self) -> Weight {
		unreachable!()
	}
}

impl<C> ExecuteXcm<C> for () {
	type Prepared = Weightless;
	fn prepare(message: Xcm<C>) -> result::Result<Self::Prepared, Xcm<C>> {
		Err(message)
	}
	fn execute(_: impl Into<MultiLocation>, _: Self::Prepared, _: XcmHash, _: Weight) -> Outcome {
		unreachable!()
	}
	fn charge_fees(_location: impl Into<MultiLocation>, _fees: MultiAssets) -> Result {
		Err(Error::Unimplemented)
	}
}

/// Error result value when attempting to send an XCM message.
#[derive(Clone, Encode, Decode, Eq, PartialEq, Debug, scale_info::TypeInfo)]
pub enum SendError {
	/// The message and destination combination was not recognized as being reachable.
	///
	/// This is not considered fatal: if there are alternative transport routes available, then
	/// they may be attempted.
	NotApplicable,
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
	/// A needed argument is `None` when it should be `Some`.
	MissingArgument,
	/// Fees needed to be paid in order to send the message and they were unavailable.
	Fees,
}

/// A hash type for identifying messages.
pub type XcmHash = [u8; 32];

/// Result value when attempting to send an XCM message.
pub type SendResult<T> = result::Result<(T, MultiAssets), SendError>;

pub trait Unwrappable {
	type Inner;
	fn none() -> Self;
	fn some(i: Self::Inner) -> Self;
	fn take(self) -> Option<Self::Inner>;
}

impl<T> Unwrappable for Option<T> {
	type Inner = T;
	fn none() -> Self {
		None
	}
	fn some(i: Self::Inner) -> Self {
		Some(i)
	}
	fn take(self) -> Option<Self::Inner> {
		self
	}
}

/// Utility for sending an XCM message to a given location.
///
/// These can be amalgamated in tuples to form sophisticated routing systems. In tuple format, each
/// router might return `NotApplicable` to pass the execution to the next sender item. Note that
/// each `NotApplicable` might alter the destination and the XCM message for to the next router.
///
/// # Example
/// ```rust
/// # use parity_scale_codec::Encode;
/// # use xcm::v3::{prelude::*, Weight};
/// # use xcm::VersionedXcm;
/// # use std::convert::Infallible;
///
/// /// A sender that only passes the message through and does nothing.
/// struct Sender1;
/// impl SendXcm for Sender1 {
///     type Ticket = Infallible;
///     fn validate(_: &mut Option<MultiLocation>, _: &mut Option<Xcm<()>>) -> SendResult<Infallible> {
///         Err(SendError::NotApplicable)
///     }
///     fn deliver(_: Infallible) -> Result<XcmHash, SendError> {
///         unreachable!()
///     }
/// }
///
/// /// A sender that accepts a message that has an X2 junction, otherwise stops the routing.
/// struct Sender2;
/// impl SendXcm for Sender2 {
///     type Ticket = ();
///     fn validate(destination: &mut Option<MultiLocation>, message: &mut Option<Xcm<()>>) -> SendResult<()> {
///         match destination.as_ref().ok_or(SendError::MissingArgument)? {
///             MultiLocation { parents: 0, interior: X2(j1, j2) } => Ok(((), MultiAssets::new())),
///             _ => Err(SendError::Unroutable),
///         }
///     }
///     fn deliver(_: ()) -> Result<XcmHash, SendError> {
///         Ok([0; 32])
///     }
/// }
///
/// /// A sender that accepts a message from a parent, passing through otherwise.
/// struct Sender3;
/// impl SendXcm for Sender3 {
///     type Ticket = ();
///     fn validate(destination: &mut Option<MultiLocation>, message: &mut Option<Xcm<()>>) -> SendResult<()> {
///         match destination.as_ref().ok_or(SendError::MissingArgument)? {
///             MultiLocation { parents: 1, interior: Here } => Ok(((), MultiAssets::new())),
///             _ => Err(SendError::NotApplicable),
///         }
///     }
///     fn deliver(_: ()) -> Result<XcmHash, SendError> {
///         Ok([0; 32])
///     }
/// }
///
/// // A call to send via XCM. We don't really care about this.
/// # fn main() {
/// let call: Vec<u8> = ().encode();
/// let message = Xcm(vec![Instruction::Transact {
///     origin_kind: OriginKind::Superuser,
///     require_weight_at_most: Weight::zero(),
///     call: call.into(),
/// }]);
/// let message_hash = message.using_encoded(sp_io::hashing::blake2_256);
///
/// // Sender2 will block this.
/// assert!(send_xcm::<(Sender1, Sender2, Sender3)>(Parent.into(), message.clone()).is_err());
///
/// // Sender3 will catch this.
/// assert!(send_xcm::<(Sender1, Sender3)>(Parent.into(), message.clone()).is_ok());
/// # }
/// ```
pub trait SendXcm {
	/// Intermediate value which connects the two phaases of the send operation.
	type Ticket;

	/// Check whether the given `_message` is deliverable to the given `_destination` and if so
	/// determine the cost which will be paid by this chain to do so, returning a `Validated` token
	/// which can be used to enact delivery.
	///
	/// The `destination` and `message` must be `Some` (or else an error will be returned) and they
	/// may only be consumed if the `Err` is not `NotApplicable`.
	///
	/// If it is not a destination which can be reached with this type but possibly could by others,
	/// then this *MUST* return `NotApplicable`. Any other error will cause the tuple
	/// implementation to exit early without trying other type fields.
	fn validate(
		destination: &mut Option<MultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket>;

	/// Actually carry out the delivery operation for a previously validated message sending.
	fn deliver(ticket: Self::Ticket) -> result::Result<XcmHash, SendError>;
}

#[impl_trait_for_tuples::impl_for_tuples(30)]
impl SendXcm for Tuple {
	for_tuples! { type Ticket = (#( Option<Tuple::Ticket> ),* ); }

	fn validate(
		destination: &mut Option<MultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		let mut maybe_cost: Option<MultiAssets> = None;
		let one_ticket: Self::Ticket = (for_tuples! { #(
			if maybe_cost.is_some() {
				None
			} else {
				match Tuple::validate(destination, message) {
					Err(SendError::NotApplicable) => None,
					Err(e) => { return Err(e) },
					Ok((v, c)) => {
						maybe_cost = Some(c);
						Some(v)
					},
				}
			}
		),* });
		if let Some(cost) = maybe_cost {
			Ok((one_ticket, cost))
		} else {
			Err(SendError::NotApplicable)
		}
	}

	fn deliver(one_ticket: Self::Ticket) -> result::Result<XcmHash, SendError> {
		for_tuples!( #(
			if let Some(validated) = one_ticket.Tuple {
				return Tuple::deliver(validated);
			}
		)* );
		Err(SendError::Unroutable)
	}
}

/// Convenience function for using a `SendXcm` implementation. Just interprets the `dest` and wraps
/// both in `Some` before passing them as as mutable references into `T::send_xcm`.
pub fn validate_send<T: SendXcm>(dest: MultiLocation, msg: Xcm<()>) -> SendResult<T::Ticket> {
	T::validate(&mut Some(dest), &mut Some(msg))
}

/// Convenience function for using a `SendXcm` implementation. Just interprets the `dest` and wraps
/// both in `Some` before passing them as as mutable references into `T::send_xcm`.
///
/// Returns either `Ok` with the price of the delivery, or `Err` with the reason why the message
/// could not be sent.
///
/// Generally you'll want to validate and get the price first to ensure that the sender can pay it
/// before actually doing the delivery.
pub fn send_xcm<T: SendXcm>(
	dest: MultiLocation,
	msg: Xcm<()>,
) -> result::Result<(XcmHash, MultiAssets), SendError> {
	let (ticket, price) = T::validate(&mut Some(dest), &mut Some(msg))?;
	let hash = T::deliver(ticket)?;
	Ok((hash, price))
}
