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

use crate::{
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest, SchedulerRoster},
	FromOverseer, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemResult,
};
use futures::{
	channel::{mpsc, oneshot},
	future::Either,
	prelude::*,
	select,
	stream::Stream,
	task,
};
use futures_timer::Delay;
use keystore::KeyStorePtr;
use parity_scale_codec::Encode;
use pin_project::{pin_project, pinned_drop};
use polkadot_primitives::v1::{
	EncodeAs, Hash, HeadData, Id as ParaId, Signed, SigningContext,
	ValidatorId, ValidatorIndex, ValidatorPair,
};
use sp_core::{
	Pair,
	traits::SpawnNamed,
};
use std::{
	collections::HashMap,
	convert::{TryFrom, TryInto},
	marker::Unpin,
	pin::Pin,
	time::Duration,
};
use streamunordered::{StreamUnordered, StreamYield};

/// Duration a job will wait after sending a stop signal before hard-aborting.
pub const JOB_GRACEFUL_STOP_DURATION: Duration = Duration::from_secs(1);
/// Capacity of channels to and from individual jobs
pub const JOB_CHANNEL_CAPACITY: usize = 64;

/// Utility errors
#[derive(Debug, derive_more::From)]
pub enum Error {
	/// Attempted to send or receive on a oneshot channel which had been canceled
	#[from]
	Oneshot(oneshot::Canceled),
	/// Attempted to send on a MPSC channel which has been canceled
	#[from]
	Mpsc(mpsc::SendError),
	/// Attempted to convert from an AllMessages to a FromJob, and failed.
	SenderConversion(String),
	/// The local node is not a validator.
	NotAValidator,
	/// The desired job is not present in the jobs list.
	JobNotFound(Hash),
}

/// Request some data from the `RuntimeApi`.
pub async fn request_from_runtime<RequestBuilder, Response, FromJob>(
	parent: Hash,
	sender: &mut mpsc::Sender<FromJob>,
	request_builder: RequestBuilder,
) -> Result<oneshot::Receiver<Response>, Error>
where
	RequestBuilder: FnOnce(oneshot::Sender<Response>) -> RuntimeApiRequest,
	FromJob: TryFrom<AllMessages>,
	<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
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

/// Request a validator set from the `RuntimeApi`.
pub async fn request_validators<FromJob>(
	parent: Hash,
	s: &mut mpsc::Sender<FromJob>,
) -> Result<oneshot::Receiver<Vec<ValidatorId>>, Error>
where
	FromJob: TryFrom<AllMessages>,
	<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::Validators(tx)).await
}

/// Request the scheduler roster from `RuntimeApi`.
pub async fn request_validator_groups<FromJob>(
	parent: Hash,
	s: &mut mpsc::Sender<FromJob>,
) -> Result<oneshot::Receiver<SchedulerRoster>, Error>
where
	FromJob: TryFrom<AllMessages>,
	<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::ValidatorGroups(tx)).await
}

/// Request a `SigningContext` from the `RuntimeApi`.
pub async fn request_signing_context<FromJob>(
	parent: Hash,
	s: &mut mpsc::Sender<FromJob>,
) -> Result<oneshot::Receiver<SigningContext>, Error>
where
	FromJob: TryFrom<AllMessages>,
	<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::SigningContext(tx)).await
}

/// Request `HeadData` for some `ParaId` from `RuntimeApi`.
pub async fn request_head_data<FromJob>(
	parent: Hash,
	s: &mut mpsc::Sender<FromJob>,
	id: ParaId,
) -> Result<oneshot::Receiver<HeadData>, Error>
where
	FromJob: TryFrom<AllMessages>,
	<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
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
	pub async fn new<FromJob>(
		parent: Hash,
		keystore: KeyStorePtr,
		mut sender: mpsc::Sender<FromJob>,
	) -> Result<Self, Error>
	where
		FromJob: TryFrom<AllMessages>,
		<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
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
	///
	/// Validation can only succeed if `signed.validator_index() == self.index()`.
	/// Normally, this will always be the case for a properly operating program,
	/// but it's double-checked here anyway.
	pub fn check_payload<Payload: EncodeAs<RealPayload>, RealPayload: Encode>(
		&self,
		signed: Signed<Payload, RealPayload>,
	) -> Result<(), ()> {
		if signed.validator_index() != self.index {
			return Err(());
		}
		signed.check_signature(&self.signing_context, &self.id())
	}
}

/// ToJob is expected to be an enum declaring the set of messages of interest to a particular job.
///
/// Normally, this will be some subset of `Allmessages`, and a `Stop` variant.
pub trait ToJobTrait: TryFrom<AllMessages> {
	/// The `Stop` variant of the ToJob enum.
	const STOP: Self;

	/// If the message variant contains its relay parent, return it here
	fn relay_parent(&self) -> Option<Hash>;
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
pub trait JobTrait: Unpin {
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
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>>;

	/// Handle a message which has no relay parent, and therefore can't be dispatched to a particular job
	///
	/// By default, this is implemented with a NOP function. However, if
	/// ToJob occasionally has messages which do not correspond to a particular
	/// parent relay hash, then this function will be spawned as a one-off
	/// task to handle those messages.
	// TODO: the API here is likely not precisely what we want; figure it out more
	// once we're implementing a subsystem which actually needs this feature.
	// In particular, we're quite likely to want this to return a future instead of
	// interrupting the active thread for the duration of the handler.
	fn handle_unanchored_msg(_msg: Self::ToJob) -> Result<(), Self::Error> {
		Ok(())
	}
}

/// Jobs manager for a subsystem
///
/// - Spawns new jobs for a given relay-parent on demand.
/// - Closes old jobs for a given relay-parent on demand.
/// - Dispatches messages to the appropriate job for a given relay-parent.
/// - When dropped, aborts all remaining jobs.
/// - implements `Stream<Item=Job::FromJob>`, collecting all messages from subordinate jobs.
#[pin_project(PinnedDrop)]
pub struct Jobs<Spawner, Job: JobTrait> {
	spawner: Spawner,
	running: HashMap<Hash, JobHandle<Job::ToJob>>,
	#[pin]
	outgoing_msgs: StreamUnordered<mpsc::Receiver<Job::FromJob>>,
	job: std::marker::PhantomData<Job>,
}

impl<Spawner: SpawnNamed, Job: JobTrait> Jobs<Spawner, Job> {
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
		self.spawner.spawn(Job::NAME, future.boxed());

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
			None => Err(Error::JobNotFound(parent_hash)),
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
}

// Note that on drop, we don't have the chance to gracefully spin down each of the remaining handles;
// we just abort them all. Still better than letting them dangle.
#[pinned_drop]
impl<Spawner, Job: JobTrait> PinnedDrop for Jobs<Spawner, Job> {
	fn drop(self: Pin<&mut Self>) {
		for job_handle in self.running.values() {
			job_handle.abort_handle.abort();
		}
	}
}

impl<Spawner, Job> Stream for Jobs<Spawner, Job>
where
	Spawner: SpawnNamed,
	Job: JobTrait,
{
	type Item = Job::FromJob;

	fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context) -> task::Poll<Option<Self::Item>> {
		// pin-project the outgoing messages
		self.project()
			.outgoing_msgs
			.poll_next(cx)
			.map(|opt| opt.and_then(|(stream_yield, _)| match stream_yield {
				StreamYield::Item(msg) => Some(msg),
				StreamYield::Finished(_) => None,
		}))
	}
}

/// A basic implementation of a subsystem.
///
/// This struct is responsible for handling message traffic between
/// this subsystem and the overseer. It spawns and kills jobs on the
/// appropriate Overseer messages, and dispatches standard traffic to
/// the appropriate job the rest of the time.
pub struct JobManager<Spawner, Context, Job: JobTrait> {
	spawner: Spawner,
	run_args: Job::RunArgs,
	context: std::marker::PhantomData<Context>,
	job: std::marker::PhantomData<Job>,
}

impl<Spawner, Context, Job> JobManager<Spawner, Context, Job>
where
	Spawner: SpawnNamed + Clone + Send + Unpin,
	Context: SubsystemContext,
	Job: JobTrait,
	Job::RunArgs: Clone,
	Job::ToJob: TryFrom<AllMessages> + TryFrom<<Context as SubsystemContext>::Message> + Sync,
{
	/// Creates a new `Subsystem`.
	pub fn new(spawner: Spawner, run_args: Job::RunArgs) -> Self {
		Self {
			spawner,
			run_args,
			context: std::marker::PhantomData,
			job: std::marker::PhantomData,
		}
	}

	/// Run this subsystem
	///
	/// Conceptually, this is very simple: it just loops forever.
	///
	/// - On incoming overseer messages, it starts or stops jobs as appropriate.
	/// - On other incoming messages, if they can be converted into Job::ToJob and
	///   include a hash, then they're forwarded to the appropriate individual job.
	/// - On outgoing messages from the jobs, it forwards them to the overseer.
	pub async fn run(mut ctx: Context, run_args: Job::RunArgs, spawner: Spawner) {
		let mut jobs = Jobs::new(spawner.clone());

		loop {
			select! {
				incoming = ctx.recv().fuse() => if Self::handle_incoming(incoming, &mut jobs, &run_args).await { break },
				outgoing = jobs.next().fuse() => if Self::handle_outgoing(outgoing, &mut ctx).await { break },
				complete => break,
			}
		}
	}

	// handle an incoming message. return true if we should break afterwards.
	async fn handle_incoming(
		incoming: SubsystemResult<FromOverseer<Context::Message>>,
		jobs: &mut Jobs<Spawner, Job>,
		run_args: &Job::RunArgs,
	) -> bool {
		use crate::FromOverseer::{Communication, Signal};
		use crate::OverseerSignal::{Conclude, StartWork, StopWork};

		match incoming {
			Ok(Signal(StartWork(hash))) => {
				if let Err(e) = jobs.spawn_job(hash, run_args.clone()) {
					log::error!("Failed to spawn a job: {:?}", e);
					return true;
				}
			}
			Ok(Signal(StopWork(hash))) => {
				if let Err(e) = jobs.stop_job(hash).await {
					log::error!("Failed to stop a job: {:?}", e);
					return true;
				}
			}
			Ok(Signal(Conclude)) => {
				// Breaking the loop ends fn run, which drops `jobs`, which immediately drops all ongoing work.
				// We can afford to wait a little while to shut them all down properly before doing that.
				//
				// Forwarding the stream to a drain means we wait until all of the items in the stream
				// have completed. Contrast with `into_future`, which turns it into a future of `(head, rest_stream)`.
				use futures::stream::StreamExt;
				use futures::stream::FuturesUnordered;

				let unordered = jobs.running
					.drain()
					.map(|(_, handle)| handle.stop())
					.collect::<FuturesUnordered<_>>();
				// now wait for all the futures to complete; collect a vector of their results
				// this is strictly less efficient than draining them into oblivion, but this compiles, and that doesn't
				// https://github.com/paritytech/polkadot/pull/1376#pullrequestreview-446488645
				let _ = async move { unordered.collect::<Vec<_>>() }.await;

				return true;
			}
			Ok(Communication { msg }) => {
				if let Ok(to_job) = <Job::ToJob>::try_from(msg) {
					match to_job.relay_parent() {
						Some(hash) => {
							if let Err(err) = jobs.send_msg(hash, to_job).await {
								log::error!("Failed to send a message to a job: {:?}", err);
								return true;
							}
						}
						None => {
							if let Err(err) = Job::handle_unanchored_msg(to_job) {
								log::error!("Failed to handle unhashed message: {:?}", err);
								return true;
							}
						}
					}
				}
			}
			Err(err) => {
				log::error!("error receiving message from subsystem context: {:?}", err);
				return true;
			}
		}
		false
	}

	// handle an outgoing message. return true if we should break afterwards.
	async fn handle_outgoing(outgoing: Option<Job::FromJob>, ctx: &mut Context) -> bool {
		match outgoing {
			Some(msg) => {
				// discard errors when sending the message upstream
				let _ = ctx.send_message(msg.into()).await;
			}
			None => return true,
		}
		false
	}
}

impl<Spawner, Context, Job> Subsystem<Context> for JobManager<Spawner, Context, Job>
where
	Spawner: SpawnNamed + Send + Clone + Unpin + 'static,
	Context: SubsystemContext,
	<Context as SubsystemContext>::Message: Into<Job::ToJob>,
	Job: JobTrait + Send,
	Job::RunArgs: Clone + Sync,
	Job::ToJob: TryFrom<AllMessages> + Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let spawner = self.spawner.clone();
		let run_args = self.run_args.clone();

		let future = Box::pin(async move {
			Self::run(ctx, run_args, spawner).await;
		});

		SpawnedSubsystem {
			name: "JobManager",
			future,
		}
	}
}
