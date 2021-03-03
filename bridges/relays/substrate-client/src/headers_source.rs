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

//! Default generic implementation of headers source for basic Substrate client.

use crate::chain::{BlockWithJustification, Chain};
use crate::client::Client;
use crate::error::Error;

use async_trait::async_trait;
use headers_relay::{
	sync_loop::SourceClient,
	sync_types::{HeaderIdOf, HeadersSyncPipeline, QueuedHeader, SourceHeader},
};
use relay_utils::relay_loop::Client as RelayClient;
use sp_runtime::{traits::Header as HeaderT, Justification};
use std::marker::PhantomData;

/// Substrate node as headers source.
pub struct HeadersSource<C: Chain, P> {
	client: Client<C>,
	_phantom: PhantomData<P>,
}

impl<C: Chain, P> HeadersSource<C, P> {
	/// Create new headers source using given client.
	pub fn new(client: Client<C>) -> Self {
		HeadersSource {
			client,
			_phantom: Default::default(),
		}
	}
}

impl<C: Chain, P> Clone for HeadersSource<C, P> {
	fn clone(&self) -> Self {
		HeadersSource {
			client: self.client.clone(),
			_phantom: Default::default(),
		}
	}
}

#[async_trait]
impl<C: Chain, P: HeadersSyncPipeline> RelayClient for HeadersSource<C, P> {
	type Error = Error;

	async fn reconnect(&mut self) -> Result<(), Error> {
		self.client.reconnect().await
	}
}

#[async_trait]
impl<C, P> SourceClient<P> for HeadersSource<C, P>
where
	C: Chain,
	C::BlockNumber: relay_utils::BlockNumberBase,
	C::Header: Into<P::Header>,
	P: HeadersSyncPipeline<Extra = (), Completion = Justification, Hash = C::Hash, Number = C::BlockNumber>,
	P::Header: SourceHeader<C::Hash, C::BlockNumber>,
{
	async fn best_block_number(&self) -> Result<P::Number, Error> {
		// we **CAN** continue to relay headers if source node is out of sync, because
		// target node may be missing headers that are already available at the source
		Ok(*self.client.best_header().await?.number())
	}

	async fn header_by_hash(&self, hash: P::Hash) -> Result<P::Header, Error> {
		self.client
			.header_by_hash(hash)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}

	async fn header_by_number(&self, number: P::Number) -> Result<P::Header, Error> {
		self.client
			.header_by_number(number)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}

	async fn header_completion(&self, id: HeaderIdOf<P>) -> Result<(HeaderIdOf<P>, Option<P::Completion>), Error> {
		let hash = id.1;
		let signed_block = self.client.get_block(Some(hash)).await?;
		let grandpa_justification = signed_block.justification().cloned();

		Ok((id, grandpa_justification))
	}

	async fn header_extra(&self, id: HeaderIdOf<P>, _header: QueuedHeader<P>) -> Result<(HeaderIdOf<P>, ()), Error> {
		Ok((id, ()))
	}
}
