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

use futures::channel::oneshot;
use futures::prelude::Future;

use parity_scale_codec::{Decode, Encode, Error as DecodingError};
use sc_network as network;
use sc_network::config as netconfig;
use sc_network::PeerId;

use polkadot_primitives::v1::AuthorityDiscoveryId;

use super::{v1, Protocol};

/// Common properties of any `Request`.
pub trait IsRequest {
	/// Each request has a corresponding `Response`.
	type Response;

	/// What protocol this `Request` implements.
	const PROTOCOL: Protocol;
}

/// All requests that can be sent to the network bridge via `NetworkBridgeMessage::SendRequest`.
#[derive(Debug)]
pub enum Requests {
	/// Request an availability chunk from a node.
	AvailabilityFetching(OutgoingRequest<v1::AvailabilityFetchingRequest>),
}

impl Requests {
	/// Get the protocol this request conforms to.
	pub fn get_protocol(&self) -> Protocol {
		match self {
			Self::AvailabilityFetching(_) => Protocol::AvailabilityFetching,
		}
	}

	/// Encode the request.
	///
	/// The corresponding protocol is returned as well, as we are now leaving typed territory.
	///
	/// Note: `Requests` is just an enum collecting all supported requests supported by network
	/// bridge, it is never sent over the wire. This function just encodes the individual requests
	/// contained in the enum.
	pub fn encode_request(self) -> (Protocol, OutgoingRequest<Vec<u8>>) {
		match self {
			Self::AvailabilityFetching(r) => r.encode_request(),
		}
	}
}

/// A request to be sent to the network bridge, including a sender for sending responses/failures.
///
/// The network implementation will make use of that sender for informing the requesting subsystem
/// about responses/errors.
#[derive(Debug)]
pub struct OutgoingRequest<Req> {
	/// Intendent recipient of this request.
	pub peer: AuthorityDiscoveryId,
	/// The actual request to send over the wire.
	pub payload: Req,
	/// Sender which is used by networking to get us back a response.
	pub pending_response: oneshot::Sender<Result<Vec<u8>, network::RequestFailure>>,
}

/// Any error that can occur when sending a request.
pub enum RequestError {
	/// Response could not be decoded.
	InvalidResponse(DecodingError),

	/// Some error in substrate/libp2p happened.
	NetworkError(network::RequestFailure),

	/// Response got canceled by networking.
	Canceled(oneshot::Canceled),
}

impl<Req> OutgoingRequest<Req>
where
	Req: IsRequest + Encode,
	Req::Response: Decode,
{
	/// Create a new `OutgoingRequest`.
	///
	/// It will contain a sender that is used by the networking for sending back responses. The
	/// connected receiver is returned as the second element in the returned tuple.
	pub fn new(
		peer: AuthorityDiscoveryId,
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
		(r, receive_response::<Req>(rx))
	}

	/// Encode a request into a `Vec<u8>`.
	///
	/// As this throws away type information, we also return the `Protocol` this encoded request
	/// adheres to.
	pub fn encode_request(self) -> (Protocol, OutgoingRequest<Vec<u8>>) {
		let OutgoingRequest {
			peer,
			payload,
			pending_response,
		} = self;
		let encoded = OutgoingRequest {
			peer,
			payload: payload.encode(),
			pending_response,
		};
		(Req::PROTOCOL, encoded)
	}
}

impl From<DecodingError> for RequestError {
	fn from(err: DecodingError) -> Self {
		Self::InvalidResponse(err)
	}
}

impl From<network::RequestFailure> for RequestError {
	fn from(err: network::RequestFailure) -> Self {
		Self::NetworkError(err)
	}
}

impl From<oneshot::Canceled> for RequestError {
	fn from(err: oneshot::Canceled) -> Self {
		Self::Canceled(err)
	}
}

/// A request coming in, including a sender for sending responses.
///
/// `IncomingRequest`s are produced by `RequestMultiplexer` on behalf of the network bridge.
#[derive(Debug)]
pub struct IncomingRequest<Req> {
	/// PeerId of sending peer.
	pub peer: PeerId,
	/// The sent request.
	pub payload: Req,
	pending_response: oneshot::Sender<netconfig::OutgoingResponse>,
}

impl<Req> IncomingRequest<Req>
where
	Req: IsRequest,
	Req::Response: Encode,
{
	/// Create new `IncomingRequest`.
	pub fn new(
		peer: PeerId,
		payload: Req,
		pending_response: oneshot::Sender<netconfig::OutgoingResponse>,
	) -> Self {
		Self {
			peer,
			payload,
			pending_response,
		}
	}

	/// Send the response back.
	///
	/// On success we return Ok(()), on error we return the not sent `Response`.
	///
	/// netconfig::OutgoingResponse exposes a way of modifying the peer's reputation. If needed we
	/// can change this function to expose this feature as well.
	pub fn send_response(self, resp: Req::Response) -> Result<(), Req::Response> {
		self.pending_response
			.send(netconfig::OutgoingResponse {
				result: Ok(resp.encode()),
				reputation_changes: Vec::new(),
			})
			.map_err(|_| resp)
	}
}

/// Future for actually receiving a typed response for an OutgoingRequest.
async fn receive_response<Req>(
	rec: oneshot::Receiver<Result<Vec<u8>, network::RequestFailure>>,
) -> Result<Req::Response, RequestError>
where
	Req: IsRequest,
	Req::Response: Decode,
{
	let raw = rec.await??;
	Ok(Decode::decode(&mut raw.as_ref())?)
}
