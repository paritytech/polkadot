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
	OverseerSignal, FromOverseer,
	messages::{ChainSelectionMessage, ChainApiMessage},
	errors::ChainApiError,
};

use parity_scale_codec::Error as CodecError;
use futures::channel::oneshot;

const LOG_TARGET: &str = "parachain::chain-selection";

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

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),

	#[error(transparent)]
	Codec(#[from] CodecError),
}

impl Error {
	fn trace(&self) {
		match self {
			// don't spam the log with spurious errors
			Self::Oneshot(_) => tracing::debug!(target: LOG_TARGET, err = ?self),
			// it's worth reporting otherwise
			_ => tracing::warn!(target: LOG_TARGET, err = ?self),
		}
	}
}

enum BackendWriteOp {
	WriteBlockEntry(Hash, BlockEntry),
	DeleteBlockEntry(Hash),
	WriteActiveLeaves(Vec<LeafEntry>),
}

// An abstraction over backend for the logic of this subsystem.
trait Backend {
	/// Load a block entry from the DB.
	fn load_block_entry(&self) -> Result<BlockEntry, Error>;
	/// Load the active-leaves set.
	fn load_leaves(&self) -> Result<Vec<LeafEntry>, Error>;
	/// Load all stagnant lists up to and including the given unix timestamp.
	fn load_stagnant_up_to(&self, up_to: Timestamp) -> Result<Vec<(Timestamp, Vec<Hash>)>, Error>;

	/// Atomically write the list of operations, with later operations taking precedence over prior.
	fn write(&mut self, ops: Vec<BackendWriteOp>) -> Result<(), Error>;
}

async fn run<Context, B>(mut ctx: Context, mut backend: B)
	where
		Context: SubsystemContext<Message = ChainSelectionMessage>,
		B: Backend,
{
	loop {
		let res = run_iteration(&mut ctx, &mut backend).await;
		match res {
			Err(e) => {
				e.trace();

				if let Error::Subsystem(SubsystemError::Context(_)) = e {
					break;
				}
			}
			Ok(()) => {
				tracing::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
				break;
			}
		}
	}
}

// Run the subsystem until an error is encountered or a `conclude` signal is received.
// Most errors are non-fatal and should lead to another call to this function.
//
// A return value of `Ok` indicates that an exit should be made, while non-fatal errors
// lead to another call to this function.
async fn run_iteration<Context, B>(ctx: &mut Context, backend: &mut B)
	-> Result<(), Error>
	where
		Context: SubsystemContext<Message = ChainSelectionMessage>,
		B: Backend,
{
	loop {
		let ops: Vec<BackendWriteOp> = match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::Conclude) => {
				return Ok(())
			}
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(update)) => {
				unimplemented!()
			}
			FromOverseer::Signal(OverseerSignal::BlockFinalized(_, _)) => {
				unimplemented!()
			}
			FromOverseer::Communication { msg } => {
				unimplemented!()
			}
		};

		if !ops.is_empty() {
			backend.write(ops)?;
		}
	}
}
