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

use crate::{AllMessages, OverseerSignal};
use polkadot_node_subsystem_types::errors::SubsystemError;
use polkadot_overseer_gen::{FromOverseer, SpawnedSubsystem, Subsystem, SubsystemContext};

/// A dummy subsystem that implements [`Subsystem`] for all
/// types of messages. Used for tests or as a placeholder.
#[derive(Clone, Copy, Debug)]
pub struct DummySubsystem;

impl<Context> Subsystem<Context, SubsystemError> for DummySubsystem
where
	Context: SubsystemContext<
		Signal = OverseerSignal,
		Error = SubsystemError,
		AllMessages = AllMessages,
	>,
{
	fn start(self, mut ctx: Context) -> SpawnedSubsystem<SubsystemError> {
		let future = Box::pin(async move {
			loop {
				match ctx.recv().await {
					Err(_) => return Ok(()),
					Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => return Ok(()),
					Ok(overseer_msg) => {
						tracing::debug!(
							target: "dummy-subsystem",
							"Discarding a message sent from overseer {:?}",
							overseer_msg
						);
						continue
					},
				}
			}
		});

		SpawnedSubsystem { name: "dummy-subsystem", future }
	}
}
