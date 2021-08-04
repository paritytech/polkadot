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

use futures::{channel::oneshot, prelude::Future};

use parity_scale_codec::{Decode, Encode, Error as DecodingError};
use sc_network as network;
use sc_network::{config as netconfig, PeerId};
use thiserror::Error;

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
