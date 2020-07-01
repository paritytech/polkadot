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

use sc_network::{ObservedRole, ReputationChange, PeerId};
use polkadot_primitives::{BlockNumber, Hash, Signature};
use polkadot_primitives::parachain::{
	AbridgedCandidateReceipt, PoVBlock, ErasureChunk, BackedCandidate, Id as ParaId,
	SignedAvailabilityBitfield, SigningContext, ValidatorId, ValidationCode, ValidatorIndex,
};
use polkadot_node_primitives::{
	MisbehaviorReport, SignedFullStatement, View, ProtocolId,
};

/// A notification of a new backed candidate.
#[derive(Debug)]
pub struct NewBackedCandidate(pub BackedCandidate);

/// Messages received by the Candidate Selection subsystem.
#[derive(Debug)]
pub enum CandidateSelectionMessage {
	/// We recommended a particular candidate to be seconded, but it was invalid; penalize the collator.
	/// The hash is the relay parent.
	Invalid(Hash, AbridgedCandidateReceipt),
}

/// Messages received by the Candidate Backing subsystem.
#[derive(Debug)]
pub enum CandidateBackingMessage {
	/// Registers a stream listener for updates to the set of backable candidates that could be backed
	/// in a child of the given relay-parent, referenced by its hash.
	RegisterBackingWatcher(Hash, mpsc::Sender<NewBackedCandidate>),
	/// Note that the Candidate Backing subsystem should second the given candidate in the context of the
	/// given relay-parent (ref. by hash). This candidate must be validated.
	Second(Hash, AbridgedCandidateReceipt),
	/// Note a validator's statement about a particular candidate. Disagreements about validity must be escalated
	/// to a broader check by Misbehavior Arbitration. Agreements are simply tallied until a quorum is reached.
	Statement(Hash, SignedFullStatement),
}

/// Blanket error for validation failing.
#[derive(Debug)]
pub struct ValidationFailed;

/// Messages received by the Validation subsystem
#[derive(Debug)]
pub enum CandidateValidationMessage {
	/// Validate a candidate, sending a side-channel response of valid or invalid.
	///
	/// Provide the relay-parent in whose context this should be validated, the full candidate receipt,
	/// and the PoV.
	Validate(
		Hash,
		AbridgedCandidateReceipt,
		PoVBlock,
		oneshot::Sender<Result<(), ValidationFailed>>,
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
	/// Query a `PoVBlock` from the AV store.
	QueryPoV(Hash, oneshot::Sender<Option<PoVBlock>>),

	/// Query an `ErasureChunk` from the AV store.
	QueryChunk(Hash, ValidatorIndex, oneshot::Sender<ErasureChunk>),

	/// Store an `ErasureChunk` in the AV store.
	StoreChunk(Hash, ValidatorIndex, ErasureChunk),
}

/// A request to the Runtime API subsystem.
#[derive(Debug)]
pub enum RuntimeApiRequest {
	/// Get the current validator set.
	Validators(oneshot::Sender<Vec<ValidatorId>>),
	/// Get a signing context for bitfields and statements.
	SigningContext(oneshot::Sender<SigningContext>),
	/// Get the validation code for a specific para, assuming execution under given block number, and
	/// an optional block number representing an intermediate parablock executed in the context of
	/// that block.
	ValidationCode(ParaId, BlockNumber, Option<BlockNumber>, oneshot::Sender<ValidationCode>),
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
	Dispute(Hash, Signature),
}

/// Message to the Provisioner.
///
/// In all cases, the Hash is that of the relay parent.
#[derive(Debug)]
pub enum ProvisionerMessage {
	/// This message allows potential block authors to be kept updated with all new authorship data
	/// as it becomes available.
	RequestBlockAuthorshipData(Hash, mpsc::Sender<ProvisionableData>),
	/// This data should become part of a relay chain block
	ProvisionableData(ProvisionableData),
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
	/// Message for the Runtime API subsystem.
	RuntimeApi(RuntimeApiMessage),
	/// Message for the availability store subsystem.
	AvailabilityStore(AvailabilityStoreMessage),
}
