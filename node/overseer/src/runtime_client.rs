use async_trait::async_trait;
use polkadot_primitives::{
	runtime_api::ParachainHost,
	v2::{
		Block, BlockId, BlockNumber, CandidateCommitments, CandidateEvent, CandidateHash,
		CommittedCandidateReceipt, CoreState, DisputeState, GroupRotationInfo, Hash, Header, Id,
		InboundDownwardMessage, InboundHrmpMessage, OccupiedCoreAssumption,
		PersistedValidationData, PvfCheckStatement, ScrapedOnChainVotes, SessionIndex, SessionInfo,
		Slot, ValidationCode, ValidationCodeHash, ValidatorId, ValidatorIndex, ValidatorSignature,
	},
};
use sp_api::{ApiError, ApiExt, ProvideRuntimeApi};
use sp_authority_discovery::AuthorityDiscoveryApi;
use sp_consensus_babe::{
	AuthorityId, BabeApi, BabeGenesisConfiguration, Epoch, EquivocationProof,
	OpaqueKeyOwnershipProof,
};
use std::collections::BTreeMap;

/// Exposes all runtime calls that are used by the runtime api subsystem.
#[async_trait]
pub trait OverseerRuntimeClient {
	/// Get the current validators.
	async fn validators(&self, at: Hash) -> Result<Vec<ValidatorId>, ApiError>;

	/// Returns the validator groups and rotation info localized based on the hypothetical child
	///  of a block whose state  this is invoked on. Note that `now` in the `GroupRotationInfo`
	/// should be the successor of the number of the block.
	async fn validator_groups(
		&self,
		at: Hash,
	) -> Result<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo<BlockNumber>), ApiError>;

	/// Yields information on all availability cores as relevant to the child block.
	/// Cores are either free or occupied. Free cores can have paras assigned to them.
	async fn availability_cores(
		&self,
		at: Hash,
	) -> Result<Vec<CoreState<Hash, BlockNumber>>, ApiError>;

	/// Yields the persisted validation data for the given `ParaId` along with an assumption that
	/// should be used if the para currently occupies a core.
	///
	/// Returns `None` if either the para is not registered or the assumption is `Freed`
	/// and the para already occupies a core.
	async fn persisted_validation_data(
		&self,
		at: Hash,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<PersistedValidationData<Hash, BlockNumber>>, ApiError>;

	/// Returns the persisted validation data for the given `ParaId` along with the corresponding
	/// validation code hash. Instead of accepting assumption about the para, matches the validation
	/// data hash against an expected one and yields `None` if they're not equal.
	async fn assumed_validation_data(
		&self,
		at: Hash,
		para_id: Id,
		expected_persisted_validation_data_hash: Hash,
	) -> Result<Option<(PersistedValidationData<Hash, BlockNumber>, ValidationCodeHash)>, ApiError>;

	/// Checks if the given validation outputs pass the acceptance criteria.
	async fn check_validation_outputs(
		&self,
		at: Hash,
		para_id: Id,
		outputs: CandidateCommitments,
	) -> Result<bool, ApiError>;

	/// Returns the session index expected at a child of the block.
	///
	/// This can be used to instantiate a `SigningContext`.
	async fn session_index_for_child(&self, at: Hash) -> Result<SessionIndex, ApiError>;

	/// Fetch the validation code used by a para, making the given `OccupiedCoreAssumption`.
	///
	/// Returns `None` if either the para is not registered or the assumption is `Freed`
	/// and the para already occupies a core.
	async fn validation_code(
		&self,
		at: Hash,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<ValidationCode>, ApiError>;

	/// Get the receipt of a candidate pending availability. This returns `Some` for any paras
	/// assigned to occupied cores in `availability_cores` and `None` otherwise.
	async fn candidate_pending_availability(
		&self,
		at: Hash,
		para_id: Id,
	) -> Result<Option<CommittedCandidateReceipt<Hash>>, ApiError>;

	/// Get a vector of events concerning candidates that occurred within a block.
	async fn candidate_events(&self, at: Hash) -> Result<Vec<CandidateEvent<Hash>>, ApiError>;

	/// Get all the pending inbound messages in the downward message queue for a para.
	async fn dmq_contents(
		&self,
		at: Hash,
		recipient: Id,
	) -> Result<Vec<InboundDownwardMessage<BlockNumber>>, ApiError>;

	/// Get the contents of all channels addressed to the given recipient. Channels that have no
	/// messages in them are also included.
	async fn inbound_hrmp_channels_contents(
		&self,
		at: Hash,
		recipient: Id,
	) -> Result<BTreeMap<Id, Vec<InboundHrmpMessage<BlockNumber>>>, ApiError>;

	/// Get the validation code from its hash.
	async fn validation_code_by_hash(
		&self,
		at: Hash,
		hash: ValidationCodeHash,
	) -> Result<Option<ValidationCode>, ApiError>;

	/// Scrape dispute relevant from on-chain, backing votes and resolved disputes.
	async fn on_chain_votes(&self, at: Hash)
		-> Result<Option<ScrapedOnChainVotes<Hash>>, ApiError>;

	/***** Added in v2 *****/

	/// Get the session info for the given session, if stored.
	///
	/// NOTE: This function is only available since parachain host version 2.
	async fn session_info(
		&self,
		at: Hash,
		index: SessionIndex,
	) -> Result<Option<SessionInfo>, ApiError>;

	/// Get the session info for the given session, if stored.
	///
	/// NOTE: This function is only available since parachain host version 2.
	async fn session_info_before_version_2(
		&self,
		at: Hash,
		index: SessionIndex,
	) -> Result<Option<polkadot_primitives::v2::OldV1SessionInfo>, ApiError>;

	/// Submits a PVF pre-checking statement into the transaction pool.
	///
	/// NOTE: This function is only available since parachain host version 2.
	async fn submit_pvf_check_statement(
		&self,
		at: Hash,
		stmt: PvfCheckStatement,
		signature: ValidatorSignature,
	) -> Result<(), ApiError>;

	/// Returns code hashes of PVFs that require pre-checking by validators in the active set.
	///
	/// NOTE: This function is only available since parachain host version 2.
	async fn pvfs_require_precheck(&self, at: Hash) -> Result<Vec<ValidationCodeHash>, ApiError>;

	/// Fetch the hash of the validation code used by a para, making the given `OccupiedCoreAssumption`.
	///
	/// NOTE: This function is only available since parachain host version 2.
	async fn validation_code_hash(
		&self,
		at: Hash,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<ValidationCodeHash>, ApiError>;

	/// ===BABE===
	///
	///
	/// Return the genesis configuration for BABE. The configuration is only read on genesis.
	async fn configuration(&self, at: Hash) -> Result<BabeGenesisConfiguration, ApiError>;

	/// Returns the slot that started the current epoch.
	async fn current_epoch_start(&self, at: Hash) -> Result<Slot, ApiError>;

	/// Returns information regarding the current epoch.
	async fn current_epoch(&self, at: Hash) -> Result<Epoch, ApiError>;

	/// Returns information regarding the next epoch (which was already
	/// previously announced).
	async fn next_epoch(&self, at: Hash) -> Result<Epoch, ApiError>;

	/// Generates a proof of key ownership for the given authority in the
	/// current epoch. An example usage of this module is coupled with the
	/// session historical module to prove that a given authority key is
	/// tied to a given staking identity during a specific session. Proofs
	/// of key ownership are necessary for submitting equivocation reports.
	/// NOTE: even though the API takes a `slot` as parameter the current
	/// implementations ignores this parameter and instead relies on this
	/// method being called at the correct block height, i.e. any point at
	/// which the epoch for the given slot is live on-chain. Future
	/// implementations will instead use indexed data through an offchain
	/// worker, not requiring older states to be available.
	async fn generate_key_ownership_proof(
		&self,
		at: Hash,
		slot: Slot,
		authority_id: AuthorityId,
	) -> Result<Option<OpaqueKeyOwnershipProof>, ApiError>;

	/// Submits an unsigned extrinsic to report an equivocation. The caller
	/// must provide the equivocation proof and a key ownership proof
	/// (should be obtained using `generate_key_ownership_proof`). The
	/// extrinsic will be unsigned and should only be accepted for local
	/// authorship (not to be broadcast to the network). This method returns
	/// `None` when creation of the extrinsic fails, e.g. if equivocation
	/// reporting is disabled for the given runtime (i.e. this method is
	/// hardcoded to return `None`). Only useful in an offchain context.
	async fn submit_report_equivocation_unsigned_extrinsic(
		&self,
		at: Hash,
		equivocation_proof: EquivocationProof<Header>,
		key_owner_proof: OpaqueKeyOwnershipProof,
	) -> Result<Option<()>, ApiError>;

	/// Retrieve authority identifiers of the current and next authority set.
	async fn authorities(
		&self,
		at: Hash,
	) -> std::result::Result<Vec<sp_authority_discovery::AuthorityId>, ApiError>;

	/// Parachain host api version
	async fn api_version_parachain_host(&self, at: Hash) -> Result<Option<u32>, ApiError>;

	/// Returns all onchain disputes.
	/// This is a staging method! Do not use on production runtimes!
	async fn staging_get_disputes(
		&self,
		at: Hash,
	) -> Result<Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>, ApiError>;
}

#[async_trait]
impl<T> OverseerRuntimeClient for T
where
	T: ProvideRuntimeApi<Block> + Send + Sync,
	T::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
{
	async fn validators(&self, at: Hash) -> Result<Vec<ValidatorId>, ApiError> {
		self.runtime_api().validators(&BlockId::Hash(at))
	}

	async fn validator_groups(
		&self,
		at: Hash,
	) -> Result<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo<BlockNumber>), ApiError> {
		self.runtime_api().validator_groups(&BlockId::Hash(at))
	}

	async fn availability_cores(
		&self,
		at: Hash,
	) -> Result<Vec<CoreState<Hash, BlockNumber>>, ApiError> {
		self.runtime_api().availability_cores(&BlockId::Hash(at))
	}

	async fn persisted_validation_data(
		&self,
		at: Hash,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<PersistedValidationData<Hash, BlockNumber>>, ApiError> {
		self.runtime_api()
			.persisted_validation_data(&BlockId::Hash(at), para_id, assumption)
	}

	async fn assumed_validation_data(
		&self,
		at: Hash,
		para_id: Id,
		expected_persisted_validation_data_hash: Hash,
	) -> Result<Option<(PersistedValidationData<Hash, BlockNumber>, ValidationCodeHash)>, ApiError>
	{
		self.runtime_api().assumed_validation_data(
			&BlockId::Hash(at),
			para_id,
			expected_persisted_validation_data_hash,
		)
	}

	async fn check_validation_outputs(
		&self,
		at: Hash,
		para_id: Id,
		outputs: CandidateCommitments,
	) -> Result<bool, ApiError> {
		self.runtime_api()
			.check_validation_outputs(&BlockId::Hash(at), para_id, outputs)
	}

	async fn session_index_for_child(&self, at: Hash) -> Result<SessionIndex, ApiError> {
		self.runtime_api().session_index_for_child(&BlockId::Hash(at))
	}

	async fn validation_code(
		&self,
		at: Hash,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<ValidationCode>, ApiError> {
		self.runtime_api().validation_code(&BlockId::Hash(at), para_id, assumption)
	}

	async fn candidate_pending_availability(
		&self,
		at: Hash,
		para_id: Id,
	) -> Result<Option<CommittedCandidateReceipt<Hash>>, ApiError> {
		self.runtime_api().candidate_pending_availability(&BlockId::Hash(at), para_id)
	}

	async fn candidate_events(&self, at: Hash) -> Result<Vec<CandidateEvent<Hash>>, ApiError> {
		self.runtime_api().candidate_events(&BlockId::Hash(at))
	}

	async fn dmq_contents(
		&self,
		at: Hash,
		recipient: Id,
	) -> Result<Vec<InboundDownwardMessage<BlockNumber>>, ApiError> {
		self.runtime_api().dmq_contents(&BlockId::Hash(at), recipient)
	}

	async fn inbound_hrmp_channels_contents(
		&self,
		at: Hash,
		recipient: Id,
	) -> Result<BTreeMap<Id, Vec<InboundHrmpMessage<BlockNumber>>>, ApiError> {
		self.runtime_api().inbound_hrmp_channels_contents(&BlockId::Hash(at), recipient)
	}

	async fn validation_code_by_hash(
		&self,
		at: Hash,
		hash: ValidationCodeHash,
	) -> Result<Option<ValidationCode>, ApiError> {
		self.runtime_api().validation_code_by_hash(&BlockId::Hash(at), hash)
	}

	async fn on_chain_votes(
		&self,
		at: Hash,
	) -> Result<Option<ScrapedOnChainVotes<Hash>>, ApiError> {
		self.runtime_api().on_chain_votes(&BlockId::Hash(at))
	}

	async fn session_info(
		&self,
		at: Hash,
		index: SessionIndex,
	) -> Result<Option<SessionInfo>, ApiError> {
		self.runtime_api().session_info(&BlockId::Hash(at), index)
	}

	async fn submit_pvf_check_statement(
		&self,
		at: Hash,
		stmt: PvfCheckStatement,
		signature: ValidatorSignature,
	) -> Result<(), ApiError> {
		self.runtime_api()
			.submit_pvf_check_statement(&BlockId::Hash(at), stmt, signature)
	}

	async fn pvfs_require_precheck(&self, at: Hash) -> Result<Vec<ValidationCodeHash>, ApiError> {
		self.runtime_api().pvfs_require_precheck(&BlockId::Hash(at))
	}

	async fn validation_code_hash(
		&self,
		at: Hash,
		para_id: Id,
		assumption: OccupiedCoreAssumption,
	) -> Result<Option<ValidationCodeHash>, ApiError> {
		self.runtime_api().validation_code_hash(&BlockId::Hash(at), para_id, assumption)
	}

	async fn configuration(&self, at: Hash) -> Result<BabeGenesisConfiguration, ApiError> {
		self.runtime_api().configuration(&BlockId::Hash(at))
	}

	async fn current_epoch_start(&self, at: Hash) -> Result<Slot, ApiError> {
		self.runtime_api().current_epoch_start(&BlockId::Hash(at))
	}

	async fn current_epoch(&self, at: Hash) -> Result<Epoch, ApiError> {
		self.runtime_api().current_epoch(&BlockId::Hash(at))
	}

	async fn next_epoch(&self, at: Hash) -> Result<Epoch, ApiError> {
		self.runtime_api().next_epoch(&BlockId::Hash(at))
	}

	async fn generate_key_ownership_proof(
		&self,
		at: Hash,
		slot: Slot,
		authority_id: AuthorityId,
	) -> Result<Option<OpaqueKeyOwnershipProof>, ApiError> {
		self.runtime_api()
			.generate_key_ownership_proof(&BlockId::Hash(at), slot, authority_id)
	}

	async fn submit_report_equivocation_unsigned_extrinsic(
		&self,
		at: Hash,
		equivocation_proof: EquivocationProof<Header>,
		key_owner_proof: OpaqueKeyOwnershipProof,
	) -> Result<Option<()>, ApiError> {
		self.runtime_api().submit_report_equivocation_unsigned_extrinsic(
			&BlockId::Hash(at),
			equivocation_proof,
			key_owner_proof,
		)
	}

	async fn authorities(
		&self,
		at: Hash,
	) -> std::result::Result<Vec<sp_authority_discovery::AuthorityId>, ApiError> {
		self.runtime_api().authorities(&BlockId::Hash(at))
	}

	async fn api_version_parachain_host(&self, at: Hash) -> Result<Option<u32>, ApiError> {
		self.runtime_api().api_version::<dyn ParachainHost<Block>>(&BlockId::Hash(at))
	}

	#[warn(deprecated)]
	async fn session_info_before_version_2(
		&self,
		at: Hash,
		index: SessionIndex,
	) -> Result<Option<polkadot_primitives::v2::OldV1SessionInfo>, ApiError> {
		#[allow(deprecated)]
		self.runtime_api().session_info_before_version_2(&BlockId::Hash(at), index)
	}

	async fn staging_get_disputes(
		&self,
		at: Hash,
	) -> Result<Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)>, ApiError> {
		self.runtime_api().staging_get_disputes(&BlockId::Hash(at))
	}
}
