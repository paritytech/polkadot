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
use parity_scale_codec::{Encode, Decode};

use super::{MultiLocation, Xcm, XcmGeneric};

#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum Error {
	Undefined,
	Unimplemented,
	UnhandledXcmVersion,
	UnhandledXcmMessage,
	UnhandledEffect,
	EscalationOfPrivilege,
	UntrustedReserveLocation,
	UntrustedTeleportLocation,
	DestinationBufferOverflow,
	CannotReachDestination(#[codec(skip)] &'static str),
	MultiLocationFull,
	FailedToDecode,
	BadOrigin,
	ExceedsMaxMessageSize,
	FailedToTransactAsset(#[codec(skip)] &'static str),
	WeightLimitReached,
	Wildcard,
	/// The case where an XCM message has specified a optional weight limit and the weight required for
	/// processing is too great.
	///
	/// Used by:
	/// - `Transact`
	TooMuchWeightRequired,
	/// The fees specified by the XCM message were not found in the holding account.
	///
	/// Used by:
	/// - `BuyExecution`
	NotHoldingFees,
	/// The weight of an XCM message is not computable ahead of execution. This generally means at least part
	/// of the message is invalid, which could be due to it containing overly nested structures or an invalid
	/// nested data segment (e.g. for the call in `Transact`).
	WeightNotComputable,
	/// The XCM did noto pass the barrier condition for execution. The barrier condition differs on different
	/// chains and in different circumstances, but generally it means that the conditions surrounding the message
	/// were not such that the chain considers the message worth spending time executing. Since most chains
	/// lift the barrier to execution on apropriate payment, presentation of an NFT voucher, or based on the
	/// message origin, it means that none of those were the case.
	Barrier,
	/// Indicates that it is not possible for a location to have an asset be withdrawn or transferred from its
	/// ownership. This probably means it doesn't own (enough of) it, but may also indicate that it is under a
	/// lock, hold, freeze or is otherwise unavailable.
	NotWithdrawable,
	/// Indicates that the consensus system cannot deposit an asset under the ownership of a particular location.
	LocationCannotHold,
	/// We attempted to send an XCM to the local consensus system. Execution was not possible probably due to
	/// no execution weight being assigned.
	DestinationIsLocal,
}

impl From<()> for Error {
	fn from(_: ()) -> Self {
		Self::Undefined
	}
}

pub type Result = result::Result<(), Error>;

/// Local weight type; execution time in picoseconds.
pub type Weight = u64;

/// Outcome of an XCM excution.
#[derive(Copy, Clone, Encode, Decode, Eq, PartialEq, Debug)]
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

pub trait ExecuteXcm<Call: Encode> {
	fn execute_xcm(origin: MultiLocation, message: XcmGeneric<Call>, weight_limit: Weight) -> Outcome;
}

impl<C: Encode> ExecuteXcm<C> for () {
	fn execute_xcm(_origin: MultiLocation, _message: XcmGeneric<C>, _weight_limit: Weight) -> Outcome {
		Outcome::Error(Error::Unimplemented)
	}
}

pub trait SendXcm {
	fn send_xcm(dest: MultiLocation, msg: Xcm) -> Result;
}

impl SendXcm for () {
	fn send_xcm(_dest: MultiLocation, _msg: Xcm) -> Result {
		Err(Error::Unimplemented)
	}
}
