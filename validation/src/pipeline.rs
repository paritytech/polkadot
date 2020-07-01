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

use codec::Encode;
use polkadot_erasure_coding as erasure;
use polkadot_primitives::parachain::{
	CollationInfo, PoVBlock, LocalValidationData, GlobalValidationSchedule, OmittedValidationData,
	AvailableData, FeeSchedule, CandidateCommitments, ErasureChunk, ParachainHost,
	Id as ParaId, AbridgedCandidateReceipt, ValidationCode,
};
use polkadot_primitives::{Block, BlockId, Balance, Hash};
use parachain::{
	wasm_executor::{self, ExecutionMode},
	primitives::{UpwardMessage, ValidationParams},
};
use runtime_primitives::traits::{BlakeTwo256, Hash as HashT};
use sp_api::ProvideRuntimeApi;
use crate::Error;

pub use parachain::wasm_executor::ValidationPool;

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
	local_validation: &'a LocalValidationData,
	upward_messages: Vec<UpwardMessage>,
	fees: Balance,
	processed_downward_messages: u32,
}

impl<'a> ValidatedCandidate<'a> {
	/// Fully-compute the commitments and outputs of the candidate. Provide the number
	/// of validators. This computes the erasure-coding.
	pub fn full_output(self, n_validators: usize) -> Result<FullOutput, Error> {
		let ValidatedCandidate {
			pov_block,
			global_validation,
			local_validation,
			upward_messages,
			fees,
			processed_downward_messages,
		} = self;

		let omitted_validation = OmittedValidationData {
			global_validation: global_validation.clone(),
			local_validation: local_validation.clone(),
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
			new_validation_code: None,
			processed_downward_messages,
		};

		Ok(FullOutput {
			available_data,
			commitments,
			erasure_chunks: chunks,
			n_validators,
		})
	}
}

/// Validate that the given `UpwardMessage`s are covered by the given `free_balance`.
///
/// Will return an error if the `free_balance` does not cover the required fees to the
/// given `msgs`. On success it returns the fees that need to be charged for the `msgs`.
fn validate_upward_messages(
	msgs: &[UpwardMessage],
	fee_schedule: FeeSchedule,
	free_balance: Balance,
) -> Result<Balance, Error> {
	msgs.iter().try_fold(Balance::from(0u128), |fees_charged, msg| {
		let fees = fee_schedule.compute_message_fee(msg.data.len());
		let fees_charged = fees_charged.saturating_add(fees);

		if fees_charged > free_balance {
			Err(Error::CouldNotCoverFee)
		} else {
			Ok(fees_charged)
		}
	})
}

/// Does full checks of a collation, with provided PoV-block and contextual data.
pub fn validate<'a>(
	validation_pool: Option<&'_ ValidationPool>,
	collation: &'a CollationInfo,
	pov_block: &'a PoVBlock,
	local_validation: &'a LocalValidationData,
	global_validation: &'a GlobalValidationSchedule,
	validation_code: &ValidationCode,
) -> Result<ValidatedCandidate<'a>, Error> {
	if collation.head_data.0.len() > global_validation.max_head_data_size as _ {
		return Err(Error::HeadDataTooLarge(
			collation.head_data.0.len(),
			global_validation.max_head_data_size as _,
		));
	}

	let params = ValidationParams {
		parent_head: local_validation.parent_head.clone(),
		block_data: pov_block.block_data.clone(),
		max_code_size: global_validation.max_code_size,
		max_head_data_size: global_validation.max_head_data_size,
		relay_chain_height: global_validation.block_number,
		code_upgrade_allowed: local_validation.code_upgrade_allowed,
	};

	// TODO: remove when ext does not do this.
	let fee_schedule = FeeSchedule {
		base: 0,
		per_byte: 0,
	};

	let execution_mode = validation_pool
		.map(ExecutionMode::Remote)
		.unwrap_or(ExecutionMode::Local);

	match wasm_executor::validate_candidate(
		&validation_code.0,
		params,
		execution_mode,
	) {
		Ok(result) => {
			if result.head_data == collation.head_data {
				let fees = validate_upward_messages(
					&result.upward_messages,
					fee_schedule,
					local_validation.balance,
				)?;

				Ok(ValidatedCandidate {
					pov_block,
					global_validation,
					local_validation,
					upward_messages: result.upward_messages,
					fees,
					processed_downward_messages: result.processed_downward_messages,
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
	-> Result<(LocalValidationData, GlobalValidationSchedule, ValidationCode), Error>
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
	validation_pool: Option<&ValidationPool>,
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
		.and_then(|()| {
			let res = validate(
				validation_pool,
				&collation,
				&pov_block,
				&local_validation,
				&global_validation,
				&validation_code,
			);

			match res {
				Err(ref err) => log::debug!(
					target: "validation",
					"Failed to validate PoVBlock for parachain ({}): {:?}",
					para_id,
					err,
				),
				Ok(_) => log::debug!(
					target: "validation",
					"Successfully validated PoVBlock for parachain ({}).",
					para_id,
				),
			}

			res
		})
		.and_then(|validated| validated.full_output(n_validators))
}

#[cfg(test)]
mod tests {
	use super::*;
	use parachain::primitives::ParachainDispatchOrigin;

	fn add_msg(size: usize, msgs: &mut Vec<UpwardMessage>) {
		let msg = UpwardMessage { data: vec![0; size], origin: ParachainDispatchOrigin::Parachain };
		msgs.push(msg);
	}

	#[test]
	fn validate_upward_messages_works() {
		let fee_schedule = FeeSchedule {
			base: 1000,
			per_byte: 10,
		};
		let free_balance = 1_000_000;
		let mut msgs = Vec::new();

		add_msg(100, &mut msgs);
		assert_eq!(2000, validate_upward_messages(&msgs, fee_schedule, free_balance).unwrap());

		add_msg(100, &mut msgs);
		assert_eq!(4000, validate_upward_messages(&msgs, fee_schedule, free_balance).unwrap());

		add_msg((1_000_000 - 4000 - 1000) / 10, &mut msgs);
		assert_eq!(1_000_000, validate_upward_messages(&msgs, fee_schedule, free_balance).unwrap());

		// cannot pay fee.
		add_msg(1, &mut msgs);
		let err = validate_upward_messages(&msgs, fee_schedule, free_balance).unwrap_err();
		assert!(matches!(err, Error::CouldNotCoverFee));
	}
}
