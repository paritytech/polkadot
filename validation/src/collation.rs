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

use polkadot_primitives::{Block, Hash, BlockId, Balance, parachain::{
	CollatorId, ConsolidatedIngress, StructuredUnroutedIngress, CandidateReceipt, ParachainHost,
	Id as ParaId, Collation, TargetedMessage, OutgoingMessages, UpwardMessage, FeeSchedule,
}};
use runtime_primitives::traits::ProvideRuntimeApi;
use parachain::{wasm_executor::{self, ExternalitiesError, ExecutionMode}, MessageRef, UpwardMessageRef};
use trie::TrieConfiguration;
use futures::prelude::*;
use log::debug;

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

	/// Note a bad collator. TODO: take proof (https://github.com/paritytech/polkadot/issues/217)
	fn note_bad_collator(&self, collator: CollatorId);
}

/// A future which resolves when a collation is available.
///
/// This future is fused.
pub struct CollationFetch<C: Collators, P> {
	parachain: ParaId,
	relay_parent_hash: Hash,
	relay_parent: BlockId,
	collators: C,
	live_fetch: Option<<C::Collation as IntoFuture>::Future>,
	client: Arc<P>,
	max_block_data_size: Option<u64>,
}

impl<C: Collators, P> CollationFetch<C, P> {
	/// Create a new collation fetcher for the given chain.
	pub fn new(
		parachain: ParaId,
		relay_parent_hash: Hash,
		collators: C,
		client: Arc<P>,
		max_block_data_size: Option<u64>,
	) -> Self {
		CollationFetch {
			relay_parent: BlockId::hash(relay_parent_hash),
			relay_parent_hash,
			collators,
			client,
			parachain,
			live_fetch: None,
			max_block_data_size,
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
	type Item = (Collation, OutgoingMessages);
	type Error = C::Error;

	fn poll(&mut self) -> Poll<(Collation, OutgoingMessages), C::Error> {
		loop {
			let collation = {
				let parachain = self.parachain.clone();
				let (r, c)  = (self.relay_parent_hash, &self.collators);
				let poll = self.live_fetch
					.get_or_insert_with(move || c.collate(parachain, r).into_future())
					.poll();

				futures::try_ready!(poll)
			};

			let res = validate_collation(&*self.client, &self.relay_parent, &collation, self.max_block_data_size);

			match res {
				Ok(e) => {
					return Ok(Async::Ready((collation, e)))
				}
				Err(e) => {
					debug!("Failed to validate parachain due to API error: {}", e);

					// just continue if we got a bad collation or failed to validate
					self.live_fetch = None;
					self.collators.note_bad_collator(collation.receipt.collator)
				}
			}
		}
	}
}

// Errors that can occur when validating a parachain.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Client error
	Client(client::error::Error),
	/// Wasm validation error
	WasmValidation(wasm_executor::Error),
	/// Collated for inactive parachain
	#[display(fmt = "Collated for inactive parachain: {:?}", _0)]
	InactiveParachain(ParaId),
	/// Unexpected egress root
	#[display(fmt = "Got unexpected egress root to {:?}. (expected: {:?}, got {:?})", id, expected, got)]
	EgressRootMismatch { id: ParaId, expected: Hash, got: Hash },
	/// Unexpected ingress root
	#[display(fmt = "Got unexpected ingress root to {:?}. (expected: {:?}, got {:?})", id, expected, got)]
	IngressRootMismatch { id: ParaId, expected: Hash, got: Hash },
	/// Ingress from wrong chain
	#[display(fmt = "Got ingress from wrong chain. (expected: {:?}, got {:?})", expected, got)]
	IngressChainMismatch { expected: ParaId, got: ParaId },
	/// Ingress canonicality mismatch
	#[display(fmt = "Got data for {} roots, expected {}", expected, got)]
	IngressCanonicalityMismatch { expected: usize, got: usize },
	/// Missing or extra egress root
	#[display(fmt = "Missing or extra egress root. (expected: {:?}, got {:?})", expected, got)]
	MissingEgressRoot { expected: Option<ParaId>, got: Option<ParaId>, },
	/// Parachain validation produced wrong head data
	#[display(fmt = "Parachain validation produced wrong head data (expected: {:?}, got {:?})", expected, got)]
	WrongHeadData { expected: Vec<u8>, got: Vec<u8> },
	/// Block data is too big
	#[display(fmt = "Block data is too big (maximum allowed size: {}, actual size: {})", size, max_size)]
	BlockDataTooBig { size: u64, max_size: u64 },
	/// Parachain validation produced wrong relay-chain messages
	#[display(fmt = "Parachain validation produced wrong relay-chain messages (expected: {:?}, got {:?})", expected, got)]
	UpwardMessagesInvalid { expected: Vec<UpwardMessage>, got: Vec<UpwardMessage> },
	/// Parachain validation produced wrong fees to charge to parachain.
	#[display(fmt = "Parachain validation produced wrong relay-chain fees (expected: {:?}, got {:?})", expected, got)]
	FeesChargedInvalid { expected: Balance, got: Balance },
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Error::Client(ref err) => Some(err),
			Error::WasmValidation(ref err) => Some(err),
			_ => None,
		}
	}
}

/// Compute a trie root for a set of messages, given the raw message data.
pub fn message_queue_root<A, I: IntoIterator<Item=A>>(messages: I) -> Hash
	where A: AsRef<[u8]>
{
	trie::trie_types::Layout::<primitives::Blake2Hasher>::ordered_trie_root(messages)
}

/// Compute the set of egress roots for all given outgoing messages.
pub fn egress_roots(outgoing: &mut [TargetedMessage]) -> Vec<(ParaId, Hash)> {
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

fn check_egress(
	mut outgoing: Vec<TargetedMessage>,
	expected_egress_roots: &[(ParaId, Hash)],
) -> Result<OutgoingMessages, Error> {
	// stable sort messages by parachain ID.
	outgoing.sort_by_key(|msg| ParaId::from(msg.target));

	{
		let mut messages_iter = outgoing.iter().peekable();
		let mut expected_egress_roots = expected_egress_roots.iter();
		while let Some(batch_target) = messages_iter.peek().map(|o| o.target) {
			let expected_root = match expected_egress_roots.next() {
				None => return Err(Error::MissingEgressRoot {
					expected: Some(batch_target),
					got: None
				}),
				Some(&(id, ref root)) => if id == batch_target {
					root
				} else {
					return Err(Error::MissingEgressRoot{
						expected: Some(batch_target),
						got: Some(id)
					});
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
				return Err(Error::EgressRootMismatch {
					id: batch_target,
					expected: expected_root.clone(),
					got: computed_root,
				});
			}
		}

		// also check that there are no more additional expected roots.
		if let Some((next_target, _)) = expected_egress_roots.next() {
			return Err(Error::MissingEgressRoot { expected: None, got: Some(*next_target) });
		}
	}

	Ok(OutgoingMessages { outgoing_messages: outgoing })
}

struct Externalities {
	parachain_index: ParaId,
	outgoing: Vec<TargetedMessage>,
	upward: Vec<UpwardMessage>,
	fees_charged: Balance,
	free_balance: Balance,
	fee_schedule: FeeSchedule,
}

impl wasm_executor::Externalities for Externalities {
	fn post_message(&mut self, message: MessageRef) -> Result<(), ExternalitiesError> {
		let target: ParaId = message.target.into();
		if target == self.parachain_index {
			return Err(ExternalitiesError::CannotPostMessage("posted message to self"));
		}

		self.apply_message_fee(message.data.len())?;
		self.outgoing.push(TargetedMessage {
			target,
			data: message.data.to_vec(),
		});

		Ok(())
	}

	fn post_upward_message(&mut self, message: UpwardMessageRef)
		-> Result<(), ExternalitiesError>
	{
		self.apply_message_fee(message.data.len())?;

		self.upward.push(UpwardMessage {
			origin: message.origin,
			data: message.data.to_vec(),
		});
		Ok(())
	}
}

impl Externalities {
	fn apply_message_fee(&mut self, message_len: usize) -> Result<(), ExternalitiesError> {
		let fee = self.fee_schedule.compute_fee(message_len);
		let new_fees_charged = self.fees_charged.saturating_add(fee);
		if new_fees_charged > self.free_balance {
			Err(ExternalitiesError::CannotPostMessage("could not cover fee."))
		} else {
			self.fees_charged = new_fees_charged;
			Ok(())
		}
	}

	// Performs final checks of validity, producing the outgoing message data.
	fn final_checks(
		self,
		candidate: &CandidateReceipt,
	) -> Result<OutgoingMessages, Error> {
		if &self.upward != &candidate.upward_messages {
			return Err(Error::UpwardMessagesInvalid {
				expected: candidate.upward_messages.clone(),
				got: self.upward.clone(),
			});
		}

		if self.fees_charged != candidate.fees {
			return Err(Error::FeesChargedInvalid {
				expected: candidate.fees.clone(),
				got: self.fees_charged.clone(),
			});
		}

		check_egress(
			self.outgoing,
			&candidate.egress_queue_roots[..],
		)
	}
}

/// Validate incoming messages against expected roots.
pub fn validate_incoming(
	roots: &StructuredUnroutedIngress,
	ingress: &ConsolidatedIngress,
) -> Result<(), Error> {
	if roots.len() != ingress.0.len() {
		return Err(Error::IngressCanonicalityMismatch {
			expected: roots.0.len(),
			got: ingress.0.len()
		});
	}

	let all_iter = roots.iter().zip(&ingress.0);
	for ((_, expected_from, root), (got_id, messages)) in all_iter {
		if expected_from != got_id {
			return Err(Error::IngressChainMismatch {
				expected: *expected_from,
				got: *got_id
			});
		}

		let got_root = message_queue_root(messages.iter().map(|msg| &msg.0[..]));
		if &got_root != root {
			return Err(Error::IngressRootMismatch{
				id: *expected_from,
				expected: *root,
				got: got_root
			});
		}
	}

	Ok(())
}

/// Check whether a given collation is valid. Returns `Ok` on success, error otherwise.
///
/// This assumes that basic validity checks have been done:
///   - Block data hash is the same as linked in candidate receipt.
pub fn validate_collation<P>(
	client: &P,
	relay_parent: &BlockId,
	collation: &Collation,
	max_block_data_size: Option<u64>,
) -> Result<OutgoingMessages, Error> where
	P: ProvideRuntimeApi,
	P::Api: ParachainHost<Block>,
{
	use parachain::{IncomingMessage, ValidationParams};

	if let Some(max_size) = max_block_data_size {
		let block_data_size = collation.pov.block_data.0.len() as u64;
		if block_data_size > max_size {
			return Err(Error::BlockDataTooBig { size: block_data_size, max_size });
		}
	}

	let api = client.runtime_api();
	let para_id = collation.receipt.parachain_index;
	let validation_code = api.parachain_code(relay_parent, para_id)?
		.ok_or_else(|| Error::InactiveParachain(para_id))?;

	let chain_status = api.parachain_status(relay_parent, para_id)?
		.ok_or_else(|| Error::InactiveParachain(para_id))?;

	let roots = api.ingress(relay_parent, para_id, None)?
		.ok_or_else(|| Error::InactiveParachain(para_id))?;

	validate_incoming(&roots, &collation.pov.ingress)?;

	let params = ValidationParams {
		parent_head: chain_status.head_data.0,
		block_data: collation.pov.block_data.0.clone(),
		ingress: collation.pov.ingress.0.iter()
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
		upward: Vec::new(),
		free_balance: chain_status.balance,
		fee_schedule: chain_status.fee_schedule,
		fees_charged: 0,
	};

	match wasm_executor::validate_candidate(&validation_code, params, &mut ext, ExecutionMode::Remote) {
		Ok(result) => {
			if result.head_data == collation.receipt.head_data.0 {
				ext.final_checks(&collation.receipt)
			} else {
				Err(Error::WrongHeadData {
					expected: collation.receipt.head_data.0.clone(),
					got: result.head_data
				})
			}
		}
		Err(e) => Err(e.into())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use parachain::wasm_executor::Externalities as ExternalitiesTrait;
	use parachain::ParachainDispatchOrigin;
	use polkadot_primitives::parachain::{CandidateReceipt, HeadData};

	#[test]
	fn compute_and_check_egress() {
		let messages = vec![
			TargetedMessage { target: 3.into(), data: vec![1, 1, 1] },
			TargetedMessage { target: 1.into(), data: vec![1, 2, 3] },
			TargetedMessage { target: 2.into(), data: vec![4, 5, 6] },
			TargetedMessage { target: 1.into(), data: vec![7, 8, 9] },
		];

		let root_1 = message_queue_root(&[vec![1, 2, 3], vec![7, 8, 9]]);
		let root_2 = message_queue_root(&[vec![4, 5, 6]]);
		let root_3 = message_queue_root(&[vec![1, 1, 1]]);

		assert!(check_egress(
			messages.clone(),
			&[(1.into(), root_1), (2.into(), root_2), (3.into(), root_3)],
		).is_ok());

		let egress_roots = egress_roots(&mut messages.clone()[..]);

		assert!(check_egress(
			messages.clone(),
			&egress_roots[..],
		).is_ok());

		// missing root.
		assert!(check_egress(
			messages.clone(),
			&[(1.into(), root_1), (3.into(), root_3)],
		).is_err());

		// extra root.
		assert!(check_egress(
			messages.clone(),
			&[(1.into(), root_1), (2.into(), root_2), (3.into(), root_3), (4.into(), Default::default())],
		).is_err());

		// root mismatch.
		assert!(check_egress(
			messages.clone(),
			&[(1.into(), root_2), (2.into(), root_1), (3.into(), root_3)],
		).is_err());
	}

	#[test]
	fn ext_rejects_local_message() {
		let mut ext = Externalities {
			parachain_index: 5.into(),
			outgoing: Vec::new(),
			upward: Vec::new(),
			fees_charged: 0,
			free_balance: 1_000_000,
			fee_schedule: FeeSchedule {
				base: 1000,
				per_byte: 10,
			},
		};

		assert!(ext.post_message(MessageRef { target: 1.into(), data: &[] }).is_ok());
		assert!(ext.post_message(MessageRef { target: 5.into(), data: &[] }).is_err());
	}

	#[test]
	fn ext_checks_upward_messages() {
		let ext = || Externalities {
			parachain_index: 5.into(),
			outgoing: Vec::new(),
			upward: vec![
				UpwardMessage{ data: vec![42], origin: ParachainDispatchOrigin::Parachain },
			],
			fees_charged: 0,
			free_balance: 1_000_000,
			fee_schedule: FeeSchedule {
				base: 1000,
				per_byte: 10,
			},
		};
		let receipt = CandidateReceipt {
			parachain_index: 5.into(),
			collator: Default::default(),
			signature: Default::default(),
			head_data: HeadData(Vec::new()),
			egress_queue_roots: Vec::new(),
			fees: 0,
			block_data_hash: Default::default(),
			upward_messages: vec![
				UpwardMessage{ data: vec![42], origin: ParachainDispatchOrigin::Signed },
				UpwardMessage{ data: vec![69], origin: ParachainDispatchOrigin::Parachain },
			],
		};
		assert!(ext().final_checks(&receipt).is_err());
		let receipt = CandidateReceipt {
			parachain_index: 5.into(),
			collator: Default::default(),
			signature: Default::default(),
			head_data: HeadData(Vec::new()),
			egress_queue_roots: Vec::new(),
			fees: 0,
			block_data_hash: Default::default(),
			upward_messages: vec![
				UpwardMessage{ data: vec![42], origin: ParachainDispatchOrigin::Signed },
			],
		};
		assert!(ext().final_checks(&receipt).is_err());
		let receipt = CandidateReceipt {
			parachain_index: 5.into(),
			collator: Default::default(),
			signature: Default::default(),
			head_data: HeadData(Vec::new()),
			egress_queue_roots: Vec::new(),
			fees: 0,
			block_data_hash: Default::default(),
			upward_messages: vec![
				UpwardMessage{ data: vec![69], origin: ParachainDispatchOrigin::Parachain },
			],
		};
		assert!(ext().final_checks(&receipt).is_err());
		let receipt = CandidateReceipt {
			parachain_index: 5.into(),
			collator: Default::default(),
			signature: Default::default(),
			head_data: HeadData(Vec::new()),
			egress_queue_roots: Vec::new(),
			fees: 0,
			block_data_hash: Default::default(),
			upward_messages: vec![
				UpwardMessage{ data: vec![42], origin: ParachainDispatchOrigin::Parachain },
			],
		};
		assert!(ext().final_checks(&receipt).is_ok());
	}

	#[test]
	fn ext_checks_fees_and_updates_correctly() {
		let mut ext = Externalities {
			parachain_index: 5.into(),
			outgoing: Vec::new(),
			upward: vec![
				UpwardMessage{ data: vec![42], origin: ParachainDispatchOrigin::Parachain },
			],
			fees_charged: 0,
			free_balance: 1_000_000,
			fee_schedule: FeeSchedule {
				base: 1000,
				per_byte: 10,
			},
		};

		ext.apply_message_fee(100).unwrap();
		assert_eq!(ext.fees_charged, 2000);

		ext.post_message(MessageRef {
			target: 1.into(),
			data: &[0u8; 100],
		}).unwrap();
		assert_eq!(ext.fees_charged, 4000);

		ext.post_upward_message(UpwardMessageRef {
			origin: ParachainDispatchOrigin::Signed,
			data: &[0u8; 100],
		}).unwrap();
		assert_eq!(ext.fees_charged, 6000);


		ext.apply_message_fee((1_000_000 - 6000 - 1000) / 10).unwrap();
		assert_eq!(ext.fees_charged, 1_000_000);

		// cannot pay fee.
		assert!(ext.apply_message_fee(1).is_err());
	}
}
