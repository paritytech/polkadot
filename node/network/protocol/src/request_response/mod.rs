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
//! `enum Protocol` .... List of all supported protocols.
//!
//! `enum Requests`  .... List of all supported requests, each entry matches one in protocols, but
//! has the actual request as payload.
//!
//! `struct IncomingRequest` .... wrapper for incoming requests, containing a sender for sending
//! responses.
//!
//! `struct OutgoingRequest` .... wrapper for outgoing requests, containing a sender used by the
//! networking code for delivering responses/delivery errors.
//!
//! `trait IsRequest` .... A trait describing a particular request. It is used for gathering meta
//! data, like what is the corresponding response type.
//!
//!  Versioned (v1 module): The actual requests and responses as sent over the network.

use std::{collections::HashMap, time::Duration, u64};

use futures::channel::mpsc;
use polkadot_primitives::v2::{MAX_CODE_SIZE, MAX_POV_SIZE};
use strum::{EnumIter, IntoEnumIterator};

pub use sc_network::{config as network, config::RequestResponseConfig, ProtocolName};

/// Everything related to handling of incoming requests.
pub mod incoming;
/// Everything related to handling of outgoing requests.
pub mod outgoing;

pub use incoming::{IncomingRequest, IncomingRequestReceiver};

pub use outgoing::{OutgoingRequest, OutgoingResult, Recipient, Requests, ResponseSender};

///// Multiplexer for incoming requests.
// pub mod multiplexer;

/// Actual versioned requests and responses, that are sent over the wire.
pub mod v1;

/// A protocol per subsystem seems to make the most sense, this way we don't need any dispatching
/// within protocols.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, EnumIter)]
pub enum Protocol {
	/// Protocol for chunk fetching, used by availability distribution and availability recovery.
	ChunkFetchingV1,
	/// Protocol for fetching collations from collators.
	CollationFetchingV1,
	/// Protocol for fetching seconded PoVs from validators of the same group.
	PoVFetchingV1,
	/// Protocol for fetching available data.
	AvailableDataFetchingV1,
	/// Fetching of statements that are too large for gossip.
	StatementFetchingV1,
	/// Sending of dispute statements with application level confirmations.
	DisputeSendingV1,
}

/// Minimum bandwidth we expect for validators - 500Mbit/s is the recommendation, so approximately
/// 50MB per second:
const MIN_BANDWIDTH_BYTES: u64 = 50 * 1024 * 1024;

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

/// Timeout for requesting availability chunks.
pub const CHUNK_REQUEST_TIMEOUT: Duration = DEFAULT_REQUEST_TIMEOUT_CONNECTED;

/// This timeout is based on what seems sensible from a time budget perspective, considering 6
/// second block time. This is going to be tough, if we have multiple forks and large PoVs, but we
/// only have so much time.
const POV_REQUEST_TIMEOUT_CONNECTED: Duration = Duration::from_millis(1200);

/// We want timeout statement requests fast, so we don't waste time on slow nodes. Responders will
/// try their best to either serve within that timeout or return an error immediately. (We need to
/// fit statement distribution within a block of 6 seconds.)
const STATEMENTS_TIMEOUT: Duration = Duration::from_secs(1);

/// We don't want a slow peer to slow down all the others, at the same time we want to get out the
/// data quickly in full to at least some peers (as this will reduce load on us as they then can
/// start serving the data). So this value is a tradeoff. 3 seems to be sensible. So we would need
/// to have 3 slow nodes connected, to delay transfer for others by `STATEMENTS_TIMEOUT`.
pub const MAX_PARALLEL_STATEMENT_REQUESTS: u32 = 3;

/// Response size limit for responses of POV like data.
///
/// This is larger than `MAX_POV_SIZE` to account for protocol overhead and for additional data in
/// `CollationFetchingV1` or `AvailableDataFetchingV1` for example. We try to err on larger limits here
/// as a too large limit only allows an attacker to waste our bandwidth some more, a too low limit
/// might have more severe effects.
const POV_RESPONSE_SIZE: u64 = MAX_POV_SIZE as u64 + 10_000;

/// Maximum response sizes for `StatementFetchingV1`.
///
/// This is `MAX_CODE_SIZE` plus some additional space for protocol overhead.
const STATEMENT_RESPONSE_SIZE: u64 = MAX_CODE_SIZE as u64 + 10_000;

/// We can have relative large timeouts here, there is no value of hitting a
/// timeout as we want to get statements through to each node in any case.
pub const DISPUTE_REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

impl Protocol {
	/// Get a configuration for a given Request response protocol.
	///
	/// Returns a `ProtocolConfig` for this protocol.
	/// Use this if you plan only to send requests for this protocol.
	pub fn get_outbound_only_config(
		self,
		req_protocol_names: &ReqProtocolNames,
	) -> RequestResponseConfig {
		self.create_config(req_protocol_names, None)
	}

	/// Get a configuration for a given Request response protocol.
	///
	/// Returns a receiver for messages received on this protocol and the requested
	/// `ProtocolConfig`.
	pub fn get_config(
		self,
		req_protocol_names: &ReqProtocolNames,
	) -> (mpsc::Receiver<network::IncomingRequest>, RequestResponseConfig) {
		let (tx, rx) = mpsc::channel(self.get_channel_size());
		let cfg = self.create_config(req_protocol_names, Some(tx));
		(rx, cfg)
	}

	fn create_config(
		self,
		req_protocol_names: &ReqProtocolNames,
		tx: Option<mpsc::Sender<network::IncomingRequest>>,
	) -> RequestResponseConfig {
		let name = req_protocol_names.get_name(self);
		let fallback_names = self.get_fallback_names();
		match self {
			Protocol::ChunkFetchingV1 => RequestResponseConfig {
				name,
				fallback_names,
				max_request_size: 1_000,
				max_response_size: POV_RESPONSE_SIZE as u64 * 3,
				// We are connected to all validators:
				request_timeout: CHUNK_REQUEST_TIMEOUT,
				inbound_queue: tx,
			},
			Protocol::CollationFetchingV1 => RequestResponseConfig {
				name,
				fallback_names,
				max_request_size: 1_000,
				max_response_size: POV_RESPONSE_SIZE,
				// Taken from initial implementation in collator protocol:
				request_timeout: POV_REQUEST_TIMEOUT_CONNECTED,
				inbound_queue: tx,
			},
			Protocol::PoVFetchingV1 => RequestResponseConfig {
				name,
				fallback_names,
				max_request_size: 1_000,
				max_response_size: POV_RESPONSE_SIZE,
				request_timeout: POV_REQUEST_TIMEOUT_CONNECTED,
				inbound_queue: tx,
			},
			Protocol::AvailableDataFetchingV1 => RequestResponseConfig {
				name,
				fallback_names,
				max_request_size: 1_000,
				// Available data size is dominated by the PoV size.
				max_response_size: POV_RESPONSE_SIZE,
				request_timeout: POV_REQUEST_TIMEOUT_CONNECTED,
				inbound_queue: tx,
			},
			Protocol::StatementFetchingV1 => RequestResponseConfig {
				name,
				fallback_names,
				max_request_size: 1_000,
				// Available data size is dominated code size.
				max_response_size: STATEMENT_RESPONSE_SIZE,
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
				inbound_queue: tx,
			},
			Protocol::DisputeSendingV1 => RequestResponseConfig {
				name,
				fallback_names,
				max_request_size: 1_000,
				/// Responses are just confirmation, in essence not even a bit. So 100 seems
				/// plenty.
				max_response_size: 100,
				request_timeout: DISPUTE_REQUEST_TIMEOUT,
				inbound_queue: tx,
			},
		}
	}

	// Channel sizes for the supported protocols.
	fn get_channel_size(self) -> usize {
		match self {
			// Hundreds of validators will start requesting their chunks once they see a candidate
			// awaiting availability on chain. Given that they will see that block at different
			// times (due to network delays), 100 seems big enough to accomodate for "bursts",
			// assuming we can service requests relatively quickly, which would need to be measured
			// as well.
			Protocol::ChunkFetchingV1 => 100,
			// 10 seems reasonable, considering group sizes of max 10 validators.
			Protocol::CollationFetchingV1 => 10,
			// 10 seems reasonable, considering group sizes of max 10 validators.
			Protocol::PoVFetchingV1 => 10,
			// Validators are constantly self-selecting to request available data which may lead
			// to constant load and occasional burstiness.
			Protocol::AvailableDataFetchingV1 => 100,
			// Our queue size approximation is how many blocks of the size of
			// a runtime we can transfer within a statements timeout, minus the requests we handle
			// in parallel.
			Protocol::StatementFetchingV1 => {
				// We assume we can utilize up to 70% of the available bandwidth for statements.
				// This is just a guess/estimate, with the following considerations: If we are
				// faster than that, queue size will stay low anyway, even if not - requesters will
				// get an immediate error, but if we are slower, requesters will run in a timeout -
				// wasting precious time.
				let available_bandwidth = 7 * MIN_BANDWIDTH_BYTES / 10;
				let size = u64::saturating_sub(
					STATEMENTS_TIMEOUT.as_millis() as u64 * available_bandwidth /
						(1000 * MAX_CODE_SIZE as u64),
					MAX_PARALLEL_STATEMENT_REQUESTS as u64,
				);
				debug_assert!(
					size > 0,
					"We should have a channel size greater zero, otherwise we won't accept any requests."
				);
				size as usize
			},
			// Incoming requests can get bursty, we should also be able to handle them fast on
			// average, so something in the ballpark of 100 should be fine. Nodes will retry on
			// failure, so having a good value here is mostly about performance tuning.
			Protocol::DisputeSendingV1 => 100,
		}
	}

	/// Fallback protocol names of this protocol, as understood by substrate networking.
	fn get_fallback_names(self) -> Vec<ProtocolName> {
		std::iter::once(self.get_legacy_name().into()).collect()
	}

	/// Legacy protocol name associated with each peer set.
	const fn get_legacy_name(self) -> &'static str {
		match self {
			Protocol::ChunkFetchingV1 => "/polkadot/req_chunk/1",
			Protocol::CollationFetchingV1 => "/polkadot/req_collation/1",
			Protocol::PoVFetchingV1 => "/polkadot/req_pov/1",
			Protocol::AvailableDataFetchingV1 => "/polkadot/req_available_data/1",
			Protocol::StatementFetchingV1 => "/polkadot/req_statement/1",
			Protocol::DisputeSendingV1 => "/polkadot/send_dispute/1",
		}
	}
}

/// Common properties of any `Request`.
pub trait IsRequest {
	/// Each request has a corresponding `Response`.
	type Response;

	/// What protocol this `Request` implements.
	const PROTOCOL: Protocol;
}

/// Type for getting on the wire [`Protocol`] names using genesis hash & fork id.
pub struct ReqProtocolNames {
	names: HashMap<Protocol, ProtocolName>,
}

impl ReqProtocolNames {
	/// Construct [`ReqProtocolNames`] from `genesis_hash` and `fork_id`.
	pub fn new<Hash: AsRef<[u8]>>(genesis_hash: Hash, fork_id: Option<&str>) -> Self {
		let mut names = HashMap::new();
		for protocol in Protocol::iter() {
			names.insert(protocol, Self::generate_name(protocol, &genesis_hash, fork_id));
		}
		Self { names }
	}

	/// Get on the wire [`Protocol`] name.
	pub fn get_name(&self, protocol: Protocol) -> ProtocolName {
		self.names
			.get(&protocol)
			.expect("All `Protocol` enum variants are added above via `strum`; qed")
			.clone()
	}

	/// Protocol name of this protocol based on `genesis_hash` and `fork_id`.
	fn generate_name<Hash: AsRef<[u8]>>(
		protocol: Protocol,
		genesis_hash: &Hash,
		fork_id: Option<&str>,
	) -> ProtocolName {
		let prefix = if let Some(fork_id) = fork_id {
			format!("/{}/{}", hex::encode(genesis_hash), fork_id)
		} else {
			format!("/{}", hex::encode(genesis_hash))
		};

		let short_name = match protocol {
			Protocol::ChunkFetchingV1 => "/req_chunk/1",
			Protocol::CollationFetchingV1 => "/req_collation/1",
			Protocol::PoVFetchingV1 => "/req_pov/1",
			Protocol::AvailableDataFetchingV1 => "/req_available_data/1",
			Protocol::StatementFetchingV1 => "/req_statement/1",
			Protocol::DisputeSendingV1 => "/send_dispute/1",
		};

		format!("{}{}", prefix, short_name).into()
	}
}
