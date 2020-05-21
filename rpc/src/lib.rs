// Copyright 2019-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot-specific RPCs implementation.

#![warn(missing_docs)]

use std::sync::Arc;

use polkadot_primitives::{Block, BlockNumber, AccountId, Nonce, Balance, Hash};
use sp_api::ProvideRuntimeApi;
use txpool_api::TransactionPool;
use sp_blockchain::{HeaderBackend, HeaderMetadata, Error as BlockChainError};
use sp_consensus::SelectChain;
use sp_consensus_babe::BabeApi;
use sc_client_api::light::{Fetcher, RemoteBlockchain};
use sc_consensus_babe::Epoch;
use sc_rpc::DenyUnsafe;

/// A type representing all RPC extensions.
pub type RpcExtension = jsonrpc_core::IoHandler<sc_rpc::Metadata>;

/// Light client extra dependencies.
pub struct LightDeps<C, F, P> {
	/// The client instance to use.
	pub client: Arc<C>,
	/// Transaction pool instance.
	pub pool: Arc<P>,
	/// Remote access to the blockchain (async).
	pub remote_blockchain: Arc<dyn RemoteBlockchain<Block>>,
	/// Fetcher instance.
	pub fetcher: Arc<F>,
}

/// Extra dependencies for BABE.
pub struct BabeDeps {
	/// BABE protocol config.
	pub babe_config: sc_consensus_babe::Config,
	/// BABE pending epoch changes.
	pub shared_epoch_changes: sc_consensus_epochs::SharedEpochChanges<Block, Epoch>,
	/// The keystore that manages the keys of the node.
	pub keystore: sc_keystore::KeyStorePtr,
}

/// Dependencies for GRANDPA
pub struct GrandpaDeps {
	/// Voting round info.
	pub shared_voter_state: sc_finality_grandpa::SharedVoterState,
	/// Authority set info.
	pub shared_authority_set: sc_finality_grandpa::SharedAuthoritySet<Hash, BlockNumber>,
}

/// Full client dependencies
pub struct FullDeps<C, P, SC> {
	/// The client instance to use.
	pub client: Arc<C>,
	/// Transaction pool instance.
	pub pool: Arc<P>,
	/// The SelectChain Strategy
	pub select_chain: SC,
	/// Whether to deny unsafe calls
	pub deny_unsafe: DenyUnsafe,
	/// BABE specific dependencies.
	pub babe: BabeDeps,
	/// GRANDPA specific dependencies.
	pub grandpa: GrandpaDeps,
}

/// Instantiate all RPC extensions.
pub fn create_full<C, P, UE, SC>(deps: FullDeps<C, P, SC>) -> RpcExtension where
	C: ProvideRuntimeApi<Block>,
	C: HeaderBackend<Block> + HeaderMetadata<Block, Error=BlockChainError>,
	C: Send + Sync + 'static,
	C::Api: frame_rpc_system::AccountNonceApi<Block, AccountId, Nonce>,
	C::Api: pallet_transaction_payment_rpc::TransactionPaymentRuntimeApi<Block, Balance, UE>,
	C::Api: BabeApi<Block>,
	P: TransactionPool + Sync + Send + 'static,
	UE: codec::Codec + Send + Sync + 'static,
	SC: SelectChain<Block> + 'static,
{
	use frame_rpc_system::{FullSystem, SystemApi};
	use pallet_transaction_payment_rpc::{TransactionPayment, TransactionPaymentApi};
	use sc_finality_grandpa_rpc::{GrandpaApi, GrandpaRpcHandler};
	use sc_consensus_babe_rpc::BabeRpcHandler;

	let mut io = jsonrpc_core::IoHandler::default();
	let FullDeps {
		client,
		pool,
		select_chain,
		deny_unsafe,
		babe,
		grandpa,
	} = deps;
	let BabeDeps {
		keystore,
		babe_config,
		shared_epoch_changes,
	} = babe;
	let GrandpaDeps {
		shared_voter_state,
		shared_authority_set,
	} = grandpa;

	io.extend_with(
		SystemApi::to_delegate(FullSystem::new(client.clone(), pool))
	);
	io.extend_with(
		TransactionPaymentApi::to_delegate(TransactionPayment::new(client.clone()))
	);
	io.extend_with(
		sc_consensus_babe_rpc::BabeApi::to_delegate(
			BabeRpcHandler::new(
				client,
				shared_epoch_changes,
				keystore,
				babe_config,
				select_chain,
				deny_unsafe,
			)
		)
	);
	io.extend_with(
		GrandpaApi::to_delegate(GrandpaRpcHandler::new(
			shared_authority_set,
			shared_voter_state,
		))
	);
	io
}

/// Instantiate all RPC extensions for light node.
pub fn create_light<C, P, F, UE>(deps: LightDeps<C, F, P>) -> RpcExtension
	where
		C: ProvideRuntimeApi<Block>,
		C: HeaderBackend<Block>,
		C: Send + Sync + 'static,
		C::Api: frame_rpc_system::AccountNonceApi<Block, AccountId, Nonce>,
		C::Api: pallet_transaction_payment_rpc::TransactionPaymentRuntimeApi<Block, Balance, UE>,
		P: TransactionPool + Sync + Send + 'static,
		F: Fetcher<Block> + 'static,
		UE: codec::Codec + Send + Sync + 'static,
{
	use frame_rpc_system::{LightSystem, SystemApi};

	let LightDeps {
		client,
		pool,
		remote_blockchain,
		fetcher,
	} = deps;
	let mut io = jsonrpc_core::IoHandler::default();
	io.extend_with(
		SystemApi::<AccountId, Nonce>::to_delegate(LightSystem::new(client, remote_blockchain, fetcher, pool))
	);
	io
}
