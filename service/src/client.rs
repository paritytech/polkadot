// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot Client meta trait

use consensus_common::BlockImport;
use sp_api::{ProvideRuntimeApi, ConstructRuntimeApi, CallApiAt};
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use sc_block_builder::BlockBuilderProvider;
use sc_client_api::{Backend as BackendT, BlockchainEvents, TransactionFor};

/// Polkadot client abstraction, this super trait only pulls in functionality required for
/// polkadot internal crates like polkadot-collator.
pub trait PolkadotClient<Block, Backend, Runtime>:
	BlockchainEvents<Block> + Sized + Send + Sync
	+ ProvideRuntimeApi<Block, Api = Runtime::RuntimeApi>
	+ HeaderBackend<Block>
	+ BlockBuilderProvider<Backend, Block, Self>
	+ CallApiAt<
		Block,
		Error = sp_blockchain::Error,
		StateBackend = Backend ::State
	>
	where
		Block: BlockT,
		Backend: BackendT<Block>,
		Runtime: ConstructRuntimeApi<Block, Self>,
		for<'r> &'r Self: BlockImport<Block, Error = consensus_common::Error, Transaction = TransactionFor<Backend, Block>>,
{}

impl<Block, Backend, Runtime, Client> PolkadotClient<Block, Backend, Runtime> for Client
	where
		Block: BlockT,
		Runtime: ConstructRuntimeApi<Block, Self>,
		Backend: BackendT<Block>,
		Client: BlockchainEvents<Block> + ProvideRuntimeApi<Block, Api = Runtime::RuntimeApi> + HeaderBackend<Block>
			+ BlockBuilderProvider<Backend, Block, Self>
			+ Sized + Send + Sync
			+ CallApiAt<
				Block,
				Error = sp_blockchain::Error,
				StateBackend = Backend ::State
			>,
		for<'r> &'r Client: BlockImport<Block, Error = consensus_common::Error, Transaction = TransactionFor<Backend, Block>>,
{}
