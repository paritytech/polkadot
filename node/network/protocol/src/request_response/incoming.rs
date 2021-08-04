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

use std::marker::PhantomData;

use futures::channel::oneshot;
use thiserror::Error;

use parity_scale_codec::{Decode, Encode, Error as DecodingError};

use sc_network::{config as netconfig, PeerId};

use crate::UnifiedReputationChange;



/// Things that can go wrong when decoding an incoming request.
#[derive(Debug, Error)]
pub enum ReceiveError {
	/// Decoding failed, we were able to change the peer's reputation accordingly.
	#[error("Decoding request failed for peer {0}.")]
	DecodingError(PeerId, #[source] DecodingError),

	/// Decoding failed, but sending reputation change failed.
	#[error("Decoding request failed for peer {0}, and changing reputation failed.")]
	DecodingErrorNoReputationChange(PeerId, #[source] DecodingError),
}

/// A request coming in, including a sender for sending responses.
///
/// `IncomingRequest`s are produced by `RequestMultiplexer` on behalf of the network bridge.
#[derive(Debug)]
pub struct IncomingRequest<Req> {
	/// `PeerId` of sending peer.
	pub peer: PeerId,
	/// The sent request.
	pub payload: Req,
	/// Sender for sending response back.
	pub pending_response: OutgoingResponseSender<Req>,
}

impl<Req> IncomingRequest<Req>
where
	Req: IsRequest + Decode,
	Req::Response: Encode,
{
	/// Create
	pub fn get_config_receiver() -> (mpsc::Receiver<nSelf, RequestResponseConfig) {
	}
	/// Create new `IncomingRequest`.
	pub fn new(
		peer: PeerId,
		payload: Req,
		pending_response: oneshot::Sender<netconfig::OutgoingResponse>,
	) -> Self {
		Self {
			peer,
			payload,
			pending_response: OutgoingResponseSender { pending_response, phantom: PhantomData {} },
		}
	}

	/// Try building from raw substrate request.
	///
	/// This function will fail if the request cannot be decoded and will apply passed in
	/// reputation changes in that case.
	///
	/// Params:
	///		- The raw request to decode
	///		- Reputation changes to apply for the peer in case decoding fails.
	pub fn try_from_raw(
		raw: sc_network::config::IncomingRequest,
		reputation_changes: Vec<UnifiedReputationChange>,
	) -> Result<Self, ReceiveError> {
		let sc_network::config::IncomingRequest { payload, peer, pending_response } = raw;
		let payload = match Req::decode(&mut payload.as_ref()) {
			Ok(payload) => payload,
			Err(err) => {
				let reputation_changes =
					reputation_changes.into_iter().map(|r| r.into_base_rep()).collect();
				let response = sc_network::config::OutgoingResponse {
					result: Err(()),
					reputation_changes,
					sent_feedback: None,
				};

				if let Err(_) = pending_response.send(response) {
					return Err(ReceiveError::DecodingErrorNoReputationChange(peer, err))
				}
				return Err(ReceiveError::DecodingError(peer, err))
			},
		};
		Ok(Self::new(peer, payload, pending_response))
	}

	/// Send the response back.
	///
	/// Calls [`OutgoingResponseSender::send_response`].
	pub fn send_response(self, resp: Req::Response) -> Result<(), Req::Response> {
		self.pending_response.send_response(resp)
	}

	/// Send response with additional options.
	///
	/// Calls [`OutgoingResponseSender::send_outgoing_response`].
	pub fn send_outgoing_response(
		self,
		resp: OutgoingResponse<<Req as IsRequest>::Response>,
	) -> Result<(), ()> {
		self.pending_response.send_outgoing_response(resp)
	}
}


/// Sender for sending back responses on an `IncomingRequest`.
#[derive(Debug)]
pub struct OutgoingResponseSender<Req> {
	pending_response: oneshot::Sender<netconfig::OutgoingResponse>,
	phantom: PhantomData<Req>,
}

impl<Req> OutgoingResponseSender<Req>
where
	Req: IsRequest + Decode,
	Req::Response: Encode,
{
	/// Send the response back.
	///
	/// On success we return `Ok(())`, on error we return the not sent `Response`.
	///
	/// `netconfig::OutgoingResponse` exposes a way of modifying the peer's reputation. If needed we
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
	pub fn send_outgoing_response(
		self,
		resp: OutgoingResponse<<Req as IsRequest>::Response>,
	) -> Result<(), ()> {
		let OutgoingResponse { result, reputation_changes, sent_feedback } = resp;

		let response = netconfig::OutgoingResponse {
			result: result.map(|v| v.encode()),
			reputation_changes: reputation_changes.into_iter().map(|c| c.into_base_rep()).collect(),
			sent_feedback,
		};

		self.pending_response.send(response).map_err(|_| ())
	}
}

/// Typed variant of [`netconfig::OutgoingResponse`].
///
/// Responses to `IncomingRequest`s.
pub struct OutgoingResponse<Response> {
	/// The payload of the response.
	///
	/// `Err(())` if none is available e.g. due an error while handling the request.
	pub result: Result<Response, ()>,

	/// Reputation changes accrued while handling the request. To be applied to the reputation of
	/// the peer sending the request.
	pub reputation_changes: Vec<UnifiedReputationChange>,

	/// If provided, the `oneshot::Sender` will be notified when the request has been sent to the
	/// peer.
	pub sent_feedback: Option<oneshot::Sender<()>>,
}

