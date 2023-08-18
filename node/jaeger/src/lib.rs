// Copyright (C) Parity Technologies (UK) Ltd.
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

//! Polkadot Jaeger related primitives
//!
//! Provides primitives used by Polkadot for interfacing with Jaeger.
//!
//! # Integration
//!
//! See <https://www.jaegertracing.io/> for an introduction.
//!
//! The easiest way to try Jaeger is:
//!
//! - Start a docker container with the all-in-one docker image (see below).
//! - Open your browser and navigate to <https://localhost:16686> to access the UI.
//!
//! The all-in-one image can be started with:
//!
//! ```not_rust
//! podman login docker.io
//! podman run -d --name jaeger \
//!  -e COLLECTOR_ZIPKIN_HTTP_PORT=9411 \
//!  -p 5775:5775/udp \
//!  -p 6831:6831/udp \
//!  -p 6832:6832/udp \
//!  -p 5778:5778 \
//!  -p 16686:16686 \
//!  -p 14268:14268 \
//!  -p 14250:14250 \
//!  -p 9411:9411 \
//!  docker.io/jaegertracing/all-in-one:1.21
//! ```

#![forbid(unused_imports)]

mod config;
mod errors;
mod spans;

pub use self::{
	config::{JaegerConfig, JaegerConfigBuilder},
	errors::JaegerError,
	spans::{hash_to_trace_identifier, PerLeafSpan, Span, Stage},
};

use self::spans::TraceIdentifier;

use sp_core::traits::SpawnNamed;

use parking_lot::RwLock;
use std::{result, sync::Arc};

lazy_static::lazy_static! {
	static ref INSTANCE: RwLock<Jaeger> = RwLock::new(Jaeger::None);
}

/// Stateful convenience wrapper around [`mick_jaeger`].
pub enum Jaeger {
	/// Launched and operational state.
	Launched {
		/// [`mick_jaeger`] provided API to record spans to.
		traces_in: Arc<mick_jaeger::TracesIn>,
	},
	/// Preparation state with the necessary config to launch the collector.
	Prep(JaegerConfig),
	/// Uninitialized, suggests wrong API usage if encountered.
	None,
}

impl Jaeger {
	/// Spawn the jaeger instance.
	pub fn new(cfg: JaegerConfig) -> Self {
		Jaeger::Prep(cfg)
	}

	/// Spawn the background task in order to send the tracing information out via UDP
	#[cfg(target_os = "unknown")]
	pub fn launch<S: SpawnNamed>(self, _spawner: S) -> result::Result<(), JaegerError> {
		Ok(())
	}

	/// Provide a no-thrills test setup helper.
	#[cfg(test)]
	pub fn test_setup() {
		let mut instance = INSTANCE.write();
		match *instance {
			Self::Launched { .. } => {},
			_ => {
				let (traces_in, _traces_out) = mick_jaeger::init(mick_jaeger::Config {
					service_name: "polkadot-jaeger-test".to_owned(),
				});
				*instance = Self::Launched { traces_in };
			},
		}
	}

	/// Spawn the background task in order to send the tracing information out via UDP
	#[cfg(not(target_os = "unknown"))]
	pub fn launch<S: SpawnNamed>(self, spawner: S) -> result::Result<(), JaegerError> {
		let cfg = match self {
			Self::Prep(cfg) => Ok(cfg),
			Self::Launched { .. } => return Err(JaegerError::AlreadyLaunched),
			Self::None => Err(JaegerError::MissingConfiguration),
		}?;

		let jaeger_agent = cfg.agent_addr;

		log::info!("🐹 Collecting jaeger spans for {:?}", &jaeger_agent);

		let (traces_in, mut traces_out) = mick_jaeger::init(mick_jaeger::Config {
			service_name: format!("polkadot-{}", cfg.node_name),
		});

		// Spawn a background task that pulls span information and sends them on the network.
		spawner.spawn(
			"jaeger-collector",
			Some("jaeger"),
			Box::pin(async move {
				match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
					Ok(udp_socket) => loop {
						let buf = traces_out.next().await;
						// UDP sending errors happen only either if the API is misused or in case of
						// missing privilege.
						if let Err(e) = udp_socket.send_to(&buf, jaeger_agent).await {
							log::debug!(target: "jaeger", "UDP send error: {}", e);
						}
					},
					Err(e) => {
						log::warn!(target: "jaeger", "UDP socket open error: {}", e);
					},
				}
			}),
		);

		*INSTANCE.write() = Self::Launched { traces_in };
		Ok(())
	}

	/// Create a span, but defer the evaluation/transformation into a `TraceIdentifier`.
	///
	/// The deferral allows to avoid the additional CPU runtime cost in case of
	/// items that are not a pre-computed hash by themselves.
	pub(crate) fn span<F>(&self, lazy_hash: F, span_name: &'static str) -> Option<mick_jaeger::Span>
	where
		F: Fn() -> TraceIdentifier,
	{
		if let Self::Launched { traces_in, .. } = self {
			let ident = lazy_hash();
			let trace_id = std::num::NonZeroU128::new(ident)?;
			Some(traces_in.span(trace_id, span_name))
		} else {
			None
		}
	}
}
