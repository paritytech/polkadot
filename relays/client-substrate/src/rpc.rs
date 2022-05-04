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

//! The most generic Substrate node RPC interface.

use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use pallet_transaction_payment_rpc_runtime_api::FeeDetails;
use sc_rpc_api::{state::ReadProof, system::Health};
use sp_core::{
	storage::{StorageData, StorageKey},
	Bytes,
};
use sp_rpc::number::NumberOrHex;
use sp_version::RuntimeVersion;

#[rpc(client)]
pub(crate) trait Substrate<AccountId, BlockNumber, Hash, Header, Index, SignedBlock> {
	#[method(name = "system_health", param_kind = array)]
	async fn system_health(&self) -> RpcResult<Health>;
	#[method(name = "system_properties", param_kind = array)]
	async fn system_properties(&self) -> RpcResult<sc_chain_spec::Properties>;
	#[method(name = "chain_getHeader", param_kind = array)]
	async fn chain_get_header(&self, block_hash: Option<Hash>) -> RpcResult<Header>;
	#[method(name = "chain_getFinalizedHead", param_kind = array)]
	async fn chain_get_finalized_head(&self) -> RpcResult<Hash>;
	#[method(name = "chain_getBlock", param_kind = array)]
	async fn chain_get_block(&self, block_hash: Option<Hash>) -> RpcResult<SignedBlock>;
	#[method(name = "chain_getBlockHash", param_kind = array)]
	async fn chain_get_block_hash(&self, block_number: Option<BlockNumber>) -> RpcResult<Hash>;
	#[method(name = "system_accountNextIndex", param_kind = array)]
	async fn system_account_next_index(&self, account_id: AccountId) -> RpcResult<Index>;
	#[method(name = "author_submitExtrinsic", param_kind = array)]
	async fn author_submit_extrinsic(&self, extrinsic: Bytes) -> RpcResult<Hash>;
	#[method(name = "author_pendingExtrinsics", param_kind = array)]
	async fn author_pending_extrinsics(&self) -> RpcResult<Vec<Bytes>>;
	#[method(name = "state_call", param_kind = array)]
	async fn state_call(
		&self,
		method: String,
		data: Bytes,
		at_block: Option<Hash>,
	) -> RpcResult<Bytes>;
	#[method(name = "state_getStorage", param_kind = array)]
	async fn state_get_storage(
		&self,
		key: StorageKey,
		at_block: Option<Hash>,
	) -> RpcResult<Option<StorageData>>;
	#[method(name = "state_getReadProof", param_kind = array)]
	async fn state_prove_storage(
		&self,
		keys: Vec<StorageKey>,
		hash: Option<Hash>,
	) -> RpcResult<ReadProof<Hash>>;
	#[method(name = "state_getRuntimeVersion", param_kind = array)]
	async fn state_runtime_version(&self) -> RpcResult<RuntimeVersion>;
	#[method(name = "payment_queryFeeDetails", param_kind = array)]
	async fn payment_query_fee_details(
		&self,
		extrinsic: Bytes,
		at_block: Option<Hash>,
	) -> RpcResult<FeeDetails<NumberOrHex>>;
}
