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

use futures::channel::oneshot;
use sc_network::Multiaddr;
use thiserror::Error;

pub use sc_network::IfDisconnected;

use polkadot_node_network_protocol::{
	self as net_protocol, peer_set::PeerSet, request_response::Requests, PeerId,
	UnifiedReputationChange,
};
use polkadot_node_primitives::{
	approval::{BlockApprovalMeta, IndirectAssignmentCert, IndirectSignedApprovalVote},
	AvailableData, BabeEpoch, BlockWeight, CandidateVotes, CollationGenerationConfig,
	CollationSecondedSignal, DisputeMessage, DisputeStatus, ErasureChunk, PoV,
	SignedDisputeStatement, SignedFullStatement, ValidationResult,
};
use polkadot_primitives::v2::{
	AuthorityDiscoveryId, BackedCandidate, BlockNumber, CandidateEvent, CandidateHash,
	CandidateIndex, CandidateReceipt, CollatorId, CommittedCandidateReceipt, CoreState,
	DisputeState, GroupIndex, GroupRotationInfo, Hash, Header as BlockHeader, Id as ParaId,
	InboundDownwardMessage, InboundHrmpMessage, MultiDisputeStatementSet, OccupiedCoreAssumption,
	PersistedValidationData, PvfCheckStatement, SessionIndex, SessionInfo,
	SignedAvailabilityBitfield, SignedAvailabilityBitfields, ValidationCode, ValidationCodeHash,
	ValidatorId, ValidatorIndex, ValidatorSignature,
};
use polkadot_statement_table::v2::Misbehavior;
use std::{
	collections::{BTreeMap, HashMap, HashSet},
	sync::Arc,
	time::Duration,
};

/// Network events as transmitted to other subsystems, wrapped in their message types.
pub mod network_bridge_event;
pub use network_bridge_event::NetworkBridgeEvent;

/// Subsystem messages where each message is always bound to a relay parent.
pub trait BoundToRelayParent {
	/// Returns the relay parent this message is bound to.
	fn relay_parent(&self) -> Hash;
}

/// Messages received by the Candidate Backing subsystem.
#[derive(Debug)]
pub enum CandidateBackingMessage {
	/// Requests a set of backable candidates that could be backed in a child of the given
	/// relay-parent, referenced by its hash.
	GetBackedCandidates(Hash, Vec<CandidateHash>, oneshot::Sender<Vec<BackedCandidate>>),
	/// Note that the Candidate Backing subsystem should second the given candidate in the context of the
	/// given relay-parent (ref. by hash). This candidate must be validated.
	Second(Hash, CandidateReceipt, PoV),
	/// Note a validator's statement about a particular candidate. Disagreements about validity must be escalated
	/// to a broader check by Misbehavior Arbitration. Agreements are simply tallied until a quorum is reached.
	Statement(Hash, SignedFullStatement),
}

impl BoundToRelayParent for CandidateBackingMessage {
	fn relay_parent(&self) -> Hash {
		match self {
			Self::GetBackedCandidates(hash, _, _) => *hash,
			Self::Second(hash, _, _) => *hash,
			Self::Statement(hash, _) => *hash,
		}
	}
}

/// Blanket error for validation failing for internal reasons.
#[derive(Debug, Error)]
#[error("Validation failed with {0:?}")]
pub struct ValidationFailed(pub String);

/// The outcome of the candidate-validation's PVF pre-check request.
#[derive(Debug, PartialEq)]
pub enum PreCheckOutcome {
	/// The PVF has been compiled successfully within the given constraints.
	Valid,
	/// The PVF could not be compiled. This variant is used when the candidate-validation subsystem
	/// can be sure that the PVF is invalid. To give a couple of examples: a PVF that cannot be
	/// decompressed or that does not represent a structurally valid WebAssembly file.
	Invalid,
	/// This variant is used when the PVF cannot be compiled but for other reasons that are not
	/// included into [`PreCheckOutcome::Invalid`]. This variant can indicate that the PVF in
	/// question is invalid, however it is not necessary that PVF that received this judgement
	/// is invalid.
	///
	/// For example, if during compilation the preparation worker was killed we cannot be sure why
	/// it happened: because the PVF was malicious made the worker to use too much memory or its
	/// because the host machine is under severe memory pressure and it decided to kill the worker.
	Failed,
}

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
	/// of the `CandidateReceipt`.
	///
	/// This will also perform checking of validation outputs against the acceptance criteria.
	///
	/// If there is no state available which can provide this data or the core for
	/// the para is not free at the relay-parent, an error is returned.
	ValidateFromChainState(
		CandidateReceipt,
		Arc<PoV>,
		/// Execution timeout
		Duration,
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
		CandidateReceipt,
		Arc<PoV>,
		/// Execution timeout
		Duration,
		oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	),
	/// Try to compile the given validation code and send back
	/// the outcome.
	///
	/// The validation code is specified by the hash and will be queried from the runtime API at the
	/// given relay-parent.
	PreCheck(
		// Relay-parent
		Hash,
		ValidationCodeHash,
		oneshot::Sender<PreCheckOutcome>,
	),
}

impl CandidateValidationMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::ValidateFromChainState(_, _, _, _) => None,
			Self::ValidateFromExhaustive(_, _, _, _, _, _) => None,
			Self::PreCheck(relay_parent, _, _) => Some(*relay_parent),
		}
	}
}

/// Messages received by the Collator Protocol subsystem.
#[derive(Debug, derive_more::From)]
pub enum CollatorProtocolMessage {
	/// Signal to the collator protocol that it should connect to validators with the expectation
	/// of collating on the given para. This is only expected to be called once, early on, if at all,
	/// and only by the Collation Generation subsystem. As such, it will overwrite the value of
	/// the previous signal.
	///
	/// This should be sent before any `DistributeCollation` message.
	CollateOn(ParaId),
	/// Provide a collation to distribute to validators with an optional result sender.
	///
	/// The result sender should be informed when at least one parachain validator seconded the collation. It is also
	/// completely okay to just drop the sender.
	DistributeCollation(CandidateReceipt, PoV, Option<oneshot::Sender<CollationSecondedSignal>>),
	/// Report a collator as having provided an invalid collation. This should lead to disconnect
	/// and blacklist of the collator.
	ReportCollator(CollatorId),
	/// Get a network bridge update.
	#[from]
	NetworkBridgeUpdate(NetworkBridgeEvent<net_protocol::CollatorProtocolMessage>),
	/// We recommended a particular candidate to be seconded, but it was invalid; penalize the collator.
	///
	/// The hash is the relay parent.
	Invalid(Hash, CandidateReceipt),
	/// The candidate we recommended to be seconded was validated successfully.
	///
	/// The hash is the relay parent.
	Seconded(Hash, SignedFullStatement),
}

impl Default for CollatorProtocolMessage {
	fn default() -> Self {
		Self::CollateOn(Default::default())
	}
}

impl BoundToRelayParent for CollatorProtocolMessage {
	fn relay_parent(&self) -> Hash {
		Default::default()
	}
}

/// Messages received by the dispute coordinator subsystem.
///
/// NOTE: Any response oneshots might get cancelled if the `DisputeCoordinator` was not yet
/// properly initialized for some reason.
#[derive(Debug)]
pub enum DisputeCoordinatorMessage {
	/// Import statements by validators about a candidate.
	///
	/// The subsystem will silently discard ancient statements or sets of only dispute-specific statements for
	/// candidates that are previously unknown to the subsystem. The former is simply because ancient
	/// data is not relevant and the latter is as a DoS prevention mechanism. Both backing and approval
	/// statements already undergo anti-DoS procedures in their respective subsystems, but statements
	/// cast specifically for disputes are not necessarily relevant to any candidate the system is
	/// already aware of and thus present a DoS vector. Our expectation is that nodes will notify each
	/// other of disputes over the network by providing (at least) 2 conflicting statements, of which one is either
	/// a backing or validation statement.
	///
	/// This does not do any checking of the message signature.
	ImportStatements {
		/// The candidate receipt itself.
		candidate_receipt: CandidateReceipt,
		/// The session the candidate appears in.
		session: SessionIndex,
		/// Statements, with signatures checked, by validators participating in disputes.
		///
		/// The validator index passed alongside each statement should correspond to the index
		/// of the validator in the set.
		statements: Vec<(SignedDisputeStatement, ValidatorIndex)>,
		/// Inform the requester once we finished importing (if a sender was provided).
		///
		/// This is:
		/// - we discarded the votes because
		///		- they were ancient or otherwise invalid (result: `InvalidImport`)
		///		- or we were not able to recover availability for an unknown candidate (result:
		///		`InvalidImport`)
		///		- or were known already (in that case the result will still be `ValidImport`)
		/// - or we recorded them because (`ValidImport`)
		///		- we cast our own vote already on that dispute
		///		- or we have approval votes on that candidate
		///		- or other explicit votes on that candidate already recorded
		///		- or recovered availability for the candidate
		///		- or the imported statements are backing/approval votes, which are always accepted.
		pending_confirmation: Option<oneshot::Sender<ImportStatementsResult>>,
	},
	/// Fetch a list of all recent disputes the coordinator is aware of.
	/// These are disputes which have occurred any time in recent sessions,
	/// and which may have already concluded.
	RecentDisputes(oneshot::Sender<Vec<(SessionIndex, CandidateHash, DisputeStatus)>>),
	/// Fetch a list of all active disputes that the coordinator is aware of.
	/// These disputes are either not yet concluded or recently concluded.
	ActiveDisputes(oneshot::Sender<Vec<(SessionIndex, CandidateHash, DisputeStatus)>>),
	/// Get candidate votes for a candidate.
	QueryCandidateVotes(
		Vec<(SessionIndex, CandidateHash)>,
		oneshot::Sender<Vec<(SessionIndex, CandidateHash, CandidateVotes)>>,
	),
	/// Sign and issue local dispute votes. A value of `true` indicates validity, and `false` invalidity.
	IssueLocalStatement(SessionIndex, CandidateHash, CandidateReceipt, bool),
	/// Determine the highest undisputed block within the given chain, based on where candidates
	/// were included. If even the base block should not be finalized due to a dispute,
	/// then `None` should be returned on the channel.
	///
	/// The block descriptions begin counting upwards from the block after the given `base_number`. The `base_number`
	/// is typically the number of the last finalized block but may be slightly higher. This block
	/// is inevitably going to be finalized so it is not accounted for by this function.
	DetermineUndisputedChain {
		/// The lowest possible block to vote on.
		base: (BlockNumber, Hash),
		/// Descriptions of all the blocks counting upwards from the block after the base number
		block_descriptions: Vec<BlockDescription>,
		/// The block to vote on, might be base in case there is no better.
		tx: oneshot::Sender<(BlockNumber, Hash)>,
	},
}

/// The result of `DisputeCoordinatorMessage::ImportStatements`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImportStatementsResult {
	/// Import was invalid (candidate was not available)  and the sending peer should get banned.
	InvalidImport,
	/// Import was valid and can be confirmed to peer.
	ValidImport,
}

/// Messages going to the dispute distribution subsystem.
#[derive(Debug)]
pub enum DisputeDistributionMessage {
	/// Tell dispute distribution to distribute an explicit dispute statement to
	/// validators.
	SendDispute(DisputeMessage),
}

/// Messages received from other subsystems.
#[derive(Debug)]
pub enum NetworkBridgeRxMessage {
	/// Inform the distribution subsystems about the new
	/// gossip network topology formed.
	///
	/// The only reason to have this here, is the availability of the
	/// authority discovery service, otherwise, the `GossipSupport`
	/// subsystem would make more sense.
	NewGossipTopology {
		/// The session info this gossip topology is concerned with.
		session: SessionIndex,
		/// Our validator index in the session, if any.
		local_index: Option<ValidatorIndex>,
		/// The canonical shuffling of validators for the session.
		canonical_shuffling: Vec<(AuthorityDiscoveryId, ValidatorIndex)>,
		/// The reverse mapping of `canonical_shuffling`: from validator index
		/// to the index in `canonical_shuffling`
		shuffled_indices: Vec<usize>,
	},
}

/// Messages received from other subsystems by the network bridge subsystem.
#[derive(Debug)]
pub enum NetworkBridgeTxMessage {
	/// Report a peer for their actions.
	ReportPeer(PeerId, UnifiedReputationChange),

	/// Disconnect a peer from the given peer-set without affecting their reputation.
	DisconnectPeer(PeerId, PeerSet),

	/// Send a message to one or more peers on the validation peer-set.
	SendValidationMessage(Vec<PeerId>, net_protocol::VersionedValidationProtocol),

	/// Send a message to one or more peers on the collation peer-set.
	SendCollationMessage(Vec<PeerId>, net_protocol::VersionedCollationProtocol),

	/// Send a batch of validation messages.
	///
	/// NOTE: Messages will be processed in order (at least statement distribution relies on this).
	SendValidationMessages(Vec<(Vec<PeerId>, net_protocol::VersionedValidationProtocol)>),

	/// Send a batch of collation messages.
	///
	/// NOTE: Messages will be processed in order.
	SendCollationMessages(Vec<(Vec<PeerId>, net_protocol::VersionedCollationProtocol)>),

	/// Send requests via substrate request/response.
	/// Second parameter, tells what to do if we are not yet connected to the peer.
	SendRequests(Vec<Requests>, IfDisconnected),

	/// Connect to peers who represent the given `validator_ids`.
	///
	/// Also ask the network to stay connected to these peers at least
	/// until a new request is issued.
	///
	/// Because it overrides the previous request, it must be ensured
	/// that `validator_ids` include all peers the subsystems
	/// are interested in (per `PeerSet`).
	///
	/// A caller can learn about validator connections by listening to the
	/// `PeerConnected` events from the network bridge.
	ConnectToValidators {
		/// Ids of the validators to connect to.
		validator_ids: Vec<AuthorityDiscoveryId>,
		/// The underlying protocol to use for this request.
		peer_set: PeerSet,
		/// Sends back the number of `AuthorityDiscoveryId`s which
		/// authority discovery has failed to resolve.
		failed: oneshot::Sender<usize>,
	},
	/// Alternative to `ConnectToValidators` in case you already know the `Multiaddrs` you want to be
	/// connected to.
	ConnectToResolvedValidators {
		/// Each entry corresponds to the addresses of an already resolved validator.
		validator_addrs: Vec<HashSet<Multiaddr>>,
		/// The peer set we want the connection on.
		peer_set: PeerSet,
	},
}

impl NetworkBridgeTxMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::ReportPeer(_, _) => None,
			Self::DisconnectPeer(_, _) => None,
			Self::SendValidationMessage(_, _) => None,
			Self::SendCollationMessage(_, _) => None,
			Self::SendValidationMessages(_) => None,
			Self::SendCollationMessages(_) => None,
			Self::ConnectToValidators { .. } => None,
			Self::ConnectToResolvedValidators { .. } => None,
			Self::SendRequests { .. } => None,
		}
	}
}

/// Availability Distribution Message.
#[derive(Debug)]
pub enum AvailabilityDistributionMessage {
	/// Instruct availability distribution to fetch a remote PoV.
	///
	/// NOTE: The result of this fetch is not yet locally validated and could be bogus.
	FetchPoV {
		/// The relay parent giving the necessary context.
		relay_parent: Hash,
		/// Validator to fetch the PoV from.
		from_validator: ValidatorIndex,
		/// The id of the parachain that produced this PoV.
		/// This field is only used to provide more context when logging errors
		/// from the `AvailabilityDistribution` subsystem.
		para_id: ParaId,
		/// Candidate hash to fetch the PoV for.
		candidate_hash: CandidateHash,
		/// Expected hash of the PoV, a PoV not matching this hash will be rejected.
		pov_hash: Hash,
		/// Sender for getting back the result of this fetch.
		///
		/// The sender will be canceled if the fetching failed for some reason.
		tx: oneshot::Sender<PoV>,
	},
}

/// Availability Recovery Message.
#[derive(Debug, derive_more::From)]
pub enum AvailabilityRecoveryMessage {
	/// Recover available data from validators on the network.
	RecoverAvailableData(
		CandidateReceipt,
		SessionIndex,
		Option<GroupIndex>, // Optional backing group to request from first.
		oneshot::Sender<Result<AvailableData, crate::errors::RecoveryError>>,
	),
}

/// Bitfield distribution message.
#[derive(Debug, derive_more::From)]
pub enum BitfieldDistributionMessage {
	/// Distribute a bitfield via gossip to other validators.
	DistributeBitfield(Hash, SignedAvailabilityBitfield),

	/// Event from the network bridge.
	#[from]
	NetworkBridgeUpdate(NetworkBridgeEvent<net_protocol::BitfieldDistributionMessage>),
}

impl BitfieldDistributionMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		match self {
			Self::DistributeBitfield(hash, _) => Some(*hash),
			Self::NetworkBridgeUpdate(_) => None,
		}
	}
}

/// Bitfield signing message.
///
/// Currently non-instantiable.
#[derive(Debug)]
pub enum BitfieldSigningMessage {}

impl BoundToRelayParent for BitfieldSigningMessage {
	fn relay_parent(&self) -> Hash {
		match *self {}
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

	/// Query all chunks that we have for the given candidate hash.
	QueryAllChunks(CandidateHash, oneshot::Sender<Vec<ErasureChunk>>),

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
		/// The chunk itself.
		chunk: ErasureChunk,
		/// Sending side of the channel to send result to.
		tx: oneshot::Sender<Result<(), ()>>,
	},

	/// Store a `AvailableData` and all of its chunks in the AV store.
	///
	/// Return `Ok(())` if the store operation succeeded, `Err(())` if it failed.
	StoreAvailableData {
		/// A hash of the candidate this `available_data` belongs to.
		candidate_hash: CandidateHash,
		/// The number of validators in the session.
		n_validators: u32,
		/// The `AvailableData` itself.
		available_data: AvailableData,
		/// Sending side of the channel to send result to.
		tx: oneshot::Sender<Result<(), ()>>,
	},
}

impl AvailabilityStoreMessage {
	/// In fact, none of the `AvailabilityStore` messages assume a particular relay parent.
	pub fn relay_parent(&self) -> Option<Hash> {
		None
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
	/// Get the cumulative weight of the given block, by hash.
	/// If the block or weight is unknown, this returns `None`.
	///
	/// Note: this is the weight within the low-level fork-choice rule,
	/// not the high-level one implemented in the chain-selection subsystem.
	///
	/// Weight is used for comparing blocks in a fork-choice rule.
	BlockWeight(Hash, ChainApiResponseChannel<Option<BlockWeight>>),
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
	/// `parent`, `grandparent`, ... up to the hash of genesis block
	/// with number 0, including it.
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

/// Chain selection subsystem messages
#[derive(Debug)]
pub enum ChainSelectionMessage {
	/// Signal to the chain selection subsystem that a specific block has been approved.
	Approved(Hash),
	/// Request the leaves in descending order by score.
	Leaves(oneshot::Sender<Vec<Hash>>),
	/// Request the best leaf containing the given block in its ancestry. Return `None` if
	/// there is no such leaf.
	BestLeafContaining(Hash, oneshot::Sender<Option<Hash>>),
}

impl ChainSelectionMessage {
	/// If the current variant contains the relay parent hash, return it.
	pub fn relay_parent(&self) -> Option<Hash> {
		// None of the messages, even the ones containing specific
		// block hashes, can be considered to have those blocks as
		// a relay parent.
		match *self {
			ChainSelectionMessage::Approved(_) => None,
			ChainSelectionMessage::Leaves(_) => None,
			ChainSelectionMessage::BestLeafContaining(..) => None,
		}
	}
}

/// A sender for the result of a runtime API request.
pub type RuntimeApiSender<T> = oneshot::Sender<Result<T, crate::errors::RuntimeApiError>>;

/// A request to the Runtime API subsystem.
#[derive(Debug)]
pub enum RuntimeApiRequest {
	/// Get the version of the runtime API, if any.
	Version(RuntimeApiSender<u32>),
	/// Get the next, current and some previous authority discovery set deduplicated.
	Authorities(RuntimeApiSender<Vec<AuthorityDiscoveryId>>),
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
	/// Get the persisted validation data for a particular para along with the current validation code
	/// hash, matching the data hash against an expected one.
	AssumedValidationData(
		ParaId,
		Hash,
		RuntimeApiSender<Option<(PersistedValidationData, ValidationCodeHash)>>,
	),
	/// Sends back `true` if the validation outputs pass all acceptance criteria checks.
	CheckValidationOutputs(
		ParaId,
		polkadot_primitives::v2::CandidateCommitments,
		RuntimeApiSender<bool>,
	),
	/// Get the session index that a child of the block will have.
	SessionIndexForChild(RuntimeApiSender<SessionIndex>),
	/// Get the validation code for a para, taking the given `OccupiedCoreAssumption`, which
	/// will inform on how the validation data should be computed if the para currently
	/// occupies a core.
	ValidationCode(ParaId, OccupiedCoreAssumption, RuntimeApiSender<Option<ValidationCode>>),
	/// Get validation code by its hash, either past, current or future code can be returned, as long as state is still
	/// available.
	ValidationCodeByHash(ValidationCodeHash, RuntimeApiSender<Option<ValidationCode>>),
	/// Get a the candidate pending availability for a particular parachain by parachain / core index
	CandidatePendingAvailability(ParaId, RuntimeApiSender<Option<CommittedCandidateReceipt>>),
	/// Get all events concerning candidates (backing, inclusion, time-out) in the parent of
	/// the block in whose state this request is executed.
	CandidateEvents(RuntimeApiSender<Vec<CandidateEvent>>),
	/// Get the session info for the given session, if stored.
	SessionInfo(SessionIndex, RuntimeApiSender<Option<SessionInfo>>),
	/// Get all the pending inbound messages in the downward message queue for a para.
	DmqContents(ParaId, RuntimeApiSender<Vec<InboundDownwardMessage<BlockNumber>>>),
	/// Get the contents of all channels addressed to the given recipient. Channels that have no
	/// messages in them are also included.
	InboundHrmpChannelsContents(
		ParaId,
		RuntimeApiSender<BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>>,
	),
	/// Get information about the BABE epoch the block was included in.
	CurrentBabeEpoch(RuntimeApiSender<BabeEpoch>),
	/// Get all disputes in relation to a relay parent.
	FetchOnChainVotes(RuntimeApiSender<Option<polkadot_primitives::v2::ScrapedOnChainVotes>>),
	/// Submits a PVF pre-checking statement into the transaction pool.
	SubmitPvfCheckStatement(PvfCheckStatement, ValidatorSignature, RuntimeApiSender<()>),
	/// Returns code hashes of PVFs that require pre-checking by validators in the active set.
	PvfsRequirePrecheck(RuntimeApiSender<Vec<ValidationCodeHash>>),
	/// Get the validation code used by the specified para, taking the given `OccupiedCoreAssumption`, which
	/// will inform on how the validation data should be computed if the para currently occupies a core.
	ValidationCodeHash(
		ParaId,
		OccupiedCoreAssumption,
		RuntimeApiSender<Option<ValidationCodeHash>>,
	),
	/// Returns all on-chain disputes at given block number. Available in `v3`.
	Disputes(RuntimeApiSender<Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>>),
}

impl RuntimeApiRequest {
	/// Runtime version requirements for each message

	/// `Disputes`
	pub const DISPUTES_RUNTIME_REQUIREMENT: u32 = 3;
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
#[derive(Debug, derive_more::From)]
pub enum StatementDistributionMessage {
	/// We have originated a signed statement in the context of
	/// given relay-parent hash and it should be distributed to other validators.
	Share(Hash, SignedFullStatement),
	/// Event from the network bridge.
	#[from]
	NetworkBridgeUpdate(NetworkBridgeEvent<net_protocol::StatementDistributionMessage>),
}

/// This data becomes intrinsics or extrinsics which should be included in a future relay chain block.
// It needs to be cloneable because multiple potential block authors can request copies.
#[derive(Debug, Clone)]
pub enum ProvisionableData {
	/// This bitfield indicates the availability of various candidate blocks.
	Bitfield(Hash, SignedAvailabilityBitfield),
	/// The Candidate Backing subsystem believes that this candidate is valid, pending availability.
	BackedCandidate(CandidateReceipt),
	/// Misbehavior reports are self-contained proofs of validator misbehavior.
	MisbehaviorReport(Hash, ValidatorIndex, Misbehavior),
	/// Disputes trigger a broad dispute resolution process.
	Dispute(Hash, ValidatorSignature),
}

/// Inherent data returned by the provisioner
#[derive(Debug, Clone)]
pub struct ProvisionerInherentData {
	/// Signed bitfields.
	pub bitfields: SignedAvailabilityBitfields,
	/// Backed candidates.
	pub backed_candidates: Vec<BackedCandidate>,
	/// Dispute statement sets.
	pub disputes: MultiDisputeStatementSet,
}

/// Message to the Provisioner.
///
/// In all cases, the Hash is that of the relay parent.
#[derive(Debug)]
pub enum ProvisionerMessage {
	/// This message allows external subsystems to request the set of bitfields and backed candidates
	/// associated with a particular potential block hash.
	///
	/// This is expected to be used by a proposer, to inject that information into the `InherentData`
	/// where it can be assembled into the `ParaInherent`.
	RequestInherentData(Hash, oneshot::Sender<ProvisionerInherentData>),
	/// This data should become part of a relay chain block
	ProvisionableData(Hash, ProvisionableData),
}

impl BoundToRelayParent for ProvisionerMessage {
	fn relay_parent(&self) -> Hash {
		match self {
			Self::RequestInherentData(hash, _) => *hash,
			Self::ProvisionableData(hash, _) => *hash,
		}
	}
}

/// Message to the Collation Generation subsystem.
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

/// The result type of [`ApprovalVotingMessage::CheckAndImportAssignment`] request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignmentCheckResult {
	/// The vote was accepted and should be propagated onwards.
	Accepted,
	/// The vote was valid but duplicate and should not be propagated onwards.
	AcceptedDuplicate,
	/// The vote was valid but too far in the future to accept right now.
	TooFarInFuture,
	/// The vote was bad and should be ignored, reporting the peer who propagated it.
	Bad(AssignmentCheckError),
}

/// The error result type of [`ApprovalVotingMessage::CheckAndImportAssignment`] request.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum AssignmentCheckError {
	#[error("Unknown block: {0:?}")]
	UnknownBlock(Hash),
	#[error("Unknown session index: {0}")]
	UnknownSessionIndex(SessionIndex),
	#[error("Invalid candidate index: {0}")]
	InvalidCandidateIndex(CandidateIndex),
	#[error("Invalid candidate {0}: {1:?}")]
	InvalidCandidate(CandidateIndex, CandidateHash),
	#[error("Invalid cert: {0:?}, reason: {1}")]
	InvalidCert(ValidatorIndex, String),
	#[error("Internal state mismatch: {0:?}, {1:?}")]
	Internal(Hash, CandidateHash),
}

/// The result type of [`ApprovalVotingMessage::CheckAndImportApproval`] request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalCheckResult {
	/// The vote was accepted and should be propagated onwards.
	Accepted,
	/// The vote was bad and should be ignored, reporting the peer who propagated it.
	Bad(ApprovalCheckError),
}

/// The error result type of [`ApprovalVotingMessage::CheckAndImportApproval`] request.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ApprovalCheckError {
	#[error("Unknown block: {0:?}")]
	UnknownBlock(Hash),
	#[error("Unknown session index: {0}")]
	UnknownSessionIndex(SessionIndex),
	#[error("Invalid candidate index: {0}")]
	InvalidCandidateIndex(CandidateIndex),
	#[error("Invalid validator index: {0:?}")]
	InvalidValidatorIndex(ValidatorIndex),
	#[error("Invalid candidate {0}: {1:?}")]
	InvalidCandidate(CandidateIndex, CandidateHash),
	#[error("Invalid signature: {0:?}")]
	InvalidSignature(ValidatorIndex),
	#[error("No assignment for {0:?}")]
	NoAssignment(ValidatorIndex),
	#[error("Internal state mismatch: {0:?}, {1:?}")]
	Internal(Hash, CandidateHash),
}

/// Describes a relay-chain block by the para-chain candidates
/// it includes.
#[derive(Clone, Debug)]
pub struct BlockDescription {
	/// The relay-chain block hash.
	pub block_hash: Hash,
	/// The session index of this block.
	pub session: SessionIndex,
	/// The set of para-chain candidates.
	pub candidates: Vec<CandidateHash>,
}

/// Response type to `ApprovalVotingMessage::ApprovedAncestor`.
#[derive(Clone, Debug)]
pub struct HighestApprovedAncestorBlock {
	/// The block hash of the highest viable ancestor.
	pub hash: Hash,
	/// The block number of the highest viable ancestor.
	pub number: BlockNumber,
	/// Block descriptions in the direct path between the
	/// initially provided hash and the highest viable ancestor.
	/// Primarily for use with `DetermineUndisputedChain`.
	/// Must be sorted from lowest to highest block number.
	pub descriptions: Vec<BlockDescription>,
}

/// Message to the Approval Voting subsystem.
#[derive(Debug)]
pub enum ApprovalVotingMessage {
	/// Check if the assignment is valid and can be accepted by our view of the protocol.
	/// Should not be sent unless the block hash is known.
	CheckAndImportAssignment(
		IndirectAssignmentCert,
		CandidateIndex,
		oneshot::Sender<AssignmentCheckResult>,
	),
	/// Check if the approval vote is valid and can be accepted by our view of the
	/// protocol.
	///
	/// Should not be sent unless the block hash within the indirect vote is known.
	CheckAndImportApproval(IndirectSignedApprovalVote, oneshot::Sender<ApprovalCheckResult>),
	/// Returns the highest possible ancestor hash of the provided block hash which is
	/// acceptable to vote on finality for.
	/// The `BlockNumber` provided is the number of the block's ancestor which is the
	/// earliest possible vote.
	///
	/// It can also return the same block hash, if that is acceptable to vote upon.
	/// Return `None` if the input hash is unrecognized.
	ApprovedAncestor(Hash, BlockNumber, oneshot::Sender<Option<HighestApprovedAncestorBlock>>),

	/// Retrieve all available approval signatures for a candidate from approval-voting.
	///
	/// This message involves a linear search for candidates on each relay chain fork and also
	/// requires calling into `approval-distribution`: Calls should be infrequent and bounded.
	GetApprovalSignaturesForCandidate(
		CandidateHash,
		oneshot::Sender<HashMap<ValidatorIndex, ValidatorSignature>>,
	),
}

/// Message to the Approval Distribution subsystem.
#[derive(Debug, derive_more::From)]
pub enum ApprovalDistributionMessage {
	/// Notify the `ApprovalDistribution` subsystem about new blocks
	/// and the candidates contained within them.
	NewBlocks(Vec<BlockApprovalMeta>),
	/// Distribute an assignment cert from the local validator. The cert is assumed
	/// to be valid, relevant, and for the given relay-parent and validator index.
	DistributeAssignment(IndirectAssignmentCert, CandidateIndex),
	/// Distribute an approval vote for the local validator. The approval vote is assumed to be
	/// valid, relevant, and the corresponding approval already issued.
	/// If not, the subsystem is free to drop the message.
	DistributeApproval(IndirectSignedApprovalVote),
	/// An update from the network bridge.
	#[from]
	NetworkBridgeUpdate(NetworkBridgeEvent<net_protocol::ApprovalDistributionMessage>),

	/// Get all approval signatures for all chains a candidate appeared in.
	GetApprovalSignatures(
		HashSet<(Hash, CandidateIndex)>,
		oneshot::Sender<HashMap<ValidatorIndex, ValidatorSignature>>,
	),
}

/// Message to the Gossip Support subsystem.
#[derive(Debug, derive_more::From)]
pub enum GossipSupportMessage {
	/// Dummy constructor, so we can receive networking events.
	#[from]
	NetworkBridgeUpdate(NetworkBridgeEvent<net_protocol::GossipSupportNetworkMessage>),
}

/// PVF checker message.
///
/// Currently non-instantiable.
#[derive(Debug)]
pub enum PvfCheckerMessage {}
