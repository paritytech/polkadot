// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Utility module for subsystems
//!
//! Many subsystems have common interests such as canceling a bunch of spawned jobs,
//! or determining what their validator ID is. These common interests are factored into
//! this module.

use crate::messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest, SchedulerRoster};
use futures::{
	channel::{mpsc, oneshot},
	future::{AbortHandle, Either},
	prelude::*,
	task::{Spawn, SpawnError, SpawnExt},
};
use futures_timer::Delay;
use keystore::KeyStorePtr;
use parity_scale_codec::Encode;
use polkadot_primitives::{
	parachain::{
		EncodeAs, GlobalValidationSchedule, HeadData, Id as ParaId, LocalValidationData, Signed,
		SigningContext, ValidatorId, ValidatorIndex, ValidatorPair,
	},
	Hash,
};
use sp_core::Pair;
use std::{
	collections::HashMap,
	convert::{TryFrom, TryInto},
	ops::{Deref, DerefMut},
	pin::Pin,
	time::Duration,
};
use streamunordered::{StreamUnordered, StreamYield};

/// Duration a job will wait after sending a stop signal before hard-aborting.
pub const JOB_GRACEFUL_STOP_DURATION: Duration = Duration::from_secs(1);
/// Capacity of channels to and from individual jobs
pub const JOB_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, derive_more::From)]
pub enum Error {
	#[from]
	Oneshot(oneshot::Canceled),
	#[from]
	Mpsc(mpsc::SendError),
	#[from]
	Spawn(SpawnError),
	SenderConversion(String),
	/// The local node is not a validator.
	NotAValidator,
	/// The desired job is not present in the jobs list.
	JobNotFound(Hash),
}

/// Request some data from the `RuntimeApi`.
pub async fn request_from_runtime<RequestBuilder, Response, SenderMessage>(
	parent: Hash,
	sender: &mut mpsc::Sender<SenderMessage>,
	request_builder: RequestBuilder,
) -> Result<oneshot::Receiver<Response>, Error>
where
	RequestBuilder: FnOnce(oneshot::Sender<Response>) -> RuntimeApiRequest,
	SenderMessage: TryFrom<AllMessages>,
	<SenderMessage as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	let (tx, rx) = oneshot::channel();

	sender
		.send(
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(parent, request_builder(tx)))
				.try_into()
				.map_err(|err| Error::SenderConversion(format!("{:?}", err)))?,
		)
		.await?;

	Ok(rx)
}

/// Request a `GlobalValidationSchedule` from `RuntimeApi`.
pub async fn request_global_validation_schedule<SenderMessage>(
	parent: Hash,
	s: &mut mpsc::Sender<SenderMessage>,
) -> Result<oneshot::Receiver<GlobalValidationSchedule>, Error>
where
	SenderMessage: TryFrom<AllMessages>,
	<SenderMessage as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| {
		RuntimeApiRequest::GlobalValidationSchedule(tx)
	})
	.await
}

/// Request a `LocalValidationData` from `RuntimeApi`.
pub async fn request_local_validation_data<SenderMessage>(
	parent: Hash,
	para_id: ParaId,
	s: &mut mpsc::Sender<SenderMessage>,
) -> Result<oneshot::Receiver<Option<LocalValidationData>>, Error>
where
	SenderMessage: TryFrom<AllMessages>,
	<SenderMessage as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| {
		RuntimeApiRequest::LocalValidationData(para_id, tx)
	})
	.await
}

/// Request a validator set from the `RuntimeApi`.
pub async fn request_validators<SenderMessage>(
	parent: Hash,
	s: &mut mpsc::Sender<SenderMessage>,
) -> Result<oneshot::Receiver<Vec<ValidatorId>>, Error>
where
	SenderMessage: TryFrom<AllMessages>,
	<SenderMessage as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::Validators(tx)).await
}

/// Request the scheduler roster from `RuntimeApi`.
pub async fn request_validator_groups<SenderMessage>(
	parent: Hash,
	s: &mut mpsc::Sender<SenderMessage>,
) -> Result<oneshot::Receiver<SchedulerRoster>, Error>
where
	SenderMessage: TryFrom<AllMessages>,
	<SenderMessage as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::ValidatorGroups(tx)).await
}

/// Request a `SigningContext` from the `RuntimeApi`.
pub async fn request_signing_context<SenderMessage>(
	parent: Hash,
	s: &mut mpsc::Sender<SenderMessage>,
) -> Result<oneshot::Receiver<SigningContext>, Error>
where
	SenderMessage: TryFrom<AllMessages>,
	<SenderMessage as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::SigningContext(tx)).await
}

/// Request `HeadData` for some `ParaId` from `RuntimeApi`.
pub async fn request_head_data<SenderMessage>(
	parent: Hash,
	s: &mut mpsc::Sender<SenderMessage>,
	id: ParaId,
) -> Result<oneshot::Receiver<HeadData>, Error>
where
	SenderMessage: TryFrom<AllMessages>,
	<SenderMessage as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::HeadData(id, tx)).await
}

/// From the given set of validators, find the first key we can sign with, if any.
pub fn signing_key(validators: &[ValidatorId], keystore: &KeyStorePtr) -> Option<ValidatorPair> {
	let keystore = keystore.read();
	validators
		.iter()
		.find_map(|v| keystore.key_pair::<ValidatorPair>(&v).ok())
}

/// Local validator information
///
/// It can be created if the local node is a validator in the context of a particular
/// relay chain block.
pub struct Validator {
	signing_context: SigningContext,
	key: ValidatorPair,
	index: ValidatorIndex,
}

impl Validator {
	/// Get a struct representing this node's validator if this node is in fact a validator in the context of the given block.
	pub async fn new<SenderMessage>(
		parent: Hash,
		keystore: KeyStorePtr,
		mut sender: mpsc::Sender<SenderMessage>,
	) -> Result<Self, Error>
	where
		SenderMessage: TryFrom<AllMessages>,
		<SenderMessage as TryFrom<AllMessages>>::Error: std::fmt::Debug,
	{
		// Note: request_validators and request_signing_context do not and cannot run concurrently: they both
		// have a mutable handle to the same sender.
		// However, each of them returns a oneshot::Receiver, and those are resolved concurrently.
		let (validators, signing_context) = futures::try_join!(
			request_validators(parent, &mut sender).await?,
			request_signing_context(parent, &mut sender).await?,
		)?;

		Self::construct(&validators, signing_context, keystore)
	}

	/// Construct a validator instance without performing runtime fetches.
	///
	/// This can be useful if external code also needs the same data.
	pub fn construct(
		validators: &[ValidatorId],
		signing_context: SigningContext,
		keystore: KeyStorePtr,
	) -> Result<Self, Error> {
		let key = signing_key(validators, &keystore).ok_or(Error::NotAValidator)?;
		let index = validators
			.iter()
			.enumerate()
			.find(|(_, k)| k == &&key.public())
			.map(|(idx, _)| idx as ValidatorIndex)
			.expect("signing_key would have already returned NotAValidator if the item we're searching for isn't in this list; qed");

		Ok(Validator {
			signing_context,
			key,
			index,
		})
	}

	/// Get this validator's id.
	pub fn id(&self) -> ValidatorId {
		self.key.public()
	}

	/// Get this validator's local index.
	pub fn index(&self) -> ValidatorIndex {
		self.index
	}

	/// Get the current signing context.
	pub fn signing_context(&self) -> &SigningContext {
		&self.signing_context
	}

	/// Sign a payload with this validator
	pub fn sign<Payload: EncodeAs<RealPayload>, RealPayload: Encode>(
		&self,
		payload: Payload,
	) -> Signed<Payload, RealPayload> {
		Signed::sign(payload, &self.signing_context, self.index, &self.key)
	}

	/// Validate the payload with this validator
	pub fn check_payload<Payload: EncodeAs<RealPayload>, RealPayload: Encode>(
		&self,
		signed: Signed<Payload, RealPayload>,
	) -> Result<(), ()> {
		signed.check_signature(&self.signing_context, &self.id())
	}
}

/// ToJob is expected to be an enum declaring messages which can be sent to a particular job.
///
/// Normally, this will be some subset of `Allmessages`, and a `Stop` variant.
pub trait ToJobTrait: TryFrom<AllMessages> {
	/// The `Stop` variant of the ToJob enum.
	const STOP: Self;
}

/// A JobHandle manages a particular job for a subsystem.
pub struct JobHandle<ToJob> {
	abort_handle: future::AbortHandle,
	to_job: mpsc::Sender<ToJob>,
	finished: oneshot::Receiver<()>,
	outgoing_msgs_handle: usize,
}

impl<ToJob> JobHandle<ToJob> {
	/// Send a message to the job.
	pub async fn send_msg(&mut self, msg: ToJob) -> Result<(), Error> {
		self.to_job.send(msg).await.map_err(Into::into)
	}

	/// Abort the job without waiting for a graceful shutdown
	pub fn abort(self) {
		self.abort_handle.abort();
	}
}

impl<ToJob: ToJobTrait> JobHandle<ToJob> {
	/// Stop this job gracefully.
	///
	/// If it hasn't shut itself down after `JOB_GRACEFUL_STOP_DURATION`, abort it.
	pub async fn stop(mut self) {
		// we don't actually care if the message couldn't be sent
		let _ = self.to_job.send(ToJob::STOP).await;
		let stop_timer = Delay::new(JOB_GRACEFUL_STOP_DURATION);

		match future::select(stop_timer, self.finished).await {
			Either::Left((_, _)) => {}
			Either::Right((_, _)) => {
				self.abort_handle.abort();
			}
		}
	}
}

/// This trait governs jobs.
///
/// Jobs are instantiated and killed automatically on appropriate overseer messages.
/// Other messages are passed along to and from the job via the overseer to other
/// subsystems.
pub trait JobTrait {
	/// Message type to the job. Typically a subset of AllMessages.
	type ToJob: 'static + ToJobTrait + Send;
	/// Message type from the job. Typically a subset of AllMessages.
	type FromJob: 'static + Into<AllMessages> + Send;
	/// Job runtime error.
	type Error: std::fmt::Debug;
	/// Extra arguments this job needs to run properly.
	///
	/// If no extra information is needed, it is perfectly acceptable to set it to `()`.
	type RunArgs: 'static + Send;

	/// Name of the job, i.e. `CandidateBackingJob`
	const NAME: &'static str;

	/// Run a job for the parent block indicated
	fn run(
		parent: Hash,
		run_args: Self::RunArgs,
		rx_to: mpsc::Receiver<Self::ToJob>,
		tx_from: mpsc::Sender<Self::FromJob>,
	) -> Pin<Box<dyn Future<Output=Result<(), Self::Error>> + Send>>;
}

pub struct Jobs<Spawner, Job: JobTrait> {
	spawner: Spawner,
	running: HashMap<Hash, JobHandle<Job::ToJob>>,
	outgoing_msgs: StreamUnordered<mpsc::Receiver<Job::FromJob>>,
	job: std::marker::PhantomData<Job>,
}

impl<Spawner: Spawn, Job: JobTrait> Jobs<Spawner, Job> {
	/// Create a new Jobs manager which handles spawning appropriate jobs.
	pub fn new(spawner: Spawner) -> Self {
		Self {
			spawner,
			running: HashMap::new(),
			outgoing_msgs: StreamUnordered::new(),
			job: std::marker::PhantomData,
		}
	}

	/// Spawn a new job for this `parent_hash`, with whatever args are appropriate.
	fn spawn_job(&mut self, parent_hash: Hash, run_args: Job::RunArgs) -> Result<(), Error> {
		let (to_job_tx, to_job_rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);
		let (from_job_tx, from_job_rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);
		let (finished_tx, finished) = oneshot::channel();

		let (future, abort_handle) = future::abortable(async move {
			if let Err(e) = Job::run(parent_hash, run_args, to_job_rx, from_job_tx).await {
				log::error!(
					"{}({}) finished with an error {:?}",
					Job::NAME,
					parent_hash,
					e,
				);
			}
		});

		// discard output
		let future = async move {
			let _ = future.await;
			let _ = finished_tx.send(());
		};
		self.spawner.spawn(future)?;

		// this handle lets us remove the appropriate receiver from self.outgoing_msgs
		// when it's time to stop the job.
		let outgoing_msgs_handle = self.outgoing_msgs.push(from_job_rx);

		let handle = JobHandle {
			abort_handle,
			to_job: to_job_tx,
			finished,
			outgoing_msgs_handle,
		};

		self.running.insert(parent_hash, handle);

		Ok(())
	}

	/// Stop the job associated with this `parent_hash`.
	pub async fn stop_job(&mut self, parent_hash: Hash) -> Result<(), Error> {
		match self.running.remove(&parent_hash) {
			Some(handle) => {
				Pin::new(&mut self.outgoing_msgs).remove(handle.outgoing_msgs_handle);
				handle.stop().await;
				Ok(())
			}
			None => Err(Error::JobNotFound(parent_hash))
		}
	}

	/// Send a message to the appropriate job for this `parent_hash`.
	async fn send_msg(&mut self, parent_hash: Hash, msg: Job::ToJob) -> Result<(), Error> {
		match self.running.get_mut(&parent_hash) {
			Some(job) => job.send_msg(msg).await?,
			None => return Err(Error::JobNotFound(parent_hash)),
		}
		Ok(())
	}

	/// Get the next message from any of the underlying jobs.
	async fn next(&mut self) -> Option<Job::FromJob> {
		self.outgoing_msgs.next().await.and_then(|(e, _)| match e {
			StreamYield::Item(e) => Some(e),
			_ => None,
		})
	}
}

// Note that on drop, we don't have the chance to gracefully spin down each of the remaining handles;
// we just abort them all. Still better than letting them dangle.
impl<Spawner, Job: JobTrait> Drop for Jobs<Spawner, Job> {
	fn drop(&mut self) {
		for job_handle in self.running.values() {
			job_handle.abort_handle.abort();
		}
	}
}
