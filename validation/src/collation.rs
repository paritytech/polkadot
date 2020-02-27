// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

use polkadot_primitives::{
	BlakeTwo256, Block, Hash, HashT, BlockId, Balance,
	parachain::{
		CollatorId, CandidateReceipt, CollationInfo,
		ParachainHost, Id as ParaId, Collation, FeeSchedule, ErasureChunk,
		HeadData, PoVBlock,
	},
};
use polkadot_erasure_coding as erasure;
use sp_api::ProvideRuntimeApi;
use parachain::{
	wasm_executor::{self, ExecutionMode}, UpwardMessage,
};
use trie::TrieConfiguration;
use futures::prelude::*;
use log::debug;
use parking_lot::Mutex;

/// Encapsulates connections to collators and allows collation on any parachain.
///
/// This is expected to be a lightweight, shared type like an `Arc`.
pub trait Collators: Clone {
	/// Errors when producing collations.
	type Error: std::fmt::Debug;
	/// A full collation.
	type Collation: Future<Output=Result<Collation, Self::Error>>;

	/// Collate on a specific parachain, building on a given relay chain parent hash.
	///
	/// The returned collation should be checked for basic validity in the signature
	/// and will be checked for state-transition validity by the consumer of this trait.
	///
	/// This does not have to guarantee local availability, as a valid collation
	/// will be passed to the `TableRouter` instance.
	///
	/// The returned future may be prematurely concluded if the `relay_parent` goes
	/// out of date.
	fn collate(&self, parachain: ParaId, relay_parent: Hash) -> Self::Collation;

	/// Note a bad collator. TODO: take proof (https://github.com/paritytech/polkadot/issues/217)
	fn note_bad_collator(&self, collator: CollatorId);
}

/// A future which resolves when a collation is available.
pub async fn collation_fetch<C: Collators, P>(
	parachain: ParaId,
	relay_parent_hash: Hash,
	collators: C,
	client: Arc<P>,
	max_block_data_size: Option<u64>,
) -> Result<(Collation, HeadData, Balance),C::Error>
	where
		P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
		C: Collators + Unpin,
		P: ProvideRuntimeApi<Block>,
		<C as Collators>::Collation: Unpin,
{
	let relay_parent = BlockId::hash(relay_parent_hash);

	loop {
		let collation = collators.collate(parachain, relay_parent_hash)
			.await?;

		let res = validate_collation(
			&*client,
			&relay_parent,
			&collation,
			max_block_data_size,
		);

		match res {
			Ok((parent_head, fees)) => {
				return Ok((collation, parent_head, fees))
			}
			Err(e) => {
				debug!("Failed to validate parachain due to API error: {}", e);

				// just continue if we got a bad collation or failed to validate
				collators.note_bad_collator(collation.info.collator)
			}
		}
	}
}

// Errors that can occur when validating a parachain.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
	/// Client error
	Client(sp_blockchain::Error),
	/// Wasm validation error
	WasmValidation(wasm_executor::Error),
	/// Erasure-encoding error.
	Erasure(erasure::Error),
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
	/// Candidate block has an erasure-encoded root that mismatches the actual
	/// erasure-encoded root of block data and extrinsics.
	#[display(fmt = "Got unexpected erasure root (expected: {:?}, got {:?})", expected, got)]
	ErasureRootMismatch { expected: Hash, got: Hash },
	/// Candidate block collation info doesn't match candidate receipt.
	#[display(fmt = "Got receipt mismatch for candidate {:?}", candidate)]
	CandidateReceiptMismatch { candidate: Hash },
	/// The parent header given in the candidate did not match current relay-chain
	/// state.
	#[display(fmt = "Got unexpected parachain parent.")]
	ParentMismatch { expected: HeadData, got: HeadData },
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

struct ExternalitiesInner {
	upward: Vec<UpwardMessage>,
	fees_charged: Balance,
	free_balance: Balance,
	fee_schedule: FeeSchedule,
}

impl wasm_executor::Externalities for ExternalitiesInner {
	fn post_upward_message(&mut self, message: UpwardMessage) -> Result<(), String> {
		self.apply_message_fee(message.data.len())?;

		self.upward.push(message);

		Ok(())
	}
}

impl ExternalitiesInner {
	fn new(free_balance: Balance, fee_schedule: FeeSchedule) -> Self {
		Self {
			free_balance,
			fee_schedule,
			fees_charged: 0,
			upward: Vec::new(),
		}
	}

	fn apply_message_fee(&mut self, message_len: usize) -> Result<(), String> {
		let fee = self.fee_schedule.compute_fee(message_len);
		let new_fees_charged = self.fees_charged.saturating_add(fee);
		if new_fees_charged > self.free_balance {
			Err("could not cover fee.".into())
		} else {
			self.fees_charged = new_fees_charged;
			Ok(())
		}
	}

	// Performs final checks of validity, producing the outgoing message data.
	fn final_checks(
		&mut self,
		upward_messages: &[UpwardMessage],
		fees_charged: Option<Balance>,
	) -> Result<Balance, Error> {
		if self.upward != upward_messages {
			return Err(Error::UpwardMessagesInvalid {
				expected: upward_messages.to_vec(),
				got: self.upward.clone(),
			});
		}

		if let Some(fees_charged) = fees_charged {
			if self.fees_charged != fees_charged {
				return Err(Error::FeesChargedInvalid {
					expected: fees_charged.clone(),
					got: self.fees_charged.clone(),
				});
			}
		}


		Ok(self.fees_charged)
	}
}

#[derive(Clone)]
struct Externalities(Arc<Mutex<ExternalitiesInner>>);

impl Externalities {
	fn new(free_balance: Balance, fee_schedule: FeeSchedule) -> Self {
		Self(Arc::new(Mutex::new(
			ExternalitiesInner::new(free_balance, fee_schedule)
		)))
	}
}

impl wasm_executor::Externalities for Externalities {
	fn post_upward_message(&mut self, message: UpwardMessage) -> Result<(), String> {
		self.0.lock().post_upward_message(message)
	}
}

/// Validate an erasure chunk against an expected root.
pub fn validate_chunk(
	root: &Hash,
	chunk: &ErasureChunk,
) -> Result<(), Error> {
	let expected = erasure::branch_hash(root, &chunk.proof, chunk.index as usize)?;
	let got = BlakeTwo256::hash(&chunk.chunk);

	if expected != got {
		return Err(Error::ErasureRootMismatch {
			expected,
			got,
		})
	}

	Ok(())
}

// A utility function that implements most of the collation validation logic.
//
// Reused by `validate_collation` and `validate_receipt`.
// Returns outgoing messages, parent nead data, and fees charged for later reuse.
fn do_validation<P>(
	client: &P,
	relay_parent: &BlockId,
	pov_block: &PoVBlock,
	para_id: ParaId,
	max_block_data_size: Option<u64>,
	fees_charged: Option<Balance>,
	head_data: &HeadData,
	upward_messages: &Vec<UpwardMessage>,
) -> Result<(HeadData, Balance), Error> where
	P: ProvideRuntimeApi<Block>,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	use parachain::ValidationParams;

	if let Some(max_size) = max_block_data_size {
		let block_data_size = pov_block.block_data.0.len() as u64;
		if block_data_size > max_size {
			return Err(Error::BlockDataTooBig { size: block_data_size, max_size });
		}
	}

	let api = client.runtime_api();
	let validation_code = api.parachain_code(relay_parent, para_id)?
		.ok_or_else(|| Error::InactiveParachain(para_id))?;

	let chain_status = api.parachain_status(relay_parent, para_id)?
		.ok_or_else(|| Error::InactiveParachain(para_id))?;


	let params = ValidationParams {
		parent_head: chain_status.head_data.0.clone(),
		block_data: pov_block.block_data.0.clone(),
	};

	let ext = Externalities::new(chain_status.balance, chain_status.fee_schedule);

	match wasm_executor::validate_candidate(
		&validation_code,
		params,
		ext.clone(),
		ExecutionMode::Remote,
	) {
		Ok(result) => {
			if result.head_data == head_data.0 {
				let fees = ext.0.lock().final_checks(
					upward_messages,
					fees_charged
				)?;

				Ok((chain_status.head_data, fees))
			} else {
				Err(Error::WrongHeadData {
					expected: head_data.0.clone(),
					got: result.head_data
				})
			}
		}
		Err(e) => Err(e.into())
	}
}

/// Produce a `CandidateReceipt` and erasure encoding chunks with a given collation.
///
/// To produce a `CandidateReceipt` among other things the root of erasure encoding of
/// the block data and messages needs to be known. To avoid redundant re-computations
/// of erasure encoding this method creates an encoding and produces a candidate with
/// encoding's root returning both for re-use.
pub fn produce_receipt_and_chunks(
	n_validators: usize,
	parent_head: HeadData,
	pov: &PoVBlock,
	fees: Balance,
	info: &CollationInfo,
) -> Result<(CandidateReceipt, Vec<ErasureChunk>), Error>
{
	let erasure_chunks = erasure::obtain_chunks(
		n_validators,
		&pov.block_data,
	)?;

	let branches = erasure::branches(erasure_chunks.as_ref());
	let erasure_root = branches.root();

	let chunks: Vec<_> = erasure_chunks
			.iter()
			.zip(branches.map(|(proof, _)| proof))
			.enumerate()
			.map(|(index, (chunk, proof))| ErasureChunk {
				// branches borrows the original chunks, but this clone could probably be dodged.
				chunk: chunk.clone(),
				index: index as u32,
				proof,
			})
			.collect();

	let receipt = CandidateReceipt {
		parachain_index: info.parachain_index,
		collator: info.collator.clone(),
		signature: info.signature.clone(),
		head_data: info.head_data.clone(),
		parent_head,
		fees,
		block_data_hash: info.block_data_hash.clone(),
		upward_messages: info.upward_messages.clone(),
		erasure_root,
	};

	Ok((receipt, chunks))
}

/// Check if a given candidate receipt is valid with a given collation.
///
/// This assumes that basic validity checks have been done:
///   - Block data hash is the same as linked in collation info and a receipt.
pub fn validate_receipt<P>(
	client: &P,
	relay_parent: &BlockId,
	pov_block: &PoVBlock,
	receipt: &CandidateReceipt,
	max_block_data_size: Option<u64>,
) -> Result<Vec<ErasureChunk>, Error> where
	P: ProvideRuntimeApi<Block>,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	let (parent_head, _fees) = do_validation(
		client,
		relay_parent,
		pov_block,
		receipt.parachain_index,
		max_block_data_size,
		Some(receipt.fees),
		&receipt.head_data,
		&receipt.upward_messages,
	)?;

	if parent_head != receipt.parent_head {
		return Err(Error::ParentMismatch {
			expected: receipt.parent_head.clone(),
			got: parent_head,
		});
	}

	let api = client.runtime_api();
	let validators = api.validators(&relay_parent)?;
	let n_validators = validators.len();

	let (validated_receipt, chunks) = produce_receipt_and_chunks(
		n_validators,
		parent_head,
		pov_block,
		receipt.fees,
		&receipt.clone().into(),
	)?;

	if validated_receipt.erasure_root != receipt.erasure_root {
		return Err(Error::ErasureRootMismatch {
			expected: validated_receipt.erasure_root,
			got: receipt.erasure_root,
		});
	}

	Ok(chunks)
}

/// Check whether a given collation is valid. Returns `Ok` on success, error otherwise.
/// Returns outgoing messages, parent head-data, and fees.
///
/// This assumes that basic validity checks have been done:
///   - Block data hash is the same as linked in collation info.
pub fn validate_collation<P>(
	client: &P,
	relay_parent: &BlockId,
	collation: &Collation,
	max_block_data_size: Option<u64>,
) -> Result<(HeadData, Balance), Error> where
	P: ProvideRuntimeApi<Block>,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	let para_id = collation.info.parachain_index;

	debug!("Validating collation for parachain {} at relay parent: {}", para_id, relay_parent);

	do_validation(
		client,
		relay_parent,
		&collation.pov,
		para_id,
		max_block_data_size,
		None,
		&collation.info.head_data,
		&collation.info.upward_messages,
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use parachain::wasm_executor::Externalities as ExternalitiesTrait;
	use parachain::ParachainDispatchOrigin;
	use polkadot_primitives::parachain::{CandidateReceipt, HeadData};

	#[test]
	fn ext_checks_upward_messages() {
		let ext = || ExternalitiesInner {
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
			parent_head: HeadData(Vec::new()),
			fees: 0,
			block_data_hash: Default::default(),
			upward_messages: vec![
				UpwardMessage{ data: vec![42], origin: ParachainDispatchOrigin::Signed },
				UpwardMessage{ data: vec![69], origin: ParachainDispatchOrigin::Parachain },
			],
			erasure_root: [1u8; 32].into(),
		};
		assert!(ext().final_checks(
			&receipt.upward_messages,
			Some(receipt.fees),
		).is_err());
		let receipt = CandidateReceipt {
			parachain_index: 5.into(),
			collator: Default::default(),
			signature: Default::default(),
			head_data: HeadData(Vec::new()),
			parent_head: HeadData(Vec::new()),
			fees: 0,
			block_data_hash: Default::default(),
			upward_messages: vec![
				UpwardMessage{ data: vec![42], origin: ParachainDispatchOrigin::Signed },
			],
			erasure_root: [1u8; 32].into(),
		};
		assert!(ext().final_checks(
			&receipt.upward_messages,
			Some(receipt.fees),
		).is_err());
		let receipt = CandidateReceipt {
			parachain_index: 5.into(),
			collator: Default::default(),
			signature: Default::default(),
			head_data: HeadData(Vec::new()),
			parent_head: HeadData(Vec::new()),
			fees: 0,
			block_data_hash: Default::default(),
			upward_messages: vec![
				UpwardMessage{ data: vec![69], origin: ParachainDispatchOrigin::Parachain },
			],
			erasure_root: [1u8; 32].into(),
		};
		assert!(ext().final_checks(
			&receipt.upward_messages,
			Some(receipt.fees),
		).is_err());
		let receipt = CandidateReceipt {
			parachain_index: 5.into(),
			collator: Default::default(),
			signature: Default::default(),
			head_data: HeadData(Vec::new()),
			parent_head: HeadData(Vec::new()),
			fees: 0,
			block_data_hash: Default::default(),
			upward_messages: vec![
				UpwardMessage{ data: vec![42], origin: ParachainDispatchOrigin::Parachain },
			],
			erasure_root: [1u8; 32].into(),
		};
		assert!(ext().final_checks(
			&receipt.upward_messages,
			Some(receipt.fees),
		).is_ok());
	}

	#[test]
	fn ext_checks_fees_and_updates_correctly() {
		let mut ext = ExternalitiesInner {
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

		ext.post_upward_message(UpwardMessage {
			origin: ParachainDispatchOrigin::Signed,
			data: vec![0u8; 100],
		}).unwrap();
		assert_eq!(ext.fees_charged, 4000);

		ext.apply_message_fee((1_000_000 - 4000 - 1000) / 10).unwrap();
		assert_eq!(ext.fees_charged, 1_000_000);

		// cannot pay fee.
		assert!(ext.apply_message_fee(1).is_err());
	}
}
