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
use parity_scale_codec::Encode;
use sp_std::{marker::PhantomData, result::Result};
use xcm::prelude::*;

/// Wrapper router which, if the message does not already end with a `SetTopic` instruction,
/// appends one to the message filled with a universally unique ID. This ID is returned from a
/// successful `deliver`.
///
/// If the message does already end with a `SetTopic` instruction, then it is the responsibility
/// of the code author to ensure that the ID supplied to `SetTopic` is universally unique. Due to
/// this property, consumers of the topic ID must be aware that a user-supplied ID may not be
/// unique.
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
		let unique_id = if let Some(SetTopic(id)) = message.last() {
			*id
		} else {
			let unique_id = unique(&message);
			message.0.push(SetTopic(unique_id));
			unique_id
		};
		let (ticket, assets) = Inner::validate(destination, &mut Some(message))
			.map_err(|_| SendError::NotApplicable)?;
		Ok(((ticket, unique_id), assets))
	}

	fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		let (ticket, unique_id) = ticket;
		Inner::deliver(ticket)?;
		Ok(unique_id)
	}
}

pub trait SourceTopic {
	fn source_topic(entropy: impl Encode) -> XcmHash;
}

impl SourceTopic for () {
	fn source_topic(_: impl Encode) -> XcmHash {
		[0u8; 32]
	}
}

/// Wrapper router which, if the message does not already end with a `SetTopic` instruction,
/// prepends one to the message filled with an ID from `TopicSource`. This ID is returned from a
/// successful `deliver`.
///
/// This is designed to be at the top-level of any routers, since it will always mutate the
/// passed `message` reference into a `None`. Don't try to combine it within a tuple except as the
/// last element.
pub struct WithTopicSource<Inner, TopicSource>(PhantomData<(Inner, TopicSource)>);
impl<Inner: SendXcm, TopicSource: SourceTopic> SendXcm for WithTopicSource<Inner, TopicSource> {
	type Ticket = (Inner::Ticket, [u8; 32]);

	fn validate(
		destination: &mut Option<MultiLocation>,
		message: &mut Option<Xcm<()>>,
	) -> SendResult<Self::Ticket> {
		let mut message = message.take().ok_or(SendError::MissingArgument)?;
		let unique_id = if let Some(SetTopic(id)) = message.last() {
			*id
		} else {
			let unique_id = TopicSource::source_topic(&message);
			message.0.push(SetTopic(unique_id));
			unique_id
		};
		let (ticket, assets) = Inner::validate(destination, &mut Some(message))
			.map_err(|_| SendError::NotApplicable)?;
		Ok(((ticket, unique_id), assets))
	}

	fn deliver(ticket: Self::Ticket) -> Result<XcmHash, SendError> {
		let (ticket, unique_id) = ticket;
		Inner::deliver(ticket)?;
		Ok(unique_id)
	}
}
