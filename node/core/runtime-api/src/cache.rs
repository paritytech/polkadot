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

use polkadot_primitives::v1::{
	BlockNumber, CandidateCommitments, CommittedCandidateReceipt, CandidateEvent,
	CoreState, GroupRotationInfo, InboundDownwardMessage, InboundHrmpMessage, Hash,
	PersistedValidationData, Id as ParaId, OccupiedCoreAssumption,
	SessionIndex, SessionInfo, ValidationCode, ValidatorId, ValidatorIndex,
};
use sp_consensus_babe::Epoch;
use parity_util_mem::{MallocSizeOf, MallocSizeOfExt};


use memory_lru::{MemoryLruCache, ResidentSize};

use std::collections::btree_map::BTreeMap;

const VALIDATORS_CACHE_SIZE: usize = 64 * 1024;
const VALIDATOR_GROUPS_CACHE_SIZE: usize = 64 * 1024;
const AVAILABILITY_CORES_CACHE_SIZE: usize = 64 * 1024;
const PERSISTED_VALIDATION_DATA_CACHE_SIZE: usize = 64 * 1024;
const CHECK_VALIDATION_OUTPUTS_CACHE_SIZE: usize = 64 * 1024;
const SESSION_INDEX_FOR_CHILD_CACHE_SIZE: usize = 64 * 1024;
const VALIDATION_CODE_CACHE_SIZE: usize = 10 * 1024 * 1024;
const HISTORICAL_VALIDATION_CODE_CACHE_SIZE: usize = 10 * 1024 * 1024;
const CANDIDATE_PENDING_AVAILABILITY_CACHE_SIZE: usize = 64 * 1024;
const CANDIDATE_EVENTS_CACHE_SIZE: usize = 64 * 1024;
const SESSION_INFO_CACHE_SIZE: usize = 64 * 1024;
const DMQ_CONTENTS_CACHE_SIZE: usize = 64 * 1024;
const INBOUND_HRMP_CHANNELS_CACHE_SIZE: usize = 64 * 1024;
const CURRENT_BABE_EPOCH_CACHE_SIZE: usize = 64 * 1024;

struct ResidentSizeOf<T>(T);

impl<T: MallocSizeOf> ResidentSize for ResidentSizeOf<T> {
	fn resident_size(&self) -> usize {
		std::mem::size_of::<Self>() + self.0.malloc_size_of()
	}
}

struct DoesNotAllocate<T>(T);

impl<T> ResidentSize for DoesNotAllocate<T> {
	fn resident_size(&self) -> usize {
		std::mem::size_of::<Self>()
	}
}

pub(crate) struct RequestResultCache {
	validators: MemoryLruCache<Hash, ResidentSizeOf<Vec<ValidatorId>>>,
	validator_groups: MemoryLruCache<Hash, ResidentSizeOf<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo)>>,
	availability_cores: MemoryLruCache<Hash, ResidentSizeOf<Vec<CoreState>>>,
	persisted_validation_data: MemoryLruCache<(Hash, ParaId, OccupiedCoreAssumption), ResidentSizeOf<Option<PersistedValidationData>>>,
	check_validation_outputs: MemoryLruCache<(Hash, ParaId, CandidateCommitments), ResidentSizeOf<bool>>,
	session_index_for_child: MemoryLruCache<Hash, ResidentSizeOf<SessionIndex>>,
	validation_code: MemoryLruCache<(Hash, ParaId, OccupiedCoreAssumption), ResidentSizeOf<Option<ValidationCode>>>,
	historical_validation_code: MemoryLruCache<(Hash, ParaId, BlockNumber), ResidentSizeOf<Option<ValidationCode>>>,
	candidate_pending_availability: MemoryLruCache<(Hash, ParaId), ResidentSizeOf<Option<CommittedCandidateReceipt>>>,
	candidate_events: MemoryLruCache<Hash, ResidentSizeOf<Vec<CandidateEvent>>>,
	session_info: MemoryLruCache<(Hash, SessionIndex), ResidentSizeOf<Option<SessionInfo>>>,
	dmq_contents: MemoryLruCache<(Hash, ParaId), ResidentSizeOf<Vec<InboundDownwardMessage<BlockNumber>>>>,
	inbound_hrmp_channels_contents: MemoryLruCache<(Hash, ParaId), ResidentSizeOf<BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>>>,
	current_babe_epoch: MemoryLruCache<Hash, DoesNotAllocate<Epoch>>,
}

impl Default for RequestResultCache {
	fn default() -> Self {
		Self {
			validators: MemoryLruCache::new(VALIDATORS_CACHE_SIZE),
			validator_groups: MemoryLruCache::new(VALIDATOR_GROUPS_CACHE_SIZE),
			availability_cores: MemoryLruCache::new(AVAILABILITY_CORES_CACHE_SIZE),
			persisted_validation_data: MemoryLruCache::new(PERSISTED_VALIDATION_DATA_CACHE_SIZE),
			check_validation_outputs: MemoryLruCache::new(CHECK_VALIDATION_OUTPUTS_CACHE_SIZE),
			session_index_for_child: MemoryLruCache::new(SESSION_INDEX_FOR_CHILD_CACHE_SIZE),
			validation_code: MemoryLruCache::new(VALIDATION_CODE_CACHE_SIZE),
			historical_validation_code: MemoryLruCache::new(HISTORICAL_VALIDATION_CODE_CACHE_SIZE),
			candidate_pending_availability: MemoryLruCache::new(CANDIDATE_PENDING_AVAILABILITY_CACHE_SIZE),
			candidate_events: MemoryLruCache::new(CANDIDATE_EVENTS_CACHE_SIZE),
			session_info: MemoryLruCache::new(SESSION_INFO_CACHE_SIZE),
			dmq_contents: MemoryLruCache::new(DMQ_CONTENTS_CACHE_SIZE),
			inbound_hrmp_channels_contents: MemoryLruCache::new(INBOUND_HRMP_CHANNELS_CACHE_SIZE),
			current_babe_epoch: MemoryLruCache::new(CURRENT_BABE_EPOCH_CACHE_SIZE),
		}
	}
}

impl RequestResultCache {
	pub(crate) fn validators(&mut self, relay_parent: &Hash) -> Option<&Vec<ValidatorId>> {
		self.validators.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_validators(&mut self, relay_parent: Hash, validators: Vec<ValidatorId>) {
		self.validators.insert(relay_parent, ResidentSizeOf(validators));
	}

	pub(crate) fn validator_groups(&mut self, relay_parent: &Hash) -> Option<&(Vec<Vec<ValidatorIndex>>, GroupRotationInfo)> {
		self.validator_groups.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_validator_groups(&mut self, relay_parent: Hash, groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo)) {
		self.validator_groups.insert(relay_parent, ResidentSizeOf(groups));
	}

	pub(crate) fn availability_cores(&mut self, relay_parent: &Hash) -> Option<&Vec<CoreState>> {
		self.availability_cores.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_availability_cores(&mut self, relay_parent: Hash, cores: Vec<CoreState>) {
		self.availability_cores.insert(relay_parent, ResidentSizeOf(cores));
	}

	pub(crate) fn persisted_validation_data(&mut self, key: (Hash, ParaId, OccupiedCoreAssumption)) -> Option<&Option<PersistedValidationData>> {
		self.persisted_validation_data.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_persisted_validation_data(&mut self, key: (Hash, ParaId, OccupiedCoreAssumption), data: Option<PersistedValidationData>) {
		self.persisted_validation_data.insert(key, ResidentSizeOf(data));
	}

	pub(crate) fn check_validation_outputs(&mut self, key: (Hash, ParaId, CandidateCommitments)) -> Option<&bool> {
		self.check_validation_outputs.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_check_validation_outputs(&mut self, key: (Hash, ParaId, CandidateCommitments), value: bool) {
		self.check_validation_outputs.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn session_index_for_child(&mut self, relay_parent: &Hash) -> Option<&SessionIndex> {
		self.session_index_for_child.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_session_index_for_child(&mut self, relay_parent: Hash, index: SessionIndex) {
		self.session_index_for_child.insert(relay_parent, ResidentSizeOf(index));
	}

	pub(crate) fn validation_code(&mut self, key: (Hash, ParaId, OccupiedCoreAssumption)) -> Option<&Option<ValidationCode>> {
		self.validation_code.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_validation_code(&mut self, key: (Hash, ParaId, OccupiedCoreAssumption), value: Option<ValidationCode>) {
		self.validation_code.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn historical_validation_code(&mut self, key: (Hash, ParaId, BlockNumber)) -> Option<&Option<ValidationCode>> {
		self.historical_validation_code.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_historical_validation_code(&mut self, key: (Hash, ParaId, BlockNumber), value: Option<ValidationCode>) {
		self.historical_validation_code.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn candidate_pending_availability(&mut self, key: (Hash, ParaId)) -> Option<&Option<CommittedCandidateReceipt>> {
		self.candidate_pending_availability.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_candidate_pending_availability(&mut self, key: (Hash, ParaId), value: Option<CommittedCandidateReceipt>) {
		self.candidate_pending_availability.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn candidate_events(&mut self, relay_parent: &Hash) -> Option<&Vec<CandidateEvent>> {
		self.candidate_events.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_candidate_events(&mut self, relay_parent: Hash, events: Vec<CandidateEvent>) {
		self.candidate_events.insert(relay_parent, ResidentSizeOf(events));
	}

	pub(crate) fn session_info(&mut self, key: (Hash, SessionIndex)) -> Option<&Option<SessionInfo>> {
		self.session_info.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_session_info(&mut self, key: (Hash, SessionIndex), value: Option<SessionInfo>) {
		self.session_info.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn dmq_contents(&mut self, key: (Hash, ParaId)) -> Option<&Vec<InboundDownwardMessage<BlockNumber>>> {
		self.dmq_contents.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_dmq_contents(&mut self, key: (Hash, ParaId), value: Vec<InboundDownwardMessage<BlockNumber>>) {
		self.dmq_contents.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn inbound_hrmp_channels_contents(&mut self, key: (Hash, ParaId)) -> Option<&BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>> {
		self.inbound_hrmp_channels_contents.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_inbound_hrmp_channel_contents(&mut self, key: (Hash, ParaId), value: BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>) {
		self.inbound_hrmp_channels_contents.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn current_babe_epoch(&mut self, relay_parent: &Hash) -> Option<&Epoch> {
		self.current_babe_epoch.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_current_babe_epoch(&mut self, relay_parent: Hash, epoch: Epoch) {
		self.current_babe_epoch.insert(relay_parent, DoesNotAllocate(epoch));
	}
}

pub(crate) enum RequestResult {
	Validators(Hash, Vec<ValidatorId>),
	ValidatorGroups(Hash, (Vec<Vec<ValidatorIndex>>, GroupRotationInfo)),
	AvailabilityCores(Hash, Vec<CoreState>),
	PersistedValidationData(Hash, ParaId, OccupiedCoreAssumption, Option<PersistedValidationData>),
	CheckValidationOutputs(Hash, ParaId, CandidateCommitments, bool),
	SessionIndexForChild(Hash, SessionIndex),
	ValidationCode(Hash, ParaId, OccupiedCoreAssumption, Option<ValidationCode>),
	HistoricalValidationCode(Hash, ParaId, BlockNumber, Option<ValidationCode>),
	CandidatePendingAvailability(Hash, ParaId, Option<CommittedCandidateReceipt>),
	CandidateEvents(Hash, Vec<CandidateEvent>),
	SessionInfo(Hash, SessionIndex, Option<SessionInfo>),
	DmqContents(Hash, ParaId, Vec<InboundDownwardMessage<BlockNumber>>),
	InboundHrmpChannelsContents(Hash, ParaId, BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>),
	CurrentBabeEpoch(Hash, Epoch),
}
