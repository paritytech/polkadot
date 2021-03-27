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

//! Ethereum PoA -> Rialto-Substrate synchronization.

use crate::ethereum_client::EthereumHighLevelRpc;
use crate::instances::BridgeInstance;
use crate::rialto_client::{SubmitEthereumHeaders, SubstrateHighLevelRpc};
use crate::rpc_errors::RpcError;
use crate::substrate_types::{into_substrate_ethereum_header, into_substrate_ethereum_receipts};

use async_trait::async_trait;
use codec::Encode;
use headers_relay::{
	sync::{HeadersSyncParams, TargetTransactionMode},
	sync_loop::{SourceClient, TargetClient},
	sync_types::{HeadersSyncPipeline, QueuedHeader, SourceHeader, SubmittedHeaders},
};
use relay_ethereum_client::{
	types::{HeaderHash, HeaderId as EthereumHeaderId, Receipt, SyncHeader as Header},
	Client as EthereumClient, ConnectionParams as EthereumConnectionParams,
};
use relay_rialto_client::{Rialto, SigningParams as RialtoSigningParams};
use relay_substrate_client::{
	Chain as SubstrateChain, Client as SubstrateClient, ConnectionParams as SubstrateConnectionParams,
};
use relay_utils::{metrics::MetricsParams, relay_loop::Client as RelayClient};

use std::fmt::Debug;
use std::{collections::HashSet, sync::Arc, time::Duration};

pub mod consts {
	use super::*;

	/// Interval at which we check new Ethereum headers when we are synced/almost synced.
	pub const ETHEREUM_TICK_INTERVAL: Duration = Duration::from_secs(10);
	/// Max number of headers in single submit transaction.
	pub const MAX_HEADERS_IN_SINGLE_SUBMIT: usize = 32;
	/// Max total size of headers in single submit transaction. This only affects signed
	/// submissions, when several headers are submitted at once. 4096 is the maximal **expected**
	/// size of the Ethereum header + transactions receipts (if they're required).
	pub const MAX_HEADERS_SIZE_IN_SINGLE_SUBMIT: usize = MAX_HEADERS_IN_SINGLE_SUBMIT * 4096;
	/// Max Ethereum headers we want to have in all 'before-submitted' states.
	pub const MAX_FUTURE_HEADERS_TO_DOWNLOAD: usize = 128;
	/// Max Ethereum headers count we want to have in 'submitted' state.
	pub const MAX_SUBMITTED_HEADERS: usize = 128;
	/// Max depth of in-memory headers in all states. Past this depth they will be forgotten (pruned).
	pub const PRUNE_DEPTH: u32 = 4096;
}

/// Ethereum synchronization parameters.
#[derive(Debug)]
pub struct EthereumSyncParams {
	/// Ethereum connection params.
	pub eth_params: EthereumConnectionParams,
	/// Substrate connection params.
	pub sub_params: SubstrateConnectionParams,
	/// Substrate signing params.
	pub sub_sign: RialtoSigningParams,
	/// Synchronization parameters.
	pub sync_params: HeadersSyncParams,
	/// Metrics parameters.
	pub metrics_params: Option<MetricsParams>,
	/// Instance of the bridge pallet being synchronized.
	pub instance: Arc<dyn BridgeInstance>,
}

/// Ethereum synchronization pipeline.
#[derive(Clone, Copy, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct EthereumHeadersSyncPipeline;

impl HeadersSyncPipeline for EthereumHeadersSyncPipeline {
	const SOURCE_NAME: &'static str = "Ethereum";
	const TARGET_NAME: &'static str = "Substrate";

	type Hash = HeaderHash;
	type Number = u64;
	type Header = Header;
	type Extra = Vec<Receipt>;
	type Completion = ();

	fn estimate_size(source: &QueuedHeader<Self>) -> usize {
		into_substrate_ethereum_header(source.header()).encode().len()
			+ into_substrate_ethereum_receipts(source.extra())
				.map(|extra| extra.encode().len())
				.unwrap_or(0)
	}
}

/// Queued ethereum header ID.
pub type QueuedEthereumHeader = QueuedHeader<EthereumHeadersSyncPipeline>;

/// Ethereum client as headers source.
#[derive(Clone)]
struct EthereumHeadersSource {
	/// Ethereum node client.
	client: EthereumClient,
}

impl EthereumHeadersSource {
	fn new(client: EthereumClient) -> Self {
		Self { client }
	}
}

#[async_trait]
impl RelayClient for EthereumHeadersSource {
	type Error = RpcError;

	async fn reconnect(&mut self) -> Result<(), RpcError> {
		self.client.reconnect();
		Ok(())
	}
}

#[async_trait]
impl SourceClient<EthereumHeadersSyncPipeline> for EthereumHeadersSource {
	async fn best_block_number(&self) -> Result<u64, RpcError> {
		// we **CAN** continue to relay headers if Ethereum node is out of sync, because
		// Substrate node may be missing headers that are already available at the Ethereum

		self.client.best_block_number().await.map_err(Into::into)
	}

	async fn header_by_hash(&self, hash: HeaderHash) -> Result<Header, RpcError> {
		self.client
			.header_by_hash(hash)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}

	async fn header_by_number(&self, number: u64) -> Result<Header, RpcError> {
		self.client
			.header_by_number(number)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}

	async fn header_completion(&self, id: EthereumHeaderId) -> Result<(EthereumHeaderId, Option<()>), RpcError> {
		Ok((id, None))
	}

	async fn header_extra(
		&self,
		id: EthereumHeaderId,
		header: QueuedEthereumHeader,
	) -> Result<(EthereumHeaderId, Vec<Receipt>), RpcError> {
		self.client
			.transaction_receipts(id, header.header().transactions.clone())
			.await
	}
}

#[derive(Clone)]
struct SubstrateHeadersTarget {
	/// Substrate node client.
	client: SubstrateClient<Rialto>,
	/// Whether we want to submit signed (true), or unsigned (false) transactions.
	sign_transactions: bool,
	/// Substrate signing params.
	sign_params: RialtoSigningParams,
	/// Bridge instance used in Ethereum to Substrate sync.
	bridge_instance: Arc<dyn BridgeInstance>,
}

impl SubstrateHeadersTarget {
	fn new(
		client: SubstrateClient<Rialto>,
		sign_transactions: bool,
		sign_params: RialtoSigningParams,
		bridge_instance: Arc<dyn BridgeInstance>,
	) -> Self {
		Self {
			client,
			sign_transactions,
			sign_params,
			bridge_instance,
		}
	}
}

#[async_trait]
impl RelayClient for SubstrateHeadersTarget {
	type Error = RpcError;

	async fn reconnect(&mut self) -> Result<(), RpcError> {
		Ok(self.client.reconnect().await?)
	}
}

#[async_trait]
impl TargetClient<EthereumHeadersSyncPipeline> for SubstrateHeadersTarget {
	async fn best_header_id(&self) -> Result<EthereumHeaderId, RpcError> {
		// we can't continue to relay headers if Substrate node is out of sync, because
		// it may have already received (some of) headers that we're going to relay
		self.client.ensure_synced().await?;

		self.client.best_ethereum_block().await
	}

	async fn is_known_header(&self, id: EthereumHeaderId) -> Result<(EthereumHeaderId, bool), RpcError> {
		Ok((id, self.client.ethereum_header_known(id).await?))
	}

	async fn submit_headers(&self, headers: Vec<QueuedEthereumHeader>) -> SubmittedHeaders<EthereumHeaderId, RpcError> {
		let (sign_params, bridge_instance, sign_transactions) = (
			self.sign_params.clone(),
			self.bridge_instance.clone(),
			self.sign_transactions,
		);
		self.client
			.submit_ethereum_headers(sign_params, bridge_instance, headers, sign_transactions)
			.await
	}

	async fn incomplete_headers_ids(&self) -> Result<HashSet<EthereumHeaderId>, RpcError> {
		Ok(HashSet::new())
	}

	#[allow(clippy::unit_arg)]
	async fn complete_header(&self, id: EthereumHeaderId, _completion: ()) -> Result<EthereumHeaderId, RpcError> {
		Ok(id)
	}

	async fn requires_extra(&self, header: QueuedEthereumHeader) -> Result<(EthereumHeaderId, bool), RpcError> {
		// we can minimize number of receipts_check calls by checking header
		// logs bloom here, but it may give us false positives (when authorities
		// source is contract, we never need any logs)
		let id = header.header().id();
		let sub_eth_header = into_substrate_ethereum_header(header.header());
		Ok((id, self.client.ethereum_receipts_required(sub_eth_header).await?))
	}
}

/// Run Ethereum headers synchronization.
pub fn run(params: EthereumSyncParams) -> Result<(), RpcError> {
	let EthereumSyncParams {
		eth_params,
		sub_params,
		sub_sign,
		sync_params,
		metrics_params,
		instance,
	} = params;

	let eth_client = EthereumClient::new(eth_params);
	let sub_client = async_std::task::block_on(async { SubstrateClient::<Rialto>::new(sub_params).await })?;

	let sign_sub_transactions = match sync_params.target_tx_mode {
		TargetTransactionMode::Signed | TargetTransactionMode::Backup => true,
		TargetTransactionMode::Unsigned => false,
	};

	let source = EthereumHeadersSource::new(eth_client);
	let target = SubstrateHeadersTarget::new(sub_client, sign_sub_transactions, sub_sign, instance);

	headers_relay::sync_loop::run(
		source,
		consts::ETHEREUM_TICK_INTERVAL,
		target,
		Rialto::AVERAGE_BLOCK_INTERVAL,
		(),
		sync_params,
		metrics_params,
		futures::future::pending(),
	);

	Ok(())
}
