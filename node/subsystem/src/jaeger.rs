// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Jaeger integration.
//!
//! See <https://www.jaegertracing.io/> for an introduction.
//!
//! The easiest way to try Jaeger is:
//!
//! - Start a docker container with the all-in-one docker image (see below).
//! - Open your browser and navigate to <https://localhost:16686> to acces the UI.
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
//!  jaegertracing/all-in-one:1.21
//! ```
//!

use std::sync::Arc;
use std::sync::Mutex;

use polkadot_primitives::v1::{Hash, PoV, CandidateHash};

/// Configuration for the jaeger tracing.
#[derive(Clone)]
pub struct JaegerConfig {
    node_name: String,
    destination: url::Url,
}

impl std::default::Default for JaegerConfig {
    fn default() -> Self {
        Self {
            node_name: "unknown_".to_owned(),
            destination: "127.0.0.1:6831".parse().unwrap(),
        }
    }
}

impl JaegerConfig {
    /// Use the builder pattern to construct a configuration.
    pub fn builder() -> JaegerConfigBuilder {
        JaegerConfigBuilder::default()
    }
}


/// Jaeger configuration builder.
#[derive(Default)]
pub struct JaegerConfigBuilder {
    inner: JaegerConfig
}

impl JaegerConfigBuilder {
    /// Set the name for this node.
    pub fn named<S>(mut self, name: S) -> Self where S: AsRef<str> {
        self.inner.node_name = name.as_ref().to_owned();
        self
    }
    /// Set the recording destination url.
    pub fn destination<U>(mut self, url: U) -> Self where U: Into<url::Url> {
        self.inner.destination = url.into();
        self
    }
    /// Construct the configuration.
    pub fn build(self) -> JaegerConfig {
        self.inner
    }
}


/// Shortcut for [`candidate_hash_span`] with the hash of the `Candidate` block.
pub fn candidate_hash_span(candidate_hash: &CandidateHash, span_name: impl Into<String>) -> mick_jaeger::Span {
	hash_span(&candidate_hash.0, span_name)
}

/// Shortcut for [`hash_span`] with the hash of the `PoV`.
pub fn pov_span(pov: &PoV, span_name: impl Into<String>) -> mick_jaeger::Span {
	hash_span(&pov.hash(), span_name)
}

/// Creates a `Span` referring to the given hash. All spans created with [`hash_span`] with the
/// same hash (even from multiple different nodes) will be visible in the same view on Jaeger.
pub fn hash_span(hash: &Hash, span_name: impl Into<String>) -> mick_jaeger::Span {
    INSTANCE.lock().unwrap().span(hash, span_name)
}

/// Stateful convenience wrapper around [`mick_jaeger`].
pub enum Jaeger {
    /// Launched and operational state.
    Launched {
        /// [`mick_jaeger`] provided API to record spans to.
        traces_in: Arc<mick_jaeger::TracesIn>,
    },
    /// Preparation state with the necessary config to launch an instance.
    Prep(JaegerConfig),
    /// Uninitialized, suggests wrong API usage if encountered.
    None,
}

impl Jaeger {
    /// Spawn the jaeger instance.
    pub fn new(cfg: JaegerConfig) -> Self {
        Jaeger::Prep(cfg)
    }

    /// Spawn the background task in order to send the tracing information out via udp
    pub fn launch(self) {
        let cfg = match self {
            Self::Prep(cfg) => cfg,
            _ => { panic!("Must be a jaeger instance that was not launched yet, but has pending stuff") }
        };

        let (traces_in, mut traces_out) = mick_jaeger::init(mick_jaeger::Config {
            service_name: format!("polkadot-{}", cfg.node_name),
        });

        let resolved = cfg.destination.socket_addrs(|| None).unwrap();
        if resolved.is_empty() {
            panic!("Resolving of jaeger address always succeeds.");
        }
        let jaeger_agent: std::net::SocketAddr = resolved[0];

        // Spawn a background task that pulls span information and sends them on the network.
        let _handle = async_std::task::spawn(async move {
            let udp_socket = async_std::net::UdpSocket::bind("0.0.0.0:0").await.unwrap();

            loop {
                let buf = traces_out.next().await;
                // UDP sending errors happen only either if the API is misused (in which case
                // panicking is desirable) or in case of missing priviledge, in which case a
                // panic is preferable in order to inform the user.
                udp_socket.send_to(&buf, jaeger_agent).await.unwrap();
            }
        });

        let jaeger = Self::Launched {
            traces_in,
        };

        *INSTANCE.lock().unwrap() = jaeger;
    }

    #[inline(always)]
    fn traces_in(&self) -> Option<&Arc<mick_jaeger::TracesIn>> {
        match self {
            Self::Launched {
                traces_in,
                ..
            } => Some(&traces_in),
            _ => None,
        }
    }

    #[inline(always)]
    fn span(&self, hash: &Hash, span_name: impl Into<String>) -> Option<mick_jaeger::Span> {
        if let Some(traces_in) = self.traces_in() {
            let trace_id = {
                let mut buf = [0u8; 16];
                buf.copy_from_slice(&hash.as_ref()[0..16]);
                std::num::NonZeroU128::new(u128::from_be_bytes(buf)).unwrap()
            };
            Some(traces_in.span(trace_id, span_name))
        } else {
            None
        }
    }

}

lazy_static::lazy_static! {
    static ref INSTANCE: Mutex<Jaeger> = Mutex::new(Jaeger::None);
}


