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

use crate::prepare::PrepareStats;
use parity_scale_codec::{Decode, Encode};
use std::fmt;

/// Result of PVF preparation performed by the validation host. Contains stats about the preparation if
/// successful
pub type PrepareResult = Result<PrepareStats, PrepareError>;

/// An error that occurred during the prepare part of the PVF pipeline.
#[derive(Debug, Clone, Encode, Decode)]
pub enum PrepareError {
	/// During the prevalidation stage of preparation an issue was found with the PVF.
	Prevalidation(String),
	/// Compilation failed for the given PVF.
	Preparation(String),
	/// An unexpected panic has occurred in the preparation worker.
	Panic(String),
	/// Failed to prepare the PVF due to the time limit.
	TimedOut,
	/// An IO error occurred. This state is reported by either the validation host or by the worker.
	IoErr(String),
	/// The temporary file for the artifact could not be created at the given cache path. This state is reported by the
	/// validation host (not by the worker).
	CreateTmpFileErr(String),
	/// The response from the worker is received, but the file cannot be renamed (moved) to the final destination
	/// location. This state is reported by the validation host (not by the worker).
	RenameTmpFileErr(String),
}

impl PrepareError {
	/// Returns whether this is a deterministic error, i.e. one that should trigger reliably. Those
	/// errors depend on the PVF itself and the sc-executor/wasmtime logic.
	///
	/// Non-deterministic errors can happen spuriously. Typically, they occur due to resource
	/// starvation, e.g. under heavy load or memory pressure. Those errors are typically transient
	/// but may persist e.g. if the node is run by overwhelmingly underpowered machine.
	pub fn is_deterministic(&self) -> bool {
		use PrepareError::*;
		match self {
			Prevalidation(_) | Preparation(_) | Panic(_) => true,
			TimedOut | IoErr(_) | CreateTmpFileErr(_) | RenameTmpFileErr(_) => false,
		}
	}
}

impl fmt::Display for PrepareError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use PrepareError::*;
		match self {
			Prevalidation(err) => write!(f, "prevalidation: {}", err),
			Preparation(err) => write!(f, "preparation: {}", err),
			Panic(err) => write!(f, "panic: {}", err),
			TimedOut => write!(f, "prepare: timeout"),
			IoErr(err) => write!(f, "prepare: io error while receiving response: {}", err),
			CreateTmpFileErr(err) => write!(f, "prepare: error creating tmp file: {}", err),
			RenameTmpFileErr(err) => write!(f, "prepare: error renaming tmp file: {}", err),
		}
	}
}

/// A error raised during validation of the candidate.
#[derive(Debug, Clone)]
pub enum ValidationError {
	/// The error was raised because the candidate is invalid.
	InvalidCandidate(InvalidCandidate),
	/// This error is raised due to inability to serve the request.
	InternalError(String),
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
	/// (a) Some sort of transient glitch caused the worker process to abort. An example would be that
	///     the host machine ran out of free memory and the OOM killer started killing the processes,
	///     and in order to save the parent it will "sacrifice child" first.
	///
	/// (b) The candidate triggered a code path that has lead to the process death. For example,
	///     the PVF found a way to consume unbounded amount of resources and then it either exceeded
	///     an `rlimit` (if set) or, again, invited OOM killer. Another possibility is a bug in
	///     wasmtime allowed the PVF to gain control over the execution worker.
	///
	/// We attribute such an event to an invalid candidate in either case.
	///
	/// The rationale for this is that a glitch may lead to unfair rejecting candidate by a single
	/// validator. If the glitch is somewhat more persistent the validator will reject all candidate
	/// thrown at it and hopefully the operator notices it by decreased reward performance of the
	/// validator. On the other hand, if the worker died because of (b) we would have better chances
	/// to stop the attack.
	AmbiguousWorkerDeath,
	/// PVF execution (compilation is not included) took more time than was allotted.
	HardTimeout,
}

impl From<PrepareError> for ValidationError {
	fn from(error: PrepareError) -> Self {
		// Here we need to classify the errors into two errors: deterministic and non-deterministic.
		// See [`PrepareError::is_deterministic`].
		if error.is_deterministic() {
			ValidationError::InvalidCandidate(InvalidCandidate::PrepareError(error.to_string()))
		} else {
			ValidationError::InternalError(error.to_string())
		}
	}
}
