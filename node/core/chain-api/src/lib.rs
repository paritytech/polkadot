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
//! * TODO (now): ancestors

use polkadot_subsystem::{
	FromOverseer, OverseerSignal,
	SpawnedSubsystem, Subsystem, SubsystemResult, SubsystemContext,
	messages::ChainApiRequestMessage,
};
use polkadot_primitives::v1::Block;
use sp_blockchain::HeaderBackend;

use futures::prelude::*;

/// The Chain API Subsystem implementation.
pub struct ChainApiSubsystem<Client> {
	client: Client,
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
					let result = client.number(hash);
					let _ = sender.send(result.ok().flatten());
				},
				ChainApiRequestMessage::FinalizedBlockHash(number, sender) => {
					// TODO: do we need to verify it's finalized?
					let result = client.hash(number);
					let _ = sender.send(result.ok().flatten());
				},
				ChainApiRequestMessage::FinalizedBlockNumber(sender) => {
					let result = client.info().finalized_number;
					// TODO: this is always infallible
					let _ = sender.send(Some(result));
				},
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::v1::{Hash, BlockNumber, BlockId, Header};
	use sp_blockchain::Info as BlockInfo;
	use std::collections::BTreeMap;

	struct TestClient {
		blocks: BTreeMap<Hash, BlockNumber>,
		finalized_blocks: BTreeMap<BlockNumber, Hash>,
	}

	impl Default for TestClient {
		fn default() -> Self {
			Self {
				blocks: maplit::btreemap! {
					Hash::repeat_byte(0x01) => 1,
					Hash::repeat_byte(0x02) => 2,
					Hash::repeat_byte(0x03) => 3,
					Hash::repeat_byte(0x04) => 4,
				},
				finalized_blocks: maplit::btreemap! {
					1 => Hash::repeat_byte(0x01),
					3 => Hash::repeat_byte(0x03),
				},
			}
		}
	}

	fn last_key_value<K: Clone, V: Clone>(map: &BTreeMap<K, V>) -> (K, V) {
		assert!(!map.is_empty());
		map.iter()
			.last()
			.map(|(k, v)| (k.clone(), v.clone()))
			.unwrap()
	} 

	impl HeaderBackend<Block> for TestClient {
		fn info(&self) -> BlockInfo<Block> {
			let genesis_hash = self.blocks.iter().next().map(|(h, _)| *h).unwrap();
			let (best_hash, best_number) = last_key_value(&self.blocks);
			let (finalized_number, finalized_hash) = last_key_value(&self.finalized_blocks);

			BlockInfo {
				best_hash,
				best_number,
				genesis_hash,
				finalized_hash,
				finalized_number,
				number_leaves: 0,
			}
		}
		fn number(&self, hash: Hash) -> sp_blockchain::Result<Option<BlockNumber>> {
			Ok(self.blocks.get(&hash).copied())
		}
		fn hash(&self, number: BlockNumber) -> sp_blockchain::Result<Option<Hash>> {
			Ok(self.finalized_blocks.get(&number).copied())
		}
		fn header(&self, _id: BlockId) -> sp_blockchain::Result<Option<Header>> {
			unimplemented!()
		}
		fn status(&self, _id: BlockId) -> sp_blockchain::Result<sp_blockchain::BlockStatus> {
			unimplemented!()
		}
	}

	#[test]
	fn it_works() {
		todo!()
	}
}
