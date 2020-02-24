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

//! Polkadot-specific network implementation.
//!
//! This manages routing for parachain statements, parachain block and outgoing message
//! data fetching, communication between collators and validators, and more.

pub mod collator_pool;
pub mod local_collations;
pub mod gossip;

use codec::Decode;
use futures::prelude::*;
use polkadot_primitives::{Block, Hash};
use polkadot_primitives::parachain::ErasureChunk;
use sc_network::{
	PeerId, Context, StatusMessage as GenericFullStatus,
	specialization::NetworkSpecialization as Specialization,
};
use sc_network_gossip::TopicNotification;
use log::debug;

use std::pin::Pin;
use std::task::{Context as PollContext, Poll};

use self::gossip::{GossipMessage, ErasureChunkMessage, RegisteredMessageValidator};
use crate::{cost, benefit};

#[cfg(test)]
mod tests;

type FullStatus = GenericFullStatus<Block>;
type RequestId = u64;

/// Specialization of the network service for the polkadot protocol.
pub type PolkadotNetworkService = sc_network::NetworkService<Block, PolkadotProtocol, Hash>;

/// Basic gossip functionality that a network has to fulfill.
pub trait GossipService: Send + Sync + 'static {
	/// Get a stream of gossip messages for a given hash.
	fn gossip_messages_for(&self, topic: Hash) -> GossipMessageStream;

	/// Gossip a message on given topic.
	fn gossip_message(&self, topic: Hash, message: GossipMessage);

	/// Send a message to a specific peer we're connected to.
	fn send_message(&self, who: PeerId, message: GossipMessage);
}

/// This is a newtype that implements a [`ProvideGossipMessages`] shim trait.
///
/// For any wrapped [`NetworkService`] type it implements a [`ProvideGossipMessages`].
/// For more details see documentation of [`ProvideGossipMessages`].
///
/// [`NetworkService`]: ./trait.NetworkService.html
/// [`ProvideGossipMessages`]: ../polkadot_availability_store/trait.ProvideGossipMessages.html
#[derive(Clone)]
pub struct AvailabilityNetworkShim<S: NetworkSpecialization<Block>>(
	pub RegisteredMessageValidator<S>
);

impl<S: NetworkSpecialization<Block>> av_store::ProvideGossipMessages
	for AvailabilityNetworkShim<S>
{
	fn gossip_messages_for(&self, topic: Hash)
		-> Pin<Box<dyn Stream<Item = (Hash, Hash, ErasureChunk)> + Send>>
	{
		self.0.gossip_messages_for(topic)
			.filter_map(|(msg, _)| async move {
				match msg {
					GossipMessage::ErasureChunk(chunk) => {
						Some((chunk.relay_parent, chunk.candidate_hash, chunk.chunk))
					},
					_ => None,
				}
			})
			.boxed()
	}

	fn gossip_erasure_chunk(
		&self,
		relay_parent: Hash,
		candidate_hash: Hash,
		erasure_root: Hash,
		chunk: ErasureChunk,
	) {
		let topic = av_store::erasure_coding_topic(relay_parent, erasure_root, chunk.index);
		self.0.gossip_message(
			topic,
			GossipMessage::ErasureChunk(ErasureChunkMessage {
				chunk,
				relay_parent,
				candidate_hash,
			})
		)
	}
}

/// A stream of gossip messages and an optional sender for a topic.
pub struct GossipMessageStream {
	topic_stream: Pin<Box<dyn Stream<Item = TopicNotification> + Send>>,
}

impl GossipMessageStream {
	/// Create a new instance with the given topic stream.
	pub fn new(topic_stream: Pin<Box<dyn Stream<Item = TopicNotification> + Send>>) -> Self {
		Self {
			topic_stream,
		}
	}
}

impl Stream for GossipMessageStream {
	type Item = (GossipMessage, Option<PeerId>);

	fn poll_next(self: Pin<&mut Self>, cx: &mut PollContext) -> Poll<Option<Self::Item>> {
		let this = Pin::into_inner(self);

		loop {
			let msg = match Pin::new(&mut this.topic_stream).poll_next(cx) {
				Poll::Ready(Some(msg)) => msg,
				Poll::Ready(None) => return Poll::Ready(None),
				Poll::Pending => return Poll::Pending,
			};

			debug!(target: "validation", "Processing statement for live validation leaf-work");
			if let Ok(gmsg) = GossipMessage::decode(&mut &msg.message[..]) {
				return Poll::Ready(Some((gmsg, msg.sender)))
			}
		}
	}
}
