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

//! Migration from V1 to V2.

use kvdb::{DBTransaction, KeyValueDB};
use polkadot_node_primitives::approval::{DelayTranche, AssignmentCert};
use polkadot_primitives::v1::{
	ValidatorIndex, GroupIndex, CandidateReceipt, SessionIndex,
	Hash, CandidateHash, ValidatorSignature,
};
use parity_scale_codec::{Encode, Decode};

use std::collections::BTreeMap;
use bitvec::{vec::BitVec, order::Lsb0 as BitOrderLsb0};

const LOG_TARGET: &str = "parachain::approval-db-v2-migration";
const CANDIDATE_ENTRY_PREFIX: [u8; 14] = *b"Approvals_cand";

/// The key a given candidate entry is stored under.
fn candidate_entry_key(candidate_hash: &CandidateHash) -> [u8; 46] {
	let mut key = [0u8; 14 + 32];
	key[0..14].copy_from_slice(&CANDIDATE_ENTRY_PREFIX);
	key[14..][..32].copy_from_slice(candidate_hash.0.as_ref());

	key
}

#[derive(Encode, Decode, Clone, Copy, Debug, PartialEq)]
struct Tick(u64);

type Bitfield = BitVec<BitOrderLsb0, u8>;


#[derive(Encode, Decode, Debug, Clone, PartialEq)]
struct OurAssignment {
	cert: AssignmentCert,
	tranche: DelayTranche,
	validator_index: ValidatorIndex,
	triggered: bool,
}

#[derive(Encode, Decode, Debug, Clone, PartialEq)]
struct TrancheEntry {
	tranche: DelayTranche,
	assignments: Vec<(ValidatorIndex, Tick)>,
}

#[derive(Encode, Decode, Debug, Clone, PartialEq)]
struct ApprovalEntry {
	tranches: Vec<TrancheEntry>,
	backing_group: GroupIndex,
	our_assignment: Option<OurAssignment>,
	our_approval_sig: Option<ValidatorSignature>,
	assignments: Bitfield,
	approved: bool,
}

#[derive(Encode, Decode, Debug, Clone, PartialEq)]
struct V1CandidateEntry {
	candidate: CandidateReceipt,
	session: SessionIndex,
	block_assignments: BTreeMap<Hash, ApprovalEntry>,
	approvals: Bitfield,
	disapprovals: Bitfield,
}

#[derive(Encode, Decode, Debug, Clone, PartialEq)]
struct V2CandidateEntry {
	candidate: CandidateReceipt,
	session: SessionIndex,
	block_assignments: BTreeMap<Hash, ApprovalEntry>,
	approvals: Bitfield,
	disapprovals: Bitfield,
}

/// Perform the migration from v1 to v2: the addition of the 'disapprovals'
/// field.
pub(crate) fn migrate_from_v1(
	col_data: u32,
	db: &dyn KeyValueDB,
) -> Result<(), std::io::Error> {
	let mut transaction = DBTransaction::new();

	let iter = db.iter_with_prefix(col_data, &CANDIDATE_ENTRY_PREFIX[..]);
	for (key, value) in iter {
		match V1CandidateEntry::decode(&mut &value[..]) {
			Err(e) => {
				tracing::error!(
					target: LOG_TARGET,
					err = ?e,
					"Failed to migrate candidate entry",
				);

				continue
			}
			Ok(val) => {
				let n_validators = val.approvals.len();

				let new_data = V2CandidateEntry {
					candidate: val.candidate,
					session: val.session,
					block_assignments: val.block_assignments,
					approvals: val.approvals,
					disapprovals: bitvec::bitvec![BitOrderLsb0, u8; 0; n_validators],
				};

				transaction.put_vec(col_data, &key[..], new_data.encode());
			}
		}
	}

	db.write(transaction)
}
