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

use futures::channel::oneshot;
use futures::prelude::Future;
use futures::task::{Context, Poll};

use parity_scale_codec::{codec, Decode, Encode, Error as DecodingError};
use sc_network as network;
use sc_network::{PeerId, RequestFailure};

use crate::request_response::{v1, IsRequest};

/// Common properties of any `Request`.
trait IsRequest {
	/// Each request has a corresponding `Response`.
	type Response;
}

/// All requests that can be sent to the network bridge via `NetworkBridgeMessage::SendRequest`.
pub enum Request {
	/// Request an availability chunk from a node.
	AvailabilityFetching(OutgoingRequest<v1::AvailabilityFetchingRequest>),
}

/// A request to be sent to the network bridge, including a sender for sending responses/failures.
///
/// The network implementation will make use of that sender for informing the requesting subsystem
/// about responses/errors.
pub struct OutgoingRequest<Req> {
	peer: PeerId,
	payload: Req,
	pending_response: oneshot::Sender<Vec<u8>>,
}

/// Any error that can occur when sending a request.
enum RequestError {
	/// Response could not be decoded.
	InvalidResponse { invalid_response: Vec<u8> },

	/// Some error in substrate/libp2p happened.
	NetworkError(network::RequestFailure),
}

impl<Req> OutgoingRequest<Req> {
	pub fn new(
		peer: PeerId,
		payload: Req,
	) -> (
		Self,
		impl Future<Output = Result<Req::Response, RequestError>>,
	) {
		let (tx, rx) = oneshot::channel();
		let r = Self {
			peer,
			payload,
			pending_response: tx,
		};
		(r, ResponseFuture { raw: rx })
	}

	/// To what peer this 
	pub fn get_peer(&self) -> &PeerId {
		&self.peer
	}
}

/// A request coming in, including a sender for sending responses.
///
/// `IncomingRequest`s are produced by `RequestReceiver` on behalf of the network bridge.
pub struct IncomingRequest<Req> {
	pub peer: PeerId,
	pub payload: Req,
	pub(crate) pending_response: oneshot::Sender<Vec<u8>>,
}

impl<Req> IncomingRequest<Req>
where
	Req: IsRequest,
	Req::Response: Encode,
{
	/// Send the response back.
	///
	/// On success we return Ok(()), on error we return the not sent `Response`.
	pub fn send_response(self, resp: Req::Response) -> Result<(), Req::Response> {
		self.pending_response.send(resp.encode())
	}
}

/// Internal future used for implementing the return value of `OutgoinRequest::new`
struct ResponseFuture<Req> {
	raw: oneshot::Receiver<Result<Vec<u8>, network::RequestFailure>>,
}

impl<Req: IsRequest> Future for ResponseFuture<Req> {
	type Output = Result<Req::Response, RequestError>;

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		match self.raw.poll(cx) {
			Poll::Pending => Poll::Pending,
			Poll::Ready(Ok(v)) => match v.decode() {
				Ok(v) => Ok(v),
				Err(err) => Err(RequestError::InvalidResponse {
					invalid_response: v,
				}),
			},
			Poll::Ready(Err(err)) => Err(RequestError::NetworkError(err)),
		}
	}
}
