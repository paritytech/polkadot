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
use polkadot_primitives::v1::Id as ParaId;
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
/// block.
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
	// TODO [now]: some session-wide config members like maximums?
	// Other expected criteria like the DMP advancement rule?
	// TODO [now]: validation code hash & allowed code upgrade.
}

// TODO [now]
pub struct Error;

/// A context used for judging parachain candidate validity.
///
/// This is a combination of base limitations, which come from a
/// base relay-chain state and a series of updates to those limitations.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
	base: ContextLimitations,
	updates: Vec<ContextUpdate>,

	// base + all updates.
	cumulative: ContextLimitations,
}

impl Context {
	/// Create a context from a given base.
	pub fn from_base(base: ContextLimitations) -> Self {
		Context { base: base.clone(), updates: Vec::new(), cumulative: base }
	}

	// TODO [now]: add error type
	pub fn from_base_and_updates(
		base: ContextLimitations,
		updates: impl IntoIterator<Item = ContextUpdate>,
	) -> Result<Self, Error> {
		let mut context = Self::from_base(base);
		for update in updates {
			context.push(update)?;
		}
		Ok(context)
	}

	/// Push an update onto a context.
	pub fn push(&mut self, update: ContextUpdate) -> Result<(), Error> {
		unimplemented!()
	}

	/// Get the limitations associated with this context.
	pub fn limitations(&self) -> &ContextLimitations {
		&self.cumulative
	}

	/// Get all updates associated with this context.
	pub fn updates(&self) -> &[ContextUpdate] {
		&self.updates[..]
	}

	/// Rebase this context onto a new base.
	pub fn rebase(&self, new_base: ContextLimitations) -> Result<Self, Error> {
		unimplemented!()

		// TODO [now]. We will want a mode where this just gets as far as it can.
		// That could be done in the error type, quite reasonably.
	}
}

// TODO [now]: this needs 2 parts: what we take away from the limitations,
// and what we add to the limitations.
//
// The first is the change from the previous relay-parent to the current state.
// And the second is based on the outputs of the candidate.
/// An update to a context.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextUpdate {
	// TODO [now] : relay-parent?
	/// The number of messages submitted to UMP
	pub ump_messages_submitted: usize,
	/// The number of message-bytes submitted to UMP
	pub ump_bytes_submitted: usize,
	/// The number of DMP messages consumed.
	pub dmp_messages_consumed: usize,
	/// Updates to inbound HRMP channels.
	pub hrmp_in: HashMap<ParaId, InboundHrmpChannelUpdate>,
	/// Updates to outbound HRMP channels.
	pub hrmp_out: HashMap<ParaId, OutboundHrmpChannelUpdate>,
}

impl ContextUpdate {
	/// Create a blank context update.
	pub fn blank() -> Self {
		ContextUpdate {
			ump_messages_submitted: 0,
			ump_bytes_submitted: 0,
			dmp_messages_consumed: 0,
			hrmp_in: HashMap::new(),
			hrmp_out: HashMap::new(),
		}
	}
}

// TODO [now] : function to compare limitations against updates.

#[cfg(test)]
mod tests {
	use super::*;

	// TODO [now]: Pushing, rebasing
}
