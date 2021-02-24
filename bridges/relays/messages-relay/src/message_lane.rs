// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! One-way message lane types. Within single one-way lane we have three 'races' where we try to:
//!
//! 1) relay new messages from source to target node;
//! 2) relay proof-of-delivery from target to source node.

use relay_utils::{BlockNumberBase, HeaderId};
use std::fmt::Debug;

/// One-way message lane.
pub trait MessageLane: Clone + Send + Sync {
	/// Name of the messages source.
	const SOURCE_NAME: &'static str;
	/// Name of the messages target.
	const TARGET_NAME: &'static str;

	/// Messages proof.
	type MessagesProof: Clone + Debug + Send + Sync;
	/// Messages receiving proof.
	type MessagesReceivingProof: Clone + Debug + Send + Sync;

	/// Number of the source header.
	type SourceHeaderNumber: BlockNumberBase;
	/// Hash of the source header.
	type SourceHeaderHash: Clone + Debug + Default + PartialEq + Send + Sync;

	/// Number of the target header.
	type TargetHeaderNumber: BlockNumberBase;
	/// Hash of the target header.
	type TargetHeaderHash: Clone + Debug + Default + PartialEq + Send + Sync;
}

/// Source header id within given one-way message lane.
pub type SourceHeaderIdOf<P> = HeaderId<<P as MessageLane>::SourceHeaderHash, <P as MessageLane>::SourceHeaderNumber>;

/// Target header id within given one-way message lane.
pub type TargetHeaderIdOf<P> = HeaderId<<P as MessageLane>::TargetHeaderHash, <P as MessageLane>::TargetHeaderNumber>;
