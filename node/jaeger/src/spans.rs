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

//! Polkadot Jaeger span definitions.
//!
//! ```rust
//! unimplemented!("How to use jaeger span definitions?");
//! ```


use polkadot_primitives::v1::{CandidateHash, Hash, PoV, ValidatorIndex, BlakeTwo256, HashT, Id as ParaId};
use parity_scale_codec::Encode;
use sc_network::PeerId;

use std::sync::Arc;

use super::INSTANCE;

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

	/// Attach a para-id to the span.
	#[inline(always)]
	pub fn with_para_id(mut self, para_id: ParaId) -> Self {
		self.span.add_para_id(para_id);
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

	/// Add the para-id to the span.
	pub fn add_para_id(&mut self, para_id: ParaId) {
		self.add_int_tag("para-id", u32::from(para_id) as i64);
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
