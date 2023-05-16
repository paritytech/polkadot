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

//! Various implementations for `SendXcm`.

use frame_system::unique;
use sp_std::{marker::PhantomData, result::Result};
use xcm::prelude::*;

/// Wrapper router which prepends a `SetTopic` instruction to the message prior to sending with a
/// universally unique topic, and returns this value from a successful `deliver`.
///
/// This is designed to be at the top-level of any routers, since it will always mutate the
/// passed `message` reference into a `None`. Don't try to combine it within a tuple except as the
/// last element.
pub struct WithUniqueTopic<Inner>(PhantomData<Inner>);
impl<Inner: SendXcm> SendXcm for WithUniqueTopic<Inner> {
	type Ticket = (Inner::Ticket, [u8; 32]);

	fn validate(
		destination: &mut Option<MultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		let mut message = message.take().ok_or(SendError::MissingArgument)?;
		let unique_id = unique(&message);
		message.0.insert(0, SetTopic(unique_id.clone()));
		let (ticket, assets) = Inner::validate(destination, &mut Some(message))
			.map_err(|_| SendError::NotApplicable)?;
		Ok(((ticket, unique_id), assets))
	}

	fn deliver(
		ticket: Self::Ticket,
	) -> Result<XcmHash, SendError> {
		let (ticket, unique_id) = ticket;
		Inner::deliver(ticket)?;
		Ok(unique_id)
	}
}
