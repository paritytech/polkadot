// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Message types for the overseer and subsystems.
//!
//! These messages are intended to define the protocol by which different subsystems communicate with each
//! other and signals that they receive from an overseer to coordinate their work.
//! This is intended for use with the `polkadot-overseer` crate.
//!
//! Subsystems' APIs are defined separately from their implementation, leading to easier mocking.

use futures::channel::{mpsc, oneshot};
use thiserror::Error;
use polkadot_node_network_protocol::{
	v1 as protocol_v1, NetworkBridgeEvent, ReputationChange, PeerId,
};
use polkadot_node_primitives::{
	CollationGenerationConfig, MisbehaviorReport, SignedFullStatement, ValidationResult,
};
use polkadot_primitives::v1::{
	AuthorityDiscoveryId, AvailableData, BackedCandidate, BlockNumber,
	Header as BlockHeader, CandidateDescriptor, CandidateEvent, CandidateReceipt,
	CollatorId, CommittedCandidateReceipt, CoreState, ErasureChunk,
	GroupRotationInfo, Hash, Id as ParaId, OccupiedCoreAssumption,
	PersistedValidationData, PoV, SessionIndex, SignedAvailabilityBitfield,
	ValidationCode, ValidatorId, ValidationData, CandidateHash,
	ValidatorIndex, ValidatorSignature, InboundDownwardMessage,
};
use std::sync::Arc;

/// A notification of a new backed candidate.
#[derive(Debug)]
pub struct NewBackedCandidate(pub BackedCandidate);

/// Messages received by the Candidate Selection subsystem.
#[derive(Debug)]
pub enum CandidateSelectionMessage {
	/// A candidate collation can be fetched from a collator and should be considered for seconding.
	Collation(Hash, ParaId, CollatorId),
	/// We recommended a particular candidate to be seconded, but it was invalid; penalize the collator.
	/// The hash is the relay parent.
	Invalid(Hash, CandidateReceipt),
}

impl CandidateSelectionMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::Collation(hash, ..) => Some(*hash),
			Self::Invalid(hash, _) => Some(*hash),
		}
	}
}

impl Default for CandidateSelectionMessage {
	fn default() -> Self {
		CandidateSelectionMessage::Invalid(Default::default(), Default::default())
	}
}

/// Messages received by the Candidate Backing subsystem.
#[derive(Debug)]
pub enum CandidateBackingMessage {
	/// Requests a set of backable candidates that could be backed in a child of the given
	/// relay-parent, referenced by its hash.
	GetBackedCandidates(Hash, oneshot::Sender<Vec<NewBackedCandidate>>),
	/// Note that the Candidate Backing subsystem should second the given candidate in the context of the
	/// given relay-parent (ref. by hash). This candidate must be validated.
	Second(Hash, CandidateReceipt, PoV),
	/// Note a validator's statement about a particular candidate. Disagreements about validity must be escalated
	/// to a broader check by Misbehavior Arbitration. Agreements are simply tallied until a quorum is reached.
	Statement(Hash, SignedFullStatement),
}

impl CandidateBackingMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::GetBackedCandidates(hash, _) => Some(*hash),
			Self::Second(hash, _, _) => Some(*hash),
			Self::Statement(hash, _) => Some(*hash),
		}
	}
}

/// Blanket error for validation failing for internal reasons.
#[derive(Debug, Error)]
#[error("Validation failed with {0:?}")]
pub struct ValidationFailed(pub String);

/// Messages received by the Validation subsystem.
///
/// ## Validation Requests
///
/// Validation requests made to the subsystem should return an error only on internal error.
/// Otherwise, they should return either `Ok(ValidationResult::Valid(_))`
/// or `Ok(ValidationResult::Invalid)`.
#[derive(Debug)]
pub enum CandidateValidationMessage {
	/// Validate a candidate with provided parameters using relay-chain state.
	///
	/// This will implicitly attempt to gather the `PersistedValidationData` and `ValidationCode`
	/// from the runtime API of the chain, based on the `relay_parent`
	/// of the `CandidateDescriptor`.
	///
	/// This will also perform checking of validation outputs against the acceptance criteria.
	///
	/// If there is no state available which can provide this data or the core for
	/// the para is not free at the relay-parent, an error is returned.
	ValidateFromChainState(
		CandidateDescriptor,
		Arc<PoV>,
		oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	),
	/// Validate a candidate with provided, exhaustive parameters for validation.
	///
	/// Explicitly provide the `PersistedValidationData` and `ValidationCode` so this can do full
	/// validation without needing to access the state of the relay-chain.
	///
	/// This request doesn't involve acceptance criteria checking, therefore only useful for the
	/// cases where the validity of the candidate is established. This is the case for the typical
	/// use-case: secondary checkers would use this request relying on the full prior checks
	/// performed by the relay-chain.
	ValidateFromExhaustive(
		PersistedValidationData,
		ValidationCode,
		CandidateDescriptor,
		Arc<PoV>,
		oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	),
}

impl CandidateValidationMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::ValidateFromChainState(_, _, _) => None,
			Self::ValidateFromExhaustive(_, _, _, _, _) => None,
		}
	}
}


/// Messages received by the Collator Protocol subsystem.
#[derive(Debug)]
pub enum CollatorProtocolMessage {
	/// Signal to the collator protocol that it should connect to validators with the expectation
	/// of collating on the given para. This is only expected to be called once, early on, if at all,
	/// and only by the Collation Generation subsystem. As such, it will overwrite the value of
	/// the previous signal.
	///
	/// This should be sent before any `DistributeCollation` message.
	CollateOn(ParaId),
	/// Provide a collation to distribute to validators.
	DistributeCollation(CandidateReceipt, PoV),
	/// Fetch a collation under the given relay-parent for the given ParaId.
	FetchCollation(Hash, CollatorId, ParaId, oneshot::Sender<(CandidateReceipt, PoV)>),
	/// Report a collator as having provided an invalid collation. This should lead to disconnect
	/// and blacklist of the collator.
	ReportCollator(CollatorId),
	/// Note a collator as having provided a good collation.
	NoteGoodCollation(CollatorId),
	/// Get a network bridge update.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<protocol_v1::CollatorProtocolMessage>),
}

impl CollatorProtocolMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::CollateOn(_) => None,
			Self::DistributeCollation(receipt, _) => Some(receipt.descriptor().relay_parent),
			Self::FetchCollation(relay_parent, _, _, _) => Some(*relay_parent),
			Self::ReportCollator(_) => None,
			Self::NoteGoodCollation(_) => None,
			Self::NetworkBridgeUpdateV1(_) => None,
		}
	}
}

/// Messages received by the network bridge subsystem.
#[derive(Debug)]
pub enum NetworkBridgeMessage {
	/// Report a peer for their actions.
	ReportPeer(PeerId, ReputationChange),

	/// Send a message to one or more peers on the validation peer-set.
	SendValidationMessage(Vec<PeerId>, protocol_v1::ValidationProtocol),

	/// Send a message to one or more peers on the collation peer-set.
	SendCollationMessage(Vec<PeerId>, protocol_v1::CollationProtocol),

	/// Connect to peers who represent the given `validator_ids`.
	///
	/// Also ask the network to stay connected to these peers at least
	/// until the request is revoked.
	ConnectToValidators {
		/// Ids of the validators to connect to.
		validator_ids: Vec<AuthorityDiscoveryId>,
		/// Response sender by which the issuer can learn the `PeerId`s of
		/// the validators as they are connected.
		/// The response is sent immediately for already connected peers.
		connected: mpsc::Sender<(AuthorityDiscoveryId, PeerId)>,
		/// By revoking the request the caller allows the network to
		/// free some peer slots thus freeing the resources.
		/// It doesn't necessarily lead to peers disconnection though.
		/// The revokation is enacted on in the next connection request.
		///
		/// This can be done by sending to the channel or dropping the sender.
		revoke: oneshot::Receiver<()>,
	},
}

impl NetworkBridgeMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::ReportPeer(_, _) => None,
			Self::SendValidationMessage(_, _) => None,
			Self::SendCollationMessage(_, _) => None,
			Self::ConnectToValidators { .. } => None,
		}
	}
}

/// Availability Distribution Message.
#[derive(Debug)]
pub enum AvailabilityDistributionMessage {
	/// Event from the network bridge.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<protocol_v1::AvailabilityDistributionMessage>),
}

impl AvailabilityDistributionMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::NetworkBridgeUpdateV1(_) => None,
		}
	}
}

/// Bitfield distribution message.
#[derive(Debug)]
pub enum BitfieldDistributionMessage {
	/// Distribute a bitfield via gossip to other validators.
	DistributeBitfield(Hash, SignedAvailabilityBitfield),

	/// Event from the network bridge.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<protocol_v1::BitfieldDistributionMessage>),
}

impl BitfieldDistributionMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::DistributeBitfield(hash, _) => Some(*hash),
			Self::NetworkBridgeUpdateV1(_) => None,
		}
	}
}

/// Bitfield signing message.
///
/// Currently non-instantiable.
#[derive(Debug)]
pub enum BitfieldSigningMessage {}

impl BitfieldSigningMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		None
	}
}

/// Availability store subsystem message.
#[derive(Debug)]
pub enum AvailabilityStoreMessage {
	/// Query a `AvailableData` from the AV store.
	QueryAvailableData(CandidateHash, oneshot::Sender<Option<AvailableData>>),

	/// Query whether a `AvailableData` exists within the AV Store.
	///
	/// This is useful in cases when existence
	/// matters, but we don't want to necessarily pass around multiple
	/// megabytes of data to get a single bit of information.
	QueryDataAvailability(CandidateHash, oneshot::Sender<bool>),

	/// Query an `ErasureChunk` from the AV store by the candidate hash and validator index.
	QueryChunk(CandidateHash, ValidatorIndex, oneshot::Sender<Option<ErasureChunk>>),

	/// Query whether an `ErasureChunk` exists within the AV Store.
	///
	/// This is useful in cases like bitfield signing, when existence
	/// matters, but we don't want to necessarily pass around large
	/// quantities of data to get a single bit of information.
	QueryChunkAvailability(CandidateHash, ValidatorIndex, oneshot::Sender<bool>),

	/// Store an `ErasureChunk` in the AV store.
	///
	/// Return `Ok(())` if the store operation succeeded, `Err(())` if it failed.
	StoreChunk {
		/// A hash of the candidate this chunk belongs to.
		candidate_hash: CandidateHash,
		/// A relevant relay parent.
		relay_parent: Hash,
		/// The index of the validator this chunk belongs to.
		validator_index: ValidatorIndex,
		/// The chunk itself.
		chunk: ErasureChunk,
		/// Sending side of the channel to send result to.
		tx: oneshot::Sender<Result<(), ()>>,
	},

	/// Store a `AvailableData` in the AV store.
	/// If `ValidatorIndex` is present store corresponding chunk also.
	///
	/// Return `Ok(())` if the store operation succeeded, `Err(())` if it failed.
	StoreAvailableData(CandidateHash, Option<ValidatorIndex>, u32, AvailableData, oneshot::Sender<Result<(), ()>>),
}

impl AvailabilityStoreMessage {
	/// In fact, none of the AvailabilityStore messages assume a particular relay parent.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			_ => None,
		}
	}
}

/// A response channel for the result of a chain API request.
pub type ChainApiResponseChannel<T> = oneshot::Sender<Result<T, crate::errors::ChainApiError>>;

/// Chain API request subsystem message.
#[derive(Debug)]
pub enum ChainApiMessage {
	/// Request the block number by hash.
	/// Returns `None` if a block with the given hash is not present in the db.
	BlockNumber(Hash, ChainApiResponseChannel<Option<BlockNumber>>),
	/// Request the block header by hash.
	/// Returns `None` if a block with the given hash is not present in the db.
	BlockHeader(Hash, ChainApiResponseChannel<Option<BlockHeader>>),
	/// Request the finalized block hash by number.
	/// Returns `None` if a block with the given number is not present in the db.
	/// Note: the caller must ensure the block is finalized.
	FinalizedBlockHash(BlockNumber, ChainApiResponseChannel<Option<Hash>>),
	/// Request the last finalized block number.
	/// This request always succeeds.
	FinalizedBlockNumber(ChainApiResponseChannel<BlockNumber>),
	/// Request the `k` ancestors block hashes of a block with the given hash.
	/// The response channel may return a `Vec` of size up to `k`
	/// filled with ancestors hashes with the following order:
	/// `parent`, `grandparent`, ...
	Ancestors {
		/// The hash of the block in question.
		hash: Hash,
		/// The number of ancestors to request.
		k: usize,
		/// The response channel.
		response_channel: ChainApiResponseChannel<Vec<Hash>>,
	},
}

impl ChainApiMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		None
	}
}

/// A sender for the result of a runtime API request.
pub type RuntimeApiSender<T> = oneshot::Sender<Result<T, crate::errors::RuntimeApiError>>;

/// A request to the Runtime API subsystem.
#[derive(Debug)]
pub enum RuntimeApiRequest {
	/// Get the current validator set.
	Validators(RuntimeApiSender<Vec<ValidatorId>>),
	/// Get the validator groups and group rotation info.
	ValidatorGroups(RuntimeApiSender<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo)>),
	/// Get information on all availability cores.
	AvailabilityCores(RuntimeApiSender<Vec<CoreState>>),
	/// Get the persisted validation data for a particular para, taking the given
	/// `OccupiedCoreAssumption`, which will inform on how the validation data should be computed
	/// if the para currently occupies a core.
	PersistedValidationData(
		ParaId,
		OccupiedCoreAssumption,
		RuntimeApiSender<Option<PersistedValidationData>>,
	),
	/// Get the full validation data for a particular para, taking the given
	/// `OccupiedCoreAssumption`, which will inform on how the validation data should be computed
	/// if the para currently occupies a core.
	FullValidationData(
		ParaId,
		OccupiedCoreAssumption,
		RuntimeApiSender<Option<ValidationData>>,
	),
	/// Sends back `true` if the validation outputs pass all acceptance criteria checks.
	CheckValidationOutputs(
		ParaId,
		polkadot_primitives::v1::ValidationOutputs,
		RuntimeApiSender<bool>,
	),
	/// Get the session index that a child of the block will have.
	SessionIndexForChild(RuntimeApiSender<SessionIndex>),
	/// Get the validation code for a para, taking the given `OccupiedCoreAssumption`, which
	/// will inform on how the validation data should be computed if the para currently
	/// occupies a core.
	ValidationCode(
		ParaId,
		OccupiedCoreAssumption,
		RuntimeApiSender<Option<ValidationCode>>,
	),
	/// Fetch the historical validation code used by a para for candidates executed in the
	/// context of a given block height in the current chain.
	///
	/// `context_height` may be no greater than the height of the block in whose
	/// state the runtime API is executed. Otherwise `None` is returned.
	HistoricalValidationCode(
		ParaId,
		BlockNumber,
		RuntimeApiSender<Option<ValidationCode>>,
	),
	/// Get a the candidate pending availability for a particular parachain by parachain / core index
	CandidatePendingAvailability(ParaId, RuntimeApiSender<Option<CommittedCandidateReceipt>>),
	/// Get all events concerning candidates (backing, inclusion, time-out) in the parent of
	/// the block in whose state this request is executed.
	CandidateEvents(RuntimeApiSender<Vec<CandidateEvent>>),
	/// Get the `AuthorityDiscoveryId`s corresponding to the given `ValidatorId`s.
	/// Currently this request is limited to validators in the current session.
	///
	/// Returns `None` for validators not found in the current session.
	ValidatorDiscovery(
		Vec<ValidatorId>,
		RuntimeApiSender<Vec<Option<AuthorityDiscoveryId>>>,
	),
	/// Get all the pending inbound messages in the downward message queue for a para.
	DmqContents(
		ParaId,
		RuntimeApiSender<Vec<InboundDownwardMessage<BlockNumber>>>,
	),
}

/// A message to the Runtime API subsystem.
#[derive(Debug)]
pub enum RuntimeApiMessage {
	/// Make a request of the runtime API against the post-state of the given relay-parent.
	Request(Hash, RuntimeApiRequest),
}

impl RuntimeApiMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::Request(hash, _) => Some(*hash),
		}
	}
}

/// Statement distribution message.
#[derive(Debug)]
pub enum StatementDistributionMessage {
	/// We have originated a signed statement in the context of
	/// given relay-parent hash and it should be distributed to other validators.
	Share(Hash, SignedFullStatement),
	/// Event from the network bridge.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<protocol_v1::StatementDistributionMessage>),
	/// Register a listener for shared statements.
	RegisterStatementListener(mpsc::Sender<SignedFullStatement>),
}

impl StatementDistributionMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::Share(hash, _) => Some(*hash),
			Self::NetworkBridgeUpdateV1(_) => None,
			Self::RegisterStatementListener(_) => None,
		}
	}
}

/// This data becomes intrinsics or extrinsics which should be included in a future relay chain block.
// It needs to be cloneable because multiple potential block authors can request copies.
#[derive(Debug, Clone)]
pub enum ProvisionableData {
	/// This bitfield indicates the availability of various candidate blocks.
	Bitfield(Hash, SignedAvailabilityBitfield),
	/// The Candidate Backing subsystem believes that this candidate is valid, pending availability.
	BackedCandidate(BackedCandidate),
	/// Misbehavior reports are self-contained proofs of validator misbehavior.
	MisbehaviorReport(Hash, MisbehaviorReport),
	/// Disputes trigger a broad dispute resolution process.
	Dispute(Hash, ValidatorSignature),
}

/// This data needs to make its way from the provisioner into the InherentData.
///
/// There, it is used to construct the InclusionInherent.
pub type ProvisionerInherentData = (Vec<SignedAvailabilityBitfield>, Vec<BackedCandidate>);

/// Message to the Provisioner.
///
/// In all cases, the Hash is that of the relay parent.
#[derive(Debug)]
pub enum ProvisionerMessage {
	/// This message allows potential block authors to be kept updated with all new authorship data
	/// as it becomes available.
	RequestBlockAuthorshipData(Hash, mpsc::Sender<ProvisionableData>),
	/// This message allows external subsystems to request the set of bitfields and backed candidates
	/// associated with a particular potential block hash.
	///
	/// This is expected to be used by a proposer, to inject that information into the InherentData
	/// where it can be assembled into the InclusionInherent.
	RequestInherentData(Hash, oneshot::Sender<ProvisionerInherentData>),
	/// This data should become part of a relay chain block
	ProvisionableData(Hash, ProvisionableData),
}

impl ProvisionerMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::RequestBlockAuthorshipData(hash, _) => Some(*hash),
			Self::RequestInherentData(hash, _) => Some(*hash),
			Self::ProvisionableData(hash, _) => Some(*hash),
		}
	}
}

/// Message to the PoV Distribution Subsystem.
#[derive(Debug)]
pub enum PoVDistributionMessage {
	/// Fetch a PoV from the network.
	///
	/// This `CandidateDescriptor` should correspond to a candidate seconded under the provided
	/// relay-parent hash.
	FetchPoV(Hash, CandidateDescriptor, oneshot::Sender<Arc<PoV>>),
	/// Distribute a PoV for the given relay-parent and CandidateDescriptor.
	/// The PoV should correctly hash to the PoV hash mentioned in the CandidateDescriptor
	DistributePoV(Hash, CandidateDescriptor, Arc<PoV>),
	/// An update from the network bridge.
	NetworkBridgeUpdateV1(NetworkBridgeEvent<protocol_v1::PoVDistributionMessage>),
}

impl PoVDistributionMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::FetchPoV(hash, _, _) => Some(*hash),
			Self::DistributePoV(hash, _, _) => Some(*hash),
			Self::NetworkBridgeUpdateV1(_) => None,
		}
	}
}

/// Message to the Collation Generation Subsystem.
#[derive(Debug)]
pub enum CollationGenerationMessage {
	/// Initialize the collation generation subsystem
	Initialize(CollationGenerationConfig),
}

impl CollationGenerationMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		None
	}
}

/// A message type tying together all message types that are used across Subsystems.
#[derive(Debug, derive_more::From)]
pub enum AllMessages {
	/// Message for the validation subsystem.
	CandidateValidation(CandidateValidationMessage),
	/// Message for the candidate backing subsystem.
	CandidateBacking(CandidateBackingMessage),
	/// Message for the candidate selection subsystem.
	CandidateSelection(CandidateSelectionMessage),
	/// Message for the Chain API subsystem.
	ChainApi(ChainApiMessage),
	/// Message for the Collator Protocol subsystem.
	CollatorProtocol(CollatorProtocolMessage),
	/// Message for the statement distribution subsystem.
	StatementDistribution(StatementDistributionMessage),
	/// Message for the availability distribution subsystem.
	AvailabilityDistribution(AvailabilityDistributionMessage),
	/// Message for the bitfield distribution subsystem.
	BitfieldDistribution(BitfieldDistributionMessage),
	/// Message for the bitfield signing subsystem.
	BitfieldSigning(BitfieldSigningMessage),
	/// Message for the Provisioner subsystem.
	Provisioner(ProvisionerMessage),
	/// Message for the PoV Distribution subsystem.
	PoVDistribution(PoVDistributionMessage),
	/// Message for the Runtime API subsystem.
	RuntimeApi(RuntimeApiMessage),
	/// Message for the availability store subsystem.
	AvailabilityStore(AvailabilityStoreMessage),
	/// Message for the network bridge subsystem.
	NetworkBridge(NetworkBridgeMessage),
	/// Message for the Collation Generation subsystem
	CollationGeneration(CollationGenerationMessage),
}
