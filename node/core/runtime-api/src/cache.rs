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

use std::{collections::btree_map::BTreeMap, num::NonZeroUsize};

use lru::LruCache;
use sp_consensus_babe::Epoch;

use polkadot_primitives::v2::{
	AuthorityDiscoveryId, BlockNumber, CandidateCommitments, CandidateEvent, CandidateHash,
	CommittedCandidateReceipt, CoreState, DisputeState, GroupRotationInfo, Hash, Id as ParaId,
	InboundDownwardMessage, InboundHrmpMessage, OccupiedCoreAssumption, PersistedValidationData,
	PvfCheckStatement, ScrapedOnChainVotes, SessionIndex, SessionInfo, ValidationCode,
	ValidationCodeHash, ValidatorId, ValidatorIndex, ValidatorSignature,
};

/// For consistency we have the same capacity for all caches. We use 128 as we'll only need that
/// much if finality stalls (we only query state for unfinalized blocks + maybe latest finalized).
/// In any case, a cache is an optimization. We should avoid a situation where having a large cache
/// leads to OOM or puts pressure on other important stuff like PVF execution/preparation.
const DEFAULT_CACHE_CAP: NonZeroUsize = match NonZeroUsize::new(128) {
	Some(cap) => cap,
	None => panic!("lru capacity must be non-zero"),
};

pub(crate) struct RequestResultCache {
	authorities: LruCache<Hash, Vec<AuthorityDiscoveryId>>,
	validators: LruCache<Hash, Vec<ValidatorId>>,
	validator_groups: LruCache<Hash, (Vec<Vec<ValidatorIndex>>, GroupRotationInfo)>,
	availability_cores: LruCache<Hash, Vec<CoreState>>,
	persisted_validation_data:
		LruCache<(Hash, ParaId, OccupiedCoreAssumption), Option<PersistedValidationData>>,
	assumed_validation_data:
		LruCache<(ParaId, Hash), Option<(PersistedValidationData, ValidationCodeHash)>>,
	check_validation_outputs: LruCache<(Hash, ParaId, CandidateCommitments), bool>,
	session_index_for_child: LruCache<Hash, SessionIndex>,
	validation_code: LruCache<(Hash, ParaId, OccupiedCoreAssumption), Option<ValidationCode>>,
	validation_code_by_hash: LruCache<ValidationCodeHash, Option<ValidationCode>>,
	candidate_pending_availability: LruCache<(Hash, ParaId), Option<CommittedCandidateReceipt>>,
	candidate_events: LruCache<Hash, Vec<CandidateEvent>>,
	session_info: LruCache<SessionIndex, SessionInfo>,
	dmq_contents: LruCache<(Hash, ParaId), Vec<InboundDownwardMessage<BlockNumber>>>,
	inbound_hrmp_channels_contents:
		LruCache<(Hash, ParaId), BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>>,
	current_babe_epoch: LruCache<Hash, Epoch>,
	on_chain_votes: LruCache<Hash, Option<ScrapedOnChainVotes>>,
	pvfs_require_precheck: LruCache<Hash, Vec<ValidationCodeHash>>,
	validation_code_hash:
		LruCache<(Hash, ParaId, OccupiedCoreAssumption), Option<ValidationCodeHash>>,
	version: LruCache<Hash, u32>,
	disputes: LruCache<Hash, Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>>,
}

impl Default for RequestResultCache {
	fn default() -> Self {
		Self {
			authorities: LruCache::new(DEFAULT_CACHE_CAP),
			validators: LruCache::new(DEFAULT_CACHE_CAP),
			validator_groups: LruCache::new(DEFAULT_CACHE_CAP),
			availability_cores: LruCache::new(DEFAULT_CACHE_CAP),
			persisted_validation_data: LruCache::new(DEFAULT_CACHE_CAP),
			assumed_validation_data: LruCache::new(DEFAULT_CACHE_CAP),
			check_validation_outputs: LruCache::new(DEFAULT_CACHE_CAP),
			session_index_for_child: LruCache::new(DEFAULT_CACHE_CAP),
			validation_code: LruCache::new(DEFAULT_CACHE_CAP),
			validation_code_by_hash: LruCache::new(DEFAULT_CACHE_CAP),
			candidate_pending_availability: LruCache::new(DEFAULT_CACHE_CAP),
			candidate_events: LruCache::new(DEFAULT_CACHE_CAP),
			session_info: LruCache::new(DEFAULT_CACHE_CAP),
			dmq_contents: LruCache::new(DEFAULT_CACHE_CAP),
			inbound_hrmp_channels_contents: LruCache::new(DEFAULT_CACHE_CAP),
			current_babe_epoch: LruCache::new(DEFAULT_CACHE_CAP),
			on_chain_votes: LruCache::new(DEFAULT_CACHE_CAP),
			pvfs_require_precheck: LruCache::new(DEFAULT_CACHE_CAP),
			validation_code_hash: LruCache::new(DEFAULT_CACHE_CAP),
			version: LruCache::new(DEFAULT_CACHE_CAP),
			disputes: LruCache::new(DEFAULT_CACHE_CAP),
		}
	}
}

impl RequestResultCache {
	pub(crate) fn authorities(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&Vec<AuthorityDiscoveryId>> {
		self.authorities.get(relay_parent)
	}

	pub(crate) fn cache_authorities(
		&mut self,
		relay_parent: Hash,
		authorities: Vec<AuthorityDiscoveryId>,
	) {
		self.authorities.put(relay_parent, authorities);
	}

	pub(crate) fn validators(&mut self, relay_parent: &Hash) -> Option<&Vec<ValidatorId>> {
		self.validators.get(relay_parent)
	}

	pub(crate) fn cache_validators(&mut self, relay_parent: Hash, validators: Vec<ValidatorId>) {
		self.validators.put(relay_parent, validators);
	}

	pub(crate) fn validator_groups(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&(Vec<Vec<ValidatorIndex>>, GroupRotationInfo)> {
		self.validator_groups.get(relay_parent)
	}

	pub(crate) fn cache_validator_groups(
		&mut self,
		relay_parent: Hash,
		groups: (Vec<Vec<ValidatorIndex>>, GroupRotationInfo),
	) {
		self.validator_groups.put(relay_parent, groups);
	}

	pub(crate) fn availability_cores(&mut self, relay_parent: &Hash) -> Option<&Vec<CoreState>> {
		self.availability_cores.get(relay_parent)
	}

	pub(crate) fn cache_availability_cores(&mut self, relay_parent: Hash, cores: Vec<CoreState>) {
		self.availability_cores.put(relay_parent, cores);
	}

	pub(crate) fn persisted_validation_data(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
	) -> Option<&Option<PersistedValidationData>> {
		self.persisted_validation_data.get(&key)
	}

	pub(crate) fn cache_persisted_validation_data(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
		data: Option<PersistedValidationData>,
	) {
		self.persisted_validation_data.put(key, data);
	}

	pub(crate) fn assumed_validation_data(
		&mut self,
		key: (Hash, ParaId, Hash),
	) -> Option<&Option<(PersistedValidationData, ValidationCodeHash)>> {
		self.assumed_validation_data.get(&(key.1, key.2))
	}

	pub(crate) fn cache_assumed_validation_data(
		&mut self,
		key: (ParaId, Hash),
		data: Option<(PersistedValidationData, ValidationCodeHash)>,
	) {
		self.assumed_validation_data.put(key, data);
	}

	pub(crate) fn check_validation_outputs(
		&mut self,
		key: (Hash, ParaId, CandidateCommitments),
	) -> Option<&bool> {
		self.check_validation_outputs.get(&key)
	}

	pub(crate) fn cache_check_validation_outputs(
		&mut self,
		key: (Hash, ParaId, CandidateCommitments),
		value: bool,
	) {
		self.check_validation_outputs.put(key, value);
	}

	pub(crate) fn session_index_for_child(&mut self, relay_parent: &Hash) -> Option<&SessionIndex> {
		self.session_index_for_child.get(relay_parent)
	}

	pub(crate) fn cache_session_index_for_child(
		&mut self,
		relay_parent: Hash,
		index: SessionIndex,
	) {
		self.session_index_for_child.put(relay_parent, index);
	}

	pub(crate) fn validation_code(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
	) -> Option<&Option<ValidationCode>> {
		self.validation_code.get(&key)
	}

	pub(crate) fn cache_validation_code(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
		value: Option<ValidationCode>,
	) {
		self.validation_code.put(key, value);
	}

	// the actual key is `ValidationCodeHash` (`Hash` is ignored),
	// but we keep the interface that way to keep the macro simple
	pub(crate) fn validation_code_by_hash(
		&mut self,
		key: (Hash, ValidationCodeHash),
	) -> Option<&Option<ValidationCode>> {
		self.validation_code_by_hash.get(&key.1)
	}

	pub(crate) fn cache_validation_code_by_hash(
		&mut self,
		key: ValidationCodeHash,
		value: Option<ValidationCode>,
	) {
		self.validation_code_by_hash.put(key, value);
	}

	pub(crate) fn candidate_pending_availability(
		&mut self,
		key: (Hash, ParaId),
	) -> Option<&Option<CommittedCandidateReceipt>> {
		self.candidate_pending_availability.get(&key)
	}

	pub(crate) fn cache_candidate_pending_availability(
		&mut self,
		key: (Hash, ParaId),
		value: Option<CommittedCandidateReceipt>,
	) {
		self.candidate_pending_availability.put(key, value);
	}

	pub(crate) fn candidate_events(&mut self, relay_parent: &Hash) -> Option<&Vec<CandidateEvent>> {
		self.candidate_events.get(relay_parent)
	}

	pub(crate) fn cache_candidate_events(
		&mut self,
		relay_parent: Hash,
		events: Vec<CandidateEvent>,
	) {
		self.candidate_events.put(relay_parent, events);
	}

	pub(crate) fn session_info(&mut self, key: SessionIndex) -> Option<&SessionInfo> {
		self.session_info.get(&key)
	}

	pub(crate) fn cache_session_info(&mut self, key: SessionIndex, value: SessionInfo) {
		self.session_info.put(key, value);
	}

	pub(crate) fn dmq_contents(
		&mut self,
		key: (Hash, ParaId),
	) -> Option<&Vec<InboundDownwardMessage<BlockNumber>>> {
		self.dmq_contents.get(&key)
	}

	pub(crate) fn cache_dmq_contents(
		&mut self,
		key: (Hash, ParaId),
		value: Vec<InboundDownwardMessage<BlockNumber>>,
	) {
		self.dmq_contents.put(key, value);
	}

	pub(crate) fn inbound_hrmp_channels_contents(
		&mut self,
		key: (Hash, ParaId),
	) -> Option<&BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>> {
		self.inbound_hrmp_channels_contents.get(&key)
	}

	pub(crate) fn cache_inbound_hrmp_channel_contents(
		&mut self,
		key: (Hash, ParaId),
		value: BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>>,
	) {
		self.inbound_hrmp_channels_contents.put(key, value);
	}

	pub(crate) fn current_babe_epoch(&mut self, relay_parent: &Hash) -> Option<&Epoch> {
		self.current_babe_epoch.get(relay_parent)
	}

	pub(crate) fn cache_current_babe_epoch(&mut self, relay_parent: Hash, epoch: Epoch) {
		self.current_babe_epoch.put(relay_parent, epoch);
	}

	pub(crate) fn on_chain_votes(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&Option<ScrapedOnChainVotes>> {
		self.on_chain_votes.get(relay_parent)
	}

	pub(crate) fn cache_on_chain_votes(
		&mut self,
		relay_parent: Hash,
		scraped: Option<ScrapedOnChainVotes>,
	) {
		self.on_chain_votes.put(relay_parent, scraped);
	}

	pub(crate) fn pvfs_require_precheck(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&Vec<ValidationCodeHash>> {
		self.pvfs_require_precheck.get(relay_parent)
	}

	pub(crate) fn cache_pvfs_require_precheck(
		&mut self,
		relay_parent: Hash,
		pvfs: Vec<ValidationCodeHash>,
	) {
		self.pvfs_require_precheck.put(relay_parent, pvfs);
	}

	pub(crate) fn validation_code_hash(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
	) -> Option<&Option<ValidationCodeHash>> {
		self.validation_code_hash.get(&key)
	}

	pub(crate) fn cache_validation_code_hash(
		&mut self,
		key: (Hash, ParaId, OccupiedCoreAssumption),
		value: Option<ValidationCodeHash>,
	) {
		self.validation_code_hash.put(key, value);
	}

	pub(crate) fn version(&mut self, relay_parent: &Hash) -> Option<&u32> {
		self.version.get(relay_parent)
	}

	pub(crate) fn cache_version(&mut self, key: Hash, value: u32) {
		self.version.put(key, value);
	}

	pub(crate) fn disputes(
		&mut self,
		relay_parent: &Hash,
	) -> Option<&Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>> {
		self.disputes.get(relay_parent)
	}

	pub(crate) fn cache_disputes(
		&mut self,
		relay_parent: Hash,
		value: Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>,
	) {
		self.disputes.put(relay_parent, value);
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
	Disputes(Hash, Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>),
}
