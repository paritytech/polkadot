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

//! Overview over request/responses as used in `Polkadot`.
//!
//! enum  Protocol .... List of all supported protocols.
//!
//! enum  Requests  .... List of all supported requests, each entry matches one in protocols, but
//! has the actual request as payload.
//!
//! struct IncomingRequest .... wrapper for incoming requests, containing a sender for sending
//! responses.
//!
//! struct OutgoingRequest .... wrapper for outgoing requests, containing a sender used by the
//! networking code for delivering responses/delivery errors.
//!
//! trait `IsRequest` .... A trait describing a particular request. It is used for gathering meta
//! data, like what is the corresponding response type.
//!
//!  Versioned (v1 module): The actual requests and responses as sent over the network.

use std::borrow::Cow;
use std::time::Duration;

use futures::channel::mpsc;
use strum::EnumIter;

pub use sc_network::config as network;
pub use sc_network::config::RequestResponseConfig;

/// All requests that can be sent to the network bridge.
pub mod request;
pub use request::{IncomingRequest, OutgoingRequest, Requests};

///// Multiplexer for incoming requests.
// pub mod multiplexer;

/// Actual versioned requests and responses, that are sent over the wire.
pub mod v1;

/// A protocol per subsystem seems to make the most sense, this way we don't need any dispatching
/// within protocols.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, EnumIter)]
pub enum Protocol {
	/// Protocol for availability fetching, used by availability distribution.
	AvailabilityFetching,
}

/// Default request timeout in seconds.
///
/// When decreasing this value, take into account that the very first request might need to open a
/// connection, which can be slow. If this causes problems, we should ensure connectivity via peer
/// sets.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(3); 

impl Protocol {
	/// Get a configuration for a given Request response protocol.
	///
	/// Returns a receiver for messages received on this protocol and the requested
	/// `ProtocolConfig`.
	///
	/// See also `dispatcher::RequestDispatcher`,  which makes use of this function and provides a more
	/// high-level interface.
	pub fn get_config(
		self,
	) -> (
		mpsc::Receiver<network::IncomingRequest>,
		RequestResponseConfig,
	) {
		let p_name = self.into_protocol_name();
		let (tx, rx) = mpsc::channel(self.get_channel_size());
		let cfg = match self {
			Protocol::AvailabilityFetching => RequestResponseConfig {
				name: p_name,
				// Arbitrary very conservative numbers:
				// TODO: Get better numbers, see https://github.com/paritytech/polkadot/issues/2370
				max_request_size: 10_000,
				max_response_size: 1_000_000,
				// Also just some relative conservative guess:
				request_timeout: DEFAULT_REQUEST_TIMEOUT,
				inbound_queue: Some(tx),
			},
		};
		(rx, cfg)
	}

	// Channel sizes for the supported protocols.
	fn get_channel_size(self) -> usize {
		match self {
			// Hundreds of validators will start requesting their chunks once they see a candidate
			// awaiting availability on chain. Given that they will see that block at different
			// times (due to network delays), 100 seems big enough to accomodate for "bursts",
			// assuming we can service requests relatively quickly, which would need to be measured
			// as well.
			Protocol::AvailabilityFetching => 100,
		}
	}

	/// Get the protocol name of this protocol, as understood by substrate networking.
	pub fn into_protocol_name(self) -> Cow<'static, str> {
		self.get_protocol_name_static().into()
	}

	/// Get the protocol name associated with each peer set as static str.
	pub const fn get_protocol_name_static(self) -> &'static str {
		match self {
			Protocol::AvailabilityFetching => "/polkadot/req_availability/1",
		}
	}
}
