// Copyright 2021 Parity Technologies (UK) Ltd.
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

#![warn(missing_docs)]

//! A crate that implements the PVF validation host.
//!
//! This is responsible for handling requests to prepare and execute PVF code blobs.

mod artifacts;
mod error;
mod execute;
mod executor_intf;
mod host;
mod metrics;
mod prepare;
mod priority;
mod pvf;
mod worker_common;

#[doc(hidden)]
pub mod testing;

#[doc(hidden)]
pub use sp_tracing;

pub use error::{InvalidCandidate, PrepareError, PrepareResult, ValidationError};
pub use priority::Priority;
pub use pvf::Pvf;

pub use host::{start, Config, ValidationHost};
pub use metrics::Metrics;

pub use execute::worker_entrypoint as execute_worker_entrypoint;
pub use prepare::worker_entrypoint as prepare_worker_entrypoint;

pub use executor_intf::{prepare, prevalidate};

pub use sc_executor_common;
pub use sp_maybe_compressed_blob;

const LOG_TARGET: &str = "parachain::pvf";
