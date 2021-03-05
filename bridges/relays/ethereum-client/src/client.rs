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

use crate::rpc::Ethereum;
use crate::types::{
	Address, Bytes, CallRequest, Header, HeaderWithTransactions, Receipt, SignedRawTx, SyncState, Transaction,
	TransactionHash, H256, U256,
};
use crate::{ConnectionParams, Error, Result};

use jsonrpsee::raw::RawClient;
use jsonrpsee::transport::http::HttpTransportClient;
use jsonrpsee::Client as RpcClient;

/// Number of headers missing from the Ethereum node for us to consider node not synced.
const MAJOR_SYNC_BLOCKS: u64 = 5;

/// The client used to interact with an Ethereum node through RPC.
#[derive(Clone)]
pub struct Client {
	params: ConnectionParams,
	client: RpcClient,
}

impl Client {
	/// Create a new Ethereum RPC Client.
	pub fn new(params: ConnectionParams) -> Self {
		Self {
			client: Self::build_client(&params),
			params,
		}
	}

	/// Build client to use in connection.
	fn build_client(params: &ConnectionParams) -> RpcClient {
		let uri = format!("http://{}:{}", params.host, params.port);
		let transport = HttpTransportClient::new(&uri);
		let raw_client = RawClient::new(transport);
		raw_client.into()
	}

	/// Reopen client connection.
	pub fn reconnect(&mut self) {
		self.client = Self::build_client(&self.params);
	}
}

impl Client {
	/// Returns true if client is connected to at least one peer and is in synced state.
	pub async fn ensure_synced(&self) -> Result<()> {
		match Ethereum::syncing(&self.client).await? {
			SyncState::NotSyncing => Ok(()),
			SyncState::Syncing(syncing) => {
				let missing_headers = syncing.highest_block.saturating_sub(syncing.current_block);
				if missing_headers > MAJOR_SYNC_BLOCKS.into() {
					return Err(Error::ClientNotSynced(missing_headers));
				}

				Ok(())
			}
		}
	}

	/// Estimate gas usage for the given call.
	pub async fn estimate_gas(&self, call_request: CallRequest) -> Result<U256> {
		Ok(Ethereum::estimate_gas(&self.client, call_request).await?)
	}

	/// Retrieve number of the best known block from the Ethereum node.
	pub async fn best_block_number(&self) -> Result<u64> {
		Ok(Ethereum::block_number(&self.client).await?.as_u64())
	}

	/// Retrieve number of the best known block from the Ethereum node.
	pub async fn header_by_number(&self, block_number: u64) -> Result<Header> {
		let get_full_tx_objects = false;
		let header = Ethereum::get_block_by_number(&self.client, block_number, get_full_tx_objects).await?;
		match header.number.is_some() && header.hash.is_some() && header.logs_bloom.is_some() {
			true => Ok(header),
			false => Err(Error::IncompleteHeader),
		}
	}

	/// Retrieve block header by its hash from Ethereum node.
	pub async fn header_by_hash(&self, hash: H256) -> Result<Header> {
		let get_full_tx_objects = false;
		let header = Ethereum::get_block_by_hash(&self.client, hash, get_full_tx_objects).await?;
		match header.number.is_some() && header.hash.is_some() && header.logs_bloom.is_some() {
			true => Ok(header),
			false => Err(Error::IncompleteHeader),
		}
	}

	/// Retrieve block header and its transactions by its number from Ethereum node.
	pub async fn header_by_number_with_transactions(&self, number: u64) -> Result<HeaderWithTransactions> {
		let get_full_tx_objects = true;
		let header = Ethereum::get_block_by_number_with_transactions(&self.client, number, get_full_tx_objects).await?;

		let is_complete_header = header.number.is_some() && header.hash.is_some() && header.logs_bloom.is_some();
		if !is_complete_header {
			return Err(Error::IncompleteHeader);
		}

		let is_complete_transactions = header.transactions.iter().all(|tx| tx.raw.is_some());
		if !is_complete_transactions {
			return Err(Error::IncompleteTransaction);
		}

		Ok(header)
	}

	/// Retrieve block header and its transactions by its hash from Ethereum node.
	pub async fn header_by_hash_with_transactions(&self, hash: H256) -> Result<HeaderWithTransactions> {
		let get_full_tx_objects = true;
		let header = Ethereum::get_block_by_hash_with_transactions(&self.client, hash, get_full_tx_objects).await?;

		let is_complete_header = header.number.is_some() && header.hash.is_some() && header.logs_bloom.is_some();
		if !is_complete_header {
			return Err(Error::IncompleteHeader);
		}

		let is_complete_transactions = header.transactions.iter().all(|tx| tx.raw.is_some());
		if !is_complete_transactions {
			return Err(Error::IncompleteTransaction);
		}

		Ok(header)
	}

	/// Retrieve transaction by its hash from Ethereum node.
	pub async fn transaction_by_hash(&self, hash: H256) -> Result<Option<Transaction>> {
		Ok(Ethereum::transaction_by_hash(&self.client, hash).await?)
	}

	/// Retrieve transaction receipt by transaction hash.
	pub async fn transaction_receipt(&self, transaction_hash: H256) -> Result<Receipt> {
		Ok(Ethereum::get_transaction_receipt(&self.client, transaction_hash).await?)
	}

	/// Get the nonce of the given account.
	pub async fn account_nonce(&self, address: Address) -> Result<U256> {
		Ok(Ethereum::get_transaction_count(&self.client, address).await?)
	}

	/// Submit an Ethereum transaction.
	///
	/// The transaction must already be signed before sending it through this method.
	pub async fn submit_transaction(&self, signed_raw_tx: SignedRawTx) -> Result<TransactionHash> {
		let transaction = Bytes(signed_raw_tx);
		let tx_hash = Ethereum::submit_transaction(&self.client, transaction).await?;
		log::trace!(target: "bridge", "Sent transaction to Ethereum node: {:?}", tx_hash);
		Ok(tx_hash)
	}

	/// Call Ethereum smart contract.
	pub async fn eth_call(&self, call_transaction: CallRequest) -> Result<Bytes> {
		Ok(Ethereum::call(&self.client, call_transaction).await?)
	}
}
