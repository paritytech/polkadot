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

use futures::future::{Fuse, fuse};

/// Multiplex incoming network requests.
///
/// This multiplexer consumes all request streams and makes them a `Stream` of a single message
/// type, useful for the network bridge to send them via the `Overseer` to other subsystems.
pub struct RequestMultiplexer<R: Stream> {
	receivers: Vec<Fuse<R>>,
	next_poll: usize,
}

/// Message type where all incoming request can be multiplexed into.
pub trait MultiplexMessage: From<IncomingRequest<v1: AvailabilityFetchingRequest>> {}

/// Multiplexing can fail in case of invalid messages.
pub struct MultiplexError {
	/// The actual error.
	pub error: DecodingError,
	/// The request that triggered could not be decoded.
	pub request: network::IncomingRequest,
}

impl<M> RequestMultiplexer<S>
where
	S: Stream<Result<M, MultiplexError>>,
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
				let receiver = rx.map(|raw| match dispatch_single(p, raw) {
					Ok(v) => Ok(v),
					Err(error) => MultiplexError { error, request: raw },
				});
				(receiver, cfg)
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

impl Stream for RequestMultiplexer<S>
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
			let next = self.receivers[self.next_poll % len];
			self.next_poll += 1;
			match next.poll_next(cx) {
				Pending => pending = true,
				Ready(None) => {}
				Ready(Some(v)) => return (Some(v)),
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
fn dispatch_single(
	p: Protocol,
	raw: &network::IncomingRequest,
) -> Result<impl MultiplexMessage, DecodingError> {
	fn mk_incoming<R>(decoded: R) -> IncomingRequest<R> {
		IncomingRequest {
			peer: raw.peer,
			payload: decoded,
			pending_response: raw.pending_response,
		}
	}
	match p {
		AvailabilityFetching => {
			mk_incoming(From::from(AvailabilityFetchingRequest::decode(rx.payload)?))
		}
	}
}

//// Stream for receiving decoded Requests.
////
//// Used by the network bridge for forwarding decoded requests to interested subsystems.
// struct RequestReceiver<Req> {
//     raw: mpsc::Receiver<network::IncomingRequest>,
// }

// impl<Req> Stream for RequestReceiver<Req> {
//     type Item = Result<IncomingRequest<Req>, DecodingError>;

//     fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
//         match self.raw.poll_next(cx) {
//             Poll::Pending => Poll::Pending,
//             Poll::Ready(IncomingRequest {
//                 peer,
//                 payload,
//                 pending_response,
//             }) => match Decode::decode(payload) {
//                 Result::Ok(req) => Poll::Ready(Some(Ok(IncomingRequest {
//                     peer,
//                     payload,
//                     pending_response,
//                 }))),
//                 Result::Err(err) => Err(err),
//             },
//         }
//     }
// }
