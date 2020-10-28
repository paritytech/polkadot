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

//! Implements the Runtime API Subsystem
//!
//! This provides a clean, ownerless wrapper around the parachain-related runtime APIs. This crate
//! can also be used to cache responses from heavy runtime APIs.

#![deny(unused_crate_dependencies)]
#![warn(missing_docs)]

use polkadot_subsystem::{
	Subsystem, SpawnedSubsystem, SubsystemResult, SubsystemContext,
	FromOverseer, OverseerSignal,
	messages::{
		RuntimeApiMessage, RuntimeApiRequest as Request,
	},
	errors::RuntimeApiError,
};
use polkadot_node_subsystem_util::{
	metrics::{self, prometheus},
};
use polkadot_primitives::v1::{Block, BlockId, Hash, ParachainHost};
use std::sync::Arc;

use sp_api::{ProvideRuntimeApi};

use futures::prelude::*;

/// The `RuntimeApiSubsystem`. See module docs for more details.
pub struct RuntimeApiSubsystem<Client> {
	client: Arc<Client>,
	metrics: Metrics,
}

impl<Client> RuntimeApiSubsystem<Client> {
	/// Create a new Runtime API subsystem wrapping the given client and metrics.
	pub fn new(client: Arc<Client>, metrics: Metrics) -> Self {
		RuntimeApiSubsystem { client, metrics }
	}
}

impl<Client, Context> Subsystem<Context> for RuntimeApiSubsystem<Client> where
	Client: ProvideRuntimeApi<Block> + Send + 'static + Sync,
	Client::Api: ParachainHost<Block>,
	Context: SubsystemContext<Message = RuntimeApiMessage>
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem {
			future: run(ctx, self).map(|_| ()).boxed(),
			name: "runtime-api-subsystem",
		}
	}
}

async fn run<Client>(
	mut ctx: impl SubsystemContext<Message = RuntimeApiMessage>,
	subsystem: RuntimeApiSubsystem<Client>,
) -> SubsystemResult<()> where
	Client: ProvideRuntimeApi<Block>,
	Client::Api: ParachainHost<Block>,
{
	loop {
		match ctx.recv().await? {
			FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
			FromOverseer::Signal(OverseerSignal::ActiveLeaves(_)) => {},
			FromOverseer::Signal(OverseerSignal::BlockFinalized(_)) => {},
			FromOverseer::Communication { msg } => match msg {
				RuntimeApiMessage::Request(relay_parent, request) => make_runtime_api_request(
					&*subsystem.client,
					&subsystem.metrics,
					relay_parent,
					request,
				),
			}
		}
	}
}

fn make_runtime_api_request<Client>(
	client: &Client,
	metrics: &Metrics,
	relay_parent: Hash,
	request: Request,
) where
	Client: ProvideRuntimeApi<Block>,
	Client::Api: ParachainHost<Block>,
{
	macro_rules! query {
		($api_name:ident ($($param:expr),*), $sender:expr) => {{
			let sender = $sender;
			let api = client.runtime_api();
			let res = api.$api_name(&BlockId::Hash(relay_parent), $($param),*)
				.map_err(|e| RuntimeApiError::from(format!("{:?}", e)));
			metrics.on_request(res.is_ok());
			let _ = sender.send(res);
		}}
	}

	match request {
		Request::Validators(sender) => query!(validators(), sender),
		Request::ValidatorGroups(sender) => query!(validator_groups(), sender),
		Request::AvailabilityCores(sender) => query!(availability_cores(), sender),
		Request::PersistedValidationData(para, assumption, sender) =>
			query!(persisted_validation_data(para, assumption), sender),
		Request::FullValidationData(para, assumption, sender) =>
			query!(full_validation_data(para, assumption), sender),
		Request::CheckValidationOutputs(para, commitments, sender) =>
			query!(check_validation_outputs(para, commitments), sender),
		Request::SessionIndexForChild(sender) => query!(session_index_for_child(), sender),
		Request::ValidationCode(para, assumption, sender) =>
			query!(validation_code(para, assumption), sender),
		Request::CandidatePendingAvailability(para, sender) =>
			query!(candidate_pending_availability(para), sender),
		Request::CandidateEvents(sender) => query!(candidate_events(), sender),
		Request::ValidatorDiscovery(ids, sender) => query!(validator_discovery(ids), sender),
		Request::DmqContents(id, sender) => query!(dmq_contents(id), sender),
	}
}

#[derive(Clone)]
struct MetricsInner {
	chain_api_requests: prometheus::CounterVec<prometheus::U64>,
}

/// Runtime API metrics.
#[derive(Default, Clone)]
pub struct Metrics(Option<MetricsInner>);

impl Metrics {
	fn on_request(&self, succeeded: bool) {
		if let Some(metrics) = &self.0 {
			if succeeded {
				metrics.chain_api_requests.with_label_values(&["succeeded"]).inc();
			} else {
				metrics.chain_api_requests.with_label_values(&["failed"]).inc();
			}
		}
	}
}

impl metrics::Metrics for Metrics {
	fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError> {
		let metrics = MetricsInner {
			chain_api_requests: prometheus::register(
				prometheus::CounterVec::new(
					prometheus::Opts::new(
						"parachain_runtime_api_requests_total",
						"Number of Runtime API requests served.",
					),
					&["success"],
				)?,
				registry,
			)?,
		};
		Ok(Metrics(Some(metrics)))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use polkadot_primitives::v1::{
		ValidatorId, ValidatorIndex, GroupRotationInfo, CoreState, PersistedValidationData,
		Id as ParaId, OccupiedCoreAssumption, ValidationData, SessionIndex, ValidationCode,
		CommittedCandidateReceipt, CandidateEvent, AuthorityDiscoveryId, InboundDownwardMessage,
	};
	use polkadot_node_subsystem_test_helpers as test_helpers;
	use sp_core::testing::TaskExecutor;

	use std::collections::HashMap;
	use futures::channel::oneshot;

	#[derive(Default, Clone)]
	struct MockRuntimeApi {
		validators: Vec<ValidatorId>,
		validator_groups: Vec<Vec<ValidatorIndex>>,
		availability_cores: Vec<CoreState>,
		validation_data: HashMap<ParaId, ValidationData>,
		session_index_for_child: SessionIndex,
		validation_code: HashMap<ParaId, ValidationCode>,
		validation_outputs_results: HashMap<ParaId, bool>,
		candidate_pending_availability: HashMap<ParaId, CommittedCandidateReceipt>,
		candidate_events: Vec<CandidateEvent>,
		dmq_contents: HashMap<ParaId, Vec<InboundDownwardMessage>>,
	}

	impl ProvideRuntimeApi<Block> for MockRuntimeApi {
		type Api = Self;

		fn runtime_api<'a>(&'a self) -> sp_api::ApiRef<'a, Self::Api> {
			self.clone().into()
		}
	}

	sp_api::mock_impl_runtime_apis! {
		impl ParachainHost<Block> for MockRuntimeApi {
			type Error = String;

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
				self.availability_cores.clone()
			}

			fn persisted_validation_data(
				&self,
				para: ParaId,
				_assumption: OccupiedCoreAssumption,
			) -> Option<PersistedValidationData> {
				self.validation_data.get(&para).map(|l| l.persisted.clone())
			}

			fn full_validation_data(
				&self,
				para: ParaId,
				_assumption: OccupiedCoreAssumption,
			) -> Option<ValidationData> {
				self.validation_data.get(&para).map(|l| l.clone())
			}

			fn check_validation_outputs(
				&self,
				para_id: ParaId,
				_commitments: polkadot_primitives::v1::ValidationOutputs,
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

			fn validator_discovery(ids: Vec<ValidatorId>) -> Vec<Option<AuthorityDiscoveryId>> {
				vec![None; ids.len()]
			}

			fn dmq_contents(
				&self,
				recipient: ParaId,
			) -> Vec<polkadot_primitives::v1::InboundDownwardMessage> {
				self.dmq_contents.get(&recipient).map(|q| q.clone()).unwrap_or_default()
			}
		}
	}

	#[test]
	fn requests_validators() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::Validators(tx))
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), runtime_api.validators);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_validator_groups() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::ValidatorGroups(tx))
			}).await;

			assert_eq!(rx.await.unwrap().unwrap().0, runtime_api.validator_groups);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_availability_cores() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::AvailabilityCores(tx))
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), runtime_api.availability_cores);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_persisted_validation_data() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let mut runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();

		Arc::get_mut(&mut runtime_api).unwrap().validation_data.insert(para_a, Default::default());

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::PersistedValidationData(para_a, OccupiedCoreAssumption::Included, tx)
				),
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), Some(Default::default()));

			let (tx, rx) = oneshot::channel();
			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::PersistedValidationData(para_b, OccupiedCoreAssumption::Included, tx)
				),
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), None);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_full_validation_data() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let mut runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();

		Arc::get_mut(&mut runtime_api).unwrap().validation_data.insert(para_a, Default::default());

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::FullValidationData(para_a, OccupiedCoreAssumption::Included, tx)
				),
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), Some(Default::default()));

			let (tx, rx) = oneshot::channel();
			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::FullValidationData(para_b, OccupiedCoreAssumption::Included, tx)
				),
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), None);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_check_validation_outputs() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let mut runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();
		let commitments = polkadot_primitives::v1::ValidationOutputs::default();

		runtime_api.validation_outputs_results.insert(para_a, false);
		runtime_api.validation_outputs_results.insert(para_b, true);

		let runtime_api = Arc::new(runtime_api);

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::CheckValidationOutputs(
						para_a,
						commitments.clone(),
						tx,
					),
				)
			}).await;
			assert_eq!(
				rx.await.unwrap().unwrap(),
				runtime_api.validation_outputs_results[&para_a],
			);

			let (tx, rx) = oneshot::channel();
			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::CheckValidationOutputs(
						para_b,
						commitments,
						tx,
					),
				)
			}).await;
			assert_eq!(
				rx.await.unwrap().unwrap(),
				runtime_api.validation_outputs_results[&para_b],
			);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_session_index_for_child() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::SessionIndexForChild(tx))
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), runtime_api.session_index_for_child);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_validation_code() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let mut runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();

		Arc::get_mut(&mut runtime_api).unwrap().validation_code.insert(para_a, Default::default());

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::ValidationCode(para_a, OccupiedCoreAssumption::Included, tx)
				),
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), Some(Default::default()));

			let (tx, rx) = oneshot::channel();
			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::ValidationCode(para_b, OccupiedCoreAssumption::Included, tx)
				),
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), None);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_candidate_pending_availability() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let mut runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();

		runtime_api.candidate_pending_availability.insert(para_a, Default::default());

		let runtime_api = Arc::new(runtime_api);

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::CandidatePendingAvailability(para_a, tx),
				)
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), Some(Default::default()));

			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::CandidatePendingAvailability(para_b, tx),
				)
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), None);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_candidate_events() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::CandidateEvents(tx))
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), runtime_api.candidate_events);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_dmq_contents() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());

		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();

		let runtime_api = Arc::new({
			let mut runtime_api = MockRuntimeApi::default();

			runtime_api.dmq_contents.insert(para_a, vec![]);
			runtime_api.dmq_contents.insert(
				para_b,
				vec![InboundDownwardMessage {
					sent_at: 228,
					msg: b"Novus Ordo Seclorum".to_vec(),
				}],
			);

			runtime_api
		});

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None));
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();
			ctx_handle
				.send(FromOverseer::Communication {
					msg: RuntimeApiMessage::Request(relay_parent, Request::DmqContents(para_a, tx)),
				})
				.await;
			assert_eq!(rx.await.unwrap().unwrap(), vec![]);

			let (tx, rx) = oneshot::channel();
			ctx_handle
				.send(FromOverseer::Communication {
					msg: RuntimeApiMessage::Request(relay_parent, Request::DmqContents(para_b, tx)),
				})
				.await;
			assert_eq!(
				rx.await.unwrap().unwrap(),
				vec![InboundDownwardMessage {
					sent_at: 228,
					msg: b"Novus Ordo Seclorum".to_vec(),
				}]
			);

			ctx_handle
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
				.await;
		};
		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

}
