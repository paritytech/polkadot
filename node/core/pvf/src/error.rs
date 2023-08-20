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

use polkadot_node_core_pvf_common::error::{InternalValidationError, PrepareError};

/// A error raised during validation of the candidate.
#[derive(Debug, Clone)]
pub enum ValidationError {
	/// The error was raised because the candidate is invalid.
	///
	/// Whenever we are unsure if the error was due to the candidate or not, we must vote invalid.
	InvalidCandidate(InvalidCandidate),
	/// Some internal error occurred.
	InternalError(InternalValidationError),
}

/// A description of an error raised during executing a PVF and can be attributed to the combination
/// of the candidate [`polkadot_parachain::primitives::ValidationParams`] and the PVF.
#[derive(Debug, Clone)]
pub enum InvalidCandidate {
	/// PVF preparation ended up with a deterministic error.
	PrepareError(String),
	/// The failure is reported by the execution worker. The string contains the error message.
	WorkerReportedError(String),
	/// The worker has died during validation of a candidate. That may fall in one of the following
	/// categories, which we cannot distinguish programmatically:
	///
	/// (a) Some sort of transient glitch caused the worker process to abort. An example would be
	/// that the host machine ran out of free memory and the OOM killer started killing the
	/// processes, and in order to save the parent it will "sacrifice child" first.
	///
	/// (b) The candidate triggered a code path that has lead to the process death. For example,
	///     the PVF found a way to consume unbounded amount of resources and then it either
	///     exceeded an `rlimit` (if set) or, again, invited OOM killer. Another possibility is a
	///     bug in wasmtime allowed the PVF to gain control over the execution worker.
	///
	/// We attribute such an event to an *invalid candidate* in either case.
	///
	/// The rationale for this is that a glitch may lead to unfair rejecting candidate by a single
	/// validator. If the glitch is somewhat more persistent the validator will reject all
	/// candidate thrown at it and hopefully the operator notices it by decreased reward
	/// performance of the validator. On the other hand, if the worker died because of (b) we would
	/// have better chances to stop the attack.
	AmbiguousWorkerDeath,
	/// PVF execution (compilation is not included) took more time than was allotted.
	HardTimeout,
	/// A panic occurred and we can't be sure whether the candidate is really invalid or some
	/// internal glitch occurred. Whenever we are unsure, we can never treat an error as internal
	/// as we would abstain from voting. This is bad because if the issue was due to the candidate,
	/// then all validators would abstain, stalling finality on the chain. So we will first retry
	/// the candidate, and if the issue persists we are forced to vote invalid.
	Panic(String),
}

impl From<InternalValidationError> for ValidationError {
	fn from(error: InternalValidationError) -> Self {
		Self::InternalError(error)
	}
}

impl From<PrepareError> for ValidationError {
	fn from(error: PrepareError) -> Self {
		// Here we need to classify the errors into two errors: deterministic and non-deterministic.
		// See [`PrepareError::is_deterministic`].
		if error.is_deterministic() {
			Self::InvalidCandidate(InvalidCandidate::PrepareError(error.to_string()))
		} else {
			Self::InternalError(InternalValidationError::NonDeterministicPrepareError(error))
		}
	}
}
