use futures::{
	channel::{mpsc, oneshot},
	future::AbortHandle,
	prelude::*,
	task::SpawnError,
};
use polkadot_node_subsystem::messages::{
	AllMessages, RuntimeApiMessage, RuntimeApiRequest, SchedulerRoster,
};
use polkadot_primitives::{
	parachain::{
		GlobalValidationSchedule, HeadData, Id as ParaId, LocalValidationData, SigningContext,
		ValidatorId,
	},
	Hash,
};
use std::{
	collections::HashMap,
	ops::{Deref, DerefMut},
};

/// JobCanceler aborts all contained abort handles on drop
#[derive(Debug, Default)]
pub struct JobCanceler(HashMap<Hash, AbortHandle>);

impl Drop for JobCanceler {
	fn drop(&mut self) {
		for abort_handle in self.0.values() {
			abort_handle.abort();
		}
	}
}

// JobCanceler is a smart pointer wrapping the contained hashmap;
// it only cares about the wrapped data insofar as it's necessary
// to implement proper Drop behavior. Therefore, it's appropriate
// to impl Deref and DerefMut.
impl Deref for JobCanceler {
	type Target = HashMap<Hash, AbortHandle>;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl DerefMut for JobCanceler {
	fn deref_mut(&mut self) -> &mut Self::Target {
		&mut self.0
	}
}

#[derive(Debug, derive_more::From)]
pub enum Error {
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	Mpsc(mpsc::SendError),
	#[from]
	Spawn(SpawnError),
}

/// Request some data from the `RuntimeApi`.
pub async fn request_from_runtime<RequestBuilder, Response>(
	parent: Hash,
	sender: &mut mpsc::Sender<AllMessages>,
	request_builder: RequestBuilder,
) -> Result<oneshot::Receiver<Response>, Error>
where
	RequestBuilder: FnOnce(oneshot::Sender<Response>) -> RuntimeApiRequest,
{
	let (tx, rx) = oneshot::channel();

	sender
		.send(AllMessages::RuntimeApi(RuntimeApiMessage::Request(
			parent,
			request_builder(tx),
		)))
		.await?;

	Ok(rx)
}

/// Request a `GlobalValidationSchedule` from `RuntimeApi`.
pub async fn request_global_validation_schedule(
	parent: Hash,
	s: &mut mpsc::Sender<AllMessages>,
) -> Result<oneshot::Receiver<GlobalValidationSchedule>, Error> {
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::GlobalValidationSchedule(tx)).await
}

/// Request a `LocalValidationData` from `RuntimeApi`.
pub async fn request_local_validation_data(
	parent: Hash,
	para_id: ParaId,
	s: &mut mpsc::Sender<AllMessages>,
) -> Result<oneshot::Receiver<Option<LocalValidationData>>, Error> {
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::LocalValidationData(para_id, tx)).await
}

/// Request a validator set from the `RuntimeApi`.
pub async fn request_validators(
	parent: Hash,
	s: &mut mpsc::Sender<AllMessages>,
) -> Result<oneshot::Receiver<Vec<ValidatorId>>, Error> {
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::Validators(tx)).await
}

/// Request the scheduler roster from `RuntimeApi`.
pub async fn request_validator_groups(
	parent: Hash,
	s: &mut mpsc::Sender<AllMessages>,
) -> Result<oneshot::Receiver<SchedulerRoster>, Error> {
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::ValidatorGroups(tx)).await
}

/// Request a `SigningContext` from the `RuntimeApi`.
pub async fn request_signing_context(
	parent: Hash,
	s: &mut mpsc::Sender<AllMessages>,
) -> Result<oneshot::Receiver<SigningContext>, Error> {
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::SigningContext(tx)).await
}

/// Request `HeadData` for some `ParaId` from `RuntimeApi`.
pub async fn request_head_data(
	parent: Hash,
	s: &mut mpsc::Sender<AllMessages>,
	id: ParaId,
) -> Result<oneshot::Receiver<HeadData>, Error> {
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::HeadData(id, tx)).await
}
