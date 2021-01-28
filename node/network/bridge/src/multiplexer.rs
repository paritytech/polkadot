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

use std::convert::From;
use std::pin::Pin;
use std::result::Result;

use futures::channel::mpsc;
use futures::stream::{Stream};
use futures::task::{Context, Poll};
use strum::IntoEnumIterator;

use parity_scale_codec::{Decode, Error as DecodingError};

use sc_network::config as network;

use polkadot_subsystem::messages::AllMessages;
use polkadot_node_network_protocol::request_response::{v1, Protocol, RequestResponseConfig, request::IncomingRequest};

/// Multiplex incoming network requests.
///
/// This multiplexer consumes all request streams and makes them a `Stream` of a single message
/// type, useful for the network bridge to send them via the `Overseer` to other subsystems.
pub struct RequestMultiplexer {
	receivers: Vec<(Protocol, mpsc::Receiver<network::IncomingRequest>)>,
	next_poll: usize,
}

/// Multiplexing can fail in case of invalid messages.
pub type RequestMultiplexError = DecodingError;

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

		(Self { receivers, next_poll: 0, }, cfgs,)
	}
}

impl Stream for RequestMultiplexer {
	type Item = Result<AllMessages, RequestMultiplexError>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let len = self.receivers.len();
		let mut count = len;
		let mut pending = false;
		// Poll streams in round robin fashion:
		while count > 0 {
			let (p, rx) = self.receivers[self.next_poll % len];
			self.next_poll += 1;
			match Pin::new(&mut rx).poll_next(cx) {
				Poll::Pending => pending = true,
				Poll::Ready(None) => {}
				Poll::Ready(Some(v)) => return Poll::Ready(Some(multiplex_single(p, v))),
			}
			count -= 1;
		}
		if pending {
			Poll::Pending
		} else {
			Poll::Ready(None)
		}
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
		AvailabilityFetching => From::from(IncomingRequest {
			peer,
			payload: v1::AvailabilityFetchingRequest::decode(&mut payload.as_ref())?,
			pending_response,
		}),
	};
	Ok(r)
}
