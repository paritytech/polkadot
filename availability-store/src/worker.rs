// Copyright 2018 Parity Technologies (UK) Ltd.
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

use std::collections::HashMap;
use std::sync::Arc;

use log::warn;
use sr_primitives::traits::{Header as HeaderT, Block as BlockT, ProvideRuntimeApi};
use substrate_primitives::{H256, Blake2Hasher};
use substrate_client::{
	CallExecutor, Client,
	error::Result as ClientResult,
	backend::Backend,
	blockchain::ProvideCache,
};
use consensus_common::{
	self, BlockImport, BlockCheckParams, BlockImportParams, Error as ConsensusError,
	ImportResult,
	import_queue::CacheKeyId,
};
use polkadot_primitives::{Block, BlockId, Hash};
use polkadot_primitives::parachain::{
	CandidateReceipt, ParachainHost, ExtrinsicsQuerying, ValidatorId,
	ValidatorPair, PoVBlock,
};
use keystore::KeyStorePtr;

use crate::Store;

#[allow(dead_code)]
#[derive(Debug)]
enum WorkerMsg {
	ErasureRoots(ErasureRoots),
	ParachainBlocks(ParachainBlocks),
	ListenForChunk(ListenForChunks),
	Chunks(Chunks),
}

#[allow(dead_code)]
#[derive(Debug)]
struct ErasureRoots {
	relay_parent: Hash,
	erasure_roots: Vec<Hash>,
}

#[allow(dead_code)]
#[derive(Debug)]
struct ParachainBlocks {
	relay_parent: Hash,
	blocks: Vec<(CandidateReceipt, Option<PoVBlock>)>,
}

#[allow(dead_code)]
#[derive(Debug)]
struct ListenForChunks;

#[allow(dead_code)]
#[derive(Debug)]
struct Chunks;

pub fn block_import<B, E, Block: BlockT<Hash=H256>, I, RA, PRA>(
	availability_store: Store,
	wrapped_block_import: I,
	client: Arc<Client<B, E, Block, RA>>,
	api: Arc<PRA>,
	keystore: KeyStorePtr,
) -> ClientResult<(AvailabilityBlockImport<B, E, Block, I, RA, PRA>)> where
	B: Backend<Block, Blake2Hasher> + 'static,
	E: CallExecutor<Block, Blake2Hasher> + 'static + Clone + Send + Sync,
	RA: Send + Sync,
{
	let import = AvailabilityBlockImport::new(
		availability_store,
		client,
		api,
		wrapped_block_import,
		keystore,
	);

	Ok(import)
}

pub struct AvailabilityBlockImport<B, E, Block: BlockT, I, RA, PRA> {
	_availability_store: Store,
	inner: I,
	_client: Arc<Client<B, E, Block, RA>>,
	api: Arc<PRA>,
	keystore: KeyStorePtr,
}

impl<B, E, I, RA, PRA> BlockImport<Block> for AvailabilityBlockImport<B, E, Block, I, RA, PRA> where
	I: BlockImport<Block> + Send + Sync,
	I::Error: Into<ConsensusError>,
	B: Backend<Block, Blake2Hasher> + 'static,
	E: CallExecutor<Block, Blake2Hasher> + 'static + Clone + Send + Sync,
	RA: Send + Sync,
	PRA: ProvideRuntimeApi + ProvideCache<Block>,
	PRA::Api: ParachainHost<Block> + ExtrinsicsQuerying<Block>,
{
	type Error = ConsensusError;

	fn import_block(
		&mut self,
		block: BlockImportParams<Block>,
		new_cache: HashMap<CacheKeyId, Vec<u8>>,
	) -> Result<ImportResult, Self::Error> {
		warn!(target: "availability", "import_block {:?}", block.header);

		if let Some(ref extrinsics) = block.body {
			let parent_id = BlockId::hash(*block.header.parent_hash());
			let validators = self.api.runtime_api().validators(&parent_id)
				.map_err(|e| ConsensusError::ChainLookup(e.to_string()))?;

			let our_id = self.our_id(&validators);
			let sh = self.api.runtime_api().get_erasure_roots(&parent_id, extrinsics.clone());
			warn!(target: "availability", "roots {:?}", sh);
			warn!(target: "availability", "our_id {:?}", our_id);
		}
		self.inner.import_block(block, new_cache).map_err(Into::into)
	}

	fn check_block(
		&mut self,
		block: BlockCheckParams<Block>,
	) -> Result<ImportResult, Self::Error> {
		self.inner.check_block(block).map_err(Into::into)
	}
}

impl<B, E, Block: BlockT, I, RA, PRA> AvailabilityBlockImport<B, E, Block, I, RA, PRA> {
	fn new(
		availability_store: Store,
		client: Arc<Client<B, E, Block, RA>>,
		api: Arc<PRA>,
		block_import: I,
		keystore: KeyStorePtr,
	) -> Self {
		AvailabilityBlockImport {
			_availability_store: availability_store,
			api,
			_client: client,
			inner: block_import,
			keystore,
		}
	}

	fn our_id(&self, validators: &[ValidatorId]) -> Option<usize> {
		let keystore = self.keystore.read();
		validators
			.iter()
			.enumerate()
			.find_map(|(i, v)| {
				keystore.key_pair::<ValidatorPair>(&v).map(|_| i).ok()
			})
	}
}
