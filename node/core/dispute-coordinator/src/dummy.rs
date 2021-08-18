// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Implements the dispute coordinator subsystem (dummy implementation).

use std::sync::Arc;

use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	messages::DisputeCoordinatorMessage,
	overseer, FromOverseer, OverseerSignal, SpawnedSubsystem, SubsystemContext, SubsystemError,
};
use polkadot_primitives::v1::BlockNumber;

use futures::{channel::oneshot, prelude::*};
use kvdb::KeyValueDB;
use parity_scale_codec::{Decode, Encode, Error as CodecError};
use sc_keystore::LocalKeystore;

const LOG_TARGET: &str = "parachain::dispute-coordinator";

/// Timestamp based on the 1 Jan 1970 UNIX base, which is persistent across node restarts and OS reboots.
type Timestamp = u64;

#[derive(Eq, PartialEq)]
enum Participation {}

struct State {}

/// Configuration for the dispute coordinator subsystem.
#[derive(Debug, Clone, Copy)]
pub struct Config {
	/// The data column in the store to use for dispute data.
	pub col_data: u32,
}

/// An implementation of the dispute coordinator subsystem.
pub struct DisputeCoordinatorSubsystem {}

impl DisputeCoordinatorSubsystem {
	/// Create a new instance of the subsystem.
	pub fn new(_: Arc<dyn KeyValueDB>, _: Config, _: Arc<LocalKeystore>) -> Self {
		DisputeCoordinatorSubsystem {}
	}
}

impl<Context> overseer::Subsystem<Context, SubsystemError> for DisputeCoordinatorSubsystem
where
	Context: SubsystemContext<Message = DisputeCoordinatorMessage>,
	Context: overseer::SubsystemContext<Message = DisputeCoordinatorMessage>,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = run(self, ctx).map(|_| Ok(())).boxed();

		SpawnedSubsystem { name: "dispute-coordinator-subsystem", future }
	}
}

#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),

	#[error(transparent)]
	ChainApi(#[from] ChainApiError),

	#[error(transparent)]
	Io(#[from] std::io::Error),

	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),

	#[error("Oneshot send failed")]
	OneshotSend,

	#[error(transparent)]
	Subsystem(#[from] SubsystemError),

	#[error(transparent)]
	Codec(#[from] CodecError),
}

impl Error {
	fn trace(&self) {
		match self {
			// don't spam the log with spurious errors
			Self::RuntimeApi(_) | Self::Oneshot(_) =>
				tracing::debug!(target: LOG_TARGET, err = ?self),
			// it's worth reporting otherwise
			_ => tracing::warn!(target: LOG_TARGET, err = ?self),
		}
	}
}

/// The status of dispute. This is a state machine which can be altered by the
/// helper methods.
#[derive(Debug, Clone, Copy, Encode, Decode, PartialEq)]
pub enum DisputeStatus {
	/// The dispute is active and unconcluded.
	#[codec(index = 0)]
	Active,
	/// The dispute has been concluded in favor of the candidate
	/// since the given timestamp.
	#[codec(index = 1)]
	ConcludedFor(Timestamp),
	/// The dispute has been concluded against the candidate
	/// since the given timestamp.
	///
	/// This takes precedence over `ConcludedFor` in the case that
	/// both are true, which is impossible unless a large amount of
	/// validators are participating on both sides.
	#[codec(index = 2)]
	ConcludedAgainst(Timestamp),
}

impl DisputeStatus {
	/// Initialize the status to the active state.
	pub fn active() -> DisputeStatus {
		DisputeStatus::Active
	}

	/// Transition the status to a new status after observing the dispute has concluded for the candidate.
	/// This may be a no-op if the status was already concluded.
	pub fn concluded_for(self, now: Timestamp) -> DisputeStatus {
		match self {
			DisputeStatus::Active => DisputeStatus::ConcludedFor(now),
			DisputeStatus::ConcludedFor(at) => DisputeStatus::ConcludedFor(std::cmp::min(at, now)),
			against => against,
		}
	}

	/// Transition the status to a new status after observing the dispute has concluded against the candidate.
	/// This may be a no-op if the status was already concluded.
	pub fn concluded_against(self, now: Timestamp) -> DisputeStatus {
		match self {
			DisputeStatus::Active => DisputeStatus::ConcludedAgainst(now),
			DisputeStatus::ConcludedFor(at) =>
				DisputeStatus::ConcludedAgainst(std::cmp::min(at, now)),
			DisputeStatus::ConcludedAgainst(at) =>
				DisputeStatus::ConcludedAgainst(std::cmp::min(at, now)),
		}
	}

	/// Whether the disputed candidate is possibly invalid.
	pub fn is_possibly_invalid(&self) -> bool {
		match self {
			DisputeStatus::Active | DisputeStatus::ConcludedAgainst(_) => true,
			DisputeStatus::ConcludedFor(_) => false,
		}
	}

	/// Yields the timestamp this dispute concluded at, if any.
	pub fn concluded_at(&self) -> Option<Timestamp> {
		match self {
			DisputeStatus::Active => None,
			DisputeStatus::ConcludedFor(at) | DisputeStatus::ConcludedAgainst(at) => Some(*at),
		}
	}
}

async fn run<Context>(subsystem: DisputeCoordinatorSubsystem, mut ctx: Context)
where
	Context: overseer::SubsystemContext<Message = DisputeCoordinatorMessage>,
	Context: SubsystemContext<Message = DisputeCoordinatorMessage>,
{
	loop {
		let res = run_until_error(&mut ctx, &subsystem).await;
		match res {
			Err(e) => {
				e.trace();

				if let Error::Subsystem(SubsystemError::Context(_)) = e {
					break
				}
			},
			Ok(()) => {
				tracing::info!(target: LOG_TARGET, "received `Conclude` signal, exiting");
				break
			},
		}
	}
}

async fn run_until_error<Context>(
	ctx: &mut Context,
	_: &DisputeCoordinatorSubsystem,
) -> Result<(), Error>
where
	Context: overseer::SubsystemContext<Message = DisputeCoordinatorMessage>,
	Context: SubsystemContext<Message = DisputeCoordinatorMessage>,
{
	let mut state = State {};

	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(_)) => {},
			FromOverseer::Signal(OverseerSignal::BlockFinalized(_, _)) => {},
			FromOverseer::Communication { msg } => handle_incoming(ctx, &mut state, msg).await?,
		}
	}
}

async fn handle_incoming(
	_: &mut impl SubsystemContext,
	_: &mut State,
	message: DisputeCoordinatorMessage,
) -> Result<(), Error> {
	match message {
		DisputeCoordinatorMessage::ImportStatements { .. } => { /* just drop confirmation */ },
		DisputeCoordinatorMessage::RecentDisputes(tx) => {
			let _ = tx.send(Vec::new());
		},
		DisputeCoordinatorMessage::ActiveDisputes(tx) => {
			let _ = tx.send(Vec::new());
		},
		DisputeCoordinatorMessage::QueryCandidateVotes(_, tx) => {
			let _ = tx.send(Vec::new());
		},
		DisputeCoordinatorMessage::IssueLocalStatement(_, _, _, _) => {},
		DisputeCoordinatorMessage::DetermineUndisputedChain {
			base_number,
			block_descriptions,
			tx,
		} => {
			let undisputed_chain = block_descriptions
				.last()
				.map(|e| (base_number + block_descriptions.len() as BlockNumber, e.block_hash));

			let _ = tx.send(undisputed_chain);
		},
	}

	Ok(())
}

#[derive(Debug, thiserror::Error)]
enum DisputeMessageCreationError {}
