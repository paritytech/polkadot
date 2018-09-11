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
use state_machine;

//use runtime::{Block, Header, Address, BlockId};
use runtime::Address;
use runtime_primitives::traits::Lookup;
use primitives::{
	Block, Header, BlockId,
	AccountId, Hash, Index, InherentData,
	SessionKey, Timestamp, UncheckedExtrinsic,
};
use primitives::parachain::{DutyRoster, Id as ParaId};
use substrate_primitives::{Blake2Hasher, RlpCodec};

use {BlockBuilder, PolkadotApi, LocalPolkadotApi, ErrorKind, Error, Result};

// set up the necessary scaffolding to execute a set of calls to the runtime.
// this creates a new block on top of the given ID and initialises it.
macro_rules! with_runtime {
	($client: ident, $at: expr, $exec: expr) => {{
		let parent = $at;
		let header = Header {
			parent_hash: $client.block_hash_from_id(&parent)?
				.ok_or_else(|| ErrorKind::UnknownBlock(format!("{:?}", parent)))?,
			number: $client.block_number_from_id(&parent)?
				.ok_or_else(|| ErrorKind::UnknownBlock(format!("{:?}", parent)))? + 1,
			state_root: Default::default(),
			extrinsics_root: Default::default(),
			digest: Default::default(),
		};

		$client.state_at(&parent).map_err(Error::from).and_then(|state| {
			let mut changes = Default::default();
			let mut ext = state_machine::Ext::new(&mut changes, &state);

			::substrate_executor::with_native_environment(&mut ext, || {
				::runtime::Executive::initialise_block(&header);
				($exec)()
			}).map_err(Into::into)
		})
	}}
}

impl<B: LocalBackend<Block, Blake2Hasher, RlpCodec>> BlockBuilder for ClientBlockBuilder<B, LocalCallExecutor<B, NativeExecutor<LocalDispatch>>, Block, Blake2Hasher, RlpCodec> {
	fn push_extrinsic(&mut self, extrinsic: UncheckedExtrinsic) -> Result<()> {
		self.push(extrinsic).map_err(Into::into)
	}

	/// Bake the block with provided extrinsics.
	fn bake(self) -> Result<Block> {
		ClientBlockBuilder::bake(self).map_err(Into::into)
	}
}

impl<B: LocalBackend<Block, Blake2Hasher, RlpCodec>> PolkadotApi for Client<B, LocalCallExecutor<B, NativeExecutor<LocalDispatch>>, Block> {
	type BlockBuilder = ClientBlockBuilder<B, LocalCallExecutor<B, NativeExecutor<LocalDispatch>>, Block, Blake2Hasher, RlpCodec>;

	fn session_keys(&self, at: &BlockId) -> Result<Vec<SessionKey>> {
		with_runtime!(self, at, ::runtime::Consensus::authorities)
	}

	fn validators(&self, at: &BlockId) -> Result<Vec<AccountId>> {
		with_runtime!(self, at, ::runtime::Session::validators)
	}

	fn random_seed(&self, at: &BlockId) -> Result<Hash> {
		with_runtime!(self, at, ::runtime::System::random_seed)
	}

	fn duty_roster(&self, at: &BlockId) -> Result<DutyRoster> {
		with_runtime!(self, at, ::runtime::Parachains::calculate_duty_roster)
	}

	fn timestamp(&self, at: &BlockId) -> Result<Timestamp> {
		with_runtime!(self, at, ::runtime::Timestamp::get)
	}

	fn evaluate_block(&self, at: &BlockId, block: Block) -> Result<bool> {
		use substrate_executor::error::ErrorKind as ExecErrorKind;
		use codec::Encode;
		use state_machine::ExecutionManager;
		use client::CallExecutor;

		let parent = at;
		let res = self.state_at(&parent).map_err(Error::from).and_then(|state| {
			let mut overlay = Default::default();
			let execution_manager = || ExecutionManager::Both(|wasm_result, native_result| {
				warn!("Consensus error between wasm and native runtime execution at block {:?}", at);
				warn!("   While executing block {:?}", (block.header.number, block.header.hash()));
				warn!("   Native result {:?}", native_result);
				warn!("   Wasm result {:?}", wasm_result);
				wasm_result
			});
			let (r, _) = self.executor().call_at_state(
				&state,
				&mut overlay,
				"execute_block",
				&block.encode(),
				execution_manager()
			)?;

			Ok(r)
		});

		match res {
			Ok(_) => Ok(true),
			Err(err) => match err.kind() {
				&ErrorKind::Executor(ExecErrorKind::Runtime) => Ok(false),
				_ => Err(err)
			}
		}
	}

	fn index(&self, at: &BlockId, account: AccountId) -> Result<Index> {
		with_runtime!(self, at, || ::runtime::System::account_nonce(account))
	}

	fn lookup(&self, at: &BlockId, address: Address) -> Result<Option<AccountId>> {
		with_runtime!(self, at, || <::runtime::Balances as Lookup>::lookup(address).ok())
	}

	fn active_parachains(&self, at: &BlockId) -> Result<Vec<ParaId>> {
		with_runtime!(self, at, ::runtime::Parachains::active_parachains)
	}

	fn parachain_code(&self, at: &BlockId, parachain: ParaId) -> Result<Option<Vec<u8>>> {
		with_runtime!(self, at, || ::runtime::Parachains::parachain_code(parachain))
	}

	fn parachain_head(&self, at: &BlockId, parachain: ParaId) -> Result<Option<Vec<u8>>> {
		with_runtime!(self, at, || ::runtime::Parachains::parachain_head(parachain))
	}

	fn build_block(&self, at: &BlockId, inherent_data: InherentData) -> Result<Self::BlockBuilder> {
		let mut block_builder = self.new_block_at(at)?;
		for inherent in self.inherent_extrinsics(at, inherent_data)? {
			block_builder.push(inherent)?;
		}

		Ok(block_builder)
	}

	fn inherent_extrinsics(&self, at: &BlockId, inherent_data: InherentData) -> Result<Vec<UncheckedExtrinsic>> {
		use codec::{Encode, Decode};

		let runtime_version = self.runtime_version_at(at)?;

		with_runtime!(self, at, || {
			let extrinsics = ::runtime::inherent_extrinsics(inherent_data, runtime_version.spec_version);
			extrinsics.into_iter()
				.map(|x| x.encode()) // get encoded representation
				.map(|x| Decode::decode(&mut &x[..])) // get byte-vec equivalent to extrinsic
				.map(|x| x.expect("UncheckedExtrinsic has encoded representation equivalent to Vec<u8>; qed"))
				.collect()
		})
	}
}

impl<B: LocalBackend<Block, Blake2Hasher, RlpCodec>> LocalPolkadotApi for Client<B, LocalCallExecutor<B, NativeExecutor<LocalDispatch>>, Block>
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

	fn client() -> Client<InMemory<Block, Blake2Hasher, RlpCodec>, LocalCallExecutor<InMemory<Block, Blake2Hasher, RlpCodec>, NativeExecutor<LocalDispatch>>, Block> {
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
		};

		::client::new_in_mem(LocalDispatch::with_heap_pages(8), genesis_config).unwrap()
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
		assert!(client.random_seed(&id).is_ok());
	}
}
