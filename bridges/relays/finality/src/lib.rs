// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! This crate has single entrypoint to run synchronization loop that is built around finality
//! proofs, as opposed to headers synchronization loop, which is built around headers. The headers
//! are still submitted to the target node, but are treated as auxiliary data as we are not trying
//! to submit all source headers to the target node.

pub use crate::finality_loop::{metrics_prefix, run, FinalitySyncParams, SourceClient, TargetClient};

use bp_header_chain::FinalityProof;
use std::fmt::Debug;

mod finality_loop;
mod finality_loop_tests;

/// Finality proofs synchronization pipeline.
pub trait FinalitySyncPipeline: Clone + Debug + Send + Sync {
	/// Name of the finality proofs source.
	const SOURCE_NAME: &'static str;
	/// Name of the finality proofs target.
	const TARGET_NAME: &'static str;

	/// Headers we're syncing are identified by this hash.
	type Hash: Eq + Clone + Copy + Send + Sync + Debug;
	/// Headers we're syncing are identified by this number.
	type Number: relay_utils::BlockNumberBase;
	/// Type of header that we're syncing.
	type Header: SourceHeader<Self::Number>;
	/// Finality proof type.
	type FinalityProof: FinalityProof<Self::Number>;
}

/// Header that we're receiving from source node.
pub trait SourceHeader<Number>: Clone + Debug + PartialEq + Send + Sync {
	/// Returns number of header.
	fn number(&self) -> Number;
	/// Returns true if this header needs to be submitted to target node.
	fn is_mandatory(&self) -> bool;
}
