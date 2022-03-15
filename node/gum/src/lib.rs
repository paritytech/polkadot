// Copyright 2022 Parity Technologies (UK) Ltd.
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

#![deny(unused_crate_dependencies)]
#![deny(missing_docs)]
#![deny(clippy::dbg_macro)]

//! A wrapper around `tracing` macros, to provide semi automatic
//! `traceID` annotation without codebase turnover.

pub use tracing::{enabled, event, Level};

#[doc(hidden)]
pub use jaeger::hash_to_trace_identifier;

#[doc(hidden)]
pub use polkadot_primitives::v2::{CandidateHash, Hash};

pub use gum_proc_macro::{debug, error, info, trace, warn};

#[cfg(test)]
mod tests;
