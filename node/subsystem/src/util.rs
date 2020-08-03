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
	messages::{
		AllMessages, RuntimeApiMessage, RuntimeApiRequest, RuntimeApiSender,
	},
	errors::{ChainApiError, RuntimeApiError},
	FromOverseer, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemError, SubsystemResult,
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
	EncodeAs, Hash, Signed, SigningContext, SessionIndex,
	ValidatorId, ValidatorIndex, ValidatorPair, GroupRotationInfo,
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
	/// A subsystem error
	#[from]
	Subsystem(SubsystemError),
	/// An error in the Chain API.
	#[from]
	ChainApi(ChainApiError),
	/// An error in the Runtime API.
	#[from]
	RuntimeApi(RuntimeApiError),
	/// The type system wants this even though it doesn't make sense
	#[from]
	Infallible(std::convert::Infallible),
	/// Attempted to convert from an AllMessages to a FromJob, and failed.
	SenderConversion(String),
	/// The local node is not a validator.
	NotAValidator,
	/// The desired job is not present in the jobs list.
	JobNotFound(Hash),
	/// Already forwarding errors to another sender
	AlreadyForwarding,
}

/// A type alias for Runtime API receivers.
pub type RuntimeApiReceiver<T> = oneshot::Receiver<Result<T, RuntimeApiError>>;

/// Request some data from the `RuntimeApi`.
pub async fn request_from_runtime<RequestBuilder, Response, FromJob>(
	parent: Hash,
	sender: &mut mpsc::Sender<FromJob>,
	request_builder: RequestBuilder,
) -> Result<RuntimeApiReceiver<Response>, Error>
where
	RequestBuilder: FnOnce(RuntimeApiSender<Response>) -> RuntimeApiRequest,
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
) -> Result<RuntimeApiReceiver<Vec<ValidatorId>>, Error>
where
	FromJob: TryFrom<AllMessages>,
	<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::Validators(tx)).await
}

/// Request the validator groups.
pub async fn request_validator_groups<FromJob>(
	parent: Hash,
	s: &mut mpsc::Sender<FromJob>,
) -> Result<RuntimeApiReceiver<(Vec<Vec<ValidatorIndex>>, GroupRotationInfo)>, Error>
where
	FromJob: TryFrom<AllMessages>,
	<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| RuntimeApiRequest::ValidatorGroups(tx)).await
}

/// Request the session index of the child block.
pub async fn request_session_index_for_child<FromJob>(
	parent: Hash,
	s: &mut mpsc::Sender<FromJob>,
) -> Result<RuntimeApiReceiver<SessionIndex>, Error>
where
	FromJob: TryFrom<AllMessages>,
	<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
{
	request_from_runtime(parent, s, |tx| {
		RuntimeApiRequest::SessionIndexForChild(tx)
	}).await
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
		// Note: request_validators and request_session_index_for_child do not and cannot
		// run concurrently: they both have a mutable handle to the same sender.
		// However, each of them returns a oneshot::Receiver, and those are resolved concurrently.
		let (validators, session_index) = futures::try_join!(
			request_validators(parent, &mut sender).await?,
			request_session_index_for_child(parent, &mut sender).await?,
		)?;

		let signing_context = SigningContext {
			session_index: session_index?,
			parent_hash: parent,
		};

		let validators = validators?;

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
struct JobHandle<ToJob> {
	abort_handle: future::AbortHandle,
	to_job: mpsc::Sender<ToJob>,
	finished: oneshot::Receiver<()>,
	outgoing_msgs_handle: usize,
}

impl<ToJob> JobHandle<ToJob> {
	/// Send a message to the job.
	async fn send_msg(&mut self, msg: ToJob) -> Result<(), Error> {
		self.to_job.send(msg).await.map_err(Into::into)
	}
}

impl<ToJob: ToJobTrait> JobHandle<ToJob> {
	/// Stop this job gracefully.
	///
	/// If it hasn't shut itself down after `JOB_GRACEFUL_STOP_DURATION`, abort it.
	async fn stop(mut self) {
		// we don't actually care if the message couldn't be sent
		if let Err(_) = self.to_job.send(ToJob::STOP).await {
			// no need to wait further here: the job is either stalled or
			// disconnected, and in either case, we can just abort it immediately
			self.abort_handle.abort();
			return;
		}
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
	type Error: 'static + std::fmt::Debug + Send;
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
		receiver: mpsc::Receiver<Self::ToJob>,
		sender: mpsc::Sender<Self::FromJob>,
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

/// Error which can be returned by the jobs manager
///
/// Wraps the utility error type and the job-specific error
#[derive(Debug, derive_more::From)]
pub enum JobsError<JobError> {
	/// utility error
	#[from]
	Utility(Error),
	/// internal job error
	Job(JobError),
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
	errors: Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>,
}

impl<Spawner: SpawnNamed, Job: 'static + JobTrait> Jobs<Spawner, Job> {
	/// Create a new Jobs manager which handles spawning appropriate jobs.
	pub fn new(spawner: Spawner) -> Self {
		Self {
			spawner,
			running: HashMap::new(),
			outgoing_msgs: StreamUnordered::new(),
			job: std::marker::PhantomData,
			errors: None,
		}
	}

	/// Monitor errors which may occur during handling of a spawned job.
	///
	/// By default, an error in a job is simply logged. Once this is called,
	/// the error is forwarded onto the provided channel.
	///
	/// Errors if the error channel already exists.
	pub fn forward_errors(&mut self, tx: mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>) -> Result<(), Error> {
		if self.errors.is_some() { return Err(Error::AlreadyForwarding) }
		self.errors = Some(tx);
		Ok(())
	}

	/// Spawn a new job for this `parent_hash`, with whatever args are appropriate.
	fn spawn_job(&mut self, parent_hash: Hash, run_args: Job::RunArgs) -> Result<(), Error> {
		let (to_job_tx, to_job_rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);
		let (from_job_tx, from_job_rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);
		let (finished_tx, finished) = oneshot::channel();

		// clone the error transmitter to move into the future
		let err_tx = self.errors.clone();

		let (future, abort_handle) = future::abortable(async move {
			if let Err(e) = Job::run(parent_hash, run_args, to_job_rx, from_job_tx).await {
				log::error!(
					"{}({}) finished with an error {:?}",
					Job::NAME,
					parent_hash,
					e,
				);

				if let Some(mut err_tx) = err_tx {
					// if we can't send the notification of error on the error channel, then
					// there's no point trying to propagate this error onto the channel too
					// all we can do is warn that error propagatio has failed
					if let Err(e) = err_tx.send((Some(parent_hash), JobsError::Job(e))).await {
						log::warn!("failed to forward error: {:?}", e);
					}
				}
			}
		});

		// the spawn mechanism requires that the spawned future has no output
		let future = async move {
			// job errors are already handled within the future, meaning
			// that any errors here are due to the abortable mechanism.
			// failure to abort isn't of interest.
			let _ = future.await;
			// transmission failure here is only possible if the receiver is closed,
			// which means the handle is dropped, which means we don't care anymore
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
	errors: Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>,
}

impl<Spawner, Context, Job> JobManager<Spawner, Context, Job>
where
	Spawner: SpawnNamed + Clone + Send + Unpin,
	Context: SubsystemContext,
	Job: 'static + JobTrait,
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
			errors: None,
		}
	}

	/// Monitor errors which may occur during handling of a spawned job.
	///
	/// By default, an error in a job is simply logged. Once this is called,
	/// the error is forwarded onto the provided channel.
	///
	/// Errors if the error channel already exists.
	pub fn forward_errors(&mut self, tx: mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>) -> Result<(), Error> {
		if self.errors.is_some() { return Err(Error::AlreadyForwarding) }
		self.errors = Some(tx);
		Ok(())
	}

	/// Run this subsystem
	///
	/// Conceptually, this is very simple: it just loops forever.
	///
	/// - On incoming overseer messages, it starts or stops jobs as appropriate.
	/// - On other incoming messages, if they can be converted into Job::ToJob and
	///   include a hash, then they're forwarded to the appropriate individual job.
	/// - On outgoing messages from the jobs, it forwards them to the overseer.
	///
	/// If `err_tx` is not `None`, errors are forwarded onto that channel as they occur.
	/// Otherwise, most are logged and then discarded.
	pub async fn run(mut ctx: Context, run_args: Job::RunArgs, spawner: Spawner, mut err_tx: Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>) {
		let mut jobs = Jobs::new(spawner.clone());
		if let Some(ref err_tx) = err_tx {
			jobs.forward_errors(err_tx.clone()).expect("we never call this twice in this context; qed");
		}

		loop {
			select! {
				incoming = ctx.recv().fuse() => if Self::handle_incoming(incoming, &mut jobs, &run_args, &mut err_tx).await { break },
				outgoing = jobs.next().fuse() => if Self::handle_outgoing(outgoing, &mut ctx, &mut err_tx).await { break },
				complete => break,
			}
		}
	}

	// if we have a channel on which to forward errors, do so
	async fn fwd_err(hash: Option<Hash>, err: JobsError<Job::Error>, err_tx: &mut Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>) {
		if let Some(err_tx) = err_tx {
			// if we can't send on the error transmission channel, we can't do anything useful about it
			// still, we can at least log the failure
			if let Err(e) = err_tx.send((hash, err)).await {
				log::warn!("failed to forward error: {:?}", e);
			}
		}
	}

	// handle an incoming message. return true if we should break afterwards.
	async fn handle_incoming(
		incoming: SubsystemResult<FromOverseer<Context::Message>>,
		jobs: &mut Jobs<Spawner, Job>,
		run_args: &Job::RunArgs,
		err_tx: &mut Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>
	) -> bool {
		use crate::FromOverseer::{Communication, Signal};
		use crate::ActiveLeavesUpdate;
		use crate::OverseerSignal::{BlockFinalized, Conclude, ActiveLeaves};

		match incoming {
			Ok(Signal(ActiveLeaves(ActiveLeavesUpdate { activated, deactivated }))) => {
				for hash in activated {
					if let Err(e) = jobs.spawn_job(hash, run_args.clone()) {
						log::error!("Failed to spawn a job: {:?}", e);
						Self::fwd_err(Some(hash), e.into(), err_tx).await;
						return true;
					}
				}

				for hash in deactivated {
					if let Err(e) = jobs.stop_job(hash).await {
						log::error!("Failed to stop a job: {:?}", e);
						Self::fwd_err(Some(hash), e.into(), err_tx).await;
						return true;
					}
				}
			}
			Ok(Signal(Conclude)) => {
				// Breaking the loop ends fn run, which drops `jobs`, which immediately drops all ongoing work.
				// We can afford to wait a little while to shut them all down properly before doing that.
				//
				// Forwarding the stream to a drain means we wait until all of the items in the stream
				// have completed. Contrast with `into_future`, which turns it into a future of `(head, rest_stream)`.
				use futures::sink::drain;
				use futures::stream::StreamExt;
				use futures::stream::FuturesUnordered;

				if let Err(e) = jobs.running
					.drain()
					.map(|(_, handle)| handle.stop())
					.collect::<FuturesUnordered<_>>()
					.map(Ok)
					.forward(drain())
					.await
				{
					log::error!("failed to stop all jobs on conclude signal: {:?}", e);
					Self::fwd_err(None, Error::from(e).into(), err_tx).await;
				}

				return true;
			}
			Ok(Communication { msg }) => {
				if let Ok(to_job) = <Job::ToJob>::try_from(msg) {
					match to_job.relay_parent() {
						Some(hash) => {
							if let Err(err) = jobs.send_msg(hash, to_job).await {
								log::error!("Failed to send a message to a job: {:?}", err);
								Self::fwd_err(Some(hash), err.into(), err_tx).await;
								return true;
							}
						}
						None => {
							if let Err(err) = Job::handle_unanchored_msg(to_job) {
								log::error!("Failed to handle unhashed message: {:?}", err);
								Self::fwd_err(None, JobsError::Job(err), err_tx).await;
								return true;
							}
						}
					}
				}
			}
			Ok(Signal(BlockFinalized(_))) => {}
			Err(err) => {
				log::error!("error receiving message from subsystem context: {:?}", err);
				Self::fwd_err(None, Error::from(err).into(), err_tx).await;
				return true;
			}
		}
		false
	}

	// handle an outgoing message. return true if we should break afterwards.
	async fn handle_outgoing(outgoing: Option<Job::FromJob>, ctx: &mut Context, err_tx: &mut Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>) -> bool {
		match outgoing {
			Some(msg) => {
				if let Err(e) = ctx.send_message(msg.into()).await {
					Self::fwd_err(None, Error::from(e).into(), err_tx).await;
				}
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
	Job: 'static + JobTrait + Send,
	Job::RunArgs: Clone + Sync,
	Job::ToJob: TryFrom<AllMessages> + Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let spawner = self.spawner.clone();
		let run_args = self.run_args.clone();
		let errors = self.errors;


		let future = Box::pin(async move {
			Self::run(ctx, run_args, spawner, errors).await;
		});

		SpawnedSubsystem {
			name: Job::NAME.strip_suffix("Job").unwrap_or(Job::NAME),
			future,
		}
	}
}

#[cfg(test)]
mod tests {
	use assert_matches::assert_matches;
	use crate::{
		messages::{AllMessages, CandidateSelectionMessage},
		test_helpers::{self, make_subsystem_context},
		util::{
			self,
			JobsError,
			JobManager,
			JobTrait,
			ToJobTrait,
		},
		ActiveLeavesUpdate,
		FromOverseer,
		OverseerSignal,
		SpawnedSubsystem,
		Subsystem,
	};
	use futures::{
		channel::mpsc,
		executor,
		Future,
		FutureExt,
		stream::{self, StreamExt},
		SinkExt,
	};
	use futures_timer::Delay;
	use polkadot_primitives::v1::Hash;
	use std::{
		collections::HashMap,
		convert::TryFrom,
		pin::Pin,
		time::Duration,
	};

	// basic usage: in a nutshell, when you want to define a subsystem, just focus on what its jobs do;
	// you can leave the subsystem itself to the job manager.

	// for purposes of demonstration, we're going to whip up a fake subsystem.
	// this will 'select' candidates which are pre-loaded in the job

	// job structs are constructed within JobTrait::run
	// most will want to retain the sender and receiver, as well as whatever other data they like
	struct FakeCandidateSelectionJob {
		receiver: mpsc::Receiver<ToJob>,
	}

	// ToJob implementations require the following properties:
	//
	// - have a Stop variant (to impl ToJobTrait)
	// - impl ToJobTrait
	// - impl TryFrom<AllMessages>
	// - impl From<CandidateSelectionMessage> (from SubsystemContext::Message)
	//
	// Mostly, they are just a type-safe subset of AllMessages that this job is prepared to receive
	enum ToJob {
		CandidateSelection(CandidateSelectionMessage),
		Stop,
	}

	impl ToJobTrait for ToJob {
		const STOP: Self = ToJob::Stop;

		fn relay_parent(&self) -> Option<Hash> {
			match self {
				Self::CandidateSelection(csm) => csm.relay_parent(),
				Self::Stop => None,
			}
		}
	}

	impl TryFrom<AllMessages> for ToJob {
		type Error = ();

		fn try_from(msg: AllMessages) -> Result<Self, Self::Error> {
			match msg {
				AllMessages::CandidateSelection(csm) => Ok(ToJob::CandidateSelection(csm)),
				_ => Err(())
			}
		}
	}

	impl From<CandidateSelectionMessage> for ToJob {
		fn from(csm: CandidateSelectionMessage) -> ToJob {
			ToJob::CandidateSelection(csm)
		}
	}

	// FromJob must be infallibly convertable into AllMessages.
	//
	// It exists to be a type-safe subset of AllMessages that this job is specified to send.
	//
	// Note: the Clone impl here is not generally required; it's just ueful for this test context because
	// we include it in the RunArgs
	#[derive(Clone)]
	enum FromJob {
		Test(String),
	}

	impl From<FromJob> for AllMessages {
		fn from(from_job: FromJob) -> AllMessages {
			match from_job {
				FromJob::Test(s) => AllMessages::Test(s),
			}
		}
	}

	// Error will mostly be a wrapper to make the try operator more convenient;
	// deriving From implementations for most variants is recommended.
	// It must implement Debug for logging.
	#[derive(Debug, derive_more::From)]
	enum Error {
		#[from]
		Sending(mpsc::SendError)
	}

	impl JobTrait for FakeCandidateSelectionJob {
		type ToJob = ToJob;
		type FromJob = FromJob;
		type Error = Error;
		// RunArgs can be anything that a particular job needs supplied from its external context
		// in order to create the Job. In this case, they're a hashmap of parents to the mock outputs
		// expected from that job.
		//
		// Note that it's not recommended to use something as heavy as a hashmap in production: the
		// RunArgs get cloned so that each job gets its own owned copy. If you need that, wrap it in
		// an Arc. Within a testing context, that efficiency is less important.
		type RunArgs = HashMap<Hash, Vec<FromJob>>;

		const NAME: &'static str = "FakeCandidateSelectionJob";

		/// Run a job for the parent block indicated
		//
		// this function is in charge of creating and executing the job's main loop
		fn run(
			parent: Hash,
			mut run_args: Self::RunArgs,
			receiver: mpsc::Receiver<ToJob>,
			mut sender: mpsc::Sender<FromJob>,
		) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
			async move {
				let job = FakeCandidateSelectionJob {
					receiver,
				};

				// most jobs will have a request-response cycle at the heart of their run loop.
				// however, in this case, we never receive valid messages, so we may as well
				// just send all of our (mock) output messages now
				let mock_output = run_args.remove(&parent).unwrap_or_default();
				let mut stream = stream::iter(mock_output.into_iter().map(Ok));
				sender.send_all(&mut stream).await?;

				// it isn't necessary to break run_loop into its own function,
				// but it's convenient to separate the concerns in this way
				job.run_loop().await
			}.boxed()
		}
	}

	impl FakeCandidateSelectionJob {
		async fn run_loop(mut self) -> Result<(), Error> {
			while let Some(msg) = self.receiver.next().await {
				match msg {
					ToJob::CandidateSelection(_csm) => {
						unimplemented!("we'd report the collator to the peer set manager here, but that's not implemented yet");
					}
					ToJob::Stop => break,
				}
			}

			Ok(())
		}
	}

	// with the job defined, it's straightforward to get a subsystem implementation.
	type FakeCandidateSelectionSubsystem<Spawner, Context> = JobManager<Spawner, Context, FakeCandidateSelectionJob>;

	// this type lets us pretend to be the overseer
	type OverseerHandle = test_helpers::TestSubsystemContextHandle<CandidateSelectionMessage>;

	fn test_harness<T: Future<Output=()>>(run_args: HashMap<Hash, Vec<FromJob>>, test: impl FnOnce(OverseerHandle, mpsc::Receiver<(Option<Hash>, JobsError<Error>)>) -> T) {
		let pool = sp_core::testing::TaskExecutor::new();
		let (context, overseer_handle) = make_subsystem_context(pool.clone());
		let (err_tx, err_rx) = mpsc::channel(16);

		let subsystem = FakeCandidateSelectionSubsystem::run(context, run_args, pool, Some(err_tx));
		let test_future = test(overseer_handle, err_rx);
		let timeout = Delay::new(Duration::from_secs(2));

		futures::pin_mut!(test_future);
		futures::pin_mut!(subsystem);
		futures::pin_mut!(timeout);

		executor::block_on(async move {
			futures::select! {
				_ = test_future.fuse() => (),
				_ = subsystem.fuse() => (),
				_ = timeout.fuse() => panic!("test timed out instead of completing"),
			}
		});
	}

	#[test]
	fn starting_and_stopping_job_works() {
		let relay_parent: Hash = [0; 32].into();
		let mut run_args = HashMap::new();
		let test_message = format!("greetings from {}", relay_parent);
		run_args.insert(relay_parent.clone(), vec![FromJob::Test(test_message.clone())]);

		test_harness(run_args, |mut overseer_handle, err_rx| async move {
			overseer_handle.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::start_work(relay_parent)))).await;
			assert_matches!(
				overseer_handle.recv().await,
				AllMessages::Test(msg) if msg == test_message
			);
			overseer_handle.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(relay_parent)))).await;

			let errs: Vec<_> = err_rx.collect().await;
			assert_eq!(errs.len(), 0);
		});
	}

	#[test]
	fn stopping_non_running_job_fails() {
		let relay_parent: Hash = [0; 32].into();
		let run_args = HashMap::new();

		test_harness(run_args, |mut overseer_handle, err_rx| async move {
			overseer_handle.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate::stop_work(relay_parent)))).await;

			let errs: Vec<_> = err_rx.collect().await;
			assert_eq!(errs.len(), 1);
			assert_eq!(errs[0].0, Some(relay_parent));
			assert_matches!(
				errs[0].1,
				JobsError::Utility(util::Error::JobNotFound(match_relay_parent)) if relay_parent == match_relay_parent
			);
		});
	}

	#[test]
	fn test_subsystem_impl_and_name_derivation() {
		let pool = sp_core::testing::TaskExecutor::new();
		let (context, _) = make_subsystem_context::<CandidateSelectionMessage, _>(pool.clone());

		let SpawnedSubsystem { name, .. } = FakeCandidateSelectionSubsystem::new(
			pool,
			HashMap::new(),
		).start(context);
		assert_eq!(name, "FakeCandidateSelection");
	}
}
