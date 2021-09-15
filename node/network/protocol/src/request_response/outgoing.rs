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

use futures::{channel::oneshot, prelude::Future};
use thiserror::Error;

use parity_scale_codec::{Decode, Encode, Error as DecodingError};

use sc_network as network;
use sc_network::PeerId;

use polkadot_primitives::v1::AuthorityDiscoveryId;

use super::{v1, IsRequest, Protocol};

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
	/// Requests for notifying about an ongoing dispute.
	DisputeSending(OutgoingRequest<v1::DisputeRequest>),
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
			Self::DisputeSending(_) => Protocol::DisputeSending,
		}
	}

	/// Encode the request.
	///
	/// The corresponding protocol is returned as well, as we are now leaving typed territory.
	///
	/// Note: `Requests` is just an enum collecting all supported requests supported by network
	/// bridge, it is never sent over the wire. This function just encodes the individual requests
	/// contained in the `enum`.
	pub fn encode_request(self) -> (Protocol, OutgoingRequest<Vec<u8>>) {
		match self {
			Self::ChunkFetching(r) => r.encode_request(),
			Self::CollationFetching(r) => r.encode_request(),
			Self::PoVFetching(r) => r.encode_request(),
			Self::AvailableDataFetching(r) => r.encode_request(),
			Self::StatementFetching(r) => r.encode_request(),
			Self::DisputeSending(r) => r.encode_request(),
		}
	}
}

/// Used by the network to send us a response to a request.
pub type ResponseSender = oneshot::Sender<Result<Vec<u8>, network::RequestFailure>>;

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
	/// Intended recipient of this request.
	pub peer: Recipient,
	/// The actual request to send over the wire.
	pub payload: Req,
	/// Sender which is used by networking to get us back a response.
	pub pending_response: ResponseSender,
}

/// Potential recipients of an outgoing request.
#[derive(Debug, Eq, Hash, PartialEq, Clone)]
pub enum Recipient {
	/// Recipient is a regular peer and we know its peer id.
	Peer(PeerId),
	/// Recipient is a validator, we address it via this `AuthorityDiscoveryId`.
	Authority(AuthorityDiscoveryId),
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
	) -> (Self, impl Future<Output = OutgoingResult<Req::Response>>) {
		let (tx, rx) = oneshot::channel();
		let r = Self { peer, payload, pending_response: tx };
		(r, receive_response::<Req>(rx))
	}

	/// Encode a request into a `Vec<u8>`.
	///
	/// As this throws away type information, we also return the `Protocol` this encoded request
	/// adheres to.
	pub fn encode_request(self) -> (Protocol, OutgoingRequest<Vec<u8>>) {
		let OutgoingRequest { peer, payload, pending_response } = self;
		let encoded = OutgoingRequest { peer, payload: payload.encode(), pending_response };
		(Req::PROTOCOL, encoded)
	}
}

/// Future for actually receiving a typed response for an `OutgoingRequest`.
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
