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

//! Polkadot types shared between the runtime and the Node-side code.

#![warn(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]

// `v4` is currently the latest stable version of the runtime API.
pub mod v4;

// The 'staging' version is special - it contains primitives which are
// still in development. Once they are considered stable, they will be
// moved to a new versioned module.
pub mod vstaging;

// `runtime_api` contains the actual API implementation. It contains stable and
// unstable functions.
pub mod runtime_api;

// Current primitives not requiring versioning are exported here.
// Primitives requiring versioning must not be exported and must be referred by an exact version.
pub use v4::{
	byzantine_threshold, check_candidate_backing, collator_signature_payload, metric_definitions,
	supermajority_threshold, well_known_keys, AbridgedHostConfiguration, AbridgedHrmpChannel,
	AccountId, AccountIndex, AccountPublic, ApprovalVote, AssignmentId, AuthorityDiscoveryId,
	AvailabilityBitfield, BackedCandidate, Balance, BlakeTwo256, Block, BlockId, BlockNumber,
	CandidateCommitments, CandidateDescriptor, CandidateEvent, CandidateHash, CandidateIndex,
	CandidateReceipt, CheckedDisputeStatementSet, CheckedMultiDisputeStatementSet, CollatorId,
	CollatorSignature, CommittedCandidateReceipt, CompactStatement, ConsensusLog, CoreIndex,
	CoreOccupied, CoreState, DisputeState, DisputeStatement, DisputeStatementSet, DownwardMessage,
	EncodeAs, ExecutorParam, ExecutorParams, ExecutorParamsHash, ExplicitDisputeStatement,
	GroupIndex, GroupRotationInfo, Hash, HashT, HeadData, Header, HrmpChannelId, Id,
	InboundDownwardMessage, InboundHrmpMessage, IndexedVec, InherentData,
	InvalidDisputeStatementKind, Moment, MultiDisputeStatementSet, Nonce, OccupiedCore,
	OccupiedCoreAssumption, OutboundHrmpMessage, ParathreadClaim, ParathreadEntry,
	PersistedValidationData, PvfCheckStatement, PvfExecTimeoutKind, PvfPrepTimeoutKind,
	RuntimeMetricLabel, RuntimeMetricLabelValue, RuntimeMetricLabelValues, RuntimeMetricLabels,
	RuntimeMetricOp, RuntimeMetricUpdate, ScheduledCore, ScrapedOnChainVotes, SessionIndex,
	SessionInfo, Signature, Signed, SignedAvailabilityBitfield, SignedAvailabilityBitfields,
	SignedStatement, SigningContext, Slot, UncheckedSigned, UncheckedSignedAvailabilityBitfield,
	UncheckedSignedAvailabilityBitfields, UncheckedSignedStatement, UpgradeGoAhead,
	UpgradeRestriction, UpwardMessage, ValidDisputeStatementKind, ValidationCode,
	ValidationCodeHash, ValidatorId, ValidatorIndex, ValidatorSignature, ValidityAttestation,
	ValidityError, ASSIGNMENT_KEY_TYPE_ID, LOWEST_PUBLIC_ID, MAX_CODE_SIZE, MAX_HEAD_DATA_SIZE,
	MAX_POV_SIZE, PARACHAINS_INHERENT_IDENTIFIER, PARACHAIN_KEY_TYPE_ID,
};

#[cfg(feature = "std")]
pub use v4::{AssignmentPair, CollatorPair, ValidatorPair};
