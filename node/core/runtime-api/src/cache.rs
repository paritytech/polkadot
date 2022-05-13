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

use std::collections::btree_map::BTreeMap;

use memory_lru::{MemoryLruCache, ResidentSize};
use parity_util_mem::{MallocSizeOf, MallocSizeOfExt};
use sp_consensus_babe::Epoch;

use polkadot_primitives::v2::{
	AuthorityDiscoveryId, BlockNumber, CandidateCommitments, CandidateEvent, CandidateHash,
	CommittedCandidateReceipt, CoreState, DisputeState, GroupRotationInfo, Hash, Id as ParaId,
	InboundDownwardMessage, InboundHrmpMessage, OccupiedCoreAssumption, PersistedValidationData,
	PvfCheckStatement, ScrapedOnChainVotes, SessionIndex, SessionInfo, ValidationCode,
	ValidationCodeHash, ValidatorId, ValidatorIndex, ValidatorSignature,
};

const AUTHORITIES_CACHE_SIZE: usize = 128 * 1024;
const VALIDATORS_CACHE_SIZE: usize = 64 * 1024;
const VALIDATOR_GROUPS_CACHE_SIZE: usize = 64 * 1024;
const AVAILABILITY_CORES_CACHE_SIZE: usize = 64 * 1024;
const PERSISTED_VALIDATION_DATA_CACHE_SIZE: usize = 64 * 1024;
const ASSUMED_VALIDATION_DATA_CACHE_SIZE: usize = 64 * 1024;
const CHECK_VALIDATION_OUTPUTS_CACHE_SIZE: usize = 64 * 1024;
const SESSION_INDEX_FOR_CHILD_CACHE_SIZE: usize = 64 * 1024;
const VALIDATION_CODE_CACHE_SIZE: usize = 10 * 1024 * 1024;
const CANDIDATE_PENDING_AVAILABILITY_CACHE_SIZE: usize = 64 * 1024;
const CANDIDATE_EVENTS_CACHE_SIZE: usize = 64 * 1024;
const SESSION_INFO_CACHE_SIZE: usize = 64 * 1024;
const DMQ_CONTENTS_CACHE_SIZE: usize = 64 * 1024;
const INBOUND_HRMP_CHANNELS_CACHE_SIZE: usize = 64 * 1024;
const CURRENT_BABE_EPOCH_CACHE_SIZE: usize = 64 * 1024;
const ON_CHAIN_VOTES_CACHE_SIZE: usize = 3 * 1024;
const PVFS_REQUIRE_PRECHECK_SIZE: usize = 1024;
const VALIDATION_CODE_HASH_CACHE_SIZE: usize = 64 * 1024;
const VERSION_CACHE_SIZE: usize = 4 * 1024;
const DISPUTES_CACHE_SIZE: usize = 64 * 1024;

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

// this is an ugly workaround for `AuthorityDiscoveryId`
// not implementing `MallocSizeOf`
struct VecOfDoesNotAllocate<T>(Vec<T>);

impl<T> ResidentSize for VecOfDoesNotAllocate<T> {
	fn resident_size(&self) -> usize {
		std::mem::size_of::<T>() * self.0.capacity()
	}
}

pub(crate) struct RequestResultCache {
	authorities: MemoryLruCache<Hash, VecOfDoesNotAllocate<AuthorityDiscoveryId>>,
	validators: MemoryLruCache<Hash, ResidentSizeOf<Vec<ValidatorId>>>,
	validator_groups:
		MemoryLruCache<Hash, ResidentSizeOf<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo)>>,
	availability_cores: MemoryLruCache<Hash, ResidentSizeOf<Vec<CoreState>>>,
	persisted_validation_data: MemoryLruCache<
		(Hash, ParaId, OccupiedCoreAssumption),
		ResidentSizeOf<Option<PersistedValidationData>>,
	>,
	assumed_validation_data: MemoryLruCache<
		(ParaId, Hash),
		ResidentSizeOf<Option<(PersistedValidationData, ValidationCodeHash)>>,
	>,
	check_validation_outputs:
		MemoryLruCache<(Hash, ParaId, CandidateCommitments), ResidentSizeOf<bool>>,
	session_index_for_child: MemoryLruCache<Hash, ResidentSizeOf<SessionIndex>>,
	validation_code: MemoryLruCache<
		(Hash, ParaId, OccupiedCoreAssumption),
		ResidentSizeOf<Option<ValidationCode>>,
	>,
	validation_code_by_hash:
		MemoryLruCache<ValidationCodeHash, ResidentSizeOf<Option<ValidationCode>>>,
	candidate_pending_availability:
		MemoryLruCache<(Hash, ParaId), ResidentSizeOf<Option<CommittedCandidateReceipt>>>,
	candidate_events: MemoryLruCache<Hash, ResidentSizeOf<Vec<CandidateEvent>>>,
	session_info: MemoryLruCache<SessionIndex, ResidentSizeOf<SessionInfo>>,
	dmq_contents:
		MemoryLruCache<(Hash, ParaId), ResidentSizeOf<Vec<InboundDownwardMessage<BlockNumber>>>>,
	inbound_hrmp_channels_contents: MemoryLruCache<
		(Hash, ParaId),
		ResidentSizeOf<BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>>,
	>,
	current_babe_epoch: MemoryLruCache<Hash, DoesNotAllocate<Epoch>>,
	on_chain_votes: MemoryLruCache<Hash, ResidentSizeOf<Option<ScrapedOnChainVotes>>>,
	pvfs_require_precheck: MemoryLruCache<Hash, ResidentSizeOf<Vec<ValidationCodeHash>>>,
	validation_code_hash: MemoryLruCache<
		(Hash, ParaId, OccupiedCoreAssumption),
		ResidentSizeOf<Option<ValidationCodeHash>>,
	>,
	version: MemoryLruCache<Hash, ResidentSizeOf<u32>>,
	disputes: MemoryLruCache<
		Hash,
		ResidentSizeOf<Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>>,
	>,
}

impl Default for RequestResultCache {
	fn default() -> Self {
		Self {
			authorities: MemoryLruCache::new(AUTHORITIES_CACHE_SIZE),
			validators: MemoryLruCache::new(VALIDATORS_CACHE_SIZE),
			validator_groups: MemoryLruCache::new(VALIDATOR_GROUPS_CACHE_SIZE),
			availability_cores: MemoryLruCache::new(AVAILABILITY_CORES_CACHE_SIZE),
			persisted_validation_data: MemoryLruCache::new(PERSISTED_VALIDATION_DATA_CACHE_SIZE),
			assumed_validation_data: MemoryLruCache::new(ASSUMED_VALIDATION_DATA_CACHE_SIZE),
			check_validation_outputs: MemoryLruCache::new(CHECK_VALIDATION_OUTPUTS_CACHE_SIZE),
			session_index_for_child: MemoryLruCache::new(SESSION_INDEX_FOR_CHILD_CACHE_SIZE),
			validation_code: MemoryLruCache::new(VALIDATION_CODE_CACHE_SIZE),
			validation_code_by_hash: MemoryLruCache::new(VALIDATION_CODE_CACHE_SIZE),
			candidate_pending_availability: MemoryLruCache::new(
				CANDIDATE_PENDING_AVAILABILITY_CACHE_SIZE,
			),
			candidate_events: MemoryLruCache::new(CANDIDATE_EVENTS_CACHE_SIZE),
			session_info: MemoryLruCache::new(SESSION_INFO_CACHE_SIZE),
			dmq_contents: MemoryLruCache::new(DMQ_CONTENTS_CACHE_SIZE),
			inbound_hrmp_channels_contents: MemoryLruCache::new(INBOUND_HRMP_CHANNELS_CACHE_SIZE),
			current_babe_epoch: MemoryLruCache::new(CURRENT_BABE_EPOCH_CACHE_SIZE),
			on_chain_votes: MemoryLruCache::new(ON_CHAIN_VOTES_CACHE_SIZE),
			pvfs_require_precheck: MemoryLruCache::new(PVFS_REQUIRE_PRECHECK_SIZE),
			validation_code_hash: MemoryLruCache::new(VALIDATION_CODE_HASH_CACHE_SIZE),
			version: MemoryLruCache::new(VERSION_CACHE_SIZE),
			disputes: MemoryLruCache::new(DISPUTES_CACHE_SIZE),
		}
	}
}

impl RequestResultCache {
	pub(crate) fn authorities(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&Vec<AuthorityDiscoveryId>> {
		self.authorities.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_authorities(
		&mut self,
		relay_parent: Hash,
		authorities: Vec<AuthorityDiscoveryId>,
	) {
		self.authorities.insert(relay_parent, VecOfDoesNotAllocate(authorities));
	}

	pub(crate) fn validators(&mut self, relay_parent: &Hash) -> Option<&Vec<ValidatorId>> {
		self.validators.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_validators(&mut self, relay_parent: Hash, validators: Vec<ValidatorId>) {
		self.validators.insert(relay_parent, ResidentSizeOf(validators));
	}

	pub(crate) fn validator_groups(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&(Vec<Vec<ValidatorIndex>>, GroupRotationInfo)> {
		self.validator_groups.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_validator_groups(
		&mut self,
		relay_parent: Hash,
		groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
	) {
		self.validator_groups.insert(relay_parent, ResidentSizeOf(groups));
	}

	pub(crate) fn availability_cores(&mut self, relay_parent: &Hash) -> Option<&Vec<CoreState>> {
		self.availability_cores.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_availability_cores(&mut self, relay_parent: Hash, cores: Vec<CoreState>) {
		self.availability_cores.insert(relay_parent, ResidentSizeOf(cores));
	}

	pub(crate) fn persisted_validation_data(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
	) -> Option<&Option<PersistedValidationData>> {
		self.persisted_validation_data.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_persisted_validation_data(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
		data: Option<PersistedValidationData>,
	) {
		self.persisted_validation_data.insert(key, ResidentSizeOf(data));
	}

	pub(crate) fn assumed_validation_data(
		&mut self,
		key: (Hash, ParaId, Hash),
	) -> Option<&Option<(PersistedValidationData, ValidationCodeHash)>> {
		self.assumed_validation_data.get(&(key.1, key.2)).map(|v| &v.0)
	}

	pub(crate) fn cache_assumed_validation_data(
		&mut self,
		key: (ParaId, Hash),
		data: Option<(PersistedValidationData, ValidationCodeHash)>,
	) {
		self.assumed_validation_data.insert(key, ResidentSizeOf(data));
	}

	pub(crate) fn check_validation_outputs(
		&mut self,
		key: (Hash, ParaId, CandidateCommitments),
	) -> Option<&bool> {
		self.check_validation_outputs.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_check_validation_outputs(
		&mut self,
		key: (Hash, ParaId, CandidateCommitments),
		value: bool,
	) {
		self.check_validation_outputs.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn session_index_for_child(&mut self, relay_parent: &Hash) -> Option<&SessionIndex> {
		self.session_index_for_child.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_session_index_for_child(
		&mut self,
		relay_parent: Hash,
		index: SessionIndex,
	) {
		self.session_index_for_child.insert(relay_parent, ResidentSizeOf(index));
	}

	pub(crate) fn validation_code(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
	) -> Option<&Option<ValidationCode>> {
		self.validation_code.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_validation_code(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
		value: Option<ValidationCode>,
	) {
		self.validation_code.insert(key, ResidentSizeOf(value));
	}

	// the actual key is `ValidationCodeHash` (`Hash` is ignored),
	// but we keep the interface that way to keep the macro simple
	pub(crate) fn validation_code_by_hash(
		&mut self,
		key: (Hash, ValidationCodeHash),
	) -> Option<&Option<ValidationCode>> {
		self.validation_code_by_hash.get(&key.1).map(|v| &v.0)
	}

	pub(crate) fn cache_validation_code_by_hash(
		&mut self,
		key: ValidationCodeHash,
		value: Option<ValidationCode>,
	) {
		self.validation_code_by_hash.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn candidate_pending_availability(
		&mut self,
		key: (Hash, ParaId),
	) -> Option<&Option<CommittedCandidateReceipt>> {
		self.candidate_pending_availability.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_candidate_pending_availability(
		&mut self,
		key: (Hash, ParaId),
		value: Option<CommittedCandidateReceipt>,
	) {
		self.candidate_pending_availability.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn candidate_events(&mut self, relay_parent: &Hash) -> Option<&Vec<CandidateEvent>> {
		self.candidate_events.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_candidate_events(
		&mut self,
		relay_parent: Hash,
		events: Vec<CandidateEvent>,
	) {
		self.candidate_events.insert(relay_parent, ResidentSizeOf(events));
	}

	pub(crate) fn session_info(&mut self, key: SessionIndex) -> Option<&SessionInfo> {
		self.session_info.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_session_info(&mut self, key: SessionIndex, value: SessionInfo) {
		self.session_info.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn dmq_contents(
		&mut self,
		key: (Hash, ParaId),
	) -> Option<&Vec<InboundDownwardMessage<BlockNumber>>> {
		self.dmq_contents.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_dmq_contents(
		&mut self,
		key: (Hash, ParaId),
		value: Vec<InboundDownwardMessage<BlockNumber>>,
	) {
		self.dmq_contents.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn inbound_hrmp_channels_contents(
		&mut self,
		key: (Hash, ParaId),
	) -> Option<&BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>> {
		self.inbound_hrmp_channels_contents.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_inbound_hrmp_channel_contents(
		&mut self,
		key: (Hash, ParaId),
		value: BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>,
	) {
		self.inbound_hrmp_channels_contents.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn current_babe_epoch(&mut self, relay_parent: &Hash) -> Option<&Epoch> {
		self.current_babe_epoch.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_current_babe_epoch(&mut self, relay_parent: Hash, epoch: Epoch) {
		self.current_babe_epoch.insert(relay_parent, DoesNotAllocate(epoch));
	}

	pub(crate) fn on_chain_votes(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&Option<ScrapedOnChainVotes>> {
		self.on_chain_votes.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_on_chain_votes(
		&mut self,
		relay_parent: Hash,
		scraped: Option<ScrapedOnChainVotes>,
	) {
		self.on_chain_votes.insert(relay_parent, ResidentSizeOf(scraped));
	}

	pub(crate) fn pvfs_require_precheck(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&Vec<ValidationCodeHash>> {
		self.pvfs_require_precheck.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_pvfs_require_precheck(
		&mut self,
		relay_parent: Hash,
		pvfs: Vec<ValidationCodeHash>,
	) {
		self.pvfs_require_precheck.insert(relay_parent, ResidentSizeOf(pvfs))
	}

	pub(crate) fn validation_code_hash(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
	) -> Option<&Option<ValidationCodeHash>> {
		self.validation_code_hash.get(&key).map(|v| &v.0)
	}

	pub(crate) fn cache_validation_code_hash(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
		value: Option<ValidationCodeHash>,
	) {
		self.validation_code_hash.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn version(&mut self, relay_parent: &Hash) -> Option<&u32> {
		self.version.get(&relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_version(&mut self, key: Hash, value: u32) {
		self.version.insert(key, ResidentSizeOf(value));
	}

	pub(crate) fn disputes(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>> {
		self.disputes.get(relay_parent).map(|v| &v.0)
	}

	pub(crate) fn cache_disputes(
		&mut self,
		relay_parent: Hash,
		value: Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>,
	) {
		self.disputes.insert(relay_parent, ResidentSizeOf(value));
	}
}

pub(crate) enum RequestResult {
	// The structure of each variant is (relay_parent, [params,]*, result)
	Authorities(Hash, Vec<AuthorityDiscoveryId>),
	Validators(Hash, Vec<ValidatorId>),
	ValidatorGroups(Hash, (Vec<Vec<ValidatorIndex>>, GroupRotationInfo)),
	AvailabilityCores(Hash, Vec<CoreState>),
	PersistedValidationData(Hash, ParaId, OccupiedCoreAssumption, Option<PersistedValidationData>),
	AssumedValidationData(
		Hash,
		ParaId,
		Hash,
		Option<(PersistedValidationData, ValidationCodeHash)>,
	),
	CheckValidationOutputs(Hash, ParaId, CandidateCommitments, bool),
	SessionIndexForChild(Hash, SessionIndex),
	ValidationCode(Hash, ParaId, OccupiedCoreAssumption, Option<ValidationCode>),
	ValidationCodeByHash(Hash, ValidationCodeHash, Option<ValidationCode>),
	CandidatePendingAvailability(Hash, ParaId, Option<CommittedCandidateReceipt>),
	CandidateEvents(Hash, Vec<CandidateEvent>),
	SessionInfo(Hash, SessionIndex, Option<SessionInfo>),
	DmqContents(Hash, ParaId, Vec<InboundDownwardMessage<BlockNumber>>),
	InboundHrmpChannelsContents(
		Hash,
		ParaId,
		BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>,
	),
	CurrentBabeEpoch(Hash, Epoch),
	FetchOnChainVotes(Hash, Option<ScrapedOnChainVotes>),
	PvfsRequirePrecheck(Hash, Vec<ValidationCodeHash>),
	// This is a request with side-effects and no result, hence ().
	SubmitPvfCheckStatement(Hash, PvfCheckStatement, ValidatorSignature, ()),
	ValidationCodeHash(Hash, ParaId, OccupiedCoreAssumption, Option<ValidationCodeHash>),
	Version(Hash, u32),
	StagingDisputes(Hash, Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>),
}
