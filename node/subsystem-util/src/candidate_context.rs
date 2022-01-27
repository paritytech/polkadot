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

#[derive(Debug, Clone, PartialEq)]
pub struct InboundHrmpChannelContext {
	messages_remaining: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboundHrmpChannelContext {
	bytes_remaining: usize,
	messages_remaining: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InboundHrmpChannelUpdate {
	messages_consumed: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutboundHrmpChannelUpdate {
	bytes_submitted: usize,
	messages_submitted: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextLimitations {
	ump_remaining: usize,
	ump_rmaining_bytes: usize,
	dmp_remaining_messages: usize,
	hrmp_channels_in: HashMap<ParaId, InboundHrmpChannelContext>,
	hrmp_channels_out: HashMap<ParaId, OutboundHrmpChannelContext>,
	// TODO [now]: some session-wide config members like maximums?
	// Other expected criteria like the DMP advancement rule?
	// TODO [now]: validation code hash & allowed code upgrade.
}

// TODO [now]
pub struct Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Context {
	base: ContextLimitations,
	updates: Vec<ContextUpdate>,

	// base + all updates.
	cumulative: ContextLimitations,
}

impl Context {
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

	pub fn push(&mut self, update: ContextUpdate) -> Result<(), Error> {
		unimplemented!()
	}

	pub fn limitations(&self) -> &ContextLimitations {
		&self.cumulative
	}

	pub fn updates(&self) -> &[ContextUpdate] {
		&self.updates[..]
	}

	pub fn rebase(&self, new_base: ContextLimitations) -> Result<Self, Error> {
		unimplemented!()

		// TODO [now]. We will want a mode where this just gets as far as it can.
		// That could be done in the error type, quite reasonably.
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextUpdate {
	ump_messages_submitted: usize,
	ump_bytes_submitted: usize,
	dmp_messages_consumed: usize,
	hrmp_in: HashMap<ParaId, InboundHrmpChannelUpdate>,
	hrmp_out: HashMap<ParaId, OutboundHrmpChannelUpdate>,
}

impl ContextUpdate {
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
