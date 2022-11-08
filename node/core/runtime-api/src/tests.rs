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

use super::*;

use ::test_helpers::{dummy_committed_candidate_receipt, dummy_validation_code};
use polkadot_node_primitives::{BabeAllowedSlots, BabeEpoch, BabeEpochConfiguration};
use polkadot_node_subsystem::SpawnGlue;
use polkadot_node_subsystem_test_helpers::make_subsystem_context;
use polkadot_primitives::{
	runtime_api::ParachainHost,
	v2::{
		AuthorityDiscoveryId, Block, CandidateEvent, CommittedCandidateReceipt, CoreState,
		GroupRotationInfo, Id as ParaId, InboundDownwardMessage, InboundHrmpMessage,
		OccupiedCoreAssumption, PersistedValidationData, PvfCheckStatement, ScrapedOnChainVotes,
		SessionIndex, SessionInfo, ValidationCode, ValidationCodeHash, ValidatorId, ValidatorIndex,
		ValidatorSignature,
	},
};
use sp_api::ProvideRuntimeApi;
use sp_authority_discovery::AuthorityDiscoveryApi;
use sp_consensus_babe::BabeApi;
use sp_core::testing::TaskExecutor;
use std::{
	collections::{BTreeMap, HashMap},
	sync::{Arc, Mutex},
};

#[derive(Default, Clone)]
struct MockRuntimeApi {
	authorities: Vec<AuthorityDiscoveryId>,
	validators: Vec<ValidatorId>,
	validator_groups: Vec<Vec<ValidatorIndex>>,
	availability_cores: Vec<CoreState>,
	availability_cores_wait: Arc<Mutex<()>>,
	validation_data: HashMap<ParaId, PersistedValidationData>,
	session_index_for_child: SessionIndex,
	session_info: HashMap<SessionIndex, SessionInfo>,
	validation_code: HashMap<ParaId, ValidationCode>,
	validation_code_by_hash: HashMap<ValidationCodeHash, ValidationCode>,
	validation_outputs_results: HashMap<ParaId, bool>,
	candidate_pending_availability: HashMap<ParaId, CommittedCandidateReceipt>,
	candidate_events: Vec<CandidateEvent>,
	dmq_contents: HashMap<ParaId, Vec<InboundDownwardMessage>>,
	hrmp_channels: HashMap<ParaId, BTreeMap<ParaId, Vec<InboundHrmpMessage>>>,
	babe_epoch: Option<BabeEpoch>,
	on_chain_votes: Option<ScrapedOnChainVotes>,
	submitted_pvf_check_statement: Arc<Mutex<Vec<(PvfCheckStatement, ValidatorSignature)>>>,
	pvfs_require_precheck: Vec<ValidationCodeHash>,
	validation_code_hash: HashMap<ParaId, ValidationCodeHash>,
}

impl ProvideRuntimeApi<Block> for MockRuntimeApi {
	type Api = Self;

	fn runtime_api<'a>(&'a self) -> sp_api::ApiRef<'a, Self::Api> {
		self.clone().into()
	}
}

sp_api::mock_impl_runtime_apis! {
	impl ParachainHost<Block> for MockRuntimeApi {
		fn validators(&self) -> Vec<ValidatorId> {
			self.validators.clone()
		}

		fn validator_groups(&self) -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo) {
			(
				self.validator_groups.clone(),
				GroupRotationInfo {
					session_start_block: 1,
					group_rotation_frequency: 100,
					now: 10,
				},
			)
		}

		fn availability_cores(&self) -> Vec<CoreState> {
			let _lock = self.availability_cores_wait.lock().unwrap();
			self.availability_cores.clone()
		}

		fn persisted_validation_data(
			&self,
			para: ParaId,
			_assumption: OccupiedCoreAssumption,
		) -> Option<PersistedValidationData> {
			self.validation_data.get(&para).cloned()
		}

		fn assumed_validation_data(
			para_id: ParaId,
			expected_persisted_validation_data_hash: Hash,
		) -> Option<(PersistedValidationData, ValidationCodeHash)> {
			self.validation_data
				.get(&para_id)
				.cloned()
				.filter(|data| data.hash() == expected_persisted_validation_data_hash)
				.zip(self.validation_code.get(&para_id).map(|code| code.hash()))
		}

		fn check_validation_outputs(
			&self,
			para_id: ParaId,
			_commitments: polkadot_primitives::v2::CandidateCommitments,
		) -> bool {
			self.validation_outputs_results
				.get(&para_id)
				.cloned()
				.expect(
					"`check_validation_outputs` called but the expected result hasn't been supplied"
				)
		}

		fn session_index_for_child(&self) -> SessionIndex {
			self.session_index_for_child.clone()
		}

		fn session_info(&self, index: SessionIndex) -> Option<SessionInfo> {
			self.session_info.get(&index).cloned()
		}

		fn validation_code(
			&self,
			para: ParaId,
			_assumption: OccupiedCoreAssumption,
		) -> Option<ValidationCode> {
			self.validation_code.get(&para).map(|c| c.clone())
		}

		fn candidate_pending_availability(
			&self,
			para: ParaId,
		) -> Option<CommittedCandidateReceipt> {
			self.candidate_pending_availability.get(&para).map(|c| c.clone())
		}

		fn candidate_events(&self) -> Vec<CandidateEvent> {
			self.candidate_events.clone()
		}

		fn dmq_contents(
			&self,
			recipient: ParaId,
		) -> Vec<InboundDownwardMessage> {
			self.dmq_contents.get(&recipient).map(|q| q.clone()).unwrap_or_default()
		}

		fn inbound_hrmp_channels_contents(
			&self,
			recipient: ParaId
		) -> BTreeMap<ParaId, Vec<InboundHrmpMessage>> {
			self.hrmp_channels.get(&recipient).map(|q| q.clone()).unwrap_or_default()
		}

		fn validation_code_by_hash(
			&self,
			hash: ValidationCodeHash,
		) -> Option<ValidationCode> {
			self.validation_code_by_hash.get(&hash).map(|c| c.clone())
		}

		fn on_chain_votes(&self) -> Option<ScrapedOnChainVotes> {
			self.on_chain_votes.clone()
		}

		fn submit_pvf_check_statement(stmt: PvfCheckStatement, signature: ValidatorSignature) {
			self
				.submitted_pvf_check_statement
				.lock()
				.expect("poisoned mutext")
				.push((stmt, signature));
		}

		fn pvfs_require_precheck() -> Vec<ValidationCodeHash> {
			self.pvfs_require_precheck.clone()
		}

		fn validation_code_hash(
			&self,
			para: ParaId,
			_assumption: OccupiedCoreAssumption,
		) -> Option<ValidationCodeHash> {
			self.validation_code_hash.get(&para).map(|c| c.clone())
		}
	}

	impl BabeApi<Block> for MockRuntimeApi {
		fn configuration(&self) -> sp_consensus_babe::BabeConfiguration {
			unimplemented!()
		}

		fn current_epoch_start(&self) -> sp_consensus_babe::Slot {
			self.babe_epoch.as_ref().unwrap().start_slot
		}

		fn current_epoch(&self) -> BabeEpoch {
			self.babe_epoch.as_ref().unwrap().clone()
		}

		fn next_epoch(&self) -> BabeEpoch {
			unimplemented!()
		}

		fn generate_key_ownership_proof(
			_slot: sp_consensus_babe::Slot,
			_authority_id: sp_consensus_babe::AuthorityId,
		) -> Option<sp_consensus_babe::OpaqueKeyOwnershipProof> {
			None
		}

		fn submit_report_equivocation_unsigned_extrinsic(
			_equivocation_proof: sp_consensus_babe::EquivocationProof<polkadot_primitives::v2::Header>,
			_key_owner_proof: sp_consensus_babe::OpaqueKeyOwnershipProof,
		) -> Option<()> {
			None
		}
	}

	impl AuthorityDiscoveryApi<Block> for MockRuntimeApi {
		fn authorities(&self) -> Vec<AuthorityDiscoveryId> {
			self.authorities.clone()
		}
	}
}

#[test]
fn requests_authorities() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let runtime_api = Arc::new(MockRuntimeApi::default());
	let relay_parent = [1; 32].into();
	let spawner = sp_core::testing::TaskExecutor::new();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::Authorities(tx)),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), runtime_api.authorities);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_validators() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let runtime_api = Arc::new(MockRuntimeApi::default());
	let relay_parent = [1; 32].into();
	let spawner = sp_core::testing::TaskExecutor::new();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::Validators(tx)),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), runtime_api.validators);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_validator_groups() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let runtime_api = Arc::new(MockRuntimeApi::default());
	let relay_parent = [1; 32].into();
	let spawner = sp_core::testing::TaskExecutor::new();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::ValidatorGroups(tx)),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap().0, runtime_api.validator_groups);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_availability_cores() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let runtime_api = Arc::new(MockRuntimeApi::default());
	let relay_parent = [1; 32].into();
	let spawner = sp_core::testing::TaskExecutor::new();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::AvailabilityCores(tx)),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), runtime_api.availability_cores);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_persisted_validation_data() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let relay_parent = [1; 32].into();
	let para_a = ParaId::from(5_u32);
	let para_b = ParaId::from(6_u32);
	let spawner = sp_core::testing::TaskExecutor::new();

	let mut runtime_api = MockRuntimeApi::default();
	runtime_api.validation_data.insert(para_a, Default::default());
	let runtime_api = Arc::new(runtime_api);

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::PersistedValidationData(para_a, OccupiedCoreAssumption::Included, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), Some(Default::default()));

		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::PersistedValidationData(para_b, OccupiedCoreAssumption::Included, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), None);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_assumed_validation_data() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let relay_parent = [1; 32].into();
	let para_a = ParaId::from(5_u32);
	let para_b = ParaId::from(6_u32);
	let spawner = sp_core::testing::TaskExecutor::new();

	let validation_code = ValidationCode(vec![1, 2, 3]);
	let expected_data_hash = <PersistedValidationData as Default>::default().hash();
	let expected_code_hash = validation_code.hash();

	let mut runtime_api = MockRuntimeApi::default();
	runtime_api.validation_data.insert(para_a, Default::default());
	runtime_api.validation_code.insert(para_a, validation_code);
	runtime_api.validation_data.insert(para_b, Default::default());
	let runtime_api = Arc::new(runtime_api);

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::AssumedValidationData(para_a, expected_data_hash, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), Some((Default::default(), expected_code_hash)));

		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::AssumedValidationData(para_a, Hash::zero(), tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), None);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_check_validation_outputs() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let mut runtime_api = MockRuntimeApi::default();
	let relay_parent = [1; 32].into();
	let para_a = ParaId::from(5_u32);
	let para_b = ParaId::from(6_u32);
	let commitments = polkadot_primitives::v2::CandidateCommitments::default();
	let spawner = sp_core::testing::TaskExecutor::new();

	runtime_api.validation_outputs_results.insert(para_a, false);
	runtime_api.validation_outputs_results.insert(para_b, true);

	let runtime_api = Arc::new(runtime_api);

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::CheckValidationOutputs(para_a, commitments.clone(), tx),
				),
			})
			.await;
		assert_eq!(rx.await.unwrap().unwrap(), runtime_api.validation_outputs_results[&para_a]);

		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::CheckValidationOutputs(para_b, commitments, tx),
				),
			})
			.await;
		assert_eq!(rx.await.unwrap().unwrap(), runtime_api.validation_outputs_results[&para_b]);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_session_index_for_child() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let runtime_api = Arc::new(MockRuntimeApi::default());
	let relay_parent = [1; 32].into();
	let spawner = sp_core::testing::TaskExecutor::new();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::SessionIndexForChild(tx)),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), runtime_api.session_index_for_child);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

fn dummy_session_info() -> SessionInfo {
	SessionInfo {
		validators: Default::default(),
		discovery_keys: vec![],
		assignment_keys: vec![],
		validator_groups: Default::default(),
		n_cores: 4u32,
		zeroth_delay_tranche_width: 0u32,
		relay_vrf_modulo_samples: 0u32,
		n_delay_tranches: 2u32,
		no_show_slots: 0u32,
		needed_approvals: 1u32,
		active_validator_indices: vec![],
		dispute_period: 6,
		random_seed: [0u8; 32],
	}
}
#[test]
fn requests_session_info() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let mut runtime_api = MockRuntimeApi::default();
	let session_index = 1;
	runtime_api.session_info.insert(session_index, dummy_session_info());
	let runtime_api = Arc::new(runtime_api);
	let spawner = sp_core::testing::TaskExecutor::new();

	let relay_parent = [1; 32].into();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::SessionInfo(session_index, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), Some(dummy_session_info()));

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_validation_code() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());

	let relay_parent = [1; 32].into();
	let para_a = ParaId::from(5_u32);
	let para_b = ParaId::from(6_u32);
	let spawner = sp_core::testing::TaskExecutor::new();
	let validation_code = dummy_validation_code();

	let mut runtime_api = MockRuntimeApi::default();
	runtime_api.validation_code.insert(para_a, validation_code.clone());
	let runtime_api = Arc::new(runtime_api);

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::ValidationCode(para_a, OccupiedCoreAssumption::Included, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), Some(validation_code));

		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::ValidationCode(para_b, OccupiedCoreAssumption::Included, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), None);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_candidate_pending_availability() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let relay_parent = [1; 32].into();
	let para_a = ParaId::from(5_u32);
	let para_b = ParaId::from(6_u32);
	let spawner = sp_core::testing::TaskExecutor::new();
	let candidate_receipt = dummy_committed_candidate_receipt(relay_parent);

	let mut runtime_api = MockRuntimeApi::default();
	runtime_api
		.candidate_pending_availability
		.insert(para_a, candidate_receipt.clone());
	let runtime_api = Arc::new(runtime_api);

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::CandidatePendingAvailability(para_a, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), Some(candidate_receipt));

		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::CandidatePendingAvailability(para_b, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), None);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_candidate_events() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let runtime_api = Arc::new(MockRuntimeApi::default());
	let relay_parent = [1; 32].into();
	let spawner = sp_core::testing::TaskExecutor::new();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::CandidateEvents(tx)),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), runtime_api.candidate_events);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_dmq_contents() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());

	let relay_parent = [1; 32].into();
	let para_a = ParaId::from(5_u32);
	let para_b = ParaId::from(6_u32);
	let spawner = sp_core::testing::TaskExecutor::new();

	let runtime_api = Arc::new({
		let mut runtime_api = MockRuntimeApi::default();

		runtime_api.dmq_contents.insert(para_a, vec![]);
		runtime_api.dmq_contents.insert(
			para_b,
			vec![InboundDownwardMessage { sent_at: 228, msg: b"Novus Ordo Seclorum".to_vec() }],
		);

		runtime_api
	});

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::DmqContents(para_a, tx)),
			})
			.await;
		assert_eq!(rx.await.unwrap().unwrap(), vec![]);

		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::DmqContents(para_b, tx)),
			})
			.await;
		assert_eq!(
			rx.await.unwrap().unwrap(),
			vec![InboundDownwardMessage { sent_at: 228, msg: b"Novus Ordo Seclorum".to_vec() }]
		);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};
	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_inbound_hrmp_channels_contents() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());

	let relay_parent = [1; 32].into();
	let para_a = ParaId::from(99_u32);
	let para_b = ParaId::from(66_u32);
	let para_c = ParaId::from(33_u32);
	let spawner = sp_core::testing::TaskExecutor::new();

	let para_b_inbound_channels = [
		(para_a, vec![]),
		(para_c, vec![InboundHrmpMessage { sent_at: 1, data: "𝙀=𝙈𝘾²".as_bytes().to_owned() }]),
	]
	.iter()
	.cloned()
	.collect::<BTreeMap<_, _>>();

	let runtime_api = Arc::new({
		let mut runtime_api = MockRuntimeApi::default();

		runtime_api.hrmp_channels.insert(para_a, BTreeMap::new());
		runtime_api.hrmp_channels.insert(para_b, para_b_inbound_channels.clone());

		runtime_api
	});

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::InboundHrmpChannelsContents(para_a, tx),
				),
			})
			.await;
		assert_eq!(rx.await.unwrap().unwrap(), BTreeMap::new());

		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::InboundHrmpChannelsContents(para_b, tx),
				),
			})
			.await;
		assert_eq!(rx.await.unwrap().unwrap(), para_b_inbound_channels);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};
	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_validation_code_by_hash() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let spawner = sp_core::testing::TaskExecutor::new();

	let (runtime_api, validation_code) = {
		let mut runtime_api = MockRuntimeApi::default();
		let mut validation_code = Vec::new();

		for n in 0..5 {
			let code = ValidationCode::from(vec![n; 32]);
			runtime_api.validation_code_by_hash.insert(code.hash(), code.clone());
			validation_code.push(code);
		}

		(runtime_api, validation_code)
	};

	let subsystem =
		RuntimeApiSubsystem::new(Arc::new(runtime_api), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());

	let relay_parent = [1; 32].into();
	let test_task = async move {
		for code in validation_code {
			let (tx, rx) = oneshot::channel();
			ctx_handle
				.send(FromOrchestra::Communication {
					msg: RuntimeApiMessage::Request(
						relay_parent,
						Request::ValidationCodeByHash(code.hash(), tx),
					),
				})
				.await;

			assert_eq!(rx.await.unwrap().unwrap(), Some(code));
		}

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn multiple_requests_in_parallel_are_working() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let runtime_api = Arc::new(MockRuntimeApi::default());
	let relay_parent = [1; 32].into();
	let spawner = sp_core::testing::TaskExecutor::new();
	let mutex = runtime_api.availability_cores_wait.clone();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		// Make all requests block until we release this mutex.
		let lock = mutex.lock().unwrap();

		let mut receivers = Vec::new();
		for _ in 0..MAX_PARALLEL_REQUESTS {
			let (tx, rx) = oneshot::channel();

			ctx_handle
				.send(FromOrchestra::Communication {
					msg: RuntimeApiMessage::Request(relay_parent, Request::AvailabilityCores(tx)),
				})
				.await;
			receivers.push(rx);
		}

		// The backpressure from reaching `MAX_PARALLEL_REQUESTS` will make the test block, we need to drop the lock.
		drop(lock);

		for _ in 0..MAX_PARALLEL_REQUESTS * 100 {
			let (tx, rx) = oneshot::channel();

			ctx_handle
				.send(FromOrchestra::Communication {
					msg: RuntimeApiMessage::Request(relay_parent, Request::AvailabilityCores(tx)),
				})
				.await;
			receivers.push(rx);
		}

		let join = future::join_all(receivers);

		join.await
			.into_iter()
			.for_each(|r| assert_eq!(r.unwrap().unwrap(), runtime_api.availability_cores));

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_babe_epoch() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let mut runtime_api = MockRuntimeApi::default();
	let epoch = BabeEpoch {
		epoch_index: 100,
		start_slot: sp_consensus_babe::Slot::from(1000),
		duration: 10,
		authorities: Vec::new(),
		randomness: [1u8; 32],
		config: BabeEpochConfiguration { c: (1, 4), allowed_slots: BabeAllowedSlots::PrimarySlots },
	};
	runtime_api.babe_epoch = Some(epoch.clone());
	let runtime_api = Arc::new(runtime_api);
	let relay_parent = [1; 32].into();
	let spawner = sp_core::testing::TaskExecutor::new();

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::CurrentBabeEpoch(tx)),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), epoch);
		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_submit_pvf_check_statement() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let spawner = sp_core::testing::TaskExecutor::new();

	let runtime_api = Arc::new(MockRuntimeApi::default());
	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());

	let relay_parent = [1; 32].into();
	let test_task = async move {
		let (stmt, sig) = fake_statement();

		// Send the same statement twice.
		//
		// Here we just want to ensure that those requests do not go through the cache.
		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::SubmitPvfCheckStatement(stmt.clone(), sig.clone(), tx),
				),
			})
			.await;
		assert_eq!(rx.await.unwrap().unwrap(), ());
		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::SubmitPvfCheckStatement(stmt.clone(), sig.clone(), tx),
				),
			})
			.await;
		assert_eq!(rx.await.unwrap().unwrap(), ());

		assert_eq!(
			&*runtime_api.submitted_pvf_check_statement.lock().expect("poisened mutex"),
			&[(stmt.clone(), sig.clone()), (stmt.clone(), sig.clone())]
		);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));

	fn fake_statement() -> (PvfCheckStatement, ValidatorSignature) {
		let stmt = PvfCheckStatement {
			accept: true,
			subject: [1; 32].into(),
			session_index: 1,
			validator_index: 1.into(),
		};
		let sig = sp_keyring::Sr25519Keyring::Alice.sign(&stmt.signing_payload()).into();
		(stmt, sig)
	}
}

#[test]
fn requests_pvfs_require_precheck() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());
	let spawner = sp_core::testing::TaskExecutor::new();

	let runtime_api = Arc::new({
		let mut runtime_api = MockRuntimeApi::default();
		runtime_api.pvfs_require_precheck = vec![[1; 32].into(), [2; 32].into()];
		runtime_api
	});

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());

	let relay_parent = [1; 32].into();
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::PvfsRequirePrecheck(tx)),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), vec![[1; 32].into(), [2; 32].into()]);
		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}

#[test]
fn requests_validation_code_hash() {
	let (ctx, mut ctx_handle) = make_subsystem_context(TaskExecutor::new());

	let relay_parent = [1; 32].into();
	let para_a = ParaId::from(5_u32);
	let para_b = ParaId::from(6_u32);
	let spawner = sp_core::testing::TaskExecutor::new();
	let validation_code_hash = dummy_validation_code().hash();

	let mut runtime_api = MockRuntimeApi::default();
	runtime_api.validation_code_hash.insert(para_a, validation_code_hash.clone());
	let runtime_api = Arc::new(runtime_api);

	let subsystem =
		RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), SpawnGlue(spawner));
	let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
	let test_task = async move {
		let (tx, rx) = oneshot::channel();

		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::ValidationCodeHash(para_a, OccupiedCoreAssumption::Included, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), Some(validation_code_hash));

		let (tx, rx) = oneshot::channel();
		ctx_handle
			.send(FromOrchestra::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::ValidationCodeHash(para_b, OccupiedCoreAssumption::Included, tx),
				),
			})
			.await;

		assert_eq!(rx.await.unwrap().unwrap(), None);

		ctx_handle.send(FromOrchestra::Signal(OverseerSignal::Conclude)).await;
	};

	futures::executor::block_on(future::join(subsystem_task, test_task));
}
