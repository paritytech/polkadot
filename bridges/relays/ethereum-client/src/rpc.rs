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

//! Ethereum node RPC interface.

// The compiler doesn't think we're using the
// code from rpc_api!
#![allow(dead_code)]
#![allow(unused_variables)]

use crate::types::{
	Address, Bytes, CallRequest, Header, HeaderWithTransactions, Receipt, SyncState, Transaction, TransactionHash,
	H256, U256, U64,
};

jsonrpsee::rpc_api! {
	pub(crate) Ethereum {
		#[rpc(method = "eth_syncing", positional_params)]
		fn syncing() -> SyncState;
		#[rpc(method = "eth_estimateGas", positional_params)]
		fn estimate_gas(call_request: CallRequest) -> U256;
		#[rpc(method = "eth_blockNumber", positional_params)]
		fn block_number() -> U64;
		#[rpc(method = "eth_getBlockByNumber", positional_params)]
		fn get_block_by_number(block_number: U64, full_tx_objs: bool) -> Header;
		#[rpc(method = "eth_getBlockByHash", positional_params)]
		fn get_block_by_hash(hash: H256, full_tx_objs: bool) -> Header;
		#[rpc(method = "eth_getBlockByNumber", positional_params)]
		fn get_block_by_number_with_transactions(number: U64, full_tx_objs: bool) -> HeaderWithTransactions;
		#[rpc(method = "eth_getBlockByHash", positional_params)]
		fn get_block_by_hash_with_transactions(hash: H256, full_tx_objs: bool) -> HeaderWithTransactions;
		#[rpc(method = "eth_getTransactionByHash", positional_params)]
		fn transaction_by_hash(hash: H256) -> Option<Transaction>;
		#[rpc(method = "eth_getTransactionReceipt", positional_params)]
		fn get_transaction_receipt(transaction_hash: H256) -> Receipt;
		#[rpc(method = "eth_getTransactionCount", positional_params)]
		fn get_transaction_count(address: Address) -> U256;
		#[rpc(method = "eth_submitTransaction", positional_params)]
		fn submit_transaction(transaction: Bytes) -> TransactionHash;
		#[rpc(method = "eth_call", positional_params)]
		fn call(transaction_call: CallRequest) -> Bytes;
	}
}
