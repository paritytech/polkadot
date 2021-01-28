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
use std::result::Result;
use std::pin::Pin;
use std::marker::PhantomData;

use futures::stream::{Stream, StreamExt};
use futures::stream::FusedStream;
use futures::channel::mpsc;
use futures::task::{Poll, Context};
use strum::IntoEnumIterator;

use parity_scale_codec::{Error as DecodingError, Decode};

use sc_network::config as network;


use super::request::IncomingRequest;
use super::v1;
use super::{RequestResponseConfig, Protocol};

/// Multiplex incoming network requests.
///
/// This multiplexer consumes all request streams and makes them a `Stream` of a single message
/// type, useful for the network bridge to send them via the `Overseer` to other subsystems.
pub struct RequestMultiplexer<M> {
	receivers: Vec<Box<dyn MultiplexStream<M>>>,
	next_poll: usize,
}


/// Message type where all incoming request can be multiplexed into.
pub trait MultiplexMessage: From<IncomingRequest<v1::AvailabilityFetchingRequest>> {}

/// Multiplexing can fail in case of invalid messages.
pub type MultiplexError = DecodingError;

impl<M, S> RequestMultiplexer<S>
where
	S: Stream<Item = Result<M, MultiplexError>> + FusedStream,
	M: MultiplexMessage,
{
	/// Create a new `RequestMultiplexer`.
	///
	/// This function uses `Protocol::get_config` for each available protocol and creates a
	/// `RequestMultiplexer` from it. The returned `RequestResponseConfig`s must be passed to the
	/// network implementation.
	pub fn new() -> (Self, Vec<RequestResponseConfig>) {
		let (receivers, cfgs): (Vec<_>, Vec<_>) = Protocol::iter()
			.map(|p| {
				let (rx, cfg) = p.get_config();
				let receiver = rx.map(|raw| Self::multiplex_single(p, raw));
				(Box::new(receiver), cfg)
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

	/// Convert a single raw incoming request into a `MultiplexMessage`.
	fn multiplex_single(
		p: Protocol,
		network::IncomingRequest { payload, peer, pending_response }: network::IncomingRequest,
		) -> Result<M, MultiplexError> {
		let r = match p {
			AvailabilityFetching => {
				From::from(IncomingRequest{
					peer,
					payload: v1::AvailabilityFetchingRequest::decode(&mut payload.as_ref())?,
					pending_response,
				})
			}
		};
		Ok(r)
	}
}

impl<M,S> Stream for RequestMultiplexer<S>
where
	S: Stream<Item = Result<M, MultiplexError>>,
	M: MultiplexMessage,
{
	type Item = Result<M, MultiplexError>;

	fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
		let len = self.receivers.len();
		let mut count = len;
		let mut pending = false;
		// Poll streams in round robin fashion:
		while count > 0 {
			let rx = self.receivers[self.next_poll % len];
			self.next_poll += 1;
			match rx.poll_next(cx) {
				Poll::Pending => pending = true,
				Poll::Ready(None) => {}
				Poll::Ready(Some(v)) => return Poll::Ready(Some(v)),
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

/// Internal structure for converting the received messages to a `MultiplexMessage`.
///
/// Normally this would be a simple `map` on the stream, unfortunately that gets us unweildly types
/// not really suitable for having in a struct.
struct MultiplexStream<M> {
	rx: mpsc::Receiver<network::IncomingRequest>,
	marker: PhantomData<M>,
}

impl Stream for MultiplexStream<M>
where M: MultiplexMessage
{
	type Item = M;
}
