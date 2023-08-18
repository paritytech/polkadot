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

use crate::error::InternalValidationError;
use parity_scale_codec::{Decode, Encode};
use polkadot_parachain::primitives::ValidationResult;
use polkadot_primitives::ExecutorParams;
use std::time::Duration;

/// The payload of the one-time handshake that is done when a worker process is created. Carries
/// data from the host to the worker.
#[derive(Encode, Decode)]
pub struct Handshake {
	/// The executor parameters.
	pub executor_params: ExecutorParams,
}

/// The response from an execution job on the worker.
#[derive(Encode, Decode)]
pub enum Response {
	/// The job completed successfully.
	Ok {
		/// The result of parachain validation.
		result_descriptor: ValidationResult,
		/// The amount of CPU time taken by the job.
		duration: Duration,
	},
	/// The candidate is invalid.
	InvalidCandidate(String),
	/// The job timed out.
	TimedOut,
	/// An unexpected panic has occurred in the execution worker.
	Panic(String),
	/// Some internal error occurred.
	InternalError(InternalValidationError),
}

impl Response {
	/// Creates an invalid response from a context `ctx` and a message `msg` (which can be empty).
	pub fn format_invalid(ctx: &'static str, msg: &str) -> Self {
		if msg.is_empty() {
			Self::InvalidCandidate(ctx.to_string())
		} else {
			Self::InvalidCandidate(format!("{}: {}", ctx, msg))
		}
	}
}
