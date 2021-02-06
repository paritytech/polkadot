// Copyright 2021 Parity Technologies (UK) Ltd.
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

use std::pin::Pin;

use futures::channel::mpsc;
use futures::stream::{FusedStream, Stream};
use futures::task::{Context, Poll};
use strum::IntoEnumIterator;

use parity_scale_codec::{Decode, Error as DecodingError};

use sc_network::config as network;
use sc_network::PeerId;

use polkadot_node_network_protocol::request_response::{
	request::IncomingRequest, v1, Protocol, RequestResponseConfig,
};
use polkadot_subsystem::messages::AllMessages;

/// Multiplex incoming network requests.
///
/// This multiplexer consumes all request streams and makes them a `Stream` of a single message
/// type, useful for the network bridge to send them via the `Overseer` to other subsystems.
///
/// The resulting stream will end once any of its input ends.
pub struct RequestMultiplexer {
	receivers: Vec<(Protocol, mpsc::Receiver<network::IncomingRequest>)>,
	next_poll: usize,
}

/// Multiplexing can fail in case of invalid messages.
#[derive(Debug, PartialEq, Eq)]
pub struct RequestMultiplexError {
	/// The peer that sent the invalid message.
	pub peer: PeerId,
	/// The error that occurred.
	pub error: DecodingError,
}

impl RequestMultiplexer {
	/// Create a new `RequestMultiplexer`.
	///
	/// This function uses `Protocol::get_config` for each available protocol and creates a
	/// `RequestMultiplexer` from it. The returned `RequestResponseConfig`s must be passed to the
	/// network implementation.
	pub fn new() -> (Self, Vec<RequestResponseConfig>) {
		let (receivers, cfgs): (Vec<_>, Vec<_>) = Protocol::iter()
			.map(|p| {
				let (rx, cfg) = p.get_config();
				((p, rx), cfg)
			})
			.unzip();

		(
			Self {
				receivers,
				next_poll: 0,
			},
			cfgs,
		)
	}
}

impl Stream for RequestMultiplexer {
	type Item = Result<AllMessages, RequestMultiplexError>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let len = self.receivers.len();
		let mut count = len;
		let mut i = self.next_poll;
		let mut result = Poll::Ready(None);
		// Poll streams in round robin fashion:
		while count > 0 {
			// % safe, because count initialized to len, loop would not be entered if 0, also
			// length of receivers is fixed.
			let (p, rx): &mut (_, _) = &mut self.receivers[i % len];
			// Avoid panic:
			if rx.is_terminated() {
				// Early return, we don't want to update next_poll.
				return Poll::Ready(None);
			}
			i += 1;
			count -= 1;
			match Pin::new(rx).poll_next(cx) {
				Poll::Pending => result = Poll::Pending,
				// We are done, once a single receiver is done.
				Poll::Ready(None) => return Poll::Ready(None),
				Poll::Ready(Some(v)) => {
					result = Poll::Ready(Some(multiplex_single(*p, v)));
					break;
				}
			}
		}
		self.next_poll = i;
		result
	}
}

impl FusedStream for RequestMultiplexer {
	fn is_terminated(&self) -> bool {
		let len = self.receivers.len();
		if len == 0 {
			return true;
		}
		let (_, rx) = &self.receivers[self.next_poll % len];
		rx.is_terminated()
	}
}

/// Convert a single raw incoming request into a `MultiplexMessage`.
fn multiplex_single(
	p: Protocol,
	network::IncomingRequest {
		payload,
		peer,
		pending_response,
	}: network::IncomingRequest,
) -> Result<AllMessages, RequestMultiplexError> {
	let r = match p {
		Protocol::AvailabilityFetching => From::from(IncomingRequest::new(
			peer,
			decode_with_peer::<v1::AvailabilityFetchingRequest>(peer, payload)?,
			pending_response,
		)),
	};
	Ok(r)
}

fn decode_with_peer<Req: Decode>(
	peer: PeerId,
	payload: Vec<u8>,
) -> Result<Req, RequestMultiplexError> {
	Req::decode(&mut payload.as_ref()).map_err(|error| RequestMultiplexError { peer, error })
}

#[cfg(test)]
mod tests {
	use futures::prelude::*;
	use futures::stream::FusedStream;

	use super::RequestMultiplexer;
	#[test]
	fn check_exhaustion_safety() {
		// Create and end streams:
		fn drop_configs() -> RequestMultiplexer {
			let (multiplexer, _) = RequestMultiplexer::new();
			multiplexer
		}
		let multiplexer = drop_configs();
		futures::executor::block_on(async move {
			let mut f = multiplexer;
			assert!(f.next().await.is_none());
			assert!(f.is_terminated());
			assert!(f.next().await.is_none());
			assert!(f.is_terminated());
			assert!(f.next().await.is_none());
			assert!(f.is_terminated());
		});
	}
}
