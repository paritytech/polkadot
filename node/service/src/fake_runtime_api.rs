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

//! Provides "fake" runtime API implementations
//!
//! These are used to provide a type that implements these runtime APIs without requiring to import
//! the native runtimes.

use beefy_primitives::ecdsa_crypto::{AuthorityId as BeefyId, Signature as BeefySignature};
use grandpa_primitives::AuthorityId as GrandpaId;
use pallet_transaction_payment::{FeeDetails, RuntimeDispatchInfo};
use polkadot_primitives::{
	runtime_api, slashing, AccountId, AuthorityDiscoveryId, Balance, Block, BlockNumber,
	CandidateCommitments, CandidateEvent, CandidateHash, CommittedCandidateReceipt, CoreState,
	DisputeState, ExecutorParams, GroupRotationInfo, Hash, Id as ParaId, InboundDownwardMessage,
	InboundHrmpMessage, Nonce, OccupiedCoreAssumption, PersistedValidationData, PvfCheckStatement,
	ScrapedOnChainVotes, SessionIndex, SessionInfo, ValidationCode, ValidationCodeHash,
	ValidatorId, ValidatorIndex, ValidatorSignature,
};
use sp_core::OpaqueMetadata;
use sp_runtime::{
	traits::Block as BlockT,
	transaction_validity::{TransactionSource, TransactionValidity},
	ApplyExtrinsicResult,
};
use sp_version::RuntimeVersion;
use sp_weights::Weight;
use std::collections::BTreeMap;

sp_api::decl_runtime_apis! {
	/// This runtime API is only implemented for the test runtime!
	pub trait GetLastTimestamp {
		/// Returns the last timestamp of a runtime.
		fn get_last_timestamp() -> u64;
	}
}

struct Runtime;

sp_api::impl_runtime_apis! {
	impl sp_api::Core<Block> for Runtime {
		fn version() -> RuntimeVersion {
			unimplemented!()
		}

		fn execute_block(_: Block) {
			unimplemented!()
		}

		fn initialize_block(_: &<Block as BlockT>::Header) {
			unimplemented!()
		}
	}

	impl sp_api::Metadata<Block> for Runtime {
		fn metadata() -> OpaqueMetadata {
			unimplemented!()
		}

		fn metadata_at_version(_: u32) -> Option<OpaqueMetadata> {
			unimplemented!()
		}

		fn metadata_versions() -> Vec<u32> {
			unimplemented!()
		}
	}

	impl sp_block_builder::BlockBuilder<Block> for Runtime {
		fn apply_extrinsic(_: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
			unimplemented!()
		}

		fn finalize_block() -> <Block as BlockT>::Header {
			unimplemented!()
		}

		fn inherent_extrinsics(_: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
			unimplemented!()
		}

		fn check_inherents(
			_: Block,
			_: sp_inherents::InherentData,
		) -> sp_inherents::CheckInherentsResult {
			unimplemented!()
		}
	}

	impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
		fn validate_transaction(
			_: TransactionSource,
			_: <Block as BlockT>::Extrinsic,
			_: <Block as BlockT>::Hash,
		) -> TransactionValidity {
			unimplemented!()
		}
	}

	impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
		fn offchain_worker(_: &<Block as BlockT>::Header) {
			unimplemented!()
		}
	}

	impl runtime_api::ParachainHost<Block, Hash, BlockNumber> for Runtime {
		fn validators() -> Vec<ValidatorId> {
			unimplemented!()
		}

		fn validator_groups() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo<BlockNumber>) {
			unimplemented!()
		}

		fn availability_cores() -> Vec<CoreState<Hash, BlockNumber>> {
			unimplemented!()
		}

		fn persisted_validation_data(_: ParaId, _: OccupiedCoreAssumption)
			-> Option<PersistedValidationData<Hash, BlockNumber>> {
			unimplemented!()
		}

		fn assumed_validation_data(
			_: ParaId,
			_: Hash,
		) -> Option<(PersistedValidationData<Hash, BlockNumber>, ValidationCodeHash)> {
			unimplemented!()
		}

		fn check_validation_outputs(
			_: ParaId,
			_: CandidateCommitments,
		) -> bool {
			unimplemented!()
		}

		fn session_index_for_child() -> SessionIndex {
			unimplemented!()
		}

		fn validation_code(_: ParaId, _: OccupiedCoreAssumption)
			-> Option<ValidationCode> {
			unimplemented!()
		}

		fn candidate_pending_availability(_: ParaId) -> Option<CommittedCandidateReceipt<Hash>> {
			unimplemented!()
		}

		fn candidate_events() -> Vec<CandidateEvent<Hash>> {
			unimplemented!()
		}

		fn session_info(_: SessionIndex) -> Option<SessionInfo> {
			unimplemented!()
		}

		fn session_executor_params(_: SessionIndex) -> Option<ExecutorParams> {
			unimplemented!()
		}

		fn dmq_contents(_: ParaId) -> Vec<InboundDownwardMessage<BlockNumber>> {
			unimplemented!()
		}

		fn inbound_hrmp_channels_contents(
			_: ParaId
		) -> BTreeMap<ParaId, Vec<InboundHrmpMessage<BlockNumber>>> {
			unimplemented!()
		}

		fn validation_code_by_hash(_: ValidationCodeHash) -> Option<ValidationCode> {
			unimplemented!()
		}

		fn on_chain_votes() -> Option<ScrapedOnChainVotes<Hash>> {
			unimplemented!()
		}

		fn submit_pvf_check_statement(
			_: PvfCheckStatement,
			_: ValidatorSignature,
		) {
			unimplemented!()
		}

		fn pvfs_require_precheck() -> Vec<ValidationCodeHash> {
			unimplemented!()
		}

		fn validation_code_hash(_: ParaId, _: OccupiedCoreAssumption)
			-> Option<ValidationCodeHash>
		{
			unimplemented!()
		}

		fn disputes() -> Vec<(SessionIndex, CandidateHash, DisputeState<BlockNumber>)> {
			unimplemented!()
		}

		fn unapplied_slashes(
		) -> Vec<(SessionIndex, CandidateHash, slashing::PendingSlashes)> {
			unimplemented!()
		}

		fn key_ownership_proof(
			_: ValidatorId,
		) -> Option<slashing::OpaqueKeyOwnershipProof> {
			unimplemented!()
		}

		fn submit_report_dispute_lost(
			_: slashing::DisputeProof,
			_: slashing::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			unimplemented!()
		}
	}

	impl beefy_primitives::BeefyApi<Block, BeefyId> for Runtime {
		fn beefy_genesis() -> Option<BlockNumber> {
			unimplemented!()
		}

		fn validator_set() -> Option<beefy_primitives::ValidatorSet<BeefyId>> {
			unimplemented!()
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			_: beefy_primitives::EquivocationProof<
				BlockNumber,
				BeefyId,
				BeefySignature,
			>,
			_: beefy_primitives::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			unimplemented!()
		}

		fn generate_key_ownership_proof(
			_: beefy_primitives::ValidatorSetId,
			_: BeefyId,
		) -> Option<beefy_primitives::OpaqueKeyOwnershipProof> {
			unimplemented!()
		}
	}

	impl sp_mmr_primitives::MmrApi<Block, Hash, BlockNumber> for Runtime {
		fn mmr_root() -> Result<Hash, sp_mmr_primitives::Error> {
			unimplemented!()
		}

		fn mmr_leaf_count() -> Result<sp_mmr_primitives::LeafIndex, sp_mmr_primitives::Error> {
			unimplemented!()
		}

		fn generate_proof(
			_: Vec<BlockNumber>,
			_: Option<BlockNumber>,
		) -> Result<(Vec<sp_mmr_primitives::EncodableOpaqueLeaf>, sp_mmr_primitives::Proof<Hash>), sp_mmr_primitives::Error> {
			unimplemented!()
		}

		fn verify_proof(_: Vec<sp_mmr_primitives::EncodableOpaqueLeaf>, _: sp_mmr_primitives::Proof<Hash>)
			-> Result<(), sp_mmr_primitives::Error>
		{
			unimplemented!()
		}

		fn verify_proof_stateless(
			_: Hash,
			_: Vec<sp_mmr_primitives::EncodableOpaqueLeaf>,
			_: sp_mmr_primitives::Proof<Hash>
		) -> Result<(), sp_mmr_primitives::Error> {
			unimplemented!()
		}
	}

	impl grandpa_primitives::GrandpaApi<Block> for Runtime {
		fn grandpa_authorities() -> Vec<(GrandpaId, u64)> {
			unimplemented!()
		}

		fn current_set_id() -> grandpa_primitives::SetId {
			unimplemented!()
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			_: grandpa_primitives::EquivocationProof<
				<Block as BlockT>::Hash,
				sp_runtime::traits::NumberFor<Block>,
			>,
			_: grandpa_primitives::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			unimplemented!()
		}

		fn generate_key_ownership_proof(
			_: grandpa_primitives::SetId,
			_: grandpa_primitives::AuthorityId,
		) -> Option<grandpa_primitives::OpaqueKeyOwnershipProof> {
			unimplemented!()
		}
	}

	impl sp_consensus_babe::BabeApi<Block> for Runtime {
		fn configuration() -> sp_consensus_babe::BabeConfiguration {
			unimplemented!()
		}

		fn current_epoch_start() -> sp_consensus_babe::Slot {
			unimplemented!()
		}

		fn current_epoch() -> sp_consensus_babe::Epoch {
			unimplemented!()
		}

		fn next_epoch() -> sp_consensus_babe::Epoch {
			unimplemented!()
		}

		fn generate_key_ownership_proof(
			_: sp_consensus_babe::Slot,
			_: sp_consensus_babe::AuthorityId,
		) -> Option<sp_consensus_babe::OpaqueKeyOwnershipProof> {
			unimplemented!()
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			_: sp_consensus_babe::EquivocationProof<<Block as BlockT>::Header>,
			_: sp_consensus_babe::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			unimplemented!()
		}
	}

	impl sp_authority_discovery::AuthorityDiscoveryApi<Block> for Runtime {
		fn authorities() -> Vec<AuthorityDiscoveryId> {
			unimplemented!()
		}
	}

	impl sp_session::SessionKeys<Block> for Runtime {
		fn generate_session_keys(_: Option<Vec<u8>>) -> Vec<u8> {
			unimplemented!()
		}

		fn decode_session_keys(
			_: Vec<u8>,
		) -> Option<Vec<(Vec<u8>, sp_core::crypto::KeyTypeId)>> {
			unimplemented!()
		}
	}

	impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
		fn account_nonce(_: AccountId) -> Nonce {
			unimplemented!()
		}
	}

	impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<
		Block,
		Balance,
	> for Runtime {
		fn query_info(_: <Block as BlockT>::Extrinsic, _: u32) -> RuntimeDispatchInfo<Balance> {
			unimplemented!()
		}
		fn query_fee_details(_: <Block as BlockT>::Extrinsic, _: u32) -> FeeDetails<Balance> {
			unimplemented!()
		}
		fn query_weight_to_fee(_: Weight) -> Balance {
			unimplemented!()
		}
		fn query_length_to_fee(_: u32) -> Balance {
			unimplemented!()
		}
	}

	impl crate::fake_runtime_api::GetLastTimestamp<Block> for Runtime {
		fn get_last_timestamp() -> u64 {
			unimplemented!()
		}
	}
}
