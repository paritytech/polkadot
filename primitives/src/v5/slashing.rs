// Copyright 2017-2023 Parity Technologies (UK) Ltd.
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

//! Primitives types used for dispute slashing.

use crate::{CandidateHash, SessionIndex, ValidatorId, ValidatorIndex};
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
use sp_std::{collections::btree_map::BTreeMap, vec::Vec};

/// The kind of the dispute offence.
#[derive(PartialEq, Eq, Clone, Copy, Encode, Decode, TypeInfo, Debug)]
pub enum SlashingOffenceKind {
	/// A severe offence when a validator backed an invalid block.
	#[codec(index = 0)]
	ForInvalid,
	/// A minor offence when a validator disputed a valid block.
	#[codec(index = 1)]
	AgainstValid,
}

/// Timeslots should uniquely identify offences and are used for the offence
/// deduplication.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Encode, Decode, TypeInfo, Debug)]
pub struct DisputesTimeSlot {
	// The order of the fields matters for `derive(Ord)`.
	/// Session index when the candidate was backed/included.
	pub session_index: SessionIndex,
	/// Candidate hash of the disputed candidate.
	pub candidate_hash: CandidateHash,
}

impl DisputesTimeSlot {
	/// Create a new instance of `Self`.
	pub fn new(session_index: SessionIndex, candidate_hash: CandidateHash) -> Self {
		Self { session_index, candidate_hash }
	}
}

/// We store most of the information about a lost dispute on chain. This struct
/// is required to identify and verify it.
#[derive(PartialEq, Eq, Clone, Encode, Decode, TypeInfo, Debug)]
pub struct DisputeProof {
	/// Time slot when the dispute occured.
	pub time_slot: DisputesTimeSlot,
	/// The dispute outcome.
	pub kind: SlashingOffenceKind,
	/// The index of the validator who lost a dispute.
	pub validator_index: ValidatorIndex,
	/// The parachain session key of the validator.
	pub validator_id: ValidatorId,
}

/// Slashes that are waiting to be applied once we have validator key
/// identification.
#[derive(Encode, Decode, TypeInfo, Debug, Clone)]
pub struct PendingSlashes {
	/// Indices and keys of the validators who lost a dispute and are pending
	/// slashes.
	pub keys: BTreeMap<ValidatorIndex, ValidatorId>,
	/// The dispute outcome.
	pub kind: SlashingOffenceKind,
}

// TODO: can we reuse this type between BABE, GRANDPA and disputes?
/// An opaque type used to represent the key ownership proof at the runtime API
/// boundary. The inner value is an encoded representation of the actual key
/// ownership proof which will be parameterized when defining the runtime. At
/// the runtime API boundary this type is unknown and as such we keep this
/// opaque representation, implementors of the runtime API will have to make
/// sure that all usages of `OpaqueKeyOwnershipProof` refer to the same type.
#[derive(Decode, Encode, PartialEq, Eq, Debug, Clone, TypeInfo)]
pub struct OpaqueKeyOwnershipProof(Vec<u8>);
impl OpaqueKeyOwnershipProof {
	/// Create a new `OpaqueKeyOwnershipProof` using the given encoded
	/// representation.
	pub fn new(inner: Vec<u8>) -> OpaqueKeyOwnershipProof {
		OpaqueKeyOwnershipProof(inner)
	}

	/// Try to decode this `OpaqueKeyOwnershipProof` into the given concrete key
	/// ownership proof type.
	pub fn decode<T: Decode>(self) -> Option<T> {
		Decode::decode(&mut &self.0[..]).ok()
	}

	/// Length of the encoded proof.
	pub fn len(&self) -> usize {
		self.0.len()
	}
}
