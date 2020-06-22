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

//! Propagation and agreement of candidates.
//!
//! Authorities are split into groups by parachain, and each authority might come
//! up its own candidate for their parachain. Within groups, authorities pass around
//! their candidates and produce statements of validity.
//!
//! Any candidate that receives majority approval by the authorities in a group
//! may be subject to inclusion, unless any authorities flag that candidate as invalid.
//!
//! Wrongly flagging as invalid should be strongly disincentivized, so that in the
//! equilibrium state it is not expected to happen. Likewise with the submission
//! of invalid blocks.
//!
//! Groups themselves may be compromised by malicious authorities.

use std::{
	collections::{HashMap, HashSet},
	sync::Arc,
};
use codec::Encode;
use polkadot_primitives::parachain::{
	Id as ParaId, Chain, DutyRoster, AbridgedCandidateReceipt,
	CompactStatement as PrimitiveStatement,
	PoVBlock, ErasureChunk, ValidatorSignature, ValidatorIndex,
	ValidatorPair, ValidatorId, SigningContext,
};
use primitives::Pair;

use futures::prelude::*;

pub use self::block_production::ProposerFactory;
pub use self::collation::Collators;
pub use self::error::Error;
pub use self::shared_table::{
	SharedTable, ParachainWork, PrimedParachainWork, Validated, Statement, SignedStatement,
	GenericStatement,
};
pub use self::validation_service::{ServiceHandle, ServiceBuilder};

pub use parachain::wasm_executor::run_worker as run_validation_worker;

mod dynamic_inclusion;
mod error;
mod shared_table;

pub mod block_production;
pub mod collation;
pub mod pipeline;
pub mod validation_service;

/// A handle to a statement table router.
///
/// This is expected to be a lightweight, shared type like an `Arc`.
/// Once all instances are dropped, consensus networking for this router
/// should be cleaned up.
pub trait TableRouter: Clone {
	/// Errors when fetching data from the network.
	type Error: std::fmt::Debug;
	/// Future that drives sending of the local collation to the network.
	type SendLocalCollation: Future<Output=Result<(), Self::Error>>;
	/// Future that resolves when candidate data is fetched.
	type FetchValidationProof: Future<Output=Result<PoVBlock, Self::Error>>;

	/// Call with local candidate data. This will sign, import, and broadcast a statement about the candidate.
	fn local_collation(
		&self,
		receipt: AbridgedCandidateReceipt,
		pov_block: PoVBlock,
		chunks: (ValidatorIndex, &[ErasureChunk]),
	) -> Self::SendLocalCollation;

	/// Fetch validation proof for a specific candidate.
	///
	/// This future must conclude once all `Clone`s of this `TableRouter` have
	/// been cleaned up.
	fn fetch_pov_block(&self, candidate: &AbridgedCandidateReceipt) -> Self::FetchValidationProof;
}

/// A long-lived network which can create parachain statement and BFT message routing processes on demand.
pub trait Network {
	/// The error type of asynchronously building the table router.
	type Error: std::fmt::Debug;

	/// The table router type. This should handle importing of any statements,
	/// routing statements to peers, and driving completion of any `StatementProducers`.
	type TableRouter: TableRouter;

	/// The future used for asynchronously building the table router.
	/// This should not fail.
	type BuildTableRouter: Future<Output=Result<Self::TableRouter,Self::Error>>;

	/// Instantiate a table router using the given shared table.
	/// Also pass through any outgoing messages to be broadcast to peers.
	#[must_use]
	fn build_table_router(
		&self,
		table: Arc<SharedTable>,
		authorities: &[ValidatorId],
	) -> Self::BuildTableRouter;
}

/// The local duty of a validator.
#[derive(Debug)]
pub struct LocalDuty {
	validation: Chain,
	index: ValidatorIndex,
}

/// Information about a specific group.
#[derive(Debug, Clone, Default)]
pub struct GroupInfo {
	/// Authorities meant to check validity of candidates.
	validity_guarantors: HashSet<ValidatorId>,
	/// Number of votes needed for validity.
	needed_validity: usize,
}

/// Sign a table statement against a parent hash.
/// The actual message signed is the encoded statement concatenated with the
/// parent hash.
pub fn sign_table_statement(
	statement: &Statement,
	key: &ValidatorPair,
	signing_context: &SigningContext,
) -> ValidatorSignature {
	let mut encoded = PrimitiveStatement::from(statement).encode();
	encoded.extend(signing_context.encode());

	key.sign(&encoded)
}

/// Check signature on table statement.
pub fn check_statement(
	statement: &Statement,
	signature: &ValidatorSignature,
	signer: ValidatorId,
	signing_context: &SigningContext,
) -> bool {
	use runtime_primitives::traits::AppVerify;

	let mut encoded = PrimitiveStatement::from(statement).encode();
	encoded.extend(signing_context.encode());

	signature.verify(&encoded[..], &signer)
}

/// Compute group info out of a duty roster and a local authority set.
pub fn make_group_info(
	roster: DutyRoster,
	authorities: &[ValidatorId],
	local_id: Option<ValidatorId>,
) -> Result<(HashMap<ParaId, GroupInfo>, Option<LocalDuty>), Error> {
	if roster.validator_duty.len() != authorities.len() {
		return Err(Error::InvalidDutyRosterLength {
			expected: authorities.len(),
			got: roster.validator_duty.len()
		});
	}

	let mut local_validation = None;
	let mut local_index = 0;
	let mut map = HashMap::new();

	let duty_iter = authorities.iter().zip(&roster.validator_duty);
	for (i, (authority, v_duty)) in duty_iter.enumerate() {
		if Some(authority) == local_id.as_ref() {
			local_validation = Some(v_duty.clone());
			local_index = i;
		}

		match *v_duty {
			Chain::Relay => {}, // does nothing for now.
			Chain::Parachain(ref id) => {
				map.entry(id.clone()).or_insert_with(GroupInfo::default)
					.validity_guarantors
					.insert(authority.clone());
			}
		}
	}

	for live_group in map.values_mut() {
		let validity_len = live_group.validity_guarantors.len();
		live_group.needed_validity = validity_len / 2 + validity_len % 2;
	}


	let local_duty = local_validation.map(|v| LocalDuty {
		validation: v,
		index: local_index as u32,
	});

	Ok((map, local_duty))
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_keyring::Sr25519Keyring;

	#[test]
	fn sign_and_check_statement() {
		let statement: Statement = GenericStatement::Valid([1; 32].into());
		let parent_hash = [2; 32].into();
		let signing_context = SigningContext {
			session_index: Default::default(),
			parent_hash,
		};

		let sig = sign_table_statement(&statement, &Sr25519Keyring::Alice.pair().into(), &signing_context);

		let wrong_signing_context = SigningContext {
			session_index: Default::default(),
			parent_hash: [0xff; 32].into(),
		};
		assert!(check_statement(&statement, &sig, Sr25519Keyring::Alice.public().into(), &signing_context));
		assert!(!check_statement(&statement, &sig, Sr25519Keyring::Alice.public().into(), &wrong_signing_context));
		assert!(!check_statement(&statement, &sig, Sr25519Keyring::Bob.public().into(), &signing_context));
	}
}
