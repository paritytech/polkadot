// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Validator-side view of collation.
//!
//! This module contains type definitions, a trait for a batch of collators, and a trait for
//! attempting to fetch a collation repeatedly until a valid one is obtained.

use std::sync::Arc;

use polkadot_primitives::{Block, Hash, BlockId};
use polkadot_primitives::parachain::{Id as ParaId, Collation, Extrinsic, CollatorId};
use polkadot_primitives::parachain::ParachainHost;
use runtime_primitives::traits::ProvideRuntimeApi;

use futures::prelude::*;

/// Encapsulates connections to collators and allows collation on any parachain.
///
/// This is expected to be a lightweight, shared type like an `Arc`.
pub trait Collators: Clone {
	/// Errors when producing collations.
	type Error;
	/// A full collation.
	type Collation: IntoFuture<Item=Collation,Error=Self::Error>;

	/// Collate on a specific parachain, building on a given relay chain parent hash.
	///
	/// The returned collation should be checked for basic validity in the signature
	/// and will be checked for state-transition validity by the consumer of this trait.
	///
	/// This does not have to guarantee local availability, as a valid collation
	/// will be passed to the `TableRouter` instance.
	fn collate(&self, parachain: ParaId, relay_parent: Hash) -> Self::Collation;

	/// Note a bad collator. TODO: take proof
	fn note_bad_collator(&self, collator: CollatorId);
}

/// A future which resolves when a collation is available.
///
/// This future is fused.
pub struct CollationFetch<C: Collators, P: ProvideRuntimeApi> {
	parachain: ParaId,
	relay_parent_hash: Hash,
	relay_parent: BlockId,
	collators: C,
	live_fetch: Option<<C::Collation as IntoFuture>::Future>,
	client: Arc<P>,
}

impl<C: Collators, P: ProvideRuntimeApi> CollationFetch<C, P> {
	/// Create a new collation fetcher for the given chain.
	pub fn new(parachain: ParaId, relay_parent: BlockId, relay_parent_hash: Hash, collators: C, client: Arc<P>) -> Self {
		CollationFetch {
			relay_parent_hash,
			relay_parent,
			collators,
			client,
			parachain,
			live_fetch: None,
		}
	}

	/// Access the underlying relay parent hash.
	pub fn relay_parent(&self) -> Hash {
		self.relay_parent_hash
	}
}

impl<C: Collators, P: ProvideRuntimeApi> Future for CollationFetch<C, P>
	where P::Api: ParachainHost<Block>,
{
	type Item = (Collation, Extrinsic);
	type Error = C::Error;

	fn poll(&mut self) -> Poll<(Collation, Extrinsic), C::Error> {
		loop {
			let x = {
				let parachain = self.parachain.clone();
				let (r, c)  = (self.relay_parent_hash, &self.collators);
				let poll = self.live_fetch
					.get_or_insert_with(move || c.collate(parachain, r).into_future())
					.poll();

				try_ready!(poll)
			};

			match validate_collation(&*self.client, &self.relay_parent, &x) {
				Ok(()) => {
					// TODO: generate extrinsic while verifying.
					return Ok(Async::Ready((x, Extrinsic)));
				}
				Err(e) => {
					debug!("Failed to validate parachain due to API error: {}", e);

					// just continue if we got a bad collation or failed to validate
					self.live_fetch = None;
					self.collators.note_bad_collator(x.receipt.collator)
				}
			}
		}
	}
}

// Errors that can occur when validating a parachain.
error_chain! {
	types { Error, ErrorKind, ResultExt; }

	errors {
		InactiveParachain(id: ParaId) {
			description("Collated for inactive parachain"),
			display("Collated for inactive parachain: {:?}", id),
		}
		ValidationFailure {
			description("Parachain candidate failed validation."),
			display("Parachain candidate failed validation."),
		}
		WrongHeadData(expected: Vec<u8>, got: Vec<u8>) {
			description("Parachain validation produced wrong head data."),
			display("Parachain validation produced wrong head data (expected: {:?}, got {:?}", expected, got),
		}
	}

	links {
		Client(::client::error::Error, ::client::error::ErrorKind);
	}
}

/// Check whether a given collation is valid. Returns `Ok`  on success, error otherwise.
pub fn validate_collation<P>(
	client: &P,
	relay_parent: &BlockId,
	collation: &Collation
) -> Result<(), Error> where
	P: ProvideRuntimeApi,
	P::Api: ParachainHost<Block>
{
	use parachain::{self, ValidationParams};

	let api = client.runtime_api();
	let para_id = collation.receipt.parachain_index;
	let validation_code = api.parachain_code(relay_parent, para_id)?
		.ok_or_else(|| ErrorKind::InactiveParachain(para_id))?;

	let chain_head = api.parachain_head(relay_parent, para_id)?
		.ok_or_else(|| ErrorKind::InactiveParachain(para_id))?;

	let params = ValidationParams {
		parent_head: chain_head,
		block_data: collation.block_data.0.clone(),
	};

	match parachain::wasm::validate_candidate(&validation_code, params) {
		Ok(result) => {
			if result.head_data == collation.receipt.head_data.0 {
				Ok(())
			} else {
				Err(ErrorKind::WrongHeadData(
					collation.receipt.head_data.0.clone(),
					result.head_data
				).into())
			}
		}
		Err(_) => Err(ErrorKind::ValidationFailure.into())
	}
}
