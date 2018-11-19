// Copyright 2017 Parity Technologies (UK) Ltd.
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

//! Strongly typed API for full Polkadot client.

use client::backend::LocalBackend;
use client::block_builder::BlockBuilder as ClientBlockBuilder;
use client::{Client, LocalCallExecutor};
use polkadot_executor::Executor as LocalDispatch;
use substrate_executor::NativeExecutor;

//use runtime::{Block, Header, Address, BlockId};
use runtime::Address;
use primitives::{
	Block, BlockId,
	AccountId, Hash, Index, InherentData,
	SessionKey, Timestamp, UncheckedExtrinsic,
};
use primitives::parachain::{DutyRoster, Id as ParaId};
use substrate_primitives::{Blake2Hasher};
use {BlockBuilder, PolkadotApi, LocalPolkadotApi, ErrorKind, Result};

impl<B: LocalBackend<Block, Blake2Hasher>> BlockBuilder for ClientBlockBuilder<B, LocalCallExecutor<B, NativeExecutor<LocalDispatch>>, Block, Blake2Hasher> {
	fn push_extrinsic(&mut self, extrinsic: UncheckedExtrinsic) -> Result<()> {
		self.push(extrinsic).map_err(Into::into)
	}

	/// Bake the block with provided extrinsics.
	fn bake(self) -> Result<Block> {
		ClientBlockBuilder::bake(self).map_err(Into::into)
	}
}

impl<B: LocalBackend<Block, Blake2Hasher>> PolkadotApi for Client<B, LocalCallExecutor<B, NativeExecutor<LocalDispatch>>, Block> {
	type BlockBuilder = ClientBlockBuilder<B, LocalCallExecutor<B, NativeExecutor<LocalDispatch>>, Block, Blake2Hasher>;

	fn session_keys(&self, at: &BlockId) -> Result<Vec<SessionKey>> {
		Ok(self.authorities_at(at)?)
	}

	fn validators(&self, at: &BlockId) -> Result<Vec<AccountId>> {
		Ok(self.call_api_at(at, "validators", &())?)
	}

	fn random_seed(&self, at: &BlockId) -> Result<Hash> {
		Ok(self.call_api_at(at, "random_seed", &())?)
	}

	fn duty_roster(&self, at: &BlockId) -> Result<DutyRoster> {
		Ok(self.call_api_at(at, "duty_roster", &())?)
	}

	fn timestamp(&self, at: &BlockId) -> Result<Timestamp> {
		Ok(self.call_api_at(at, "timestamp", &())?)
	}

	fn evaluate_block(&self, at: &BlockId, block: Block) -> Result<bool> {
		let res: Result<()> = self.call_api_at(at, "execute_block", &block).map_err(From::from);
		match res {
			Ok(_) => Ok(true),
			Err(err) => match err.kind() {
				&ErrorKind::Execution(_) => Ok(false),
				_ => Err(err)
			}
		}
	}

	fn index(&self, at: &BlockId, account: AccountId) -> Result<Index> {
		Ok(self.call_api_at(at, "account_nonce", &account)?)
	}

	fn lookup(&self, at: &BlockId, address: Address) -> Result<Option<AccountId>> {
		Ok(self.call_api_at(at, "lookup_address", &address)?)
	}

	fn active_parachains(&self, at: &BlockId) -> Result<Vec<ParaId>> {
		Ok(self.call_api_at(at, "active_parachains", &())?)
	}

	fn parachain_code(&self, at: &BlockId, parachain: ParaId) -> Result<Option<Vec<u8>>> {
		Ok(self.call_api_at(at, "parachain_code", &parachain)?)
	}

	fn parachain_head(&self, at: &BlockId, parachain: ParaId) -> Result<Option<Vec<u8>>> {
		Ok(self.call_api_at(at, "parachain_head", &parachain)?)
	}

	fn build_block(&self, at: &BlockId, inherent_data: InherentData) -> Result<Self::BlockBuilder> {
		let mut block_builder = self.new_block_at(at)?;
		for inherent in self.inherent_extrinsics(at, inherent_data)? {
			block_builder.push(inherent)?;
		}

		Ok(block_builder)
	}

	fn inherent_extrinsics(&self, at: &BlockId, inherent_data: InherentData) -> Result<Vec<UncheckedExtrinsic>> {
		let runtime_version = self.runtime_version_at(at)?;
		Ok(self.call_api_at(at, "inherent_extrinsics", &(inherent_data, runtime_version.spec_version))?)
	}
}

impl<B: LocalBackend<Block, Blake2Hasher>> LocalPolkadotApi for Client<B, LocalCallExecutor<B, NativeExecutor<LocalDispatch>>, Block>
{}

#[cfg(test)]
mod tests {
	use super::*;
	use keyring::Keyring;
	use client::LocalCallExecutor;
	use client::in_mem::Backend as InMemory;
	use substrate_executor::NativeExecutionDispatch;
	use runtime::{GenesisConfig, ConsensusConfig, SessionConfig};

	fn validators() -> Vec<AccountId> {
		vec![
			Keyring::One.to_raw_public().into(),
			Keyring::Two.to_raw_public().into(),
		]
	}

	fn session_keys() -> Vec<SessionKey> {
		vec![
			Keyring::One.to_raw_public().into(),
			Keyring::Two.to_raw_public().into(),
		]
	}

	fn client() -> Client<InMemory<Block, Blake2Hasher>, LocalCallExecutor<InMemory<Block, Blake2Hasher>, NativeExecutor<LocalDispatch>>, Block> {
		let genesis_config = GenesisConfig {
			consensus: Some(ConsensusConfig {
				code: LocalDispatch::native_equivalent().to_vec(),
				authorities: session_keys(),
			}),
			system: None,
			balances: Some(Default::default()),
			session: Some(SessionConfig {
				validators: validators(),
				session_length: 100,
			}),
			council: Some(Default::default()),
			democracy: Some(Default::default()),
			parachains: Some(Default::default()),
			staking: Some(Default::default()),
			timestamp: Some(Default::default()),
			treasury: Some(Default::default()),
		};

		::client::new_in_mem(LocalDispatch::new(), genesis_config).unwrap()
	}

	#[test]
	fn gets_session_and_validator_keys() {
		let client = client();
		let id = BlockId::number(0);
		assert_eq!(client.session_keys(&id).unwrap(), session_keys());
		assert_eq!(client.validators(&id).unwrap(), validators());
	}

	#[test]
	fn build_block_implicit_succeeds() {
		let client = client();

		let id = BlockId::number(0);
		let block_builder = client.build_block(&id, InherentData {
			timestamp: 1_000_000,
			parachain_heads: Vec::new(),
			offline_indices: Vec::new(),
		}).unwrap();
		let block = block_builder.bake().unwrap();

		assert_eq!(block.header.number, 1);
		assert!(block.header.extrinsics_root != Default::default());
		assert!(client.evaluate_block(&id, block).unwrap());
	}

	#[test]
	fn build_block_with_inherent_succeeds() {
		let client = client();

		let id = BlockId::number(0);
		let inherent = client.inherent_extrinsics(&id, InherentData {
			timestamp: 1_000_000,
			parachain_heads: Vec::new(),
			offline_indices: Vec::new(),
		}).unwrap();

		let mut block_builder = client.new_block_at(&id).unwrap();
		for extrinsic in inherent {
			block_builder.push(extrinsic).unwrap();
		}

		let block = block_builder.bake().unwrap();

		assert_eq!(block.header.number, 1);
		assert!(block.header.extrinsics_root != Default::default());
		assert!(client.evaluate_block(&id, block).unwrap());
	}

	#[test]
	fn gets_random_seed_with_genesis() {
		let client = client();

		let id = BlockId::number(0);
		client.random_seed(&id).unwrap();
	}
}
