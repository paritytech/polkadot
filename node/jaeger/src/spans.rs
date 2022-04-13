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
//! # use polkadot_primitives::v2::{CandidateHash, Hash};
//! # fn main() {
//! use polkadot_node_jaeger as jaeger;
//!
//! let relay_parent = Hash::default();
//! let candidate = CandidateHash::default();
//!
//! #[derive(Debug, Default)]
//! struct Foo {
//! 	a: u8,
//! 	b: u16,
//! 	c: u32,
//! };
//!
//! let foo = Foo::default();
//!
//! let span =
//! 	jaeger::Span::new(relay_parent, "root_of_aaall_spans")
//! 		// explicit well defined items
//! 		.with_candidate(candidate)
//! 		// anything that implements `trait std::fmt::Debug`
//! 		.with_string_fmt_debug_tag("foo", foo)
//! 		// anything that implements `trait std::str::ToString`
//! 		.with_string_tag("again", 1337_u32)
//! 		// add a `Stage` for [`dot-jaeger`](https://github.com/paritytech/dot-jaeger)
//! 		.with_stage(jaeger::Stage::CandidateBacking);
//! 		// complete by design, no completion required
//! # }
//! ```
//!
//! In a few cases additional annotations might want to be added
//! over the course of a function, for this purpose use the non-consuming
//! `fn` variants, i.e.
//! ```rust
//! # use polkadot_primitives::v2::{CandidateHash, Hash};
//! # fn main() {
//! # use polkadot_node_jaeger as jaeger;
//!
//! # let relay_parent = Hash::default();
//! # let candidate = CandidateHash::default();
//!
//! # #[derive(Debug, Default)]
//! # struct Foo {
//! # 	a: u8,
//! # 	b: u16,
//! # 	c: u32,
//! # };
//! #
//! # let foo = Foo::default();
//!
//! let root_span =
//! 	jaeger::Span::new(relay_parent, "root_of_aaall_spans");
//!
//! // the prefered way of adding additional delayed information:
//! let span = root_span.child("inner");
//!
//! // ... more operations ...
//!
//! // but this is also possible:
//!
//! let mut root_span = root_span;
//! root_span.add_string_fmt_debug_tag("foo_constructed", &foo);
//! root_span.add_string_tag("bar", true);
//! # }
//! ```

use parity_scale_codec::Encode;
use polkadot_node_primitives::PoV;
use polkadot_primitives::v2::{
	BlakeTwo256, CandidateHash, Hash, HashT, Id as ParaId, ValidatorIndex,
};
use sc_network::PeerId;

use std::{fmt, sync::Arc};

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

		Self { span, leaf_span }
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

/// A wrapper type for a span.
///
/// Handles running with and without jaeger.
pub enum Span {
	/// Running with jaeger being enabled.
	Enabled(mick_jaeger::Span),
	/// Running with jaeger disabled.
	Disabled,
}

/// Alias for the 16 byte unique identifier used with jaeger.
pub(crate) type TraceIdentifier = u128;

/// A helper to convert the hash to the fixed size representation
/// needed for jaeger.
#[inline]
pub fn hash_to_trace_identifier(hash: Hash) -> TraceIdentifier {
	let mut buf = [0u8; 16];
	buf.copy_from_slice(&hash.as_ref()[0..16]);
	// The slice bytes are copied in reading order, so if interpreted
	// in string form by a human, that means lower indices have higher
	// values and hence corresponds to BIG endian ordering of the individual
	// bytes.
	u128::from_be_bytes(buf) as TraceIdentifier
}

/// Helper to unify lazy proxy evaluation.
pub trait LazyIdent {
	/// Evaluate the type to a unique trace identifier.
	/// Called lazily on demand.
	fn eval(&self) -> TraceIdentifier;

	/// Annotate a new root item with these additional spans
	/// at construction.
	fn extra_tags(&self, _span: &mut Span) {}
}

impl<'a> LazyIdent for &'a [u8] {
	fn eval(&self) -> TraceIdentifier {
		hash_to_trace_identifier(BlakeTwo256::hash_of(self))
	}
}

impl LazyIdent for &PoV {
	fn eval(&self) -> TraceIdentifier {
		hash_to_trace_identifier(self.hash())
	}

	fn extra_tags(&self, span: &mut Span) {
		span.add_pov(self)
	}
}

impl LazyIdent for Hash {
	fn eval(&self) -> TraceIdentifier {
		hash_to_trace_identifier(*self)
	}

	fn extra_tags(&self, span: &mut Span) {
		span.add_string_fmt_debug_tag("relay-parent", self);
	}
}

impl LazyIdent for &Hash {
	fn eval(&self) -> TraceIdentifier {
		hash_to_trace_identifier(**self)
	}

	fn extra_tags(&self, span: &mut Span) {
		span.add_string_fmt_debug_tag("relay-parent", self);
	}
}

impl LazyIdent for CandidateHash {
	fn eval(&self) -> TraceIdentifier {
		hash_to_trace_identifier(self.0)
	}

	fn extra_tags(&self, span: &mut Span) {
		span.add_string_fmt_debug_tag("candidate-hash", &self.0);
		// A convenience for usage with the grafana tempo UI,
		// not a technical requirement. It merely provides an easy anchor
		// where the true trace identifier of the span is not based on
		// a candidate hash (which it should be!), but is required to
		// continue investigating.
		span.add_string_tag("traceID", self.eval().to_string());
	}
}

impl Span {
	/// Creates a new span builder based on anything that can be lazily evaluated
	/// to and identifier.
	///
	/// Attention: The primary identifier will be used for identification
	/// and as such should be
	pub fn new<I: LazyIdent>(identifier: I, span_name: &'static str) -> Span {
		let mut span = INSTANCE
			.read_recursive()
			.span(|| <I as LazyIdent>::eval(&identifier), span_name)
			.into();
		<I as LazyIdent>::extra_tags(&identifier, &mut span);
		span
	}

	/// Creates a new span builder based on an encodable type.
	/// The encoded bytes are then used to derive the true trace identifier.
	pub fn from_encodable<I: Encode>(identifier: I, span_name: &'static str) -> Span {
		INSTANCE
			.read_recursive()
			.span(
				move || {
					let bytes = identifier.encode();
					LazyIdent::eval(&bytes.as_slice())
				},
				span_name,
			)
			.into()
	}

	/// Derive a child span from `self`.
	pub fn child(&self, name: &'static str) -> Self {
		match self {
			Self::Enabled(inner) => Self::Enabled(inner.child(name)),
			Self::Disabled => Self::Disabled,
		}
	}

	#[inline(always)]
	pub fn with_string_tag<V: ToString>(mut self, tag: &'static str, val: V) -> Self {
		self.add_string_tag::<V>(tag, val);
		self
	}

	#[inline(always)]
	pub fn with_peer_id(self, peer: &PeerId) -> Self {
		self.with_string_tag("peer-id", &peer.to_base58())
	}

	/// Attach a candidate hash to the span.
	#[inline(always)]
	pub fn with_candidate(self, candidate_hash: CandidateHash) -> Self {
		self.with_string_fmt_debug_tag("candidate-hash", &candidate_hash.0)
	}

	/// Attach a para-id to the span.
	#[inline(always)]
	pub fn with_para_id(self, para_id: ParaId) -> Self {
		self.with_int_tag("para-id", u32::from(para_id) as i64)
	}

	/// Attach a candidate stage.
	/// Should always come with a `CandidateHash`.
	#[inline(always)]
	pub fn with_stage(self, stage: Stage) -> Self {
		self.with_string_tag("candidate-stage", stage as u8)
	}

	#[inline(always)]
	pub fn with_validator_index(self, validator: ValidatorIndex) -> Self {
		self.with_string_tag("validator-index", &validator.0)
	}

	#[inline(always)]
	pub fn with_chunk_index(self, chunk_index: u32) -> Self {
		self.with_string_tag("chunk-index", chunk_index)
	}

	#[inline(always)]
	pub fn with_relay_parent(self, relay_parent: Hash) -> Self {
		self.with_string_fmt_debug_tag("relay-parent", relay_parent)
	}

	#[inline(always)]
	pub fn with_claimed_validator_index(self, claimed_validator_index: ValidatorIndex) -> Self {
		self.with_string_tag("claimed-validator", &claimed_validator_index.0)
	}

	#[inline(always)]
	pub fn with_pov(mut self, pov: &PoV) -> Self {
		self.add_pov(pov);
		self
	}

	/// Add an additional int tag to the span without consuming.
	///
	/// Should be used sparingly, introduction of new types is preferred.
	#[inline(always)]
	pub fn with_int_tag(mut self, tag: &'static str, i: i64) -> Self {
		self.add_int_tag(tag, i);
		self
	}

	#[inline(always)]
	pub fn with_uint_tag(mut self, tag: &'static str, u: u64) -> Self {
		self.add_uint_tag(tag, u);
		self
	}

	#[inline(always)]
	pub fn with_string_fmt_debug_tag<V: fmt::Debug>(mut self, tag: &'static str, val: V) -> Self {
		self.add_string_tag(tag, format!("{:?}", val));
		self
	}

	/// Adds the `FollowsFrom` relationship to this span with respect to the given one.
	#[inline(always)]
	pub fn add_follows_from(&mut self, other: &Self) {
		match (self, other) {
			(Self::Enabled(ref mut inner), Self::Enabled(ref other_inner)) =>
				inner.add_follows_from(&other_inner),
			_ => {},
		}
	}

	/// Add a PoV hash meta tag with lazy hash evaluation, without consuming the span.
	#[inline(always)]
	pub fn add_pov(&mut self, pov: &PoV) {
		if self.is_enabled() {
			// avoid computing the PoV hash if jaeger is not enabled
			self.add_string_fmt_debug_tag("pov", pov.hash());
		}
	}

	#[inline(always)]
	pub fn add_para_id(&mut self, para_id: ParaId) {
		self.add_int_tag("para-id", u32::from(para_id) as i64);
	}

	/// Add a string tag, without consuming the span.
	pub fn add_string_tag<V: ToString>(&mut self, tag: &'static str, val: V) {
		match self {
			Self::Enabled(ref mut inner) => inner.add_string_tag(tag, val.to_string().as_str()),
			Self::Disabled => {},
		}
	}

	/// Add a string tag, without consuming the span.
	pub fn add_string_fmt_debug_tag<V: fmt::Debug>(&mut self, tag: &'static str, val: V) {
		match self {
			Self::Enabled(ref mut inner) =>
				inner.add_string_tag(tag, format!("{:?}", val).as_str()),
			Self::Disabled => {},
		}
	}

	pub fn add_int_tag(&mut self, tag: &'static str, value: i64) {
		match self {
			Self::Enabled(ref mut inner) => inner.add_int_tag(tag, value),
			Self::Disabled => {},
		}
	}

	pub fn add_uint_tag(&mut self, tag: &'static str, value: u64) {
		match self {
			Self::Enabled(ref mut inner) => inner.add_int_tag(tag, value as i64),
			Self::Disabled => {},
		}
	}

	/// Check whether jaeger is enabled
	/// in order to avoid computational overhead.
	pub const fn is_enabled(&self) -> bool {
		match self {
			Span::Enabled(_) => true,
			_ => false,
		}
	}

	/// Obtain the trace identifier for this set of spans.
	pub fn trace_id(&self) -> Option<TraceIdentifier> {
		match self {
			Span::Enabled(inner) => Some(inner.trace_id().get()),
			_ => None,
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::Jaeger;

	// make sure to not use `::repeat_*()` based samples, since this does not verify endianness
	const RAW: [u8; 32] = [
		0xFF, 0xAA, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x78, 0x89, 0x9A, 0xAB, 0xBC, 0xCD, 0xDE,
		0xEF, 0x00, 0x01, 0x02, 0x03, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
		0x0E, 0x0F,
	];

	#[test]
	fn hash_derived_identifier_is_leading_16bytes() {
		let candidate_hash = dbg!(Hash::from(&RAW));
		let trace_id = dbg!(hash_to_trace_identifier(candidate_hash));
		for (idx, (a, b)) in candidate_hash
			.as_bytes()
			.iter()
			.take(16)
			.zip(trace_id.to_be_bytes().iter())
			.enumerate()
		{
			assert_eq!(*a, *b, "Index [{}] does not match: {} != {}", idx, a, b);
		}
	}

	#[test]
	fn extra_tags_do_not_change_trace_id() {
		Jaeger::test_setup();
		let candidate_hash = dbg!(Hash::from(&RAW));
		let trace_id = hash_to_trace_identifier(candidate_hash);

		let span = Span::new(candidate_hash, "foo");

		assert_eq!(span.trace_id(), Some(trace_id));

		let span = span.with_int_tag("tag", 7i64);

		assert_eq!(span.trace_id(), Some(trace_id));
	}
}
