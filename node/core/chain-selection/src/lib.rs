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

//! Implements the Chain Selection Subsystem.

use polkadot_primitives::v1::Hash;
use polkadot_subsystem::{
	Subsystem, SubsystemContext, SubsystemResult, SubsystemError, SpawnedSubsystem,
};

type Weight = u64;
type Timestamp = u64;

enum Approval {
	// Approved
	Approved,
	// Unapproved but not stagnant
	Unapproved,
	// Unapproved and stagnant.
	Stagnant,
}

impl Approval {
	fn is_stagnant(&self) -> bool {
		match *self {
			Approval::Stagnant => true,
			_ => false,
		}
	}
}

struct ViabilityCriteria {
	// Whether this block has been explicitly reverted by one of its descendants.
	explicitly_reverted: bool,
	// `None` means approved. `Some` means unapproved.
	approval: Approval,
	earliest_non_viable_ancestor: Option<Hash>,
}

impl ViabilityCriteria {
	fn is_viable(&self) -> bool {
		self.earliest_non_viable_ancestor.is_none()
			&& !self.explicitly_reverted
			&& !self.approval.is_stagnant()
	}
}

struct LeafEntry {
	weight: Weight,
	block_hash: Hash,
}

struct BlockEntry {
	block_hash: Hash,
	parent_hash: Hash,
	children: Vec<Hash>,
	viability: ViabilityCriteria,
	weight: Weight,
}

enum BackendWriteOp {
	WriteBlockEntry(Hash, BlockEntry),
	DeleteBlockEntry(Hash),
	WriteActiveLeaves(Vec<LeafEntry>),
}

// An abstraction over backend for the logic of this subsystem.
trait Backend {
	/// The error type of this backend, which is assumed to indicate a
	/// fatal database error.
	type Error: Into<SubsystemError>;

	/// Load a block entry from the DB.
	fn load_block_entry(&self) -> Result<BlockEntry, Self::Error>;
	/// Load the active-leaves set.
	fn load_leaves(&self) -> Result<Vec<LeafEntry>, Self::Error>;
	/// Load all stagnant lists up to and including the given unix timestamp.
	fn load_stagnant_up_to(&self, up_to: Timestamp) -> Result<Vec<(Timestamp, Vec<Hash>)>, Self::Error>;

	/// Atomically write the list of operations, with later operations taking precedence over prior.
	fn write(&self, ops: Vec<BackendWriteOp>) -> Result<(), Self::Error>;
}
