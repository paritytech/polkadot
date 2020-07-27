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

use polkadot_primitives::v1::{
	BlockNumber, Hash,
	CandidateReceipt, PoV, ErasureChunk, BackedCandidate, Id as ParaId,
	SignedAvailabilityBitfield, SigningContext, ValidatorId, ValidationCode, ValidatorIndex,
	CoreAssignment, CoreOccupied, HeadData, CandidateDescriptor,
	ValidatorSignature, OmittedValidationData,
};
use polkadot_node_primitives::{
	MisbehaviorReport, SignedFullStatement, View, ProtocolId, ValidationResult,
};

use std::sync::Arc;

pub use sc_network::{ObservedRole, ReputationChange, PeerId};

/// A notification of a new backed candidate.
#[derive(Debug)]
pub struct NewBackedCandidate(pub BackedCandidate);

/// Messages received by the Candidate Selection subsystem.
#[derive(Debug)]
pub enum CandidateSelectionMessage {
	/// We recommended a particular candidate to be seconded, but it was invalid; penalize the collator.
	/// The hash is the relay parent.
	Invalid(Hash, CandidateReceipt),
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

/// Blanket error for validation failing.
#[derive(Debug)]
pub struct ValidationFailed;

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
	/// This will implicitly attempt to gather the `OmittedValidationData` and `ValidationCode`
	/// from the runtime API of the chain, based on the `relay_parent`
	/// of the `CandidateDescriptor`.
	/// If there is no state available which can provide this data, an error is returned.
	ValidateFromChainState(
		CandidateDescriptor,
		Arc<PoV>,
		oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	),
	/// Validate a candidate with provided, exhaustive parameters for validation.
	///
	/// Explicitly provide the `OmittedValidationData` and `ValidationCode` so this can do full
	/// validation without needing to access the state of the relay-chain.
	ValidateFromExhaustive(
		OmittedValidationData,
		ValidationCode,
		CandidateDescriptor,
		Arc<PoV>,
		oneshot::Sender<Result<ValidationResult, ValidationFailed>>,
	),
}

/// Events from network.
#[derive(Debug, Clone)]
pub enum NetworkBridgeEvent {
	/// A peer has connected.
	PeerConnected(PeerId, ObservedRole),

	/// A peer has disconnected.
	PeerDisconnected(PeerId),

	/// Peer has sent a message.
	PeerMessage(PeerId, Vec<u8>),

	/// Peer's `View` has changed.
	PeerViewChange(PeerId, View),

	/// Our `View` has changed.
	OurViewChange(View),
}

/// Messages received by the network bridge subsystem.
#[derive(Debug)]
pub enum NetworkBridgeMessage {
	/// Register an event producer on startup.
	RegisterEventProducer(ProtocolId, fn(NetworkBridgeEvent) -> AllMessages),

	/// Report a peer for their actions.
	ReportPeer(PeerId, ReputationChange),

	/// Send a message to multiple peers.
	SendMessage(Vec<PeerId>, ProtocolId, Vec<u8>),
}

/// Availability Distribution Message.
#[derive(Debug)]
pub enum AvailabilityDistributionMessage {
	/// Distribute an availability chunk to other validators.
	DistributeChunk(Hash, ErasureChunk),

	/// Fetch an erasure chunk from networking by candidate hash and chunk index.
	FetchChunk(Hash, u32),

	/// Event from the network bridge.
	NetworkBridgeUpdate(NetworkBridgeEvent),
}

/// Bitfield distribution message.
#[derive(Debug)]
pub enum BitfieldDistributionMessage {
	/// Distribute a bitfield via gossip to other validators.
	DistributeBitfield(Hash, SignedAvailabilityBitfield),

	/// Event from the network bridge.
	NetworkBridgeUpdate(NetworkBridgeEvent),
}

/// Availability store subsystem message.
#[derive(Debug)]
pub enum AvailabilityStoreMessage {
	/// Query a `PoV` from the AV store.
	QueryPoV(Hash, oneshot::Sender<Option<PoV>>),

	/// Query an `ErasureChunk` from the AV store.
	QueryChunk(Hash, ValidatorIndex, oneshot::Sender<ErasureChunk>),

	/// Store an `ErasureChunk` in the AV store.
	StoreChunk(Hash, ValidatorIndex, ErasureChunk),
}

/// The information on scheduler assignments that some somesystems may be querying.
#[derive(Debug, Clone)]
pub struct SchedulerRoster {
	/// Validator-to-groups assignments.
	pub validator_groups: Vec<Vec<ValidatorIndex>>,
	/// All scheduled paras.
	pub scheduled: Vec<CoreAssignment>,
	/// Upcoming paras (chains and threads).
	pub upcoming: Vec<ParaId>,
	/// Occupied cores.
	pub availability_cores: Vec<Option<CoreOccupied>>,
}

/// A request to the Runtime API subsystem.
#[derive(Debug)]
pub enum RuntimeApiRequest {
	/// Get the current validator set.
	Validators(oneshot::Sender<Vec<ValidatorId>>),
	/// Get the assignments of validators to cores.
	ValidatorGroups(oneshot::Sender<SchedulerRoster>),
	/// Get a signing context for bitfields and statements.
	SigningContext(oneshot::Sender<SigningContext>),
	/// Get the validation code for a specific para, assuming execution under given block number, and
	/// an optional block number representing an intermediate parablock executed in the context of
	/// that block.
	ValidationCode(ParaId, BlockNumber, Option<BlockNumber>, oneshot::Sender<ValidationCode>),
	/// Get head data for a specific para.
	HeadData(ParaId, oneshot::Sender<HeadData>),
}

/// A message to the Runtime API subsystem.
#[derive(Debug)]
pub enum RuntimeApiMessage {
	/// Make a request of the runtime API against the post-state of the given relay-parent.
	Request(Hash, RuntimeApiRequest),
}

/// Statement distribution message.
#[derive(Debug)]
pub enum StatementDistributionMessage {
	/// We have originated a signed statement in the context of
	/// given relay-parent hash and it should be distributed to other validators.
	Share(Hash, SignedFullStatement),
	/// Event from the network bridge.
	NetworkBridgeUpdate(NetworkBridgeEvent),
}

/// This data becomes intrinsics or extrinsics which should be included in a future relay chain block.
#[derive(Debug)]
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
	ProvisionableData(ProvisionableData),
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
	NetworkBridgeUpdate(NetworkBridgeEvent),
}

/// A message type tying together all message types that are used across Subsystems.
#[derive(Debug)]
pub enum AllMessages {
	/// Message for the validation subsystem.
	CandidateValidation(CandidateValidationMessage),
	/// Message for the candidate backing subsystem.
	CandidateBacking(CandidateBackingMessage),
	/// Message for the candidate selection subsystem.
	CandidateSelection(CandidateSelectionMessage),
	/// Message for the statement distribution subsystem.
	StatementDistribution(StatementDistributionMessage),
	/// Message for the availability distribution subsystem.
	AvailabilityDistribution(AvailabilityDistributionMessage),
	/// Message for the bitfield distribution subsystem.
	BitfieldDistribution(BitfieldDistributionMessage),
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
}
