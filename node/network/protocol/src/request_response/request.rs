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
use sc_network::PeerId;

use super::v1;

/// Common properties of any `Request`.
pub trait IsRequest {
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
	pending_response: oneshot::Sender<Result<Vec<u8>, network::RequestFailure>>,
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

impl<Req> OutgoingRequest<Req>
where
	Req: IsRequest,
	Req::Response: Decode,
{
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
		(r, receive_response::<Req>(rx))
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
		self.pending_response.send(resp.encode()).map_err(|_| resp)
	}
}

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
