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

use polkadot_subsystem::{
	Subsystem, SpawnedSubsystem, SubsystemResult, SubsystemContext,
	FromOverseer, OverseerSignal,
};
use polkadot_subsystem::messages::{
	RuntimeApiMessage, RuntimeApiRequest as Request,
};
use polkadot_subsystem::errors::RuntimeApiError;
use polkadot_primitives::v1::{Block, BlockId, Hash, ParachainHost};

use sp_api::{ProvideRuntimeApi};

use futures::prelude::*;

/// The `RuntimeApiSubsystem`. See module docs for more details.
pub struct RuntimeApiSubsystem<Client>(Client);

impl<Client> RuntimeApiSubsystem<Client> {
	/// Create a new Runtime API subsystem wrapping the given client.
	pub fn new(client: Client) -> Self {
		RuntimeApiSubsystem(client)
	}
}

impl<Client, Context> Subsystem<Context> for RuntimeApiSubsystem<Client> where
	Client: ProvideRuntimeApi<Block> + Send + 'static,
	Client::Api: ParachainHost<Block>,
	Context: SubsystemContext<Message = RuntimeApiMessage>
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		SpawnedSubsystem {
			future: run(ctx, self.0).map(|_| ()).boxed(),
			name: "runtime-api-subsystem",
		}
	}
}

async fn run<Client>(
	mut ctx: impl SubsystemContext<Message = RuntimeApiMessage>,
	client: Client,
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
					&client,
					relay_parent,
					request,
				),
			}
		}
	}
}

fn make_runtime_api_request<Client>(
	client: &Client,
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

			let _ = sender.send(res);
		}}
	}

	match request {
		Request::Validators(sender) => query!(validators(), sender),
		Request::ValidatorGroups(sender) => query!(validator_groups(), sender),
		Request::AvailabilityCores(sender) => query!(availability_cores(), sender),
		Request::GlobalValidationData(sender) => query!(global_validation_data(), sender),
		Request::LocalValidationData(para, assumption, sender) =>
			query!(local_validation_data(para, assumption), sender),
		Request::SessionIndexForChild(sender) => query!(session_index_for_child(), sender),
		Request::ValidationCode(para, assumption, sender) =>
			query!(validation_code(para, assumption), sender),
		Request::CandidatePendingAvailability(para, sender) =>
			query!(candidate_pending_availability(para), sender),
		Request::CandidateEvents(sender) => query!(candidate_events(), sender),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use polkadot_primitives::v1::{
		ValidatorId, ValidatorIndex, GroupRotationInfo, CoreState, GlobalValidationData,
		Id as ParaId, OccupiedCoreAssumption, LocalValidationData, SessionIndex, ValidationCode,
		CommittedCandidateReceipt, CandidateEvent,
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
		global_validation_data: GlobalValidationData,
		local_validation_data: HashMap<ParaId, LocalValidationData>,
		session_index_for_child: SessionIndex,
		validation_code: HashMap<ParaId, ValidationCode>,
		candidate_pending_availability: HashMap<ParaId, CommittedCandidateReceipt>,
		candidate_events: Vec<CandidateEvent>,
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

			fn global_validation_data(&self) -> GlobalValidationData {
				self.global_validation_data.clone()
			}

			fn local_validation_data(
				&self,
				para: ParaId,
				_assumption: OccupiedCoreAssumption,
			) -> Option<LocalValidationData> {
				self.local_validation_data.get(&para).map(|l| l.clone())
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
		}
	}

	#[test]
	fn requests_validators() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
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
		let runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
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
		let runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
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
	fn requests_global_validation_data() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(relay_parent, Request::GlobalValidationData(tx))
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), runtime_api.global_validation_data);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_local_validation_data() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let mut runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();

		runtime_api.local_validation_data.insert(para_a, Default::default());

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
		let test_task = async move {
			let (tx, rx) = oneshot::channel();

			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::LocalValidationData(para_a, OccupiedCoreAssumption::Included, tx)
				),
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), Some(Default::default()));

			let (tx, rx) = oneshot::channel();
			ctx_handle.send(FromOverseer::Communication {
				msg: RuntimeApiMessage::Request(
					relay_parent,
					Request::LocalValidationData(para_b, OccupiedCoreAssumption::Included, tx)
				),
			}).await;

			assert_eq!(rx.await.unwrap().unwrap(), None);

			ctx_handle.send(FromOverseer::Signal(OverseerSignal::Conclude)).await;
		};

		futures::executor::block_on(future::join(subsystem_task, test_task));
	}

	#[test]
	fn requests_session_index_for_child() {
		let (ctx, mut ctx_handle) = test_helpers::make_subsystem_context(TaskExecutor::new());
		let runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
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
		let mut runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();
		let para_a = 5.into();
		let para_b = 6.into();

		runtime_api.validation_code.insert(para_a, Default::default());

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
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

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
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
		let runtime_api = MockRuntimeApi::default();
		let relay_parent = [1; 32].into();

		let subsystem_task = run(ctx, runtime_api.clone()).map(|x| x.unwrap());
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
}
