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

use polkadot_erasure_coding as erasure;
use polkadot_primitives::parachain::{
	CollationInfo, PoVBlock, LocalValidationData, GlobalValidationSchedule, OmittedValidationData,
	AvailableData, FeeSchedule, CandidateCommitments, ErasureChunk, HeadData, Id as ParaId,
};
use polkadot_primitives::Balance;
use parachain::{
	wasm_executor::{self, ExecutionMode},
	TargetedMessage, UpwardMessage, ValidationParams,
};
use runtime_primitives::traits::{BlakeTwo256, Hash};
use parking_lot::Mutex;
use crate::Error;

/// Does basic checks of a collation. Provide the encoded PoV-block.
pub fn basic_checks(collation: &CollationInfo, encoded_pov: &[u8]) -> Result<(), Error> {
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
	parachain_index: ParaId,
	upward: Vec<UpwardMessage>,
	fees_charged: Balance,
	free_balance: Balance,
	fee_schedule: FeeSchedule,
}

impl wasm_executor::Externalities for ExternalitiesInner {
	fn post_message(&mut self, _message: TargetedMessage) -> Result<(), String> {
		Ok(())
	}

	fn post_upward_message(&mut self, message: UpwardMessage) -> Result<(), String> {
		self.apply_message_fee(message.data.len())?;

		self.upward.push(message);

		Ok(())
	}
}

impl ExternalitiesInner {
	fn new(parachain_index: ParaId, free_balance: Balance, fee_schedule: FeeSchedule) -> Self {
		Self {
			parachain_index,
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
	fn new(parachain_index: ParaId, free_balance: Balance, fee_schedule: FeeSchedule) -> Self {
		Self(Arc::new(Mutex::new(
			ExternalitiesInner::new(parachain_index, free_balance, fee_schedule)
		)))
	}
}

impl wasm_executor::Externalities for Externalities {
	fn post_message(&mut self, message: TargetedMessage) -> Result<(), String> {
		self.0.lock().post_message(message)
	}

	fn post_upward_message(&mut self, message: UpwardMessage) -> Result<(), String> {
		self.0.lock().post_upward_message(message)
	}
}

/// The successful result of validating a collation. If the full commitments of the
/// validation are needed, call `full_commit`. Otherwise, safely drop this value.
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
	/// of validators.
	pub fn full_commit(self, n_validators: usize)
		-> Result<(OmittedValidationData, CandidateCommitments, Vec<ErasureChunk>), Error>
	{
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
			parent_head: parent_head.clone(),
			balance,
		};

		let available = AvailableData {
			pov_block: pov_block.clone(),
			omitted_validation,
		};

		let erasure_chunks = erasure::obtain_chunks(
			n_validators,
			&available,
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

		Ok((available.omitted_validation, commitments, chunks))
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
		parent_head: collation.head_data.0.clone(),
		block_data: pov_block.block_data.0.clone(),
	};

	// TODO: remove when ext does not do this.
	let fee_schedule = FeeSchedule {
		base: 0,
		per_byte: 0,
	};

	let ext = Externalities::new(collation.parachain_index, local_validation.balance, fee_schedule);
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
