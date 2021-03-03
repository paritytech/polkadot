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

use crate::ethereum_sync_loop::QueuedEthereumHeader;
use crate::instances::BridgeInstance;
use crate::rpc_errors::RpcError;

use async_trait::async_trait;
use bp_eth_poa::AuraHeader as SubstrateEthereumHeader;
use codec::{Decode, Encode};
use headers_relay::sync_types::SubmittedHeaders;
use relay_ethereum_client::types::HeaderId as EthereumHeaderId;
use relay_rialto_client::{Rialto, SigningParams as RialtoSigningParams};
use relay_substrate_client::{Client as SubstrateClient, TransactionSignScheme};
use relay_utils::HeaderId;
use sp_core::{crypto::Pair, Bytes};
use std::{collections::VecDeque, sync::Arc};

const ETH_API_IMPORT_REQUIRES_RECEIPTS: &str = "RialtoPoAHeaderApi_is_import_requires_receipts";
const ETH_API_IS_KNOWN_BLOCK: &str = "RialtoPoAHeaderApi_is_known_block";
const ETH_API_BEST_BLOCK: &str = "RialtoPoAHeaderApi_best_block";
const ETH_API_BEST_FINALIZED_BLOCK: &str = "RialtoPoAHeaderApi_finalized_block";
const EXCH_API_FILTER_TRANSACTION_PROOF: &str = "RialtoCurrencyExchangeApi_filter_transaction_proof";

type RpcResult<T> = std::result::Result<T, RpcError>;

/// A trait which contains methods that work by using multiple low-level RPCs, or more complicated
/// interactions involving, for example, an Ethereum bridge module.
#[async_trait]
pub trait SubstrateHighLevelRpc {
	/// Returns best Ethereum block that Substrate runtime knows of.
	async fn best_ethereum_block(&self) -> RpcResult<EthereumHeaderId>;
	/// Returns best finalized Ethereum block that Substrate runtime knows of.
	async fn best_ethereum_finalized_block(&self) -> RpcResult<EthereumHeaderId>;
	/// Returns whether or not transactions receipts are required for Ethereum header submission.
	async fn ethereum_receipts_required(&self, header: SubstrateEthereumHeader) -> RpcResult<bool>;
	/// Returns whether or not the given Ethereum header is known to the Substrate runtime.
	async fn ethereum_header_known(&self, header_id: EthereumHeaderId) -> RpcResult<bool>;
}

#[async_trait]
impl SubstrateHighLevelRpc for SubstrateClient<Rialto> {
	async fn best_ethereum_block(&self) -> RpcResult<EthereumHeaderId> {
		let call = ETH_API_BEST_BLOCK.to_string();
		let data = Bytes(Vec::new());

		let encoded_response = self.state_call(call, data, None).await?;
		let decoded_response: (u64, bp_eth_poa::H256) = Decode::decode(&mut &encoded_response.0[..])?;

		let best_header_id = HeaderId(decoded_response.0, decoded_response.1);
		Ok(best_header_id)
	}

	async fn best_ethereum_finalized_block(&self) -> RpcResult<EthereumHeaderId> {
		let call = ETH_API_BEST_FINALIZED_BLOCK.to_string();
		let data = Bytes(Vec::new());

		let encoded_response = self.state_call(call, data, None).await?;
		let decoded_response: (u64, bp_eth_poa::H256) = Decode::decode(&mut &encoded_response.0[..])?;

		let best_header_id = HeaderId(decoded_response.0, decoded_response.1);
		Ok(best_header_id)
	}

	async fn ethereum_receipts_required(&self, header: SubstrateEthereumHeader) -> RpcResult<bool> {
		let call = ETH_API_IMPORT_REQUIRES_RECEIPTS.to_string();
		let data = Bytes(header.encode());

		let encoded_response = self.state_call(call, data, None).await?;
		let receipts_required: bool = Decode::decode(&mut &encoded_response.0[..])?;

		Ok(receipts_required)
	}

	// The Substrate module could prune old headers. So this function could return false even
	// if header is synced. And we'll mark corresponding Ethereum header as Orphan.
	//
	// But when we read the best header from Substrate next time, we will know that
	// there's a better header. This Orphan will either be marked as synced, or
	// eventually pruned.
	async fn ethereum_header_known(&self, header_id: EthereumHeaderId) -> RpcResult<bool> {
		let call = ETH_API_IS_KNOWN_BLOCK.to_string();
		let data = Bytes(header_id.1.encode());

		let encoded_response = self.state_call(call, data, None).await?;
		let is_known_block: bool = Decode::decode(&mut &encoded_response.0[..])?;

		Ok(is_known_block)
	}
}

/// A trait for RPC calls which are used to submit Ethereum headers to a Substrate
/// runtime. These are typically calls which use a combination of other low-level RPC
/// calls.
#[async_trait]
pub trait SubmitEthereumHeaders {
	/// Submits Ethereum header to Substrate runtime.
	async fn submit_ethereum_headers(
		&self,
		params: RialtoSigningParams,
		instance: Arc<dyn BridgeInstance>,
		headers: Vec<QueuedEthereumHeader>,
		sign_transactions: bool,
	) -> SubmittedHeaders<EthereumHeaderId, RpcError>;

	/// Submits signed Ethereum header to Substrate runtime.
	async fn submit_signed_ethereum_headers(
		&self,
		params: RialtoSigningParams,
		instance: Arc<dyn BridgeInstance>,
		headers: Vec<QueuedEthereumHeader>,
	) -> SubmittedHeaders<EthereumHeaderId, RpcError>;

	/// Submits unsigned Ethereum header to Substrate runtime.
	async fn submit_unsigned_ethereum_headers(
		&self,
		instance: Arc<dyn BridgeInstance>,
		headers: Vec<QueuedEthereumHeader>,
	) -> SubmittedHeaders<EthereumHeaderId, RpcError>;
}

#[async_trait]
impl SubmitEthereumHeaders for SubstrateClient<Rialto> {
	async fn submit_ethereum_headers(
		&self,
		params: RialtoSigningParams,
		instance: Arc<dyn BridgeInstance>,
		headers: Vec<QueuedEthereumHeader>,
		sign_transactions: bool,
	) -> SubmittedHeaders<EthereumHeaderId, RpcError> {
		if sign_transactions {
			self.submit_signed_ethereum_headers(params, instance, headers).await
		} else {
			self.submit_unsigned_ethereum_headers(instance, headers).await
		}
	}

	async fn submit_signed_ethereum_headers(
		&self,
		params: RialtoSigningParams,
		instance: Arc<dyn BridgeInstance>,
		headers: Vec<QueuedEthereumHeader>,
	) -> SubmittedHeaders<EthereumHeaderId, RpcError> {
		let ids = headers.iter().map(|header| header.id()).collect();
		let submission_result = async {
			let account_id = params.signer.public().as_array_ref().clone().into();
			let nonce = self.next_account_index(account_id).await?;

			let call = instance.build_signed_header_call(headers);
			let transaction = Rialto::sign_transaction(self, &params.signer, nonce, call);

			let _ = self.submit_extrinsic(Bytes(transaction.encode())).await?;
			Ok(())
		}
		.await;

		match submission_result {
			Ok(_) => SubmittedHeaders {
				submitted: ids,
				incomplete: Vec::new(),
				rejected: Vec::new(),
				fatal_error: None,
			},
			Err(error) => SubmittedHeaders {
				submitted: Vec::new(),
				incomplete: Vec::new(),
				rejected: ids,
				fatal_error: Some(error),
			},
		}
	}

	async fn submit_unsigned_ethereum_headers(
		&self,
		instance: Arc<dyn BridgeInstance>,
		headers: Vec<QueuedEthereumHeader>,
	) -> SubmittedHeaders<EthereumHeaderId, RpcError> {
		let mut ids = headers.iter().map(|header| header.id()).collect::<VecDeque<_>>();
		let mut submitted_headers = SubmittedHeaders::default();

		for header in headers {
			let id = ids.pop_front().expect("both collections have same size; qed");

			let call = instance.build_unsigned_header_call(header);
			let transaction = create_unsigned_submit_transaction(call);

			match self.submit_extrinsic(Bytes(transaction.encode())).await {
				Ok(_) => submitted_headers.submitted.push(id),
				Err(error) => {
					submitted_headers.rejected.push(id);
					submitted_headers.rejected.extend(ids);
					submitted_headers.fatal_error = Some(error.into());
					break;
				}
			}
		}

		submitted_headers
	}
}

/// A trait for RPC calls which are used to submit proof of Ethereum exchange transaction to a
/// Substrate runtime. These are typically calls which use a combination of other low-level RPC
/// calls.
#[async_trait]
pub trait SubmitEthereumExchangeTransactionProof {
	/// Pre-verify Ethereum exchange transaction proof.
	async fn verify_exchange_transaction_proof(
		&self,
		proof: rialto_runtime::exchange::EthereumTransactionInclusionProof,
	) -> RpcResult<bool>;
	/// Submits Ethereum exchange transaction proof to Substrate runtime.
	async fn submit_exchange_transaction_proof(
		&self,
		params: RialtoSigningParams,
		instance: Arc<dyn BridgeInstance>,
		proof: rialto_runtime::exchange::EthereumTransactionInclusionProof,
	) -> RpcResult<()>;
}

#[async_trait]
impl SubmitEthereumExchangeTransactionProof for SubstrateClient<Rialto> {
	async fn verify_exchange_transaction_proof(
		&self,
		proof: rialto_runtime::exchange::EthereumTransactionInclusionProof,
	) -> RpcResult<bool> {
		let call = EXCH_API_FILTER_TRANSACTION_PROOF.to_string();
		let data = Bytes(proof.encode());

		let encoded_response = self.state_call(call, data, None).await?;
		let is_allowed: bool = Decode::decode(&mut &encoded_response.0[..])?;

		Ok(is_allowed)
	}

	async fn submit_exchange_transaction_proof(
		&self,
		params: RialtoSigningParams,
		instance: Arc<dyn BridgeInstance>,
		proof: rialto_runtime::exchange::EthereumTransactionInclusionProof,
	) -> RpcResult<()> {
		let account_id = params.signer.public().as_array_ref().clone().into();
		let nonce = self.next_account_index(account_id).await?;

		let call = instance.build_currency_exchange_call(proof);
		let transaction = Rialto::sign_transaction(self, &params.signer, nonce, call);

		let _ = self.submit_extrinsic(Bytes(transaction.encode())).await?;
		Ok(())
	}
}

/// Create unsigned Substrate transaction for submitting Ethereum header.
fn create_unsigned_submit_transaction(call: rialto_runtime::Call) -> rialto_runtime::UncheckedExtrinsic {
	rialto_runtime::UncheckedExtrinsic::new_unsigned(call)
}
