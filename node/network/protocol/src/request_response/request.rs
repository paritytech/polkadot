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

use thiserror::Error;
use parity_scale_codec::{Decode, Encode, Error as DecodingError};
use sc_network as network;
use sc_network::config as netconfig;
use sc_network::PeerId;

use polkadot_primitives::v1::AuthorityDiscoveryId;

use crate::UnifiedReputationChange;

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
	ChunkFetching(OutgoingRequest<v1::ChunkFetchingRequest>),
	/// Fetch a collation from a collator which previously announced it.
	CollationFetching(OutgoingRequest<v1::CollationFetchingRequest>),
	/// Fetch a PoV from a validator which previously sent out a seconded statement.
	PoVFetching(OutgoingRequest<v1::PoVFetchingRequest>),
	/// Request full available data from a node.
	AvailableDataFetching(OutgoingRequest<v1::AvailableDataFetchingRequest>),
	/// Requests for fetching large statements as part of statement distribution.
	StatementFetching(OutgoingRequest<v1::StatementFetchingRequest>),
}

impl Requests {
	/// Get the protocol this request conforms to.
	pub fn get_protocol(&self) -> Protocol {
		match self {
			Self::ChunkFetching(_) => Protocol::ChunkFetching,
			Self::CollationFetching(_) => Protocol::CollationFetching,
			Self::PoVFetching(_) => Protocol::PoVFetching,
			Self::AvailableDataFetching(_) => Protocol::AvailableDataFetching,
			Self::StatementFetching(_) => Protocol::StatementFetching,
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
			Self::ChunkFetching(r) => r.encode_request(),
			Self::CollationFetching(r) => r.encode_request(),
			Self::PoVFetching(r) => r.encode_request(),
			Self::AvailableDataFetching(r) => r.encode_request(),
			Self::StatementFetching(r) => r.encode_request(),
		}
	}
}

/// Potential recipients of an outgoing request.
#[derive(Debug, Eq, Hash, PartialEq)]
pub enum Recipient {
	/// Recipient is a regular peer and we know its peer id.
	Peer(PeerId),
	/// Recipient is a validator, we address it via this `AuthorityDiscoveryId`.
	Authority(AuthorityDiscoveryId),
}

/// A request to be sent to the network bridge, including a sender for sending responses/failures.
///
/// The network implementation will make use of that sender for informing the requesting subsystem
/// about responses/errors.
///
/// When using `Recipient::Peer`, keep in mind that no address (as in IP address and port) might
/// be known for that specific peer. You are encouraged to use `Peer` for peers that you are
/// expected to be already connected to.
/// When using `Recipient::Authority`, the addresses can be found thanks to the authority
/// discovery system.
#[derive(Debug)]
pub struct OutgoingRequest<Req> {
	/// Intendent recipient of this request.
	pub peer: Recipient,
	/// The actual request to send over the wire.
	pub payload: Req,
	/// Sender which is used by networking to get us back a response.
	pub pending_response: oneshot::Sender<Result<Vec<u8>, network::RequestFailure>>,
}

/// Any error that can occur when sending a request.
#[derive(Debug, Error)]
pub enum RequestError {
	/// Response could not be decoded.
	#[error("Response could not be decoded")]
	InvalidResponse(#[source] DecodingError),

	/// Some error in substrate/libp2p happened.
	#[error("Some network error occurred")]
	NetworkError(#[source] network::RequestFailure),

	/// Response got canceled by networking.
	#[error("Response channel got canceled")]
	Canceled(#[source] oneshot::Canceled),
}

/// Responses received for an `OutgoingRequest`.
pub type OutgoingResult<Res> = Result<Res, RequestError>;

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
		peer: Recipient,
		payload: Req,
	) -> (
		Self,
		impl Future<Output = OutgoingResult<Req::Response>>,
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

/// Typed variant of [`netconfig::OutgoingResponse`].
///
/// Responses to `IncomingRequest`s.
pub struct OutgoingResponse<Response> {
	/// The payload of the response.
	pub result: Result<Response, ()>,

	/// Reputation changes accrued while handling the request. To be applied to the reputation of
	/// the peer sending the request.
	pub reputation_changes: Vec<UnifiedReputationChange>,

	/// If provided, the `oneshot::Sender` will be notified when the request has been sent to the
	/// peer.
	pub sent_feedback: Option<oneshot::Sender<()>>,
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
				sent_feedback: None,
			})
			.map_err(|_| resp)
	}

	/// Send response with additional options.
	///
	/// This variant allows for waiting for the response to be sent out, allows for changing peer's
	/// reputation and allows for not sending a response at all (for only changing the peer's
	/// reputation).
	pub fn send_outgoing_response(self, resp: OutgoingResponse<<Req as IsRequest>::Response>)
		-> Result<(), ()> {
		let OutgoingResponse {
			result,
			reputation_changes,
			sent_feedback,
		} = resp;

		let response = netconfig::OutgoingResponse {
			result: result.map(|v| v.encode()),
			reputation_changes: reputation_changes
				.into_iter()
				.map(|c| c.into_base_rep())
				.collect(),
			sent_feedback,
		};

		self.pending_response.send(response).map_err(|_| ())
	}
}

/// Future for actually receiving a typed response for an OutgoingRequest.
async fn receive_response<Req>(
	rec: oneshot::Receiver<Result<Vec<u8>, network::RequestFailure>>,
) -> OutgoingResult<Req::Response>
where
	Req: IsRequest,
	Req::Response: Decode,
{
	let raw = rec.await??;
	Ok(Decode::decode(&mut raw.as_ref())?)
}
