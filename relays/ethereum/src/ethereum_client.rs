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

use crate::rpc_errors::RpcError;
use crate::substrate_sync_loop::QueuedRialtoHeader;

use async_trait::async_trait;
use bp_eth_poa::signatures::secret_to_address;
use codec::{Decode, Encode};
use ethabi::FunctionOutputDecoder;
use headers_relay::sync_types::SubmittedHeaders;
use relay_ethereum_client::{
	sign_and_submit_transaction,
	types::{Address, CallRequest, HeaderId as EthereumHeaderId, Receipt, H256, U256},
	Client as EthereumClient, Error as EthereumNodeError, SigningParams as EthereumSigningParams,
};
use relay_rialto_client::HeaderId as RialtoHeaderId;
use relay_utils::{HeaderId, MaybeConnectionError};
use sp_runtime::Justification;
use std::collections::HashSet;

// to encode/decode contract calls
ethabi_contract::use_contract!(bridge_contract, "res/substrate-bridge-abi.json");

type RpcResult<T> = std::result::Result<T, RpcError>;

/// A trait which contains methods that work by using multiple low-level RPCs, or more complicated
/// interactions involving, for example, an Ethereum contract.
#[async_trait]
pub trait EthereumHighLevelRpc {
	/// Returns best Substrate block that PoA chain knows of.
	async fn best_substrate_block(&self, contract_address: Address) -> RpcResult<RialtoHeaderId>;

	/// Returns true if Substrate header is known to Ethereum node.
	async fn substrate_header_known(
		&self,
		contract_address: Address,
		id: RialtoHeaderId,
	) -> RpcResult<(RialtoHeaderId, bool)>;

	/// Submits Substrate headers to Ethereum contract.
	async fn submit_substrate_headers(
		&self,
		params: EthereumSigningParams,
		contract_address: Address,
		headers: Vec<QueuedRialtoHeader>,
	) -> SubmittedHeaders<RialtoHeaderId, RpcError>;

	/// Returns ids of incomplete Substrate headers.
	async fn incomplete_substrate_headers(&self, contract_address: Address) -> RpcResult<HashSet<RialtoHeaderId>>;

	/// Complete Substrate header.
	async fn complete_substrate_header(
		&self,
		params: EthereumSigningParams,
		contract_address: Address,
		id: RialtoHeaderId,
		justification: Justification,
	) -> RpcResult<RialtoHeaderId>;

	/// Submit ethereum transaction.
	async fn submit_ethereum_transaction(
		&self,
		params: &EthereumSigningParams,
		contract_address: Option<Address>,
		nonce: Option<U256>,
		double_gas: bool,
		encoded_call: Vec<u8>,
	) -> RpcResult<()>;

	/// Retrieve transactions receipts for given block.
	async fn transaction_receipts(
		&self,
		id: EthereumHeaderId,
		transactions: Vec<H256>,
	) -> RpcResult<(EthereumHeaderId, Vec<Receipt>)>;
}

#[async_trait]
impl EthereumHighLevelRpc for EthereumClient {
	async fn best_substrate_block(&self, contract_address: Address) -> RpcResult<RialtoHeaderId> {
		let (encoded_call, call_decoder) = bridge_contract::functions::best_known_header::call();
		let call_request = CallRequest {
			to: Some(contract_address),
			data: Some(encoded_call.into()),
			..Default::default()
		};

		let call_result = self.eth_call(call_request).await?;
		let (number, raw_hash) = call_decoder.decode(&call_result.0)?;
		let hash = rialto_runtime::Hash::decode(&mut &raw_hash[..])?;

		if number != number.low_u32().into() {
			return Err(RpcError::Ethereum(EthereumNodeError::InvalidSubstrateBlockNumber));
		}

		Ok(HeaderId(number.low_u32(), hash))
	}

	async fn substrate_header_known(
		&self,
		contract_address: Address,
		id: RialtoHeaderId,
	) -> RpcResult<(RialtoHeaderId, bool)> {
		let (encoded_call, call_decoder) = bridge_contract::functions::is_known_header::call(id.1);
		let call_request = CallRequest {
			to: Some(contract_address),
			data: Some(encoded_call.into()),
			..Default::default()
		};

		let call_result = self.eth_call(call_request).await?;
		let is_known_block = call_decoder.decode(&call_result.0)?;

		Ok((id, is_known_block))
	}

	async fn submit_substrate_headers(
		&self,
		params: EthereumSigningParams,
		contract_address: Address,
		headers: Vec<QueuedRialtoHeader>,
	) -> SubmittedHeaders<RialtoHeaderId, RpcError> {
		// read nonce of signer
		let address: Address = secret_to_address(&params.signer);
		let nonce = match self.account_nonce(address).await {
			Ok(nonce) => nonce,
			Err(error) => {
				return SubmittedHeaders {
					submitted: Vec::new(),
					incomplete: Vec::new(),
					rejected: headers.iter().rev().map(|header| header.id()).collect(),
					fatal_error: Some(error.into()),
				}
			}
		};

		// submit headers. Note that we're cloning self here. It is ok, because
		// cloning `jsonrpsee::Client` only clones reference to background threads
		submit_substrate_headers(
			EthereumHeadersSubmitter {
				client: self.clone(),
				params,
				contract_address,
				nonce,
			},
			headers,
		)
		.await
	}

	async fn incomplete_substrate_headers(&self, contract_address: Address) -> RpcResult<HashSet<RialtoHeaderId>> {
		let (encoded_call, call_decoder) = bridge_contract::functions::incomplete_headers::call();
		let call_request = CallRequest {
			to: Some(contract_address),
			data: Some(encoded_call.into()),
			..Default::default()
		};

		let call_result = self.eth_call(call_request).await?;

		// Q: Is is correct to call these "incomplete_ids"?
		let (incomplete_headers_numbers, incomplete_headers_hashes) = call_decoder.decode(&call_result.0)?;
		let incomplete_ids = incomplete_headers_numbers
			.into_iter()
			.zip(incomplete_headers_hashes)
			.filter_map(|(number, hash)| {
				if number != number.low_u32().into() {
					return None;
				}

				Some(HeaderId(number.low_u32(), hash))
			})
			.collect();

		Ok(incomplete_ids)
	}

	async fn complete_substrate_header(
		&self,
		params: EthereumSigningParams,
		contract_address: Address,
		id: RialtoHeaderId,
		justification: Justification,
	) -> RpcResult<RialtoHeaderId> {
		let _ = self
			.submit_ethereum_transaction(
				&params,
				Some(contract_address),
				None,
				false,
				bridge_contract::functions::import_finality_proof::encode_input(id.0, id.1, justification),
			)
			.await?;

		Ok(id)
	}

	async fn submit_ethereum_transaction(
		&self,
		params: &EthereumSigningParams,
		contract_address: Option<Address>,
		nonce: Option<U256>,
		double_gas: bool,
		encoded_call: Vec<u8>,
	) -> RpcResult<()> {
		sign_and_submit_transaction(self, params, contract_address, nonce, double_gas, encoded_call)
			.await
			.map_err(Into::into)
	}

	async fn transaction_receipts(
		&self,
		id: EthereumHeaderId,
		transactions: Vec<H256>,
	) -> RpcResult<(EthereumHeaderId, Vec<Receipt>)> {
		let mut transaction_receipts = Vec::with_capacity(transactions.len());
		for transaction in transactions {
			let transaction_receipt = self.transaction_receipt(transaction).await?;
			transaction_receipts.push(transaction_receipt);
		}
		Ok((id, transaction_receipts))
	}
}

/// Max number of headers which can be sent to Solidity contract.
pub const HEADERS_BATCH: usize = 4;

/// Substrate headers to send to the Ethereum light client.
///
/// The Solidity contract can only accept a fixed number of headers in one go.
/// This struct is meant to encapsulate this limitation.
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub struct HeadersBatch {
	pub header1: QueuedRialtoHeader,
	pub header2: Option<QueuedRialtoHeader>,
	pub header3: Option<QueuedRialtoHeader>,
	pub header4: Option<QueuedRialtoHeader>,
}

impl HeadersBatch {
	/// Create new headers from given header & ids collections.
	///
	/// This method will pop `HEADERS_BATCH` items from both collections
	/// and construct `Headers` object and a vector of `RialtoHeaderId`s.
	pub fn pop_from(
		headers: &mut Vec<QueuedRialtoHeader>,
		ids: &mut Vec<RialtoHeaderId>,
	) -> Result<(Self, Vec<RialtoHeaderId>), ()> {
		if headers.len() != ids.len() {
			log::error!(target: "bridge", "Collection size mismatch ({} vs {})", headers.len(), ids.len());
			return Err(());
		}

		let header1 = headers.pop().ok_or(())?;
		let header2 = headers.pop();
		let header3 = headers.pop();
		let header4 = headers.pop();

		let mut submitting_ids = Vec::with_capacity(HEADERS_BATCH);
		for _ in 0..HEADERS_BATCH {
			submitting_ids.extend(ids.pop().iter());
		}

		Ok((
			Self {
				header1,
				header2,
				header3,
				header4,
			},
			submitting_ids,
		))
	}

	/// Returns unified array of headers.
	///
	/// The first element is always `Some`.
	fn headers(&self) -> [Option<&QueuedRialtoHeader>; HEADERS_BATCH] {
		[
			Some(&self.header1),
			self.header2.as_ref(),
			self.header3.as_ref(),
			self.header4.as_ref(),
		]
	}

	/// Encodes all headers. If header is not present an empty vector will be returned.
	pub fn encode(&self) -> [Vec<u8>; HEADERS_BATCH] {
		let encode = |h: &QueuedRialtoHeader| h.header().encode();
		let headers = self.headers();
		[
			headers[0].map(encode).unwrap_or_default(),
			headers[1].map(encode).unwrap_or_default(),
			headers[2].map(encode).unwrap_or_default(),
			headers[3].map(encode).unwrap_or_default(),
		]
	}
	/// Returns number of contained headers.
	pub fn len(&self) -> usize {
		let is_set = |h: &Option<&QueuedRialtoHeader>| if h.is_some() { 1 } else { 0 };
		self.headers().iter().map(is_set).sum()
	}

	/// Remove headers starting from `idx` (0-based) from this collection.
	///
	/// The collection will be left with `[0, idx)` headers.
	/// Returns `Err` when `idx == 0`, since `Headers` must contain at least one header,
	/// or when `idx > HEADERS_BATCH`.
	pub fn split_off(&mut self, idx: usize) -> Result<(), ()> {
		if idx == 0 || idx > HEADERS_BATCH {
			return Err(());
		}
		let mut vals: [_; HEADERS_BATCH] = [&mut None, &mut self.header2, &mut self.header3, &mut self.header4];
		for val in vals.iter_mut().skip(idx) {
			**val = None;
		}
		Ok(())
	}
}

/// Substrate headers submitter API.
#[async_trait]
trait HeadersSubmitter {
	/// Returns Ok(0) if all given not-yet-imported headers are complete.
	/// Returns Ok(index != 0) where index is 1-based index of first header that is incomplete.
	///
	/// Returns Err(()) if contract has rejected headers. This means that the contract is
	/// unable to import first header (e.g. it may already be imported).
	async fn is_headers_incomplete(&self, headers: &HeadersBatch) -> RpcResult<usize>;

	/// Submit given headers to Ethereum node.
	async fn submit_headers(&mut self, headers: HeadersBatch) -> RpcResult<()>;
}

/// Implementation of Substrate headers submitter that sends headers to running Ethereum node.
struct EthereumHeadersSubmitter {
	client: EthereumClient,
	params: EthereumSigningParams,
	contract_address: Address,
	nonce: U256,
}

#[async_trait]
impl HeadersSubmitter for EthereumHeadersSubmitter {
	async fn is_headers_incomplete(&self, headers: &HeadersBatch) -> RpcResult<usize> {
		let [h1, h2, h3, h4] = headers.encode();
		let (encoded_call, call_decoder) = bridge_contract::functions::is_incomplete_headers::call(h1, h2, h3, h4);
		let call_request = CallRequest {
			to: Some(self.contract_address),
			data: Some(encoded_call.into()),
			..Default::default()
		};

		let call_result = self.client.eth_call(call_request).await?;
		let incomplete_index: U256 = call_decoder.decode(&call_result.0)?;
		if incomplete_index > HEADERS_BATCH.into() {
			return Err(RpcError::Ethereum(EthereumNodeError::InvalidIncompleteIndex));
		}

		Ok(incomplete_index.low_u32() as _)
	}

	async fn submit_headers(&mut self, headers: HeadersBatch) -> RpcResult<()> {
		let [h1, h2, h3, h4] = headers.encode();
		let result = self
			.client
			.submit_ethereum_transaction(
				&self.params,
				Some(self.contract_address),
				Some(self.nonce),
				false,
				bridge_contract::functions::import_headers::encode_input(h1, h2, h3, h4),
			)
			.await;

		if result.is_ok() {
			self.nonce += U256::one();
		}

		result
	}
}

/// Submit multiple Substrate headers.
async fn submit_substrate_headers(
	mut header_submitter: impl HeadersSubmitter,
	mut headers: Vec<QueuedRialtoHeader>,
) -> SubmittedHeaders<RialtoHeaderId, RpcError> {
	let mut submitted_headers = SubmittedHeaders::default();

	let mut ids = headers.iter().map(|header| header.id()).rev().collect::<Vec<_>>();
	headers.reverse();

	while !headers.is_empty() {
		let (headers, submitting_ids) =
			HeadersBatch::pop_from(&mut headers, &mut ids).expect("Headers and ids are not empty; qed");

		submitted_headers.fatal_error =
			submit_substrate_headers_batch(&mut header_submitter, &mut submitted_headers, submitting_ids, headers)
				.await;

		if submitted_headers.fatal_error.is_some() {
			ids.reverse();
			submitted_headers.rejected.extend(ids);
			break;
		}
	}

	submitted_headers
}

/// Submit 4 Substrate headers in single PoA transaction.
async fn submit_substrate_headers_batch(
	header_submitter: &mut impl HeadersSubmitter,
	submitted_headers: &mut SubmittedHeaders<RialtoHeaderId, RpcError>,
	mut ids: Vec<RialtoHeaderId>,
	mut headers: HeadersBatch,
) -> Option<RpcError> {
	debug_assert_eq!(ids.len(), headers.len(),);

	// if parent of first header is either incomplete, or rejected, we assume that contract
	// will reject this header as well
	let parent_id = headers.header1.parent_id();
	if submitted_headers.rejected.contains(&parent_id) || submitted_headers.incomplete.contains(&parent_id) {
		submitted_headers.rejected.extend(ids);
		return None;
	}

	// check if headers are incomplete
	let incomplete_header_index = match header_submitter.is_headers_incomplete(&headers).await {
		// All headers valid
		Ok(0) => None,
		Ok(incomplete_header_index) => Some(incomplete_header_index),
		Err(error) => {
			// contract has rejected all headers => we do not want to submit it
			submitted_headers.rejected.extend(ids);
			if error.is_connection_error() {
				return Some(error);
			} else {
				return None;
			}
		}
	};

	// Modify `ids` and `headers` to only contain values that are going to be accepted.
	let rejected = if let Some(idx) = incomplete_header_index {
		let len = std::cmp::min(idx, ids.len());
		headers
			.split_off(len)
			.expect("len > 0, the case where all headers are valid is converted to None; qed");
		ids.split_off(len)
	} else {
		Vec::new()
	};
	let submitted = ids;
	let submit_result = header_submitter.submit_headers(headers).await;
	match submit_result {
		Ok(_) => {
			if incomplete_header_index.is_some() {
				submitted_headers.incomplete.extend(submitted.iter().last().cloned());
			}
			submitted_headers.submitted.extend(submitted);
			submitted_headers.rejected.extend(rejected);
			None
		}
		Err(error) => {
			submitted_headers.rejected.extend(submitted);
			submitted_headers.rejected.extend(rejected);
			Some(error)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_runtime::traits::Header;

	struct TestHeadersSubmitter {
		incomplete: Vec<RialtoHeaderId>,
		failed: Vec<RialtoHeaderId>,
	}

	#[async_trait]
	impl HeadersSubmitter for TestHeadersSubmitter {
		async fn is_headers_incomplete(&self, headers: &HeadersBatch) -> RpcResult<usize> {
			if self.incomplete.iter().any(|i| i.0 == headers.header1.id().0) {
				Ok(1)
			} else {
				Ok(0)
			}
		}

		async fn submit_headers(&mut self, headers: HeadersBatch) -> RpcResult<()> {
			if self.failed.iter().any(|i| i.0 == headers.header1.id().0) {
				Err(RpcError::Ethereum(EthereumNodeError::InvalidSubstrateBlockNumber))
			} else {
				Ok(())
			}
		}
	}

	fn header(number: rialto_runtime::BlockNumber) -> QueuedRialtoHeader {
		QueuedRialtoHeader::new(
			rialto_runtime::Header::new(
				number,
				Default::default(),
				Default::default(),
				if number == 0 {
					Default::default()
				} else {
					header(number - 1).id().1
				},
				Default::default(),
			)
			.into(),
		)
	}

	#[test]
	fn descendants_of_incomplete_headers_are_not_submitted() {
		let submitted_headers = async_std::task::block_on(submit_substrate_headers(
			TestHeadersSubmitter {
				incomplete: vec![header(5).id()],
				failed: vec![],
			},
			vec![header(5), header(6)],
		));
		assert_eq!(submitted_headers.submitted, vec![header(5).id()]);
		assert_eq!(submitted_headers.incomplete, vec![header(5).id()]);
		assert_eq!(submitted_headers.rejected, vec![header(6).id()]);
		assert!(submitted_headers.fatal_error.is_none());
	}

	#[test]
	fn headers_after_fatal_error_are_not_submitted() {
		let submitted_headers = async_std::task::block_on(submit_substrate_headers(
			TestHeadersSubmitter {
				incomplete: vec![],
				failed: vec![header(9).id()],
			},
			vec![
				header(5),
				header(6),
				header(7),
				header(8),
				header(9),
				header(10),
				header(11),
			],
		));
		assert_eq!(
			submitted_headers.submitted,
			vec![header(5).id(), header(6).id(), header(7).id(), header(8).id()]
		);
		assert_eq!(submitted_headers.incomplete, vec![]);
		assert_eq!(
			submitted_headers.rejected,
			vec![header(9).id(), header(10).id(), header(11).id(),]
		);
		assert!(submitted_headers.fatal_error.is_some());
	}

	fn headers_batch() -> HeadersBatch {
		let mut init_headers = vec![header(1), header(2), header(3), header(4), header(5)];
		init_headers.reverse();
		let mut init_ids = init_headers.iter().map(|h| h.id()).collect();
		let (headers, ids) = HeadersBatch::pop_from(&mut init_headers, &mut init_ids).unwrap();
		assert_eq!(init_headers, vec![header(5)]);
		assert_eq!(init_ids, vec![header(5).id()]);
		assert_eq!(
			ids,
			vec![header(1).id(), header(2).id(), header(3).id(), header(4).id()]
		);
		headers
	}

	#[test]
	fn headers_batch_len() {
		let headers = headers_batch();
		assert_eq!(headers.len(), 4);
	}

	#[test]
	fn headers_batch_encode() {
		let headers = headers_batch();
		assert_eq!(
			headers.encode(),
			[
				header(1).header().encode(),
				header(2).header().encode(),
				header(3).header().encode(),
				header(4).header().encode(),
			]
		);
	}

	#[test]
	fn headers_batch_split_off() {
		// given
		let mut headers = headers_batch();

		// when
		assert!(headers.split_off(0).is_err());
		assert_eq!(headers.header1, header(1));
		assert!(headers.header2.is_some());
		assert!(headers.header3.is_some());
		assert!(headers.header4.is_some());

		// when
		let mut h = headers.clone();
		h.split_off(1).unwrap();
		assert!(h.header2.is_none());
		assert!(h.header3.is_none());
		assert!(h.header4.is_none());

		// when
		let mut h = headers.clone();
		h.split_off(2).unwrap();
		assert!(h.header2.is_some());
		assert!(h.header3.is_none());
		assert!(h.header4.is_none());

		// when
		let mut h = headers.clone();
		h.split_off(3).unwrap();
		assert!(h.header2.is_some());
		assert!(h.header3.is_some());
		assert!(h.header4.is_none());

		// when
		let mut h = headers;
		h.split_off(4).unwrap();
		assert!(h.header2.is_some());
		assert!(h.header3.is_some());
		assert!(h.header4.is_some());
	}
}
