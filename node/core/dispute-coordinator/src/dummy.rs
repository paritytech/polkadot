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

use polkadot_node_subsystem::{
	messages::DisputeCoordinatorMessage, overseer, FromOverseer, OverseerSignal, SpawnedSubsystem,
	SubsystemContext, SubsystemError,
};
use polkadot_primitives::v1::BlockNumber;

use futures::prelude::*;

use crate::error::Result;
use fatality::Nested;

const LOG_TARGET: &str = "parachain::dispute-coordinator";

#[derive(Eq, PartialEq)]
enum Participation {}

struct State {}

/// An implementation of the dispute coordinator subsystem.
pub struct DisputeCoordinatorSubsystem {}

impl DisputeCoordinatorSubsystem {
	/// Create a new instance of the subsystem.
	pub fn new() -> Self {
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

async fn run<Context>(subsystem: DisputeCoordinatorSubsystem, mut ctx: Context)
where
	Context: overseer::SubsystemContext<Message = DisputeCoordinatorMessage>,
	Context: SubsystemContext<Message = DisputeCoordinatorMessage>,
{
	loop {
		let res = run_until_error(&mut ctx, &subsystem).await;
		match res.into_nested() {
			Err(fatal) => {
				tracing::error!(target: LOG_TARGET, "Observed fatal issue: {:?}", fatal);
				break
			},
			Ok(Err(jfyi)) => {
				tracing::debug!(target: LOG_TARGET, "Observed issue: {:?}", jfyi);
			},
			Ok(Ok(())) => {
				tracing::info!(target: LOG_TARGET, "Received `Conclude` signal, exiting");
				break
			},
		}
	}
}

async fn run_until_error<Context>(ctx: &mut Context, _: &DisputeCoordinatorSubsystem) -> Result<()>
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
) -> Result<()> {
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
			base: (base_number, base_hash),
			block_descriptions,
			tx,
		} => {
			let undisputed_chain = block_descriptions
				.last()
				.map(|e| (base_number + block_descriptions.len() as BlockNumber, e.block_hash))
				.unwrap_or((base_number, base_hash));

			let _ = tx.send(undisputed_chain);
		},
	}

	Ok(())
}

#[derive(Debug, thiserror::Error)]
enum DisputeMessageCreationError {}
