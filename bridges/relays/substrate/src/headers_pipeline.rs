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

//! Substrate-to-Substrate headers sync entrypoint.

use crate::{headers_maintain::SubstrateHeadersToSubstrateMaintain, headers_target::SubstrateHeadersTarget};

use async_trait::async_trait;
use codec::Encode;
use headers_relay::{
	sync::{HeadersSyncParams, TargetTransactionMode},
	sync_types::{HeaderIdOf, HeadersSyncPipeline, QueuedHeader, SourceHeader},
};
use relay_substrate_client::{
	headers_source::HeadersSource, BlockNumberOf, Chain, Client, Error as SubstrateError, HashOf,
};
use relay_utils::BlockNumberBase;
use sp_runtime::Justification;
use std::marker::PhantomData;

/// Headers sync pipeline for Substrate <-> Substrate relays.
#[async_trait]
pub trait SubstrateHeadersSyncPipeline: HeadersSyncPipeline {
	/// Name of the `best_block` runtime method.
	const BEST_BLOCK_METHOD: &'static str;
	/// Name of the `finalized_block` runtime method.
	const FINALIZED_BLOCK_METHOD: &'static str;
	/// Name of the `is_known_block` runtime method.
	const IS_KNOWN_BLOCK_METHOD: &'static str;
	/// Name of the `incomplete_headers` runtime method.
	const INCOMPLETE_HEADERS_METHOD: &'static str;

	/// Signed transaction type.
	type SignedTransaction: Send + Sync + Encode;

	/// Make submit header transaction.
	async fn make_submit_header_transaction(
		&self,
		header: QueuedHeader<Self>,
	) -> Result<Self::SignedTransaction, SubstrateError>;

	/// Make completion transaction for the header.
	async fn make_complete_header_transaction(
		&self,
		id: HeaderIdOf<Self>,
		completion: Justification,
	) -> Result<Self::SignedTransaction, SubstrateError>;
}

/// Substrate-to-Substrate headers pipeline.
#[derive(Debug, Clone)]
pub struct SubstrateHeadersToSubstrate<SourceChain, SourceSyncHeader, TargetChain: Chain, TargetSign> {
	/// Client for the target chain.
	pub(crate) target_client: Client<TargetChain>,
	/// Data required to sign target chain transactions.
	pub(crate) target_sign: TargetSign,
	/// Unused generic arguments dump.
	_marker: PhantomData<(SourceChain, SourceSyncHeader)>,
}

impl<SourceChain, SourceSyncHeader, TargetChain: Chain, TargetSign>
	SubstrateHeadersToSubstrate<SourceChain, SourceSyncHeader, TargetChain, TargetSign>
{
	/// Create new Substrate-to-Substrate headers pipeline.
	pub fn new(target_client: Client<TargetChain>, target_sign: TargetSign) -> Self {
		SubstrateHeadersToSubstrate {
			target_client,
			target_sign,
			_marker: Default::default(),
		}
	}
}

impl<SourceChain, SourceSyncHeader, TargetChain, TargetSign> HeadersSyncPipeline
	for SubstrateHeadersToSubstrate<SourceChain, SourceSyncHeader, TargetChain, TargetSign>
where
	SourceChain: Clone + Chain,
	BlockNumberOf<SourceChain>: BlockNumberBase,
	SourceSyncHeader:
		SourceHeader<HashOf<SourceChain>, BlockNumberOf<SourceChain>> + std::ops::Deref<Target = SourceChain::Header>,
	TargetChain: Clone + Chain,
	TargetSign: Clone + Send + Sync,
{
	const SOURCE_NAME: &'static str = SourceChain::NAME;
	const TARGET_NAME: &'static str = TargetChain::NAME;

	type Hash = HashOf<SourceChain>;
	type Number = BlockNumberOf<SourceChain>;
	type Header = SourceSyncHeader;
	type Extra = ();
	type Completion = Justification;

	fn estimate_size(source: &QueuedHeader<Self>) -> usize {
		source.header().encode().len()
	}
}

/// Return sync parameters for Substrate-to-Substrate headers sync.
pub fn sync_params() -> HeadersSyncParams {
	HeadersSyncParams {
		max_future_headers_to_download: 32,
		max_headers_in_submitted_status: 8,
		max_headers_in_single_submit: 1,
		max_headers_size_in_single_submit: 1024 * 1024,
		prune_depth: 256,
		target_tx_mode: TargetTransactionMode::Signed,
	}
}

/// Run Substrate-to-Substrate headers sync.
pub async fn run<SourceChain, TargetChain, P>(
	pipeline: P,
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	metrics_params: Option<relay_utils::metrics::MetricsParams>,
) where
	P: SubstrateHeadersSyncPipeline<
		Hash = HashOf<SourceChain>,
		Number = BlockNumberOf<SourceChain>,
		Completion = Justification,
		Extra = (),
	>,
	P::Header: SourceHeader<HashOf<SourceChain>, BlockNumberOf<SourceChain>>,
	SourceChain: Clone + Chain,
	SourceChain::Header: Into<P::Header>,
	BlockNumberOf<SourceChain>: BlockNumberBase,
	TargetChain: Clone + Chain,
{
	let source_justifications = match source_client.clone().subscribe_justifications().await {
		Ok(source_justifications) => source_justifications,
		Err(error) => {
			log::warn!(
				target: "bridge",
				"Failed to subscribe to {} justifications: {:?}",
				SourceChain::NAME,
				error,
			);

			return;
		}
	};

	let sync_maintain = SubstrateHeadersToSubstrateMaintain::<_, SourceChain, _>::new(
		pipeline.clone(),
		target_client.clone(),
		source_justifications,
	);

	log::info!(
		target: "bridge",
		"Starting {} -> {} headers relay",
		SourceChain::NAME,
		TargetChain::NAME,
	);

	headers_relay::sync_loop::run(
		HeadersSource::new(source_client),
		SourceChain::AVERAGE_BLOCK_INTERVAL,
		SubstrateHeadersTarget::new(target_client, pipeline),
		TargetChain::AVERAGE_BLOCK_INTERVAL,
		sync_maintain,
		sync_params(),
		metrics_params,
		futures::future::pending(),
	);
}
