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

//! Substrate client as Substrate finality proof target. The chain we connect to should have
//! runtime that implements `<BridgedChainName>FinalityApi` to allow bridging with
//! <BridgedName> chain.

use crate::finality_pipeline::SubstrateFinalitySyncPipeline;

use async_trait::async_trait;
use codec::Decode;
use finality_relay::{FinalitySyncPipeline, TargetClient};
use relay_substrate_client::{Chain, Client, Error as SubstrateError};
use relay_utils::relay_loop::Client as RelayClient;

/// Substrate client as Substrate finality target.
pub struct SubstrateFinalityTarget<C: Chain, P> {
	client: Client<C>,
	pipeline: P,
	transactions_mortality: Option<u32>,
}

impl<C: Chain, P> SubstrateFinalityTarget<C, P> {
	/// Create new Substrate headers target.
	pub fn new(client: Client<C>, pipeline: P, transactions_mortality: Option<u32>) -> Self {
		SubstrateFinalityTarget { client, pipeline, transactions_mortality }
	}
}

impl<C: Chain, P: SubstrateFinalitySyncPipeline> Clone for SubstrateFinalityTarget<C, P> {
	fn clone(&self) -> Self {
		SubstrateFinalityTarget {
			client: self.client.clone(),
			pipeline: self.pipeline.clone(),
			transactions_mortality: self.transactions_mortality,
		}
	}
}

#[async_trait]
impl<C: Chain, P: SubstrateFinalitySyncPipeline> RelayClient for SubstrateFinalityTarget<C, P> {
	type Error = SubstrateError;

	async fn reconnect(&mut self) -> Result<(), SubstrateError> {
		self.client.reconnect().await
	}
}

#[async_trait]
impl<C, P> TargetClient<P::FinalitySyncPipeline> for SubstrateFinalityTarget<C, P>
where
	C: Chain,
	P: SubstrateFinalitySyncPipeline<TargetChain = C>,
	<P::FinalitySyncPipeline as FinalitySyncPipeline>::Number: Decode,
	<P::FinalitySyncPipeline as FinalitySyncPipeline>::Hash: Decode,
{
	async fn best_finalized_source_block_number(
		&self,
	) -> Result<<P::FinalitySyncPipeline as FinalitySyncPipeline>::Number, SubstrateError> {
		// we can't continue to relay finality if target node is out of sync, because
		// it may have already received (some of) headers that we're going to relay
		self.client.ensure_synced().await?;

		Ok(crate::messages_source::read_client_state::<
			C,
			<P::FinalitySyncPipeline as FinalitySyncPipeline>::Hash,
			<P::FinalitySyncPipeline as FinalitySyncPipeline>::Number,
		>(&self.client, P::BEST_FINALIZED_SOURCE_HEADER_ID_AT_TARGET)
		.await?
		.best_finalized_peer_at_best_self
		.0)
	}

	async fn submit_finality_proof(
		&self,
		header: <P::FinalitySyncPipeline as FinalitySyncPipeline>::Header,
		proof: <P::FinalitySyncPipeline as FinalitySyncPipeline>::FinalityProof,
	) -> Result<(), SubstrateError> {
		let transactions_author = self.pipeline.transactions_author();
		let pipeline = self.pipeline.clone();
		let transactions_mortality = self.transactions_mortality;
		self.client
			.submit_signed_extrinsic(
				transactions_author,
				move |best_block_id, transaction_nonce| {
					pipeline.make_submit_finality_proof_transaction(
						relay_substrate_client::TransactionEra::new(
							best_block_id,
							transactions_mortality,
						),
						transaction_nonce,
						header,
						proof,
					)
				},
			)
			.await
			.map(drop)
	}
}
