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

use polkadot_primitives::{Block, Hash, AccountId, BlockId};
use polkadot_primitives::parachain::{Id as ParaId, Collation, Extrinsic, OutgoingMessage};
use polkadot_primitives::parachain::{CandidateReceipt, ParachainHost};
use runtime_primitives::traits::ProvideRuntimeApi;
use parachain::{wasm_executor::{self, ExternalitiesError}, MessageRef};
use super::Incoming;

use futures::prelude::*;

/// Encapsulates connections to collators and allows collation on any parachain.
///
/// This is expected to be a lightweight, shared type like an `Arc`.
pub trait Collators: Clone {
	/// Errors when producing collations.
	type Error: std::fmt::Debug;
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
	fn note_bad_collator(&self, collator: AccountId);
}

/// A future which resolves when a collation is available.
///
/// This future is fused.
pub struct CollationFetch<C: Collators, P> {
	parachain: ParaId,
	relay_parent_hash: Hash,
	relay_parent: BlockId,
	collators: C,
	incoming: Incoming,
	live_fetch: Option<<C::Collation as IntoFuture>::Future>,
	client: Arc<P>,
}

impl<C: Collators, P> CollationFetch<C, P> {
	/// Create a new collation fetcher for the given chain.
	pub fn new(
		parachain: ParaId,
		relay_parent_hash: Hash,
		collators: C,
		client: Arc<P>,
		incoming: Incoming,
	) -> Self {
		CollationFetch {
			relay_parent: BlockId::hash(relay_parent_hash),
			relay_parent_hash,
			collators,
			client,
			parachain,
			live_fetch: None,
			incoming,
		}
	}

	/// Access the underlying relay parent hash.
	pub fn relay_parent(&self) -> Hash {
		self.relay_parent_hash
	}

	/// Access the local parachain ID.
	pub fn parachain(&self) -> ParaId {
		self.parachain
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

			match validate_collation(&*self.client, &self.relay_parent, &x, &self.incoming) {
				Ok(e) => {
					return Ok(Async::Ready((x, e)))
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

	links {
		Client(::client::error::Error, ::client::error::ErrorKind);
		WasmValidation(wasm_executor::Error, wasm_executor::ErrorKind);
	}

	errors {
		InactiveParachain(id: ParaId) {
			description("Collated for inactive parachain"),
			display("Collated for inactive parachain: {:?}", id),
		}
		EgressRootMismatch(id: ParaId, expected: Hash, got: Hash) {
			description("Got unexpected egress route."),
			display(
				"Got unexpected egress route to {:?}. (expected: {:?}, got {:?})",
				id, expected, got
			),
		}
		MissingEgressRoute(expected: Option<ParaId>, got: Option<ParaId>) {
			description("Missing or extra egress route."),
			display("Missing or extra egress route. (expected: {:?}, got {:?})", expected, got),
		}
		WrongHeadData(expected: Vec<u8>, got: Vec<u8>) {
			description("Parachain validation produced wrong head data."),
			display("Parachain validation produced wrong head data (expected: {:?}, got {:?}", expected, got),
		}
	}
}

/// Compute a trie root for a set of messages.
pub fn message_queue_root<A, I: IntoIterator<Item=A>>(messages: I) -> Hash
	where A: AsRef<[u8]>
{
	::trie::ordered_trie_root::<primitives::Blake2Hasher, _, _>(messages)
}

/// Compute the set of egress roots for all given outgoing messages.
pub fn egress_roots(mut outgoing: Vec<OutgoingMessage>) -> Vec<(ParaId, Hash)> {
	// stable sort messages by parachain ID.
	outgoing.sort_by_key(|msg| ParaId::from(msg.target));

	let mut egress_roots = Vec::new();
	{
		let mut messages_iter = outgoing.iter().peekable();
		while let Some(batch_target) = messages_iter.peek().map(|o| o.target) {
 			// we borrow the iterator mutably to ensure it advances so the
			// next iteration of the loop starts with `messages_iter` pointing to
			// the next batch.
			let messages_to = messages_iter
				.clone()
				.take_while(|o| o.target == batch_target)
				.map(|o| { let _ = messages_iter.next(); &o.data[..] });

			let computed_root = message_queue_root(messages_to);
			egress_roots.push((batch_target, computed_root));
		}
	}

	egress_roots
}

fn check_extrinsic(
	mut outgoing: Vec<OutgoingMessage>,
	expected_egress_roots: &[(ParaId, Hash)],
) -> Result<Extrinsic, Error> {
	// stable sort messages by parachain ID.
	outgoing.sort_by_key(|msg| ParaId::from(msg.target));

	{
		let mut messages_iter = outgoing.iter().peekable();
		let mut expected_egress_roots = expected_egress_roots.iter();
		while let Some(batch_target) = messages_iter.peek().map(|o| o.target) {
			let expected_root = match expected_egress_roots.next() {
				None => return Err(ErrorKind::MissingEgressRoute(Some(batch_target), None).into()),
				Some(&(id, ref root)) => if id == batch_target {
					root
				} else {
					return Err(ErrorKind::MissingEgressRoute(Some(batch_target), Some(id)).into());
				}
			};

 			// we borrow the iterator mutably to ensure it advances so the
			// next iteration of the loop starts with `messages_iter` pointing to
			// the next batch.
			let messages_to = messages_iter
				.clone()
				.take_while(|o| o.target == batch_target)
				.map(|o| { let _ = messages_iter.next(); &o.data[..] });

			let computed_root = message_queue_root(messages_to);
			if &computed_root != expected_root {
				return Err(ErrorKind::EgressRootMismatch(
					batch_target,
					expected_root.clone(),
					computed_root,
				).into());
			}
		}

		// also check that there are no more additional expected roots.
		if let Some((next_target, _)) = expected_egress_roots.next() {
			return Err(ErrorKind::MissingEgressRoute(None, Some(*next_target)).into());
		}
	}

	Ok(Extrinsic { outgoing_messages: outgoing })
}

struct Externalities {
	parachain_index: ParaId,
	outgoing: Vec<OutgoingMessage>,
}

impl wasm_executor::Externalities for Externalities {
	fn post_message(&mut self, message: MessageRef) -> Result<(), ExternalitiesError> {
		// TODO: https://github.com/paritytech/polkadot/issues/92
		// check per-message and per-byte fees for the parachain.
		let target: ParaId = message.target.into();
		if target == self.parachain_index {
			return Err(ExternalitiesError::CannotPostMessage("posted message to self"));
		}

		self.outgoing.push(OutgoingMessage {
			target,
			data: message.data.to_vec(),
		});

		Ok(())
	}
}

impl Externalities {
	// Performs final checks of validity, producing the extrinsic data.
	fn final_checks(
		self,
		candidate: &CandidateReceipt,
	) -> Result<Extrinsic, Error> {
		check_extrinsic(
			self.outgoing,
			&candidate.egress_queue_roots[..],
		)
	}
}

/// Check whether a given collation is valid. Returns `Ok` on success, error otherwise.
///
/// This assumes that basic validity checks have been done:
///   - Block data hash is the same as linked in candidate receipt.
///   - incoming messages have been validated against canonical ingress roots
pub fn validate_collation<P>(
	client: &P,
	relay_parent: &BlockId,
	collation: &Collation,
	incoming: &Incoming,
) -> Result<Extrinsic, Error> where
	P: ProvideRuntimeApi,
	P::Api: ParachainHost<Block>,
{
	use parachain::{IncomingMessage, ValidationParams};

	let api = client.runtime_api();
	let para_id = collation.receipt.parachain_index;
	let validation_code = api.parachain_code(relay_parent, para_id)?
		.ok_or_else(|| ErrorKind::InactiveParachain(para_id))?;

	let chain_head = api.parachain_head(relay_parent, para_id)?
		.ok_or_else(|| ErrorKind::InactiveParachain(para_id))?;

	let params = ValidationParams {
		parent_head: chain_head,
		block_data: collation.block_data.0.clone(),
		ingress: incoming.iter()
			.flat_map(|&(source, ref messages)| {
				messages.iter().map(move |msg| IncomingMessage {
					source,
					data: msg.0.clone(),
				})
			})
			.collect()
	};

	let mut ext = Externalities {
		parachain_index: collation.receipt.parachain_index.clone(),
		outgoing: Vec::new(),
	};

	match wasm_executor::validate_candidate(&validation_code, params, &mut ext) {
		Ok(result) => {
			if result.head_data == collation.receipt.head_data.0 {
				ext.final_checks(&collation.receipt)
			} else {
				Err(ErrorKind::WrongHeadData(
					collation.receipt.head_data.0.clone(),
					result.head_data
				).into())
			}
		}
		Err(e) => Err(e.into())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use parachain::wasm_executor::Externalities as ExternalitiesTrait;

	#[test]
	fn compute_and_check_egress() {
		let messages = vec![
			OutgoingMessage { target: 3.into(), data: vec![1, 1, 1] },
			OutgoingMessage { target: 1.into(), data: vec![1, 2, 3] },
			OutgoingMessage { target: 2.into(), data: vec![4, 5, 6] },
			OutgoingMessage { target: 1.into(), data: vec![7, 8, 9] },
		];

		let root_1 = message_queue_root(&[vec![1, 2, 3], vec![7, 8, 9]]);
		let root_2 = message_queue_root(&[vec![4, 5, 6]]);
		let root_3 = message_queue_root(&[vec![1, 1, 1]]);

		assert!(check_extrinsic(
			messages.clone(),
			&[(1.into(), root_1), (2.into(), root_2), (3.into(), root_3)],
		).is_ok());

		let egress_roots = egress_roots(messages.clone());

		assert!(check_extrinsic(
			messages.clone(),
			&egress_roots[..],
		).is_ok());

		// missing root.
		assert!(check_extrinsic(
			messages.clone(),
			&[(1.into(), root_1), (3.into(), root_3)],
		).is_err());

		// extra root.
		assert!(check_extrinsic(
			messages.clone(),
			&[(1.into(), root_1), (2.into(), root_2), (3.into(), root_3), (4.into(), Default::default())],
		).is_err());

		// root mismatch.
		assert!(check_extrinsic(
			messages.clone(),
			&[(1.into(), root_2), (2.into(), root_1), (3.into(), root_3)],
		).is_err());
	}

	#[test]
	fn ext_rejects_local_message() {
		let mut ext = Externalities {
			parachain_index: 5.into(),
			outgoing: Vec::new(),
		};

		assert!(ext.post_message(MessageRef { target: 1.into(), data: &[] }).is_ok());
		assert!(ext.post_message(MessageRef { target: 5.into(), data: &[] }).is_err());
	}
}
