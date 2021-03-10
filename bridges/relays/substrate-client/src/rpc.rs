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

//! The most generic Substrate node RPC interface.

// The compiler doesn't think we're using the
// code from rpc_api!
#![allow(dead_code)]
#![allow(unused_variables)]

use crate::chain::Chain;

use bp_message_lane::{LaneId, MessageNonce};
use bp_runtime::InstanceId;
use sc_rpc_api::system::Health;
use sp_core::{
	storage::{StorageData, StorageKey},
	Bytes,
};
use sp_version::RuntimeVersion;

jsonrpsee::rpc_api! {
	pub(crate) Substrate<C: Chain> {
		#[rpc(method = "system_health", positional_params)]
		fn system_health() -> Health;
		#[rpc(method = "chain_getHeader", positional_params)]
		fn chain_get_header(block_hash: Option<C::Hash>) -> C::Header;
		#[rpc(method = "chain_getFinalizedHead", positional_params)]
		fn chain_get_finalized_head() -> C::Hash;
		#[rpc(method = "chain_getBlock", positional_params)]
		fn chain_get_block(block_hash: Option<C::Hash>) -> C::SignedBlock;
		#[rpc(method = "chain_getBlockHash", positional_params)]
		fn chain_get_block_hash(block_number: Option<C::BlockNumber>) -> C::Hash;
		#[rpc(method = "system_accountNextIndex", positional_params)]
		fn system_account_next_index(account_id: C::AccountId) -> C::Index;
		#[rpc(method = "author_submitExtrinsic", positional_params)]
		fn author_submit_extrinsic(extrinsic: Bytes) -> C::Hash;
		#[rpc(method = "state_call", positional_params)]
		fn state_call(method: String, data: Bytes, at_block: Option<C::Hash>) -> Bytes;
		#[rpc(method = "state_getStorage", positional_params)]
		fn get_storage(key: StorageKey) -> Option<StorageData>;
		#[rpc(method = "state_getRuntimeVersion", positional_params)]
		fn runtime_version() -> RuntimeVersion;
	}

	pub(crate) SubstrateMessageLane<C: Chain> {
		#[rpc(method = "messageLane_proveMessages", positional_params)]
		fn prove_messages(
			instance: InstanceId,
			lane: LaneId,
			begin: MessageNonce,
			end: MessageNonce,
			include_outbound_lane_state: bool,
			block: Option<C::Hash>,
		) -> Bytes;

		#[rpc(method = "messageLane_proveMessagesDelivery", positional_params)]
		fn prove_messages_delivery(
			instance: InstanceId,
			lane: LaneId,
			block: Option<C::Hash>,
		) -> Bytes;
	}
}
