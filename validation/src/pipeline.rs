// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! The pipeline of validation functions a parachain block must pass through before
//! it can be voted for.

use std::sync::Arc;

use codec::Encode;
use polkadot_erasure_coding as erasure;
use polkadot_primitives::parachain::{
	CollationInfo, PoVBlock, LocalValidationData, GlobalValidationSchedule, OmittedValidationData,
	AvailableData, FeeSchedule, CandidateCommitments, ErasureChunk, HeadData, ParachainHost,
	Id as ParaId, AbridgedCandidateReceipt,
};
use polkadot_primitives::{Block, BlockId, Balance, Hash};
use parachain::{
	wasm_executor::{self, ExecutionMode},
	UpwardMessage, ValidationParams,
};
use runtime_primitives::traits::{BlakeTwo256, Hash as HashT};
use sp_api::ProvideRuntimeApi;
use parking_lot::Mutex;
use crate::Error;

/// Does basic checks of a collation. Provide the encoded PoV-block.
pub fn basic_checks(
	collation: &CollationInfo,
	expected_relay_parent: &Hash,
	max_block_data_size: Option<u64>,
	encoded_pov: &[u8],
) -> Result<(), Error> {
	if &collation.relay_parent != expected_relay_parent {
		return Err(Error::DisallowedRelayParent(collation.relay_parent));
	}

	if let Some(max_size) = max_block_data_size {
		if encoded_pov.len() as u64 > max_size {
			return Err(Error::BlockDataTooBig { size: encoded_pov.len() as _, max_size });
		}
	}

	let hash = BlakeTwo256::hash(encoded_pov);
	if hash != collation.pov_block_hash {
		return Err(Error::PoVHashMismatch(collation.pov_block_hash, hash));
	}

	if let Err(()) = collation.check_signature() {
		return Err(Error::InvalidCollatorSignature);
	}

	Ok(())
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

	// Returns the noted outputs of execution so far - upward messages and balances.
	fn outputs(self) -> (Vec<UpwardMessage>, Balance) {
		(self.upward, self.fees_charged)
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

/// Data from a fully-outputted validation of a parachain candidate. This contains
/// all outputs and commitments of the validation as well as all additional data to make available.
pub struct FullOutput {
	/// Data about the candidate to keep available in the network.
	pub available_data: AvailableData,
	/// Commitments issued alongside the candidate to be placed on-chain.
	pub commitments: CandidateCommitments,
	/// All erasure-chunks associated with the available data. Each validator
	/// should keep their chunk (by index). Other chunks do not need to be
	/// kept available long-term, but should be distributed to other validators.
	pub erasure_chunks: Vec<ErasureChunk>,
	/// The number of validators that were present at this validation.
	pub n_validators: usize,
}

impl FullOutput {
	/// Check consistency of the outputs produced by the validation pipeline against
	/// data contained within a candidate receipt.
	pub fn check_consistency(&self, receipt: &AbridgedCandidateReceipt) -> Result<(), Error> {
		if self.commitments != receipt.commitments {
			Err(Error::CommitmentsMismatch)
		} else {
			Ok(())
		}
	}
}

/// The successful result of validating a collation. If the full commitments of the
/// validation are needed, call `full_output`. Otherwise, safely drop this value.
pub struct ValidatedCandidate<'a> {
	pov_block: &'a PoVBlock,
	global_validation: &'a GlobalValidationSchedule,
	parent_head: &'a HeadData,
	balance: Balance,
	upward_messages: Vec<UpwardMessage>,
	fees: Balance,
}

impl<'a> ValidatedCandidate<'a> {
	/// Fully-compute the commitments and outputs of the candidate. Provide the number
	/// of validators. This computes the erasure-coding.
	pub fn full_output(self, n_validators: usize) -> Result<FullOutput, Error> {
		let ValidatedCandidate {
			pov_block,
			global_validation,
			parent_head,
			balance,
			upward_messages,
			fees,
		} = self;

		let omitted_validation = OmittedValidationData {
			global_validation: global_validation.clone(),
			local_validation: LocalValidationData {
				parent_head: parent_head.clone(),
				balance,
			},
		};

		let available_data = AvailableData {
			pov_block: pov_block.clone(),
			omitted_validation,
		};

		let erasure_chunks = erasure::obtain_chunks(
			n_validators,
			&available_data,
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

		let commitments = CandidateCommitments {
			upward_messages,
			fees,
			erasure_root,
		};

		Ok(FullOutput {
			available_data,
			commitments,
			erasure_chunks: chunks,
			n_validators,
		})
	}
}

/// Does full checks of a collation, with provided PoV-block and contextual data.
pub fn validate<'a>(
	collation: &'a CollationInfo,
	pov_block: &'a PoVBlock,
	local_validation: &'a LocalValidationData,
	global_validation: &'a GlobalValidationSchedule,
	validation_code: &[u8],
) -> Result<ValidatedCandidate<'a>, Error> {
	if collation.head_data.0.len() > global_validation.max_head_data_size as _ {
		return Err(Error::HeadDataTooLarge(
			collation.head_data.0.len(),
			global_validation.max_head_data_size as _,
		));
	}

	let params = ValidationParams {
		parent_head: local_validation.parent_head.0.clone(),
		block_data: pov_block.block_data.0.clone(),
	};

	// TODO: remove when ext does not do this.
	let fee_schedule = FeeSchedule {
		base: 0,
		per_byte: 0,
	};

	let ext = Externalities::new(local_validation.balance, fee_schedule);
	match wasm_executor::validate_candidate(
		&validation_code,
		params,
		ext.clone(),
		ExecutionMode::Remote,
	) {
		Ok(result) => {
			if result.head_data == collation.head_data.0 {
				let (upward_messages, fees) = Arc::try_unwrap(ext.0)
					.map_err(|_| "<non-unique>")
					.expect("Wasm executor drops passed externalities on completion; \
						call has concluded; qed")
					.into_inner()
					.outputs();

				Ok(ValidatedCandidate {
					pov_block,
					global_validation,
					parent_head: &local_validation.parent_head,
					balance: local_validation.balance,
					upward_messages,
					fees,
				})
			} else {
				Err(Error::HeadDataMismatch)
			}
		}
		Err(e) => Err(e.into()),
	}
}

/// Extracts validation parameters from a Polkadot runtime API for a specific parachain.
pub fn validation_params<P>(api: &P, relay_parent: Hash, para_id: ParaId)
	-> Result<(LocalValidationData, GlobalValidationSchedule, Vec<u8>), Error>
where
	P: ProvideRuntimeApi<Block>,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	let api = api.runtime_api();
	let relay_parent = BlockId::hash(relay_parent);

	// fetch all necessary data from runtime.
	let local_validation = api.local_validation_data(&relay_parent, para_id)?
		.ok_or_else(|| Error::InactiveParachain(para_id))?;

	let global_validation = api.global_validation_schedule(&relay_parent)?;
	let validation_code = api.parachain_code(&relay_parent, para_id)?
		.ok_or_else(|| Error::InactiveParachain(para_id))?;

	Ok((local_validation, global_validation, validation_code))
}

/// Does full-pipeline validation of a collation with provided contextual parameters.
pub fn full_output_validation_with_api<P>(
	api: &P,
	collation: &CollationInfo,
	pov_block: &PoVBlock,
	expected_relay_parent: &Hash,
	max_block_data_size: Option<u64>,
	n_validators: usize,
) -> Result<FullOutput, Error> where
	P: ProvideRuntimeApi<Block>,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	let para_id = collation.parachain_index;
	let (local_validation, global_validation, validation_code)
		= validation_params(&*api, collation.relay_parent, para_id)?;

	// put the parameters through the validation pipeline, producing
	// erasure chunks.
	let encoded_pov = pov_block.encode();
	basic_checks(
		&collation,
		&expected_relay_parent,
		max_block_data_size,
		&encoded_pov,
	)
		.and_then(|()| validate(
			&collation,
			&pov_block,
			&local_validation,
			&global_validation,
			&validation_code,
		))
		.and_then(|validated| validated.full_output(n_validators))
}

#[cfg(test)]
mod tests {
	use super::*;
	use parachain::wasm_executor::Externalities as ExternalitiesTrait;
	use parachain::ParachainDispatchOrigin;

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
