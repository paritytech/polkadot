// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Staging Primitives.

// Put any primitives used by staging APIs functions here
pub use crate::v5::*;
use sp_std::prelude::*;

use parity_scale_codec::{Decode, Encode};
use primitives::RuntimeDebug;
use scale_info::TypeInfo;

/// Useful type alias for Para IDs.
pub type ParaId = Id;

/// Candidate's acceptance limitations for asynchronous backing per relay parent.
#[derive(
	RuntimeDebug,
	Copy,
	Clone,
	PartialEq,
	Encode,
	Decode,
	TypeInfo,
	serde::Serialize,
	serde::Deserialize,
)]

pub struct AsyncBackingParams {
	/// The maximum number of para blocks between the para head in a relay parent
	/// and a new candidate. Restricts nodes from building arbitrary long chains
	/// and spamming other validators.
	///
	/// When async backing is disabled, the only valid value is 0.
	pub max_candidate_depth: u32,
	/// How many ancestors of a relay parent are allowed to build candidates on top
	/// of.
	///
	/// When async backing is disabled, the only valid value is 0.
	pub allowed_ancestry_len: u32,
}

/// Constraints on inbound HRMP channels.
#[derive(RuntimeDebug, Clone, PartialEq, Encode, Decode, TypeInfo)]
pub struct InboundHrmpLimitations<N = BlockNumber> {
	/// An exhaustive set of all valid watermarks, sorted ascending.
	///
	/// It's only expected to contain block numbers at which messages were
	/// previously sent to a para, excluding most recent head.
	pub valid_watermarks: Vec<N>,
}

/// Constraints on outbound HRMP channels.
#[derive(RuntimeDebug, Clone, PartialEq, Encode, Decode, TypeInfo)]
pub struct OutboundHrmpChannelLimitations {
	/// The maximum bytes that can be written to the channel.
	pub bytes_remaining: u32,
	/// The maximum messages that can be written to the channel.
	pub messages_remaining: u32,
}

/// Constraints on the actions that can be taken by a new parachain
/// block. These limitations are implicitly associated with some particular
/// parachain, which should be apparent from usage.
#[derive(RuntimeDebug, Clone, PartialEq, Encode, Decode, TypeInfo)]
pub struct Constraints<N = BlockNumber> {
	/// The minimum relay-parent number accepted under these constraints.
	pub min_relay_parent_number: N,
	/// The maximum Proof-of-Validity size allowed, in bytes.
	pub max_pov_size: u32,
	/// The maximum new validation code size allowed, in bytes.
	pub max_code_size: u32,
	/// The amount of UMP messages remaining.
	pub ump_remaining: u32,
	/// The amount of UMP bytes remaining.
	pub ump_remaining_bytes: u32,
	/// The maximum number of UMP messages allowed per candidate.
	pub max_ump_num_per_candidate: u32,
	/// Remaining DMP queue. Only includes sent-at block numbers.
	pub dmp_remaining_messages: Vec<N>,
	/// The limitations of all registered inbound HRMP channels.
	pub hrmp_inbound: InboundHrmpLimitations<N>,
	/// The limitations of all registered outbound HRMP channels.
	pub hrmp_channels_out: Vec<(ParaId, OutboundHrmpChannelLimitations)>,
	/// The maximum number of HRMP messages allowed per candidate.
	pub max_hrmp_num_per_candidate: u32,
	/// The required parent head-data of the parachain.
	pub required_parent: HeadData,
	/// The expected validation-code-hash of this parachain.
	pub validation_code_hash: ValidationCodeHash,
	/// The code upgrade restriction signal as-of this parachain.
	pub upgrade_restriction: Option<UpgradeRestriction>,
	/// The future validation code hash, if any, and at what relay-parent
	/// number the upgrade would be minimally applied.
	pub future_validation_code: Option<(N, ValidationCodeHash)>,
}

/// A candidate pending availability.
#[derive(RuntimeDebug, Clone, PartialEq, Encode, Decode, TypeInfo)]
pub struct CandidatePendingAvailability<H = Hash, N = BlockNumber> {
	/// The hash of the candidate.
	pub candidate_hash: CandidateHash,
	/// The candidate's descriptor.
	pub descriptor: CandidateDescriptor<H>,
	/// The commitments of the candidate.
	pub commitments: CandidateCommitments,
	/// The candidate's relay parent's number.
	pub relay_parent_number: N,
	/// The maximum Proof-of-Validity size allowed, in bytes.
	pub max_pov_size: u32,
}

/// The per-parachain state of the backing system, including
/// state-machine constraints and candidates pending availability.
#[derive(RuntimeDebug, Clone, PartialEq, Encode, Decode, TypeInfo)]
pub struct BackingState<H = Hash, N = BlockNumber> {
	/// The state-machine constraints of the parachain.
	pub constraints: Constraints<N>,
	/// The candidates pending availability. These should be ordered, i.e. they should form
	/// a sub-chain, where the first candidate builds on top of the required parent of the
	/// constraints and each subsequent builds on top of the previous head-data.
	pub pending_availability: Vec<CandidatePendingAvailability<H, N>>,
}
