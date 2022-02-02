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

// TODO [now]: document everything and make members public.
use polkadot_primitives::v1::{
	BlockNumber, CandidateCommitments, Id as ParaId, Hash, PersistedValidationData,
	ValidationCodeHash, HeadData,
};
use std::collections::HashMap;

/// Limitations on inbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundHrmpChannelLimitations {
	/// The number of messages remaining to be processed.
	pub messages_remaining: usize,
}

/// Limitations on outbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundHrmpChannelLimitations {
	/// The maximum bytes that can be written to the channel.
	pub bytes_remaining: usize,
	/// The maximum messages that can be written to the channel.
	pub messages_remaining: usize,
}

/// An update to inbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundHrmpChannelUpdate {
	/// The number of messages consumed from the channel.
	pub messages_consumed: usize,
}

/// An update to outbound HRMP channels.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundHrmpChannelUpdate {
	/// The number of bytes submitted to the channel.
	pub bytes_submitted: usize,
	/// The number of messages submitted to the channel.
	pub messages_submitted: usize,
}

/// Limitations on the actions that can be taken by a new parachain
/// block. These limitations are implicitly associated with some particular
/// parachain, which should be apparent from usage.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextLimitations {
	/// The amount of UMP messages remaining.
	pub ump_remaining: usize,
	/// The amount of UMP bytes remaining.
	pub ump_remaining_bytes: usize,
	/// The amount of remaining DMP messages.
	pub dmp_remaining_messages: usize,
	/// The limitations of all registered inbound HRMP channels.
	pub hrmp_channels_in: HashMap<ParaId, InboundHrmpChannelLimitations>,
	/// The limitations of all registered outbound HRMP channels.
	pub hrmp_channels_out: HashMap<ParaId, OutboundHrmpChannelLimitations>,
	/// The maximum Proof-of-Validity size allowed, in bytes.
	pub max_pov_size: usize,
	/// The required parent head-data of the parachain.
	pub required_parent: HeadData,
	/// The expected validation-code-hash of this parachain.
	pub validation_code_hash: ValidationCodeHash,
	/// Whether the go-ahead signal is set as-of this parachain.
	pub go_ahead: bool, // TODO [now] use nice enums like the runtime.
	/// Whether a code upgrade is allowed.
	pub code_upgrade_allowed: bool, // TODO [now] use nice enums like the runtime
}

// TODO [now]
pub struct Error;

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

/// A context used for judging parachain candidate validity.
///
/// A context is associated with some particular parachain, and this should be
/// apparent from its usage.
///
/// This is a combination of base limitations, which come from a
/// base relay-chain state and a series of updates to those limitations.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
	base: RelayChainBlockInfo,
	base_limitations: ContextLimitations,

	// base + all extensions.
	extensions: Vec<Extension>,
	cumulative: ContextLimitations,
}

impl Context {
	/// Create a context from a given base and base limitations.
	pub fn from_base(base: RelayChainBlockInfo, limitations: ContextLimitations) -> Self {
		Context {
			base,
			base_limitations: limitations.clone(),
			extensions: Vec::new(),
			cumulative: limitations,
		}
	}

	/// Get the limitations associated with this context.
	pub fn limitations(&self) -> &ContextLimitations {
		&self.cumulative
	}

	/// Get all extensions associated with this context.
	pub fn extensions(&self) -> &[Extension] {
		&self.extensions[..]
	}

	/// Rebase this context onto a new base.
	///
	/// If the `base` is the current `base`, this is a no-op and is guaranteed to succeed.
	/// If the `base` is the same as one of the extensions, this succeeds only if the
	pub fn rebase(&self, base: RelayChainBlockInfo, new_base: ContextLimitations) -> Result<Self, Error> {
		unimplemented!()

		// TODO [now]. We will want a mode where this just gets as far as it can.
		// That could be done in the error type, quite reasonably.
	}
}

/// An extension to a context, representing another prospective parachain block.
///
/// This has two parts: the first is the new relay-parent and its associated limitations,
/// and the second is information about the advancement of the parachain.
#[derive(Debug, Clone, PartialEq)]
pub struct Extension {
	/// The new relay-parent.
	pub relay_parent: RelayChainBlockInfo,
	/// The limitations associated with this relay-parent.
	pub limitations: ContextLimitations,
	/// The advancement of the parachain which is part of the extension.
	pub advancement: Advancement,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Advancement {
	commitments: CandidateCommitments,
	// We don't want the candidate descriptor, because that commmits to
	// things like the merkle root.
}

#[cfg(test)]
mod tests {
	use super::*;

	// TODO [now]: Pushing, rebasing
}
