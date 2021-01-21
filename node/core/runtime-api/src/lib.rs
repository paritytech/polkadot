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
use polkadot_node_subsystem_util::metrics::{self, prometheus};
use polkadot_primitives::v1::{Block, BlockId, Hash, ParachainHost};

use sp_api::ProvideRuntimeApi;
use sp_core::traits::SpawnNamed;

use futures::{prelude::*, stream::FuturesUnordered, channel::oneshot, select};
use std::{sync::Arc, collections::VecDeque, pin::Pin};

const LOG_TARGET: &str = "runtime_api";

/// The number of maximum runtime api requests can be executed in parallel. Further requests will be buffered.
const MAX_PARALLEL_REQUESTS: usize = 4;

/// The name of the blocking task that executes a runtime api request.
const API_REQUEST_TASK_NAME: &str = "polkadot-runtime-api-request";

/// The `RuntimeApiSubsystem`. See module docs for more details.
pub struct RuntimeApiSubsystem<Client> {
	client: Arc<Client>,
	metrics: Metrics,
	spawn_handle: Box<dyn SpawnNamed>,
	/// If there are [`MAX_PARALLEL_REQUESTS`] requests being executed, we buffer them in here until they can be executed.
	waiting_requests: VecDeque<(Pin<Box<dyn Future<Output = ()> + Send>>, oneshot::Receiver<()>)>,
	/// All the active runtime api requests that are currently being executed.
	active_requests: FuturesUnordered<oneshot::Receiver<()>>,
}

impl<Client> RuntimeApiSubsystem<Client> {
	/// Create a new Runtime API subsystem wrapping the given client and metrics.
	pub fn new(client: Arc<Client>, metrics: Metrics, spawn_handle: impl SpawnNamed + 'static) -> Self {
		RuntimeApiSubsystem {
			client,
			metrics,
			spawn_handle: Box::new(spawn_handle),
			waiting_requests: Default::default(),
			active_requests: Default::default(),
		}
	}
}

impl<Client, Context> Subsystem<Context> for RuntimeApiSubsystem<Client> where
	Client: ProvideRuntimeApi<Block> + Send + 'static + Sync,
	Client::Api: ParachainHost<Block>,
	Context: SubsystemContext<Message = RuntimeApiMessage>
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem {
			future: run(ctx, self).boxed(),
			name: "runtime-api-subsystem",
		}
	}
}

impl<Client> RuntimeApiSubsystem<Client> where
	Client: ProvideRuntimeApi<Block> + Send + 'static + Sync,
	Client::Api: ParachainHost<Block>,
{
	/// Spawn a runtime api request.
	///
	/// If there are already [`MAX_PARALLEL_REQUESTS`] requests being executed, the request will be buffered.
	fn spawn_request(&mut self, relay_parent: Hash, request: Request) {
		let client = self.client.clone();
		let metrics = self.metrics.clone();
		let (sender, receiver) = oneshot::channel();

		let request = async move {
			make_runtime_api_request(
				client,
				metrics,
				relay_parent,
				request,
			);
			let _ = sender.send(());
		}.boxed();

		if self.active_requests.len() >= MAX_PARALLEL_REQUESTS {
			self.waiting_requests.push_back((request, receiver));

			if self.waiting_requests.len() > MAX_PARALLEL_REQUESTS * 10 {
				tracing::warn!(
					target: LOG_TARGET,
					"{} runtime api requests waiting to be executed.",
					self.waiting_requests.len(),
				)
			}
		} else {
			self.spawn_handle.spawn_blocking(API_REQUEST_TASK_NAME, request);
			self.active_requests.push(receiver);
		}
	}

	/// Poll the active runtime api requests.
	async fn poll_requests(&mut self) {
		// If there are no active requests, this future should be pending forever.
		if self.active_requests.len() == 0 {
			return futures::pending!()
		}

		// If there are active requests, this will always resolve to `Some(_)` when a request is finished.
		let _ = self.active_requests.next().await;

		if let Some((req, recv)) = self.waiting_requests.pop_front() {
			self.spawn_handle.spawn_blocking(API_REQUEST_TASK_NAME, req);
			self.active_requests.push(recv);
		}
	}
}

#[tracing::instrument(skip(ctx, subsystem), fields(subsystem = LOG_TARGET))]
async fn run<Client>(
	mut ctx: impl SubsystemContext<Message = RuntimeApiMessage>,
	mut subsystem: RuntimeApiSubsystem<Client>,
) -> SubsystemResult<()> where
	Client: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	Client::Api: ParachainHost<Block>,
{
	loop {
		select! {
			req = ctx.recv().fuse() => match req? {
				FromOverseer::Signal(OverseerSignal::Conclude) => return Ok(()),
				FromOverseer::Signal(OverseerSignal::ActiveLeaves(_)) => {},
				FromOverseer::Signal(OverseerSignal::BlockFinalized(..)) => {},
				FromOverseer::Communication { msg } => match msg {
					RuntimeApiMessage::Request(relay_parent, request) => {
						subsystem.spawn_request(relay_parent, request);
					},
				}
			},
			_ = subsystem.poll_requests().fuse() => {},
		}
	}
}

#[tracing::instrument(level = "trace", skip(client, metrics), fields(subsystem = LOG_TARGET))]
fn make_runtime_api_request<Client>(
	client: Arc<Client>,
	metrics: Metrics,
	relay_parent: Hash,
	request: Request,
) where
	Client: ProvideRuntimeApi<Block>,
	Client::Api: ParachainHost<Block>,
{
	let _timer = metrics.time_make_runtime_api_request();

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
		Request::HistoricalValidationCode(para, at, sender) =>
			query!(historical_validation_code(para, at), sender),
		Request::CandidatePendingAvailability(para, sender) =>
			query!(candidate_pending_availability(para), sender),
		Request::CandidateEvents(sender) => query!(candidate_events(), sender),
		Request::SessionInfo(index, sender) => query!(session_info(index), sender),
		Request::DmqContents(id, sender) => query!(dmq_contents(id), sender),
		Request::InboundHrmpChannelsContents(id, sender) => query!(inbound_hrmp_channels_contents(id), sender),
	}
}

#[derive(Clone)]
struct MetricsInner {
	chain_api_requests: prometheus::CounterVec<prometheus::U64>,
	make_runtime_api_request: prometheus::Histogram,
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

	/// Provide a timer for `make_runtime_api_request` which observes on drop.
	fn time_make_runtime_api_request(&self) -> Option<metrics::prometheus::prometheus::HistogramTimer> {
		self.0.as_ref().map(|metrics| metrics.make_runtime_api_request.start_timer())
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
			make_runtime_api_request: prometheus::register(
				prometheus::Histogram::with_opts(
					prometheus::HistogramOpts::new(
						"parachain_runtime_api_make_runtime_api_request",
						"Time spent within `runtime_api::make_runtime_api_request`",
					)
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
		CommittedCandidateReceipt, CandidateEvent, InboundDownwardMessage,
		BlockNumber, InboundHrmpMessage, SessionInfo,
	};
	use polkadot_node_subsystem_test_helpers as test_helpers;
	use sp_core::testing::TaskExecutor;
	use std::{collections::{HashMap, BTreeMap}, sync::{Arc, Mutex}};
	use futures::channel::oneshot;

	#[derive(Default, Clone)]
	struct MockRuntimeApi {
		validators: Vec<ValidatorId>,
		validator_groups: Vec<Vec<ValidatorIndex>>,
		availability_cores: Vec<CoreState>,
		availability_cores_wait: Arc<Mutex<()>>,
		validation_data: HashMap<ParaId, ValidationData>,
		session_index_for_child: SessionIndex,
		session_info: HashMap<SessionIndex, SessionInfo>,
		validation_code: HashMap<ParaId, ValidationCode>,
		historical_validation_code: HashMap<ParaId, Vec<(BlockNumber, ValidationCode)>>,
		validation_outputs_results: HashMap<ParaId, bool>,
		candidate_pending_availability: HashMap<ParaId, CommittedCandidateReceipt>,
		candidate_events: Vec<CandidateEvent>,
		dmq_contents: HashMap<ParaId, Vec<InboundDownwardMessage>>,
		hrmp_channels: HashMap<ParaId, BTreeMap<ParaId, Vec<InboundHrmpMessage>>>,
	}

	impl ProvideRuntimeApi<Block> for MockRuntimeApi {
		type Api = Self;

		fn runtime_api<'a>(&'a self) -> sp_api::ApiRef<'a, Self::Api> {
			self.clone().into()
		}
	}

	sp_api::mock_impl_runtime_apis! {
		impl ParachainHost<Block> for MockRuntimeApi {
			type Error = sp_api::ApiError;

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
				let _ = self.availability_cores_wait.lock().unwrap();
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
				_commitments: polkadot_primitives::v1::CandidateCommitments,
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

			fn historical_validation_code(
				&self,
				para: ParaId,
				at: BlockNumber,
			) -> Option<ValidationCode> {
				self.historical_validation_code.get(&para).and_then(|h_code| {
					h_code.iter()
						.take_while(|(changed_at, _)| changed_at <= &at)
						.last()
						.map(|(_, code)| code.clone())
				})
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
		}
	}

	#[test]
	fn requests_validators() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();
		let spawner = sp_core::testing::TaskExecutor::new();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let spawner = sp_core::testing::TaskExecutor::new();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let spawner = sp_core::testing::TaskExecutor::new();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();
		let spawner = sp_core::testing::TaskExecutor::new();

		let mut runtime_api = MockRuntimeApi::default();
		runtime_api.validation_data.insert(para_a, Default::default());
		let runtime_api = Arc::new(runtime_api);

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();
		let spawner = sp_core::testing::TaskExecutor::new();

		let mut runtime_api = MockRuntimeApi::default();
		runtime_api.validation_data.insert(para_a, Default::default());
		let runtime_api = Arc::new(runtime_api);

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let commitments = polkadot_primitives::v1::CandidateCommitments::default();
		let spawner = sp_core::testing::TaskExecutor::new();

		runtime_api.validation_outputs_results.insert(para_a, false);
		runtime_api.validation_outputs_results.insert(para_b, true);

		let runtime_api = Arc::new(runtime_api);

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let spawner = sp_core::testing::TaskExecutor::new();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
	fn requests_session_info() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let mut runtime_api = MockRuntimeApi::default();
		let session_index = 1;
		runtime_api.session_info.insert(session_index, Default::default());
		let runtime_api = Arc::new(runtime_api);
		let spawner = sp_core::testing::TaskExecutor::new();

		let relay_parent = [1; 32].into();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::SessionInfo(session_index, tx))
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), Some(Default::default()));

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_validation_code() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());

		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();
		let spawner = sp_core::testing::TaskExecutor::new();

		let mut runtime_api = MockRuntimeApi::default();
		runtime_api.validation_code.insert(para_a, Default::default());
		let runtime_api = Arc::new(runtime_api);

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();
		let spawner = sp_core::testing::TaskExecutor::new();

		let mut runtime_api = MockRuntimeApi::default();
		runtime_api.candidate_pending_availability.insert(para_a, Default::default());
		let runtime_api = Arc::new(runtime_api);

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let spawner = sp_core::testing::TaskExecutor::new();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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
		let spawner = sp_core::testing::TaskExecutor::new();

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

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
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

	#[test]
	fn requests_inbound_hrmp_channels_contents() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());

		let relay_parent = [1; 32].into();
		let para_a = 99.into();
		let para_b = 66.into();
		let para_c = 33.into();
		let spawner = sp_core::testing::TaskExecutor::new();

		let para_b_inbound_channels = [
			(para_a, vec![]),
			(
				para_c,
				vec![InboundHrmpMessage {
					sent_at: 1,
					data: "𝙀=𝙈𝘾²".as_bytes().to_owned(),
				}],
			),
		]
		.iter()
		.cloned()
		.collect::<BTreeMap<_, _>>();

		let runtime_api = Arc::new({
			let mut runtime_api = MockRuntimeApi::default();

			runtime_api.hrmp_channels.insert(para_a, BTreeMap::new());
			runtime_api
				.hrmp_channels
				.insert(para_b, para_b_inbound_channels.clone());

			runtime_api
		});

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();
			ctx_handle
				.send(FromOverseer::Communication {
					msg: RuntimeApiMessage::Request(
						relay_parent,
						Request::InboundHrmpChannelsContents(para_a, tx),
					),
				})
				.await;
			assert_eq!(rx.await.unwrap().unwrap(), BTreeMap::new());

			let (tx, rx) = oneshot::channel();
			ctx_handle
				.send(FromOverseer::Communication {
					msg: RuntimeApiMessage::Request(
						relay_parent,
						Request::InboundHrmpChannelsContents(para_b, tx),
					),
				})
				.await;
			assert_eq!(rx.await.unwrap().unwrap(), para_b_inbound_channels,);

			ctx_handle
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
				.await;
		};
		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_historical_code() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());

		let para_a = 5.into();
		let para_b = 6.into();
		let spawner = sp_core::testing::TaskExecutor::new();

		let runtime_api = Arc::new({
			let mut runtime_api = MockRuntimeApi::default();

			runtime_api.historical_validation_code.insert(
				para_a,
				vec![(1, vec![1, 2, 3].into()), (10, vec![4, 5, 6].into())],
			);

			runtime_api.historical_validation_code.insert(
				para_b,
				vec![(5, vec![7, 8, 9].into())],
			);

			runtime_api
		});
		let relay_parent = [1; 32].into();

		let subsystem = RuntimeApiSubsystem::new(runtime_api, Metrics(None), spawner);
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			{
				let (tx, rx) = oneshot::channel();
				ctx_handle.send(FromOverseer::Communication {
					msg: RuntimeApiMessage::Request(
						relay_parent,
						Request::HistoricalValidationCode(para_a, 5, tx),
					)
				}).await;

				assert_eq!(rx.await.unwrap().unwrap(), Some(ValidationCode::from(vec![1, 2, 3])));
			}

			{
				let (tx, rx) = oneshot::channel();
				ctx_handle.send(FromOverseer::Communication {
					msg: RuntimeApiMessage::Request(
						relay_parent,
						Request::HistoricalValidationCode(para_a, 10, tx),
					)
				}).await;

				assert_eq!(rx.await.unwrap().unwrap(), Some(ValidationCode::from(vec![4, 5, 6])));
			}

			{
				let (tx, rx) = oneshot::channel();
				ctx_handle.send(FromOverseer::Communication {
					msg: RuntimeApiMessage::Request(
						relay_parent,
						Request::HistoricalValidationCode(para_b, 1, tx),
					)
				}).await;

				assert!(rx.await.unwrap().unwrap().is_none());
			}

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn multiple_requests_in_parallel_are_working() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = Arc::new(MockRuntimeApi::default());
		let relay_parent = [1; 32].into();
		let spawner = sp_core::testing::TaskExecutor::new();
		let mutex = runtime_api.availability_cores_wait.clone();

		let subsystem = RuntimeApiSubsystem::new(runtime_api.clone(), Metrics(None), spawner);
		let subsystem_task = run(ctx, subsystem).map(|x| x.unwrap());
		let test_task = async move {
			// Make all requests block until we release this mutex.
			let lock = mutex.lock().unwrap();

			let mut receivers = Vec::new();

			for _ in 0..MAX_PARALLEL_REQUESTS * 10 {
				let (tx, rx) = oneshot::channel();

				ctx_handle.send(FromOverseer::Communication {
					msg: RuntimeApiMessage::Request(relay_parent, Request::AvailabilityCores(tx))
				}).await;

				receivers.push(rx);
			}

			let join = future::join_all(receivers);

			drop(lock);

			join.await
				.into_iter()
				.for_each(|r| assert_eq!(r.unwrap().unwrap(), runtime_api.availability_cores));

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}
}
