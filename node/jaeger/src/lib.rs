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
//!  docker.io/jaegertracing/all-in-one:1.21
//! ```

use sp_core::traits::SpawnNamed;
use polkadot_primitives::v1::{CandidateHash, Hash, PoV, ValidatorIndex, BlakeTwo256, HashT};
use parity_scale_codec::Encode;
use sc_network::PeerId;

use parking_lot::RwLock;
use std::{sync::Arc, result};

/// A description of an error causing the chain API request to be unservable.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum JaegerError {
	#[error("Already launched the collector thread")]
	AlreadyLaunched,

	#[error("Missing jaeger configuration")]
	MissingConfiguration,
}

lazy_static::lazy_static! {
	static ref INSTANCE: RwLock<Jaeger> = RwLock::new(Jaeger::None);
}

/// Configuration for the jaeger tracing.
#[derive(Clone)]
pub struct JaegerConfig {
	node_name: String,
	agent_addr: std::net::SocketAddr,
}

impl std::default::Default for JaegerConfig {
	fn default() -> Self {
		Self {
			node_name: "unknown_".to_owned(),
			agent_addr: "127.0.0.1:6831".parse().expect(r#"Static "127.0.0.1:6831" is a valid socket address string. qed"#),
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

	/// Set the agent address to send the collected spans to.
	pub fn agent<U>(mut self, addr: U) -> Self where U: Into<std::net::SocketAddr> {
		self.inner.agent_addr = addr.into();
		self
	}

	/// Construct the configuration.
	pub fn build(self) -> JaegerConfig {
		self.inner
	}
}

/// A special "per leaf span".
///
/// Essentially this span wraps two spans:
///
/// 1. The span that is created per leaf in the overseer.
/// 2. Some child span of the per-leaf span.
///
/// This just works as auxiliary structure to easily store both.
#[derive(Debug)]
pub struct PerLeafSpan {
	leaf_span: Arc<Span>,
	span: Span,
}

impl PerLeafSpan {
	/// Creates a new instance.
	///
	/// Takes the `leaf_span` that is created by the overseer per leaf and a name for a child span.
	/// Both will be stored in this object, while the child span is implicitly accessible by using the
	/// [`Deref`](std::ops::Deref) implementation.
	pub fn new(leaf_span: Arc<Span>, name: &'static str) -> Self {
		let span = leaf_span.child(name);

		Self {
			span,
			leaf_span,
		}
	}

	/// Returns the leaf span.
	pub fn leaf_span(&self) -> &Arc<Span> {
		&self.leaf_span
	}
}

/// Returns a reference to the child span.
impl std::ops::Deref for PerLeafSpan {
	type Target = Span;

	fn deref(&self) -> &Span {
		&self.span
	}
}


/// A helper to annotate the stage with a numerical value
/// to ease the life of the tooling team creating viable
/// statistical metrics for which stage of the inclusion
/// pipeline drops a significant amount of candidates,
/// statistically speaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum Stage {
	CandidateSelection = 1,
	CandidateBacking = 2,
	StatementDistribution = 3,
	PoVDistribution = 4,
	AvailabilityDistribution = 5,
	AvailabilityRecovery = 6,
	BitfieldDistribution = 7,
	ApprovalChecking = 8,
	// Expand as needed, numbers should be ascending according to the stage
	// through the inclusion pipeline, or according to the descriptions
	// in [the path of a para chain block]
	// (https://polkadot.network/the-path-of-a-parachain-block/)
	// see [issue](https://github.com/paritytech/polkadot/issues/2389)
}

/// Builder pattern for children and root spans to unify
/// information annotations.
pub struct SpanBuilder {
	span: Span,
}

impl SpanBuilder {
	/// Attach a peer id to the span.
	#[inline(always)]
	pub fn with_peer_id(mut self, peer: &PeerId) -> Self {
		self.span.add_string_tag("peer-id", &peer.to_base58());
		self
	}
	/// Attach a candidate hash to the span.
	#[inline(always)]
	pub fn with_candidate(mut self, candidate_hash: &CandidateHash) -> Self  {
		self.span.add_string_tag("candidate-hash", &format!("{:?}", candidate_hash.0));
		self
	}
	/// Attach a candidate stage.
	/// Should always come with a `CandidateHash`.
	#[inline(always)]
	pub fn with_stage(mut self, stage: Stage) -> Self {
		self.span.add_stage(stage);
		self
	}

	#[inline(always)]
	pub fn with_validator_index(mut self, validator: ValidatorIndex) -> Self {
		self.span.add_string_tag("validator-index", &validator.0.to_string());
		self
	}

	#[inline(always)]
	pub fn with_chunk_index(mut self, chunk_index: u32) -> Self {
		self.span.add_string_tag("chunk-index", &format!("{}", chunk_index));
		self
	}

	#[inline(always)]
	pub fn with_relay_parent(mut self, relay_parent: &Hash) -> Self {
		self.span.add_string_tag("relay-parent", &format!("{:?}", relay_parent));
		self
	}

	#[inline(always)]
	pub fn with_claimed_validator_index(mut self, claimed_validator_index: ValidatorIndex) -> Self {
		self.span.add_string_tag(
			"claimed-validator",
			&claimed_validator_index.0.to_string(),
		);
		self
	}

	#[inline(always)]
	pub fn with_pov(mut self, pov: &PoV) -> Self {
		self.span.add_pov(pov);
		self
	}

	/// Complete construction.
	#[inline(always)]
	pub fn build(self) -> Span {
		self.span
	}
}


/// A wrapper type for a span.
///
/// Handles running with and without jaeger.
pub enum Span {
	/// Running with jaeger being enabled.
	Enabled(mick_jaeger::Span),
	/// Running with jaeger disabled.
	Disabled,
}

impl Span {
	/// Derive a child span from `self`.
	pub fn child(&self, name: &'static str) -> Self {
		match self {
			Self::Enabled(inner) => Self::Enabled(inner.child(name)),
			Self::Disabled => Self::Disabled,
		}
	}

	pub fn child_builder(&self, name: &'static str) -> SpanBuilder {
		SpanBuilder {
			span: self.child(name),
		}
	}

	/// Derive a child span from `self` but add a candidate annotation.
	/// A shortcut for
	///
	/// ```rust,no_run
	/// # use polkadot_primitives::v1::CandidateHash;
	/// # use polkadot_node_jaeger::candidate_hash_span;
	/// # let hash = CandidateHash::default();
	/// # let span = candidate_hash_span(&hash, "foo");
	/// let _span = span.child_builder("name").with_candidate(&hash).build();
	/// ```
	#[inline(always)]
	pub fn child_with_candidate(&self, name: &'static str, candidate_hash: &CandidateHash) -> Self {
		self.child_builder(name).with_candidate(candidate_hash).build()
	}

	/// Add candidate stage annotation.
	pub fn add_stage(&mut self, stage: Stage) {
		self.add_string_tag("candidate-stage", &format!("{}", stage as u8));
	}

	pub fn add_pov(&mut self, pov: &PoV) {
		if self.is_enabled() {
			// avoid computing the pov hash if jaeger is not enabled
			self.add_string_tag("pov", &format!("{:?}", pov.hash()));
		}
	}

	/// Add an additional tag to the span.
	pub fn add_string_tag(&mut self, tag: &str, value: &str) {
		match self {
			Self::Enabled(ref mut inner) => inner.add_string_tag(tag, value),
			Self::Disabled => {},
		}
	}

	/// Add an additional int tag to the span.
	pub fn add_int_tag(&mut self, tag: &str, value: i64) {
		match self {
			Self::Enabled(ref mut inner) => inner.add_int_tag(tag, value),
			Self::Disabled => {},
		}
	}

	/// Adds the `FollowsFrom` relationship to this span with respect to the given one.
	pub fn add_follows_from(&mut self, other: &Self) {
		match (self, other) {
			(Self::Enabled(ref mut inner), Self::Enabled(ref other_inner)) => inner.add_follows_from(&other_inner),
			_ => {},
		}
	}

	/// Helper to check whether jaeger is enabled
	/// in order to avoid computational overhead.
	pub const fn is_enabled(&self) -> bool {
		match self {
			Span::Enabled(_) => true,
			_ => false,
		}
	}
}

impl std::fmt::Debug for Span {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "<jaeger span>")
	}
}

impl From<Option<mick_jaeger::Span>> for Span {
	fn from(src: Option<mick_jaeger::Span>) -> Self {
		if let Some(span) = src {
			Self::Enabled(span)
		} else {
			Self::Disabled
		}
	}
}

impl From<mick_jaeger::Span> for Span {
	fn from(src: mick_jaeger::Span) -> Self {
		Self::Enabled(src)
	}
}

/// Shortcut for [`hash_span`] with the hash of the `Candidate` block.
pub fn candidate_hash_span(candidate_hash: &CandidateHash, span_name: &'static str) -> Span {
	let mut span: Span = INSTANCE.read_recursive()
		.span(|| { candidate_hash.0 }, span_name).into();

	span.add_string_tag("candidate-hash", &format!("{:?}", candidate_hash.0));
	span
}

/// Shortcut for [`hash_span`] with the hash of the `PoV`.
#[inline(always)]
pub fn pov_span(pov: &PoV, span_name: &'static str) -> Span {
	let mut span: Span = INSTANCE.read_recursive()
		.span(|| { pov.hash() }, span_name).into();

	span.add_pov(pov);
	span
}

/// Creates a `Span` referring to the given hash. All spans created with [`hash_span`] with the
/// same hash (even from multiple different nodes) will be visible in the same view on Jaeger.
///
/// This span automatically has the `relay-parent` tag set.
#[inline(always)]
pub fn hash_span(hash: &Hash, span_name: &'static str) -> Span {
	let mut span: Span = INSTANCE.read_recursive().span(|| { *hash }, span_name).into();
	span.add_string_tag("relay-parent", &format!("{:?}", hash));
	span
}

/// Creates a `Span` referring to the given descriptor, which should be unique.
#[inline(always)]
pub fn descriptor_span(descriptor: impl Encode, span_name: &'static str) -> Span {
	INSTANCE.read_recursive().span(
		|| { BlakeTwo256::hash_of(&descriptor) },
		span_name,
	).into()
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

	/// Spawn the background task in order to send the tracing information out via udp
	#[cfg(target_os = "unknown")]
	pub fn launch<S: SpawnNamed>(self, _spawner: S) -> result::Result<(), JaegerError> {
		Ok(())
	}

	/// Spawn the background task in order to send the tracing information out via udp
	#[cfg(not(target_os = "unknown"))]
	pub fn launch<S: SpawnNamed>(self, spawner: S) -> result::Result<(), JaegerError> {
		let cfg = match self {
			Self::Prep(cfg) => Ok(cfg),
			Self::Launched{ .. } => {
				return Err(JaegerError::AlreadyLaunched)
			}
			Self::None => Err(JaegerError::MissingConfiguration),
		}?;

		let jaeger_agent = cfg.agent_addr;

		log::info!("🐹 Collecting jaeger spans for {:?}", &jaeger_agent);

		let (traces_in, mut traces_out) = mick_jaeger::init(mick_jaeger::Config {
			service_name: format!("polkadot-{}", cfg.node_name),
		});

		// Spawn a background task that pulls span information and sends them on the network.
		spawner.spawn("jaeger-collector", Box::pin(async move {
			match async_std::net::UdpSocket::bind("0.0.0.0:0").await {
				Ok(udp_socket) => loop {
					let buf = traces_out.next().await;
					// UDP sending errors happen only either if the API is misused or in case of missing privilege.
					if let Err(e) = udp_socket.send_to(&buf, jaeger_agent).await {
						log::debug!(target: "jaeger", "UDP send error: {}", e);
					}
				}
				Err(e) => {
					log::warn!(target: "jaeger", "UDP socket open error: {}", e);
				}
			}
		}));

		*INSTANCE.write() = Self::Launched {
			traces_in,
		};
		Ok(())
	}

	fn span<F>(&self, lazy_hash: F, span_name: &'static str) -> Option<mick_jaeger::Span>
	where
		F: Fn() -> Hash,
	{
		if let Self::Launched { traces_in , .. } = self {
			let hash = lazy_hash();
			let mut buf = [0u8; 16];
			buf.copy_from_slice(&hash.as_ref()[0..16]);
			let trace_id = std::num::NonZeroU128::new(u128::from_be_bytes(buf))?;
			Some(traces_in.span(trace_id, span_name))
		} else {
			None
		}
	}
}
