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

/// Result of PVF preparation performed by the validation host. Contains stats about the preparation
/// if successful
pub type PrepareResult = Result<PrepareStats, PrepareError>;

/// An error that occurred during the prepare part of the PVF pipeline.
#[derive(Debug, Clone, Encode, Decode)]
pub enum PrepareError {
	/// During the prevalidation stage of preparation an issue was found with the PVF.
	Prevalidation(String),
	/// Compilation failed for the given PVF.
	Preparation(String),
	/// Instantiation of the WASM module instance failed.
	RuntimeConstruction(String),
	/// An unexpected panic has occurred in the preparation worker.
	Panic(String),
	/// Failed to prepare the PVF due to the time limit.
	TimedOut,
	/// An IO error occurred. This state is reported by either the validation host or by the
	/// worker.
	IoErr(String),
	/// The temporary file for the artifact could not be created at the given cache path. This
	/// state is reported by the validation host (not by the worker).
	CreateTmpFileErr(String),
	/// The response from the worker is received, but the file cannot be renamed (moved) to the
	/// final destination location. This state is reported by the validation host (not by the
	/// worker).
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
			// Can occur due to issues with the PVF, but also due to local errors.
			RuntimeConstruction(_) => false,
		}
	}
}

impl fmt::Display for PrepareError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use PrepareError::*;
		match self {
			Prevalidation(err) => write!(f, "prevalidation: {}", err),
			Preparation(err) => write!(f, "preparation: {}", err),
			RuntimeConstruction(err) => write!(f, "runtime construction: {}", err),
			Panic(err) => write!(f, "panic: {}", err),
			TimedOut => write!(f, "prepare: timeout"),
			IoErr(err) => write!(f, "prepare: io error while receiving response: {}", err),
			CreateTmpFileErr(err) => write!(f, "prepare: error creating tmp file: {}", err),
			RenameTmpFileErr(err) => write!(f, "prepare: error renaming tmp file: {}", err),
		}
	}
}

/// Some internal error occurred.
///
/// Should only ever be used for validation errors independent of the candidate and PVF, or for
/// errors we ruled out during pre-checking (so preparation errors are fine).
#[derive(Debug, Clone, Encode, Decode)]
pub enum InternalValidationError {
	/// Some communication error occurred with the host.
	HostCommunication(String),
	/// Could not find or open compiled artifact file.
	CouldNotOpenFile(String),
	/// An error occurred in the CPU time monitor thread. Should be totally unrelated to
	/// validation.
	CpuTimeMonitorThread(String),
	/// Some non-deterministic preparation error occurred.
	NonDeterministicPrepareError(PrepareError),
}

impl fmt::Display for InternalValidationError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use InternalValidationError::*;
		match self {
			HostCommunication(err) =>
				write!(f, "validation: some communication error occurred with the host: {}", err),
			CouldNotOpenFile(err) =>
				write!(f, "validation: could not find or open compiled artifact file: {}", err),
			CpuTimeMonitorThread(err) =>
				write!(f, "validation: an error occurred in the CPU time monitor thread: {}", err),
			NonDeterministicPrepareError(err) => write!(f, "validation: prepare: {}", err),
		}
	}
}
