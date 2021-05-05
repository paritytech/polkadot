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

//! Default generic implementation of finality source for basic Substrate client.

use crate::chain::{BlockWithJustification, Chain};
use crate::client::Client;
use crate::error::Error;
use crate::sync_header::SyncHeader;

use async_trait::async_trait;
use bp_header_chain::justification::GrandpaJustification;
use codec::Decode;
use finality_relay::{FinalitySyncPipeline, SourceClient, SourceHeader};
use futures::stream::{unfold, Stream, StreamExt};
use relay_utils::relay_loop::Client as RelayClient;
use sp_runtime::traits::Header as HeaderT;
use std::{marker::PhantomData, pin::Pin};

/// Substrate node as finality source.
pub struct FinalitySource<C: Chain, P> {
	client: Client<C>,
	_phantom: PhantomData<P>,
}

impl<C: Chain, P> FinalitySource<C, P> {
	/// Create new headers source using given client.
	pub fn new(client: Client<C>) -> Self {
		FinalitySource {
			client,
			_phantom: Default::default(),
		}
	}
}

impl<C: Chain, P> Clone for FinalitySource<C, P> {
	fn clone(&self) -> Self {
		FinalitySource {
			client: self.client.clone(),
			_phantom: Default::default(),
		}
	}
}

#[async_trait]
impl<C: Chain, P: FinalitySyncPipeline> RelayClient for FinalitySource<C, P> {
	type Error = Error;

	async fn reconnect(&mut self) -> Result<(), Error> {
		self.client.reconnect().await
	}
}

#[async_trait]
impl<C, P> SourceClient<P> for FinalitySource<C, P>
where
	C: Chain,
	C::BlockNumber: relay_utils::BlockNumberBase,
	P: FinalitySyncPipeline<
		Hash = C::Hash,
		Number = C::BlockNumber,
		Header = SyncHeader<C::Header>,
		FinalityProof = GrandpaJustification<C::Header>,
	>,
	P::Header: SourceHeader<C::BlockNumber>,
{
	type FinalityProofsStream = Pin<Box<dyn Stream<Item = GrandpaJustification<C::Header>> + Send>>;

	async fn best_finalized_block_number(&self) -> Result<P::Number, Error> {
		// we **CAN** continue to relay finality proofs if source node is out of sync, because
		// target node may be missing proofs that are already available at the source
		let finalized_header_hash = self.client.best_finalized_header_hash().await?;
		let finalized_header = self.client.header_by_hash(finalized_header_hash).await?;
		Ok(*finalized_header.number())
	}

	async fn header_and_finality_proof(
		&self,
		number: P::Number,
	) -> Result<(P::Header, Option<P::FinalityProof>), Error> {
		let header_hash = self.client.block_hash_by_number(number).await?;
		let signed_block = self.client.get_block(Some(header_hash)).await?;

		let justification = signed_block
			.justification()
			.map(|raw_justification| GrandpaJustification::<C::Header>::decode(&mut raw_justification.as_slice()))
			.transpose()
			.map_err(Error::ResponseParseFailed)?;

		Ok((signed_block.header().into(), justification))
	}

	async fn finality_proofs(&self) -> Result<Self::FinalityProofsStream, Error> {
		Ok(unfold(
			self.client.clone().subscribe_justifications().await?,
			move |mut subscription| async move {
				loop {
					let next_justification = subscription.next().await?;
					let decoded_justification =
						GrandpaJustification::<C::Header>::decode(&mut &next_justification.0[..]);

					let justification = match decoded_justification {
						Ok(j) => j,
						Err(err) => {
							log::error!(
								target: "bridge",
								"Failed to decode justification target from the {} justifications stream: {:?}",
								P::SOURCE_NAME,
								err,
							);

							continue;
						}
					};

					return Some((justification, subscription));
				}
			},
		)
		.boxed())
	}
}
