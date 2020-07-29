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

//! Implements the Chain API Subsystem
//!
//! Provides access to the chain data. Every request may return an error.
//!
//! Supported requests:
//! * Block hash to number
//! * Finalized block number to hash
//! * Last finalized block number
//! * TODO (now) complete the list

use polkadot_subsystem::{
	FromOverseer, OverseerSignal,
	SpawnedSubsystem, Subsystem, SubsystemResult, SubsystemContext,
};
use polkadot_subsystem::messages::{
	ChainApiRequestMessage,
};
use polkadot_primitives::v1::{Block, BlockId, Hash};
use sp_blockchain::{Info as BlockchainInfo, HeaderBackend};

use futures::prelude::*;

/// The Chain API Subsystem implementation.
pub struct ChainApiSubsystem<Client> {
	client: Client,
	// TODO cache?
}

impl<Client> ChainApiSubsystem<Client> {
	/// Create a new Chain API subsystem with the given client.
	pub fn new(client: Client) -> Self {
		ChainApiSubsystem {
			client
		}
	}
}

impl<Client, Context> Subsystem<Context> for ChainApiSubsystem<Client> where
	Client: HeaderBackend<Block> + 'static,
	Context: SubsystemContext<Message = ChainApiRequestMessage>
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem {
			future: run(ctx, self.client).map(|_| ()).boxed(),
			name: "ChainApiSubsystem",
		}
	}
}

async fn run<Client>(
	mut ctx: impl SubsystemContext<Message = ChainApiRequestMessage>,
	client: Client,
) -> SubsystemResult<()>
where
	Client: HeaderBackend<Block>,
{
	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(_)) => {},
			FromOverseer::Signal(OverseerSignal::BlockFinalized(_)) => {},
			FromOverseer::Communication { msg } => match msg {
				ChainApiRequestMessage::BlockNumber(hash, sender) => {
					todo!()
				},
				ChainApiRequestMessage::FinalizedBlockHash(number, sender) => {
					todo!()
				},
				ChainApiRequestMessage::FinalizedBlockNumber(sender) => {
					todo!()
				},
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn it_works() {
		todo!()
	}
}
