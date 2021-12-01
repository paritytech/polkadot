// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

use crate::{
	rpc::Ethereum,
	types::{
		Address, Bytes, CallRequest, Header, HeaderWithTransactions, Receipt, SignedRawTx,
		SyncState, Transaction, TransactionHash, H256, U256,
	},
	ConnectionParams, Error, Result,
};

use jsonrpsee_ws_client::{WsClient as RpcClient, WsClientBuilder as RpcClientBuilder};
use relay_utils::relay_loop::RECONNECT_DELAY;
use std::{future::Future, sync::Arc};

/// Number of headers missing from the Ethereum node for us to consider node not synced.
const MAJOR_SYNC_BLOCKS: u64 = 5;

/// The client used to interact with an Ethereum node through RPC.
#[derive(Clone)]
pub struct Client {
	tokio: Arc<tokio::runtime::Runtime>,
	params: ConnectionParams,
	client: Arc<RpcClient>,
}

impl Client {
	/// Create a new Ethereum RPC Client.
	///
	/// This function will keep connecting to given Ethereum node until connection is established
	/// and is functional. If attempt fail, it will wait for `RECONNECT_DELAY` and retry again.
	pub async fn new(params: ConnectionParams) -> Self {
		loop {
			match Self::try_connect(params.clone()).await {
				Ok(client) => return client,
				Err(error) => log::error!(
					target: "bridge",
					"Failed to connect to Ethereum node: {:?}. Going to retry in {}s",
					error,
					RECONNECT_DELAY.as_secs(),
				),
			}

			async_std::task::sleep(RECONNECT_DELAY).await;
		}
	}

	/// Try to connect to Ethereum node. Returns Ethereum RPC client if connection has been
	/// established or error otherwise.
	pub async fn try_connect(params: ConnectionParams) -> Result<Self> {
		let (tokio, client) = Self::build_client(&params).await?;
		Ok(Self { tokio, client, params })
	}

	/// Build client to use in connection.
	async fn build_client(
		params: &ConnectionParams,
	) -> Result<(Arc<tokio::runtime::Runtime>, Arc<RpcClient>)> {
		let tokio = tokio::runtime::Runtime::new()?;
		let uri = format!("ws://{}:{}", params.host, params.port);
		let client = tokio
			.spawn(async move { RpcClientBuilder::default().build(&uri).await })
			.await??;
		Ok((Arc::new(tokio), Arc::new(client)))
	}

	/// Reopen client connection.
	pub async fn reconnect(&mut self) -> Result<()> {
		let (tokio, client) = Self::build_client(&self.params).await?;
		self.tokio = tokio;
		self.client = client;
		Ok(())
	}
}

impl Client {
	/// Returns true if client is connected to at least one peer and is in synced state.
	pub async fn ensure_synced(&self) -> Result<()> {
		self.jsonrpsee_execute(move |client| async move {
			match Ethereum::syncing(&*client).await? {
				SyncState::NotSyncing => Ok(()),
				SyncState::Syncing(syncing) => {
					let missing_headers =
						syncing.highest_block.saturating_sub(syncing.current_block);
					if missing_headers > MAJOR_SYNC_BLOCKS.into() {
						return Err(Error::ClientNotSynced(missing_headers))
					}

					Ok(())
				},
			}
		})
		.await
	}

	/// Estimate gas usage for the given call.
	pub async fn estimate_gas(&self, call_request: CallRequest) -> Result<U256> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(Ethereum::estimate_gas(&*client, call_request).await?)
		})
		.await
	}

	/// Retrieve number of the best known block from the Ethereum node.
	pub async fn best_block_number(&self) -> Result<u64> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(Ethereum::block_number(&*client).await?.as_u64())
		})
		.await
	}

	/// Retrieve number of the best known block from the Ethereum node.
	pub async fn header_by_number(&self, block_number: u64) -> Result<Header> {
		self.jsonrpsee_execute(move |client| async move {
			let get_full_tx_objects = false;
			let header =
				Ethereum::get_block_by_number(&*client, block_number, get_full_tx_objects).await?;
			match header.number.is_some() && header.hash.is_some() && header.logs_bloom.is_some() {
				true => Ok(header),
				false => Err(Error::IncompleteHeader),
			}
		})
		.await
	}

	/// Retrieve block header by its hash from Ethereum node.
	pub async fn header_by_hash(&self, hash: H256) -> Result<Header> {
		self.jsonrpsee_execute(move |client| async move {
			let get_full_tx_objects = false;
			let header = Ethereum::get_block_by_hash(&*client, hash, get_full_tx_objects).await?;
			match header.number.is_some() && header.hash.is_some() && header.logs_bloom.is_some() {
				true => Ok(header),
				false => Err(Error::IncompleteHeader),
			}
		})
		.await
	}

	/// Retrieve block header and its transactions by its number from Ethereum node.
	pub async fn header_by_number_with_transactions(
		&self,
		number: u64,
	) -> Result<HeaderWithTransactions> {
		self.jsonrpsee_execute(move |client| async move {
			let get_full_tx_objects = true;
			let header = Ethereum::get_block_by_number_with_transactions(
				&*client,
				number,
				get_full_tx_objects,
			)
			.await?;

			let is_complete_header =
				header.number.is_some() && header.hash.is_some() && header.logs_bloom.is_some();
			if !is_complete_header {
				return Err(Error::IncompleteHeader)
			}

			let is_complete_transactions = header.transactions.iter().all(|tx| tx.raw.is_some());
			if !is_complete_transactions {
				return Err(Error::IncompleteTransaction)
			}

			Ok(header)
		})
		.await
	}

	/// Retrieve block header and its transactions by its hash from Ethereum node.
	pub async fn header_by_hash_with_transactions(
		&self,
		hash: H256,
	) -> Result<HeaderWithTransactions> {
		self.jsonrpsee_execute(move |client| async move {
			let get_full_tx_objects = true;
			let header =
				Ethereum::get_block_by_hash_with_transactions(&*client, hash, get_full_tx_objects)
					.await?;

			let is_complete_header =
				header.number.is_some() && header.hash.is_some() && header.logs_bloom.is_some();
			if !is_complete_header {
				return Err(Error::IncompleteHeader)
			}

			let is_complete_transactions = header.transactions.iter().all(|tx| tx.raw.is_some());
			if !is_complete_transactions {
				return Err(Error::IncompleteTransaction)
			}

			Ok(header)
		})
		.await
	}

	/// Retrieve transaction by its hash from Ethereum node.
	pub async fn transaction_by_hash(&self, hash: H256) -> Result<Option<Transaction>> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(Ethereum::transaction_by_hash(&*client, hash).await?)
		})
		.await
	}

	/// Retrieve transaction receipt by transaction hash.
	pub async fn transaction_receipt(&self, transaction_hash: H256) -> Result<Receipt> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(Ethereum::get_transaction_receipt(&*client, transaction_hash).await?)
		})
		.await
	}

	/// Get the nonce of the given account.
	pub async fn account_nonce(&self, address: Address) -> Result<U256> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(Ethereum::get_transaction_count(&*client, address).await?)
		})
		.await
	}

	/// Submit an Ethereum transaction.
	///
	/// The transaction must already be signed before sending it through this method.
	pub async fn submit_transaction(&self, signed_raw_tx: SignedRawTx) -> Result<TransactionHash> {
		self.jsonrpsee_execute(move |client| async move {
			let transaction = Bytes(signed_raw_tx);
			let tx_hash = Ethereum::submit_transaction(&*client, transaction).await?;
			log::trace!(target: "bridge", "Sent transaction to Ethereum node: {:?}", tx_hash);
			Ok(tx_hash)
		})
		.await
	}

	/// Call Ethereum smart contract.
	pub async fn eth_call(&self, call_transaction: CallRequest) -> Result<Bytes> {
		self.jsonrpsee_execute(move |client| async move {
			Ok(Ethereum::call(&*client, call_transaction).await?)
		})
		.await
	}

	/// Execute jsonrpsee future in tokio context.
	async fn jsonrpsee_execute<MF, F, T>(&self, make_jsonrpsee_future: MF) -> Result<T>
	where
		MF: FnOnce(Arc<RpcClient>) -> F + Send + 'static,
		F: Future<Output = Result<T>> + Send,
		T: Send + 'static,
	{
		let client = self.client.clone();
		self.tokio.spawn(async move { make_jsonrpsee_future(client).await }).await?
	}
}
