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

//! Rialto-to-Millau headers sync entrypoint.

use crate::{
	headers_pipeline::{SubstrateHeadersSyncPipeline, SubstrateHeadersToSubstrate},
	MillauClient, RialtoClient,
};

use async_trait::async_trait;
use bp_rialto::{
	BEST_RIALTO_BLOCKS_METHOD, FINALIZED_RIALTO_BLOCK_METHOD, INCOMPLETE_RIALTO_HEADERS_METHOD,
	IS_KNOWN_RIALTO_BLOCK_METHOD,
};
use headers_relay::sync_types::QueuedHeader;
use relay_millau_client::{BridgeRialtoCall, Millau, SigningParams as MillauSigningParams};
use relay_rialto_client::{HeaderId as RialtoHeaderId, Rialto, SyncHeader as RialtoSyncHeader};
use relay_substrate_client::{Error as SubstrateError, TransactionSignScheme};
use sp_core::Pair;
use sp_runtime::Justification;

/// Rialto-to-Millau headers sync pipeline.
type RialtoHeadersToMillau = SubstrateHeadersToSubstrate<Rialto, RialtoSyncHeader, Millau, MillauSigningParams>;
/// Rialto header in-the-queue.
type QueuedRialtoHeader = QueuedHeader<RialtoHeadersToMillau>;

#[async_trait]
impl SubstrateHeadersSyncPipeline for RialtoHeadersToMillau {
	const BEST_BLOCK_METHOD: &'static str = BEST_RIALTO_BLOCKS_METHOD;
	const FINALIZED_BLOCK_METHOD: &'static str = FINALIZED_RIALTO_BLOCK_METHOD;
	const IS_KNOWN_BLOCK_METHOD: &'static str = IS_KNOWN_RIALTO_BLOCK_METHOD;
	const INCOMPLETE_HEADERS_METHOD: &'static str = INCOMPLETE_RIALTO_HEADERS_METHOD;

	type SignedTransaction = <Millau as TransactionSignScheme>::SignedTransaction;

	async fn make_submit_header_transaction(
		&self,
		header: QueuedRialtoHeader,
	) -> Result<Self::SignedTransaction, SubstrateError> {
		let account_id = self.target_sign.signer.public().as_array_ref().clone().into();
		let nonce = self.target_client.next_account_index(account_id).await?;
		let call = BridgeRialtoCall::import_signed_header(header.header().clone().into_inner()).into();
		let transaction = Millau::sign_transaction(&self.target_client, &self.target_sign.signer, nonce, call);
		Ok(transaction)
	}

	async fn make_complete_header_transaction(
		&self,
		id: RialtoHeaderId,
		completion: Justification,
	) -> Result<Self::SignedTransaction, SubstrateError> {
		let account_id = self.target_sign.signer.public().as_array_ref().clone().into();
		let nonce = self.target_client.next_account_index(account_id).await?;
		let call = BridgeRialtoCall::finalize_header(id.1, completion).into();
		let transaction = Millau::sign_transaction(&self.target_client, &self.target_sign.signer, nonce, call);
		Ok(transaction)
	}
}

/// Run Rialto-to-Millau headers sync.
pub async fn run(
	rialto_client: RialtoClient,
	millau_client: MillauClient,
	millau_sign: MillauSigningParams,
	metrics_params: Option<relay_utils::metrics::MetricsParams>,
) {
	crate::headers_pipeline::run(
		RialtoHeadersToMillau::new(millau_client.clone(), millau_sign),
		rialto_client,
		millau_client,
		metrics_params,
	)
	.await;
}
