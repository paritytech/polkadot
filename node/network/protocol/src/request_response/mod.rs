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

use std::{borrow::Cow, u64};
use std::time::Duration;

use futures::channel::mpsc;
use polkadot_node_primitives::MAX_POV_SIZE;
use polkadot_primitives::v1::MAX_CODE_SIZE;
use strum::EnumIter;

pub use sc_network::config as network;
pub use sc_network::config::RequestResponseConfig;

/// All requests that can be sent to the network bridge.
pub mod request;
pub use request::{IncomingRequest, OutgoingRequest, Requests, Recipient, OutgoingResult};

///// Multiplexer for incoming requests.
// pub mod multiplexer;

/// Actual versioned requests and responses, that are sent over the wire.
pub mod v1;

/// A protocol per subsystem seems to make the most sense, this way we don't need any dispatching
/// within protocols.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, EnumIter)]
pub enum Protocol {
	/// Protocol for chunk fetching, used by availability distribution and availability recovery.
	ChunkFetching,
	/// Protocol for fetching collations from collators.
	CollationFetching,
	/// Protocol for fetching seconded PoVs from validators of the same group.
	PoVFetching,
	/// Protocol for fetching available data.
	AvailableDataFetching,
	/// Fetching of statements that are too large for gossip.
	StatementFetching,
}


/// Minimum bandwidth we expect for validators - 500Mbit/s is the recommendation, so approximately
/// 50Meg bytes per second:
const MIN_BANDWIDTH_BYTES: u64  = 50 * 1024 * 1024;

/// Default request timeout in seconds.
///
/// When decreasing this value, take into account that the very first request might need to open a
/// connection, which can be slow. If this causes problems, we should ensure connectivity via peer
/// sets.
#[allow(dead_code)]
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Request timeout where we can assume the connection is already open (e.g. we have peers in a
/// peer set as well).
const DEFAULT_REQUEST_TIMEOUT_CONNECTED: Duration = Duration::from_secs(1);

/// This timeout is based on what seems sensible from a time budget perspective, considering 6
/// second block time. This is going to be tough, if we have multiple forks and large PoVs, but we
/// only have so much time.
const POV_REQUEST_TIMEOUT_CONNECTED: Duration = Duration::from_millis(1000);

/// We want timeout statement requests fast, so we don't waste time on slow nodes. Responders will
/// try their best to either serve within that timeout or return an error immediately. (We need to
/// fit statement distribution within a block of 6 seconds.)
const STATEMENTS_TIMEOUT: Duration = Duration::from_secs(1);

/// We don't want a slow peer to slow down all the others, at the same time we want to get out the
/// data quickly in full to at least some peers (as this will reduce load on us as they then can
/// start serving the data). So this value is a tradeoff. 3 seems to be sensible. So we would need
/// to have 3 slow noded connected, to delay transfer for others by `STATEMENTS_TIMEOUT`.
pub const MAX_PARALLEL_STATEMENT_REQUESTS: u32 = 3;

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
			Protocol::ChunkFetching => RequestResponseConfig {
				name: p_name,
				max_request_size: 1_000,
				max_response_size: MAX_POV_SIZE as u64 / 10,
				// We are connected to all validators:
				request_timeout: DEFAULT_REQUEST_TIMEOUT_CONNECTED,
				inbound_queue: Some(tx),
			},
			Protocol::CollationFetching => RequestResponseConfig {
				name: p_name,
				max_request_size: 1_000,
				max_response_size: MAX_POV_SIZE as u64 + 1000,
				// Taken from initial implementation in collator protocol:
				request_timeout: POV_REQUEST_TIMEOUT_CONNECTED,
				inbound_queue: Some(tx),
			},
			Protocol::PoVFetching => RequestResponseConfig {
				name: p_name,
				max_request_size: 1_000,
				max_response_size: MAX_POV_SIZE as u64,
				request_timeout: POV_REQUEST_TIMEOUT_CONNECTED,
				inbound_queue: Some(tx),
			},
			Protocol::AvailableDataFetching => RequestResponseConfig {
				name: p_name,
				max_request_size: 1_000,
				// Available data size is dominated by the PoV size.
				max_response_size: MAX_POV_SIZE as u64 + 1000,
				request_timeout: POV_REQUEST_TIMEOUT_CONNECTED,
				inbound_queue: Some(tx),
			},
			Protocol::StatementFetching => RequestResponseConfig {
				name: p_name,
				max_request_size: 1_000,
				// Available data size is dominated code size.
				// + 1000 to account for protocol overhead (should be way less).
				max_response_size: MAX_CODE_SIZE as u64 + 1000,
				// We need statement fetching to be fast and will try our best at the responding
				// side to answer requests within that timeout, assuming a bandwidth of 500Mbit/s
				// - which is the recommended minimum bandwidth for nodes on Kusama as of April
				// 2021.
				// Responders will reject requests, if it is unlikely they can serve them within
				// the timeout, so the requester can immediately try another node, instead of
				// waiting for timeout on an overloaded node.  Fetches from slow nodes will likely
				// fail, but this is desired, so we can quickly move on to a faster one - we should
				// also decrease its reputation.
				request_timeout: Duration::from_secs(1),
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
			Protocol::ChunkFetching => 100,
			// 10 seems reasonable, considering group sizes of max 10 validators.
			Protocol::CollationFetching => 10,
			// 10 seems reasonable, considering group sizes of max 10 validators.
			Protocol::PoVFetching => 10,
			// Validators are constantly self-selecting to request available data which may lead
			// to constant load and occasional burstiness.
			Protocol::AvailableDataFetching => 100,
			// Our queue size approximation is how many blocks of the size of
			// a runtime we can transfer within a statements timeout, minus the requests we handle
			// in parallel.
			Protocol::StatementFetching => {
				// We assume we can utilize up to 70% of the available bandwidth for statements.
				// This is just a guess/estimate, with the following considerations: If we are
				// faster than that, queue size will stay low anyway, even if not - requesters will
				// get an immediate error, but if we are slower, requesters will run in a timeout -
				// waisting precious time.
				let available_bandwidth = 7 * MIN_BANDWIDTH_BYTES / 10;
				let size = u64::saturating_sub(
					STATEMENTS_TIMEOUT.as_millis() as u64 * available_bandwidth / (1000 * MAX_CODE_SIZE as u64),
					MAX_PARALLEL_STATEMENT_REQUESTS as u64
				);
				debug_assert!(
					size > 0,
					"We should have a channel size greater zero, otherwise we won't accept any requests."
				);
				size as usize
			}
		}
	}

	/// Get the protocol name of this protocol, as understood by substrate networking.
	pub fn into_protocol_name(self) -> Cow<'static, str> {
		self.get_protocol_name_static().into()
	}

	/// Get the protocol name associated with each peer set as static str.
	pub const fn get_protocol_name_static(self) -> &'static str {
		match self {
			Protocol::ChunkFetching => "/polkadot/req_chunk/1",
			Protocol::CollationFetching => "/polkadot/req_collation/1",
			Protocol::PoVFetching => "/polkadot/req_pov/1",
			Protocol::AvailableDataFetching => "/polkadot/req_available_data/1",
			Protocol::StatementFetching => "/polkadot/req_statement/1",
		}
	}
}
