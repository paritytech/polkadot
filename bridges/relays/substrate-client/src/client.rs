// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

//! Substrate node client.

use crate::chain::{Chain, ChainWithBalances};
use crate::rpc::{Substrate, SubstrateMessageLane};
use crate::{ConnectionParams, Error, Result};

use bp_message_lane::{LaneId, MessageNonce};
use bp_runtime::InstanceId;
use codec::Decode;
use frame_system::AccountInfo;
use jsonrpsee::common::DeserializeOwned;
use jsonrpsee::raw::RawClient;
use jsonrpsee::transport::ws::WsTransportClient;
use jsonrpsee::{client::Subscription, Client as RpcClient};
use num_traits::Zero;
use pallet_balances::AccountData;
use sp_core::Bytes;
use sp_trie::StorageProof;
use sp_version::RuntimeVersion;
use std::ops::RangeInclusive;

const SUB_API_GRANDPA_AUTHORITIES: &str = "GrandpaApi_grandpa_authorities";

/// Opaque justifications subscription type.
pub type JustificationsSubscription = Subscription<Bytes>;

/// Opaque GRANDPA authorities set.
pub type OpaqueGrandpaAuthoritiesSet = Vec<u8>;

/// Substrate client type.
///
/// Cloning `Client` is a cheap operation.
pub struct Client<C: Chain> {
	/// Client connection params.
	params: ConnectionParams,
	/// Substrate RPC client.
	client: RpcClient,
	/// Genesis block hash.
	genesis_hash: C::Hash,
}

impl<C: Chain> Clone for Client<C> {
	fn clone(&self) -> Self {
		Client {
			params: self.params.clone(),
			client: self.client.clone(),
			genesis_hash: self.genesis_hash,
		}
	}
}

impl<C: Chain> std::fmt::Debug for Client<C> {
	fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
		fmt.debug_struct("Client")
			.field("genesis_hash", &self.genesis_hash)
			.finish()
	}
}

impl<C: Chain> Client<C> {
	/// Returns client that is able to call RPCs on Substrate node over websocket connection.
	pub async fn new(params: ConnectionParams) -> Result<Self> {
		let client = Self::build_client(params.clone()).await?;

		let number: C::BlockNumber = Zero::zero();
		let genesis_hash = Substrate::<C, _, _>::chain_get_block_hash(&client, number).await?;

		Ok(Self {
			params,
			client,
			genesis_hash,
		})
	}

	/// Reopen client connection.
	pub async fn reconnect(&mut self) -> Result<()> {
		self.client = Self::build_client(self.params.clone()).await?;
		Ok(())
	}

	/// Build client to use in connection.
	async fn build_client(params: ConnectionParams) -> Result<RpcClient> {
		let uri = format!("ws://{}:{}", params.host, params.port);
		let transport = WsTransportClient::new(&uri).await?;
		let raw_client = RawClient::new(transport);
		Ok(raw_client.into())
	}
}

impl<C: Chain> Client<C> {
	/// Returns true if client is connected to at least one peer and is in synced state.
	pub async fn ensure_synced(&self) -> Result<()> {
		let health = Substrate::<C, _, _>::system_health(&self.client).await?;
		let is_synced = !health.is_syncing && (!health.should_have_peers || health.peers > 0);
		if is_synced {
			Ok(())
		} else {
			Err(Error::ClientNotSynced(health))
		}
	}

	/// Return hash of the genesis block.
	pub fn genesis_hash(&self) -> &C::Hash {
		&self.genesis_hash
	}

	/// Return hash of the best finalized block.
	pub async fn best_finalized_header_hash(&self) -> Result<C::Hash> {
		Ok(Substrate::<C, _, _>::chain_get_finalized_head(&self.client).await?)
	}

	/// Returns the best Substrate header.
	pub async fn best_header(&self) -> Result<C::Header>
	where
		C::Header: DeserializeOwned,
	{
		Ok(Substrate::<C, _, _>::chain_get_header(&self.client, None).await?)
	}

	/// Get a Substrate block from its hash.
	pub async fn get_block(&self, block_hash: Option<C::Hash>) -> Result<C::SignedBlock> {
		Ok(Substrate::<C, _, _>::chain_get_block(&self.client, block_hash).await?)
	}

	/// Get a Substrate header by its hash.
	pub async fn header_by_hash(&self, block_hash: C::Hash) -> Result<C::Header>
	where
		C::Header: DeserializeOwned,
	{
		Ok(Substrate::<C, _, _>::chain_get_header(&self.client, block_hash).await?)
	}

	/// Get a Substrate block hash by its number.
	pub async fn block_hash_by_number(&self, number: C::BlockNumber) -> Result<C::Hash> {
		Ok(Substrate::<C, _, _>::chain_get_block_hash(&self.client, number).await?)
	}

	/// Get a Substrate header by its number.
	pub async fn header_by_number(&self, block_number: C::BlockNumber) -> Result<C::Header>
	where
		C::Header: DeserializeOwned,
	{
		let block_hash = Self::block_hash_by_number(self, block_number).await?;
		Ok(Self::header_by_hash(self, block_hash).await?)
	}

	/// Return runtime version.
	pub async fn runtime_version(&self) -> Result<RuntimeVersion> {
		Ok(Substrate::<C, _, _>::runtime_version(&self.client).await?)
	}

	/// Return native tokens balance of the account.
	pub async fn free_native_balance(&self, account: C::AccountId) -> Result<C::NativeBalance>
	where
		C: ChainWithBalances,
	{
		let storage_key = C::account_info_storage_key(&account);
		let encoded_account_data = Substrate::<C, _, _>::get_storage(&self.client, storage_key)
			.await?
			.ok_or(Error::AccountDoesNotExist)?;
		let decoded_account_data =
			AccountInfo::<C::Index, AccountData<C::NativeBalance>>::decode(&mut &encoded_account_data.0[..])
				.map_err(Error::ResponseParseFailed)?;
		Ok(decoded_account_data.data.free)
	}

	/// Get the nonce of the given Substrate account.
	///
	/// Note: It's the caller's responsibility to make sure `account` is a valid ss58 address.
	pub async fn next_account_index(&self, account: C::AccountId) -> Result<C::Index> {
		Ok(Substrate::<C, _, _>::system_account_next_index(&self.client, account).await?)
	}

	/// Submit an extrinsic for inclusion in a block.
	///
	/// Note: The given transaction does not need be SCALE encoded beforehand.
	pub async fn submit_extrinsic(&self, transaction: Bytes) -> Result<C::Hash> {
		let tx_hash = Substrate::<C, _, _>::author_submit_extrinsic(&self.client, transaction).await?;
		log::trace!(target: "bridge", "Sent transaction to Substrate node: {:?}", tx_hash);
		Ok(tx_hash)
	}

	/// Get the GRANDPA authority set at given block.
	pub async fn grandpa_authorities_set(&self, block: C::Hash) -> Result<OpaqueGrandpaAuthoritiesSet> {
		let call = SUB_API_GRANDPA_AUTHORITIES.to_string();
		let data = Bytes(Vec::new());

		let encoded_response = Substrate::<C, _, _>::state_call(&self.client, call, data, Some(block)).await?;
		let authority_list = encoded_response.0;

		Ok(authority_list)
	}

	/// Execute runtime call at given block.
	pub async fn state_call(&self, method: String, data: Bytes, at_block: Option<C::Hash>) -> Result<Bytes> {
		Substrate::<C, _, _>::state_call(&self.client, method, data, at_block)
			.await
			.map_err(Into::into)
	}

	/// Returns proof-of-message(s) in given inclusive range.
	pub async fn prove_messages(
		&self,
		instance: InstanceId,
		lane: LaneId,
		range: RangeInclusive<MessageNonce>,
		include_outbound_lane_state: bool,
		at_block: C::Hash,
	) -> Result<StorageProof> {
		let encoded_trie_nodes = SubstrateMessageLane::<C, _, _>::prove_messages(
			&self.client,
			instance,
			lane,
			*range.start(),
			*range.end(),
			include_outbound_lane_state,
			Some(at_block),
		)
		.await
		.map_err(Error::Request)?;
		let decoded_trie_nodes: Vec<Vec<u8>> =
			Decode::decode(&mut &encoded_trie_nodes[..]).map_err(Error::ResponseParseFailed)?;
		Ok(StorageProof::new(decoded_trie_nodes))
	}

	/// Returns proof-of-message(s) delivery.
	pub async fn prove_messages_delivery(
		&self,
		instance: InstanceId,
		lane: LaneId,
		at_block: C::Hash,
	) -> Result<Vec<Vec<u8>>> {
		let encoded_trie_nodes =
			SubstrateMessageLane::<C, _, _>::prove_messages_delivery(&self.client, instance, lane, Some(at_block))
				.await
				.map_err(Error::Request)?;
		let decoded_trie_nodes: Vec<Vec<u8>> =
			Decode::decode(&mut &encoded_trie_nodes[..]).map_err(Error::ResponseParseFailed)?;
		Ok(decoded_trie_nodes)
	}

	/// Return new justifications stream.
	pub async fn subscribe_justifications(self) -> Result<JustificationsSubscription> {
		Ok(self
			.client
			.subscribe(
				"grandpa_subscribeJustifications",
				jsonrpsee::common::Params::None,
				"grandpa_unsubscribeJustifications",
			)
			.await?)
	}
}
