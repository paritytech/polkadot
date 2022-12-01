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

//! Network protocol types for parachains.

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]

use parity_scale_codec::{Decode, Encode};
use polkadot_primitives::v2::{BlockNumber, Hash};
use std::{collections::HashMap, fmt};

#[doc(hidden)]
pub use polkadot_node_jaeger as jaeger;
pub use sc_network::{IfDisconnected, PeerId};
#[doc(hidden)]
pub use std::sync::Arc;

mod reputation;
pub use self::reputation::{ReputationChange, UnifiedReputationChange};

/// Peer-sets and protocols used for parachains.
pub mod peer_set;

/// Request/response protocols used in Polkadot.
pub mod request_response;

/// Accessing authority discovery service
pub mod authority_discovery;
/// Grid topology support module
pub mod grid_topology;

/// The minimum amount of peers to send gossip messages to.
pub const MIN_GOSSIP_PEERS: usize = 25;

/// An error indicating that this the over-arching message type had the wrong variant
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WrongVariant;

impl fmt::Display for WrongVariant {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "Wrong message variant")
	}
}

impl std::error::Error for WrongVariant {}

/// The advertised role of a node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObservedRole {
	/// A light node.
	Light,
	/// A full node.
	Full,
	/// A node claiming to be an authority (unauthenticated)
	Authority,
}

impl From<sc_network::ObservedRole> for ObservedRole {
	fn from(role: sc_network::ObservedRole) -> ObservedRole {
		match role {
			sc_network::ObservedRole::Light => ObservedRole::Light,
			sc_network::ObservedRole::Authority => ObservedRole::Authority,
			sc_network::ObservedRole::Full => ObservedRole::Full,
		}
	}
}

impl Into<sc_network::ObservedRole> for ObservedRole {
	fn into(self) -> sc_network::ObservedRole {
		match self {
			ObservedRole::Light => sc_network::ObservedRole::Light,
			ObservedRole::Full => sc_network::ObservedRole::Full,
			ObservedRole::Authority => sc_network::ObservedRole::Authority,
		}
	}
}

/// Specialized wrapper around [`View`].
///
/// Besides the access to the view itself, it also gives access to the [`jaeger::Span`] per leave/head.
#[derive(Debug, Clone, Default)]
pub struct OurView {
	view: View,
	span_per_head: HashMap<Hash, Arc<jaeger::Span>>,
}

impl OurView {
	/// Creates a new instance.
	pub fn new(
		heads: impl IntoIterator<Item = (Hash, Arc<jaeger::Span>)>,
		finalized_number: BlockNumber,
	) -> Self {
		let state_per_head = heads.into_iter().collect::<HashMap<_, _>>();
		let view = View::new(state_per_head.keys().cloned(), finalized_number);
		Self { view, span_per_head: state_per_head }
	}

	/// Returns the span per head map.
	///
	/// For each head there exists one span in this map.
	pub fn span_per_head(&self) -> &HashMap<Hash, Arc<jaeger::Span>> {
		&self.span_per_head
	}
}

impl PartialEq for OurView {
	fn eq(&self, other: &Self) -> bool {
		self.view == other.view
	}
}

impl std::ops::Deref for OurView {
	type Target = View;

	fn deref(&self) -> &View {
		&self.view
	}
}

/// Construct a new [`OurView`] with the given chain heads, finalized number 0 and disabled [`jaeger::Span`]'s.
///
/// NOTE: Use for tests only.
///
/// # Example
///
/// ```
/// # use polkadot_node_network_protocol::our_view;
/// # use polkadot_primitives::v2::Hash;
/// let our_view = our_view![Hash::repeat_byte(1), Hash::repeat_byte(2)];
/// ```
#[macro_export]
macro_rules! our_view {
	( $( $hash:expr ),* $(,)? ) => {
		$crate::OurView::new(
			vec![ $( $hash.clone() ),* ].into_iter().map(|h| (h, $crate::Arc::new($crate::jaeger::Span::Disabled))),
			0,
		)
	};
}

/// A succinct representation of a peer's view. This consists of a bounded amount of chain heads
/// and the highest known finalized block number.
///
/// Up to `N` (5?) chain heads.
#[derive(Default, Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct View {
	/// A bounded amount of chain heads.
	/// Invariant: Sorted.
	heads: Vec<Hash>,
	/// The highest known finalized block number.
	pub finalized_number: BlockNumber,
}

/// Construct a new view with the given chain heads and finalized number 0.
///
/// NOTE: Use for tests only.
///
/// # Example
///
/// ```
/// # use polkadot_node_network_protocol::view;
/// # use polkadot_primitives::v2::Hash;
/// let view = view![Hash::repeat_byte(1), Hash::repeat_byte(2)];
/// ```
#[macro_export]
macro_rules! view {
	( $( $hash:expr ),* $(,)? ) => {
		$crate::View::new(vec![ $( $hash.clone() ),* ], 0)
	};
}

impl View {
	/// Construct a new view based on heads and a finalized block number.
	pub fn new(heads: impl IntoIterator<Item = Hash>, finalized_number: BlockNumber) -> Self {
		let mut heads = heads.into_iter().collect::<Vec<Hash>>();
		heads.sort();
		Self { heads, finalized_number }
	}

	/// Start with no heads, but only a finalized block number.
	pub fn with_finalized(finalized_number: BlockNumber) -> Self {
		Self { heads: Vec::new(), finalized_number }
	}

	/// Obtain the number of heads that are in view.
	pub fn len(&self) -> usize {
		self.heads.len()
	}

	/// Check if the number of heads contained, is null.
	pub fn is_empty(&self) -> bool {
		self.heads.is_empty()
	}

	/// Obtain an iterator over all heads.
	pub fn iter(&self) -> impl Iterator<Item = &Hash> {
		self.heads.iter()
	}

	/// Obtain an iterator over all heads.
	pub fn into_iter(self) -> impl Iterator<Item = Hash> {
		self.heads.into_iter()
	}

	/// Replace `self` with `new`.
	///
	/// Returns an iterator that will yield all elements of `new` that were not part of `self`.
	pub fn replace_difference(&mut self, new: View) -> impl Iterator<Item = &Hash> {
		let old = std::mem::replace(self, new);

		self.heads.iter().filter(move |h| !old.contains(h))
	}

	/// Returns an iterator of the hashes present in `Self` but not in `other`.
	pub fn difference<'a>(&'a self, other: &'a View) -> impl Iterator<Item = &'a Hash> + 'a {
		self.heads.iter().filter(move |h| !other.contains(h))
	}

	/// An iterator containing hashes present in both `Self` and in `other`.
	pub fn intersection<'a>(&'a self, other: &'a View) -> impl Iterator<Item = &'a Hash> + 'a {
		self.heads.iter().filter(move |h| other.contains(h))
	}

	/// Whether the view contains a given hash.
	pub fn contains(&self, hash: &Hash) -> bool {
		self.heads.contains(hash)
	}

	/// Check if two views have the same heads.
	///
	/// Equivalent to the `PartialEq` function,
	/// but ignores the `finalized_number` field.
	pub fn check_heads_eq(&self, other: &Self) -> bool {
		self.heads == other.heads
	}
}

/// A protocol-versioned type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Versioned<V1> {
	/// V1 type.
	V1(V1),
}

impl<V1: Clone> Versioned<&'_ V1> {
	/// Convert to a fully-owned version of the message.
	pub fn clone_inner(&self) -> Versioned<V1> {
		match *self {
			Versioned::V1(inner) => Versioned::V1(inner.clone()),
		}
	}
}

/// All supported versions of the validation protocol message.
pub type VersionedValidationProtocol = Versioned<v1::ValidationProtocol>;

impl From<v1::ValidationProtocol> for VersionedValidationProtocol {
	fn from(v1: v1::ValidationProtocol) -> Self {
		VersionedValidationProtocol::V1(v1)
	}
}

/// All supported versions of the collation protocol message.
pub type VersionedCollationProtocol = Versioned<v1::CollationProtocol>;

impl From<v1::CollationProtocol> for VersionedCollationProtocol {
	fn from(v1: v1::CollationProtocol) -> Self {
		VersionedCollationProtocol::V1(v1)
	}
}

macro_rules! impl_versioned_full_protocol_from {
	($from:ty, $out:ty, $variant:ident) => {
		impl From<$from> for $out {
			fn from(versioned_from: $from) -> $out {
				match versioned_from {
					Versioned::V1(x) => Versioned::V1(x.into()),
				}
			}
		}
	};
}

/// Implement `TryFrom` for one versioned enum variant into the inner type.
/// `$m_ty::$variant(inner) -> Ok(inner)`
macro_rules! impl_versioned_try_from {
	($from:ty, $out:ty, $v1_pat:pat => $v1_out:expr) => {
		impl TryFrom<$from> for $out {
			type Error = crate::WrongVariant;

			fn try_from(x: $from) -> Result<$out, Self::Error> {
				#[allow(unreachable_patterns)] // when there is only one variant
				match x {
					Versioned::V1($v1_pat) => Ok(Versioned::V1($v1_out)),
					_ => Err(crate::WrongVariant),
				}
			}
		}

		impl<'a> TryFrom<&'a $from> for $out {
			type Error = crate::WrongVariant;

			fn try_from(x: &'a $from) -> Result<$out, Self::Error> {
				#[allow(unreachable_patterns)] // when there is only one variant
				match x {
					Versioned::V1($v1_pat) => Ok(Versioned::V1($v1_out.clone())),
					_ => Err(crate::WrongVariant),
				}
			}
		}
	};
}

/// Version-annotated messages used by the bitfield distribution subsystem.
pub type BitfieldDistributionMessage = Versioned<v1::BitfieldDistributionMessage>;
impl_versioned_full_protocol_from!(
	BitfieldDistributionMessage,
	VersionedValidationProtocol,
	BitfieldDistribution
);
impl_versioned_try_from!(
	VersionedValidationProtocol,
	BitfieldDistributionMessage,
	v1::ValidationProtocol::BitfieldDistribution(x) => x
);

/// Version-annotated messages used by the statement distribution subsystem.
pub type StatementDistributionMessage = Versioned<v1::StatementDistributionMessage>;
impl_versioned_full_protocol_from!(
	StatementDistributionMessage,
	VersionedValidationProtocol,
	StatementDistribution
);
impl_versioned_try_from!(
	VersionedValidationProtocol,
	StatementDistributionMessage,
	v1::ValidationProtocol::StatementDistribution(x) => x
);

/// Version-annotated messages used by the approval distribution subsystem.
pub type ApprovalDistributionMessage = Versioned<v1::ApprovalDistributionMessage>;
impl_versioned_full_protocol_from!(
	ApprovalDistributionMessage,
	VersionedValidationProtocol,
	ApprovalDistribution
);
impl_versioned_try_from!(
	VersionedValidationProtocol,
	ApprovalDistributionMessage,
	v1::ValidationProtocol::ApprovalDistribution(x) => x
);

/// Version-annotated messages used by the gossip-support subsystem (this is void).
pub type GossipSupportNetworkMessage = Versioned<v1::GossipSupportNetworkMessage>;
// This is a void enum placeholder, so never gets sent over the wire.
impl TryFrom<VersionedValidationProtocol> for GossipSupportNetworkMessage {
	type Error = WrongVariant;
	fn try_from(_: VersionedValidationProtocol) -> Result<Self, Self::Error> {
		Err(WrongVariant)
	}
}

impl<'a> TryFrom<&'a VersionedValidationProtocol> for GossipSupportNetworkMessage {
	type Error = WrongVariant;
	fn try_from(_: &'a VersionedValidationProtocol) -> Result<Self, Self::Error> {
		Err(WrongVariant)
	}
}

/// Version-annotated messages used by the bitfield distribution subsystem.
pub type CollatorProtocolMessage = Versioned<v1::CollatorProtocolMessage>;
impl_versioned_full_protocol_from!(
	CollatorProtocolMessage,
	VersionedCollationProtocol,
	CollatorProtocol
);
impl_versioned_try_from!(
	VersionedCollationProtocol,
	CollatorProtocolMessage,
	v1::CollationProtocol::CollatorProtocol(x) => x
);

/// v1 notification protocol types.
pub mod v1 {
	use parity_scale_codec::{Decode, Encode};

	use polkadot_primitives::v2::{
		CandidateHash, CandidateIndex, CollatorId, CollatorSignature, CompactStatement, Hash,
		Id as ParaId, UncheckedSignedAvailabilityBitfield, ValidatorIndex, ValidatorSignature,
	};

	use polkadot_node_primitives::{
		approval::{IndirectAssignmentCert, IndirectSignedApprovalVote},
		UncheckedSignedFullStatement,
	};

	/// Network messages used by the bitfield distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum BitfieldDistributionMessage {
		/// A signed availability bitfield for a given relay-parent hash.
		#[codec(index = 0)]
		Bitfield(Hash, UncheckedSignedAvailabilityBitfield),
	}

	/// Network messages used by the statement distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum StatementDistributionMessage {
		/// A signed full statement under a given relay-parent.
		#[codec(index = 0)]
		Statement(Hash, UncheckedSignedFullStatement),
		/// Seconded statement with large payload (e.g. containing a runtime upgrade).
		///
		/// We only gossip the hash in that case, actual payloads can be fetched from sending node
		/// via request/response.
		#[codec(index = 1)]
		LargeStatement(StatementMetadata),
	}

	/// Data that makes a statement unique.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq, Hash)]
	pub struct StatementMetadata {
		/// Relay parent this statement is relevant under.
		pub relay_parent: Hash,
		/// Hash of the candidate that got validated.
		pub candidate_hash: CandidateHash,
		/// Validator that attested the validity.
		pub signed_by: ValidatorIndex,
		/// Signature of seconding validator.
		pub signature: ValidatorSignature,
	}

	impl StatementDistributionMessage {
		/// Get fingerprint describing the contained statement uniquely.
		pub fn get_fingerprint(&self) -> (CompactStatement, ValidatorIndex) {
			match self {
				Self::Statement(_, statement) => (
					statement.unchecked_payload().to_compact(),
					statement.unchecked_validator_index(),
				),
				Self::LargeStatement(meta) =>
					(CompactStatement::Seconded(meta.candidate_hash), meta.signed_by),
			}
		}

		/// Get the signature from the statement.
		pub fn get_signature(&self) -> ValidatorSignature {
			match self {
				Self::Statement(_, statement) => statement.unchecked_signature().clone(),
				Self::LargeStatement(metadata) => metadata.signature.clone(),
			}
		}

		/// Get contained relay parent.
		pub fn get_relay_parent(&self) -> Hash {
			match self {
				Self::Statement(r, _) => *r,
				Self::LargeStatement(meta) => meta.relay_parent,
			}
		}

		/// Whether this message contains a large statement.
		pub fn is_large_statement(&self) -> bool {
			if let Self::LargeStatement(_) = self {
				true
			} else {
				false
			}
		}
	}

	/// Network messages used by the approval distribution subsystem.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum ApprovalDistributionMessage {
		/// Assignments for candidates in recent, unfinalized blocks.
		///
		/// Actually checking the assignment may yield a different result.
		#[codec(index = 0)]
		Assignments(Vec<(IndirectAssignmentCert, CandidateIndex)>),
		/// Approvals for candidates in some recent, unfinalized block.
		#[codec(index = 1)]
		Approvals(Vec<IndirectSignedApprovalVote>),
	}

	/// Dummy network message type, so we will receive connect/disconnect events.
	#[derive(Debug, Clone, PartialEq, Eq)]
	pub enum GossipSupportNetworkMessage {}

	/// Network messages used by the collator protocol subsystem
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq)]
	pub enum CollatorProtocolMessage {
		/// Declare the intent to advertise collations under a collator ID, attaching a
		/// signature of the `PeerId` of the node using the given collator ID key.
		#[codec(index = 0)]
		Declare(CollatorId, ParaId, CollatorSignature),
		/// Advertise a collation to a validator. Can only be sent once the peer has
		/// declared that they are a collator with given ID.
		#[codec(index = 1)]
		AdvertiseCollation(Hash),
		/// A collation sent to a validator was seconded.
		#[codec(index = 4)]
		CollationSeconded(Hash, UncheckedSignedFullStatement),
	}

	/// All network messages on the validation peer-set.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq, derive_more::From)]
	pub enum ValidationProtocol {
		/// Bitfield distribution messages
		#[codec(index = 1)]
		#[from]
		BitfieldDistribution(BitfieldDistributionMessage),
		/// Statement distribution messages
		#[codec(index = 3)]
		#[from]
		StatementDistribution(StatementDistributionMessage),
		/// Approval distribution messages
		#[codec(index = 4)]
		#[from]
		ApprovalDistribution(ApprovalDistributionMessage),
	}

	/// All network messages on the collation peer-set.
	#[derive(Debug, Clone, Encode, Decode, PartialEq, Eq, derive_more::From)]
	pub enum CollationProtocol {
		/// Collator protocol messages
		#[codec(index = 0)]
		#[from]
		CollatorProtocol(CollatorProtocolMessage),
	}

	/// Get the payload that should be signed and included in a `Declare` message.
	///
	/// The payload is the local peer id of the node, which serves to prove that it
	/// controls the collator key it is declaring an intention to collate under.
	pub fn declare_signature_payload(peer_id: &sc_network::PeerId) -> Vec<u8> {
		let mut payload = peer_id.to_bytes();
		payload.extend_from_slice(b"COLL");
		payload
	}
}
