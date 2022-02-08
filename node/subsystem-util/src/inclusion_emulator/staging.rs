// Copyright 2017-2022 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! The implementation of the inclusion emulator for the 'staging' runtime version.
//!
//! This is currently v1 (v2?), but will evolve to v3.
// TODO https://github.com/paritytech/polkadot/issues/4803
//!
//! A set of utilities for node-side code to emulate the logic the runtime uses for checking
//! parachain blocks in order to build prospective parachains that are produced ahead of the
//! relay chain. These utilities allow the node-side to predict, with high accuracy, what
//! the relay-chain will accept in the near future.
//!
//! This module has 2 key data types: [`Constraints`] and [`Fragment`]s. [`Constraints`] exhaustively
//! define the set of valid inputs and outputs to parachain execution. A [`Fragment`] indicates
//! a parachain block, anchored to the relay-chain at a particular relay-chain block, known as the
//! relay-parent.
//!
//! Every relay-parent is implicitly associated with a unique set of [`Constraints`] that describe
//! the properties that must be true for a block to be included in a direct child of that block,
//! assuming there is no intermediate parachain block pending availability.
//!
//! However, the key factor that makes asynchronously-grown prospective chains
//! possible is the fact that the relay-chain accepts candidate blocks based on whether they
//! are valid under the constraints of the present moment, not based on whether they were
//! valid at the time of construction.
//!
//! As such, [`Fragment`]s are often, but not always constructed in such a way that they are
//! invalid at first and become valid later on, as the relay chain grows.

use polkadot_primitives::v2::{
	BlockNumber, CandidateCommitments, CollatorId, CollatorSignature, Hash, HeadData, Id as ParaId,
	PersistedValidationData, UpgradeGoAhead, UpgradeRestriction, ValidationCodeHash,
};
use std::collections::HashMap;

/// Constraints on inbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundHrmpLimitations {
	/// An exhaustive set of all valid watermarks.
	pub valid_watermarks: Vec<BlockNumber>,
}

/// Constraints on outbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundHrmpChannelLimitations {
	/// The maximum bytes that can be written to the channel.
	pub bytes_remaining: usize,
	/// The maximum messages that can be written to the channel.
	pub messages_remaining: usize,
}

/// Constraints on the actions that can be taken by a new parachain
/// block. These limitations are implicitly associated with some particular
/// parachain, which should be apparent from usage.
#[derive(Debug, Clone, PartialEq)]
pub struct Constraints {
	/// The amount of UMP messages remaining.
	pub ump_remaining: usize,
	/// The amount of UMP bytes remaining.
	pub ump_remaining_bytes: usize,
	/// The amount of remaining DMP messages.
	pub dmp_remaining_messages: usize,
	/// The limitations of all registered inbound HRMP channels.
	pub hrmp_inbound: InboundHrmpLimitations,
	/// The limitations of all registered outbound HRMP channels.
	pub hrmp_channels_out: HashMap<ParaId, OutboundHrmpChannelLimitations>,
	/// The maximum Proof-of-Validity size allowed, in bytes.
	pub max_pov_size: usize,
	/// The maximum number of HRMP messages allowed per candidate.
	pub max_hrmp_num_per_candidate: usize,
	/// The required parent head-data of the parachain.
	pub required_parent: HeadData,
	/// The expected validation-code-hash of this parachain.
	pub validation_code_hash: ValidationCodeHash,
	/// The go-ahead signal as-of this parachain.
	pub go_ahead: UpgradeGoAhead,
	/// The code upgrade restriction signal as-of this parachain.
	pub upgrade_restriction: UpgradeRestriction,
	/// The future validation code hash, if any, and at what relay-parent
	/// number the upgrade would be minimally applied.
	pub future_validation_code: Option<(BlockNumber, ValidationCodeHash)>,
}

/// Information about a relay-chain block.
#[derive(Debug, Clone, PartialEq)]
pub struct RelayChainBlockInfo {
	/// The hash of the relay-chain block.
	pub hash: Hash,
	/// The number of the relay-chain block.
	pub number: BlockNumber,
	/// The storage-root of the relay-chain block.
	pub storage_root: Hash,
}

/// A parachain fragment, representing another prospective parachain block.
///
/// This has two parts: the first is the new relay-parent and its associated limitations,
/// and the second is information about the advancement of the parachain.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
	/// The new relay-parent.
	pub relay_parent: RelayChainBlockInfo,
	/// The constraints associated with this relay-parent.
	pub relay_parent_constraints: Constraints,
	/// The core information about the prospective candidate.
	pub prospective: ProspectiveCandidate,
}

/// An update to outbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundHrmpChannelModification {
	/// The number of bytes submitted to the channel.
	pub bytes_submitted: usize,
	/// The number of messages submitted to the channel.
	pub messages_submitted: usize,
}

/// Modifications to constraints as a result of prospective candidates.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstraintModifications {
	/// The required parent head to build upon.
	/// `None` indicates 'unmodified'.
	pub required_head: Option<HeadData>,
	/// The new HRMP watermark
	pub hrmp_watermark: BlockNumber,
	/// Outbound HRMP channel modifications.
	pub outbound_hrmp: HashMap<ParaId, OutboundHrmpChannelModification>,
	/// The amount of UMP messages sent.
	pub ump_messages_sent: usize,
	/// The amount of UMP bytes sent.
	pub ump_bytes_sent: usize,
	/// The amount of DMP messages processed.
	pub dmp_messages_processed: usize,
	/// Whether a scheduled code upgrade was applied.
	pub code_upgrade_applied: usize,
}

/// The prospective candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct ProspectiveCandidate {
	/// The commitments to the output of the execution.
	pub commitments: CandidateCommitments,
	/// The collator that created the candidate.
	pub collator: CollatorId,
	/// The signature of the collator on the payload.
	pub collator_signature: CollatorSignature,
	/// The persisted validation data used to create the candidate.
	pub persisted_validation_data: PersistedValidationData,
	/// The hash of the PoV.
	pub pov_hash: Hash,
	/// The validation code hash used by the candidate.
	pub validation_code_hash: ValidationCodeHash,
}

impl ProspectiveCandidate {
	/// Produce a set of constraint modifications based on the outputs
	/// of the candidate.
	pub fn constraint_modifications(&self) -> ConstraintModifications {
		ConstraintModifications {
			required_head: Some(self.commitments.head_data.clone()),
			hrmp_watermark: self.commitments.hrmp_watermark,
			outbound_hrmp: {
				// TODO [now]: have we enforced that HRMP messages are ascending at this point?
				// probably better not to assume that and do sanity-checking at other points.
				unimplemented!()
			},
			ump_messages_sent: self.commitments.upward_messages.len(),
			ump_bytes_sent: self.commitments.upward_messages.iter().map(|msg| msg.len()).sum(),
			dmp_messages_processed: self.commitments.processed_downward_messages as _,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// TODO [now]: Pushing, rebasing
}
