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
//!
//! This crate also reexports Prometheus metric types which are expected to be implemented by subsystems.

#![warn(missing_docs)]

use polkadot_node_subsystem::{
	errors::RuntimeApiError,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest, RuntimeApiSender, BoundToRelayParent},
	FromOverseer, SpawnedSubsystem, Subsystem, SubsystemContext, SubsystemError, SubsystemSender,
	ActiveLeavesUpdate, OverseerSignal,
};
use polkadot_node_jaeger as jaeger;
use futures::{channel::{mpsc, oneshot}, prelude::*, select, stream::Stream};
use futures_timer::Delay;
use parity_scale_codec::Encode;
use pin_project::pin_project;
use polkadot_primitives::v1::{
	CandidateEvent, CommittedCandidateReceipt, CoreState, EncodeAs, PersistedValidationData,
	GroupRotationInfo, Hash, Id as ParaId, OccupiedCoreAssumption,
	SessionIndex, Signed, SigningContext, ValidationCode, ValidatorId, ValidatorIndex, SessionInfo,
	AuthorityDiscoveryId, GroupIndex,
};
use sp_core::{traits::SpawnNamed, Public};
use sp_application_crypto::AppKey;
use sp_keystore::{CryptoStore, SyncCryptoStorePtr, Error as KeystoreError};
use std::{
	collections::{HashMap, hash_map::Entry}, convert::TryFrom, marker::Unpin, pin::Pin, task::{Poll, Context},
	time::Duration, fmt, sync::Arc,
};
use streamunordered::{StreamUnordered, StreamYield};
use thiserror::Error;

pub use metered_channel as metered;
pub use polkadot_node_network_protocol::MIN_GOSSIP_PEERS;

mod error_handling;

/// Error classification.
pub use error_handling::{Fault, unwrap_non_fatal};

/// These reexports are required so that external crates can use the `delegated_subsystem` macro properly.
pub mod reexports {
	pub use sp_core::traits::SpawnNamed;
	pub use polkadot_node_subsystem::{
		SpawnedSubsystem,
		Subsystem,
		SubsystemContext,
	};
}

/// Convenient and efficient runtime info access.
pub mod runtime;

/// Duration a job will wait after sending a stop signal before hard-aborting.
pub const JOB_GRACEFUL_STOP_DURATION: Duration = Duration::from_secs(1);
/// Capacity of channels to and from individual jobs
pub const JOB_CHANNEL_CAPACITY: usize = 64;

/// Utility errors
#[derive(Debug, Error)]
pub enum Error {
	/// Attempted to send or receive on a oneshot channel which had been canceled
	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),
	/// Attempted to send on a MPSC channel which has been canceled
	#[error(transparent)]
	Mpsc(#[from] mpsc::SendError),
	/// A subsystem error
	#[error(transparent)]
	Subsystem(#[from] SubsystemError),
	/// An error in the Runtime API.
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),
	/// The type system wants this even though it doesn't make sense
	#[error(transparent)]
	Infallible(#[from] std::convert::Infallible),
	/// Attempted to convert from an AllMessages to a FromJob, and failed.
	#[error("AllMessage not relevant to Job")]
	SenderConversion(String),
	/// The local node is not a validator.
	#[error("Node is not a validator")]
	NotAValidator,
	/// Already forwarding errors to another sender
	#[error("AlreadyForwarding")]
	AlreadyForwarding,
}

/// A type alias for Runtime API receivers.
pub type RuntimeApiReceiver<T> = oneshot::Receiver<Result<T, RuntimeApiError>>;

/// Request some data from the `RuntimeApi`.
pub async fn request_from_runtime<RequestBuilder, Response, Sender>(
	parent: Hash,
	sender: &mut Sender,
	request_builder: RequestBuilder,
) -> RuntimeApiReceiver<Response>
where
	RequestBuilder: FnOnce(RuntimeApiSender<Response>) -> RuntimeApiRequest,
	Sender: SubsystemSender,
{
	let (tx, rx) = oneshot::channel();

	sender.send_message(RuntimeApiMessage::Request(parent, request_builder(tx)).into()).await;

	rx
}

/// Construct specialized request functions for the runtime.
///
/// These would otherwise get pretty repetitive.
macro_rules! specialize_requests {
	// expand return type name for documentation purposes
	(fn $func_name:ident( $( $param_name:ident : $param_ty:ty ),* ) -> $return_ty:ty ; $request_variant:ident;) => {
		specialize_requests!{
			named stringify!($request_variant) ; fn $func_name( $( $param_name : $param_ty ),* ) -> $return_ty ; $request_variant;
		}
	};

	// create a single specialized request function
	(named $doc_name:expr ; fn $func_name:ident( $( $param_name:ident : $param_ty:ty ),* ) -> $return_ty:ty ; $request_variant:ident;) => {
		#[doc = "Request `"]
		#[doc = $doc_name]
		#[doc = "` from the runtime"]
		pub async fn $func_name(
			parent: Hash,
			$(
				$param_name: $param_ty,
			)*
			sender: &mut impl SubsystemSender,
		) -> RuntimeApiReceiver<$return_ty> {
			request_from_runtime(parent, sender, |tx| RuntimeApiRequest::$request_variant(
				$( $param_name, )* tx
			)).await
		}
	};

	// recursive decompose
	(
		fn $func_name:ident( $( $param_name:ident : $param_ty:ty ),* ) -> $return_ty:ty ; $request_variant:ident;
		$(
			fn $t_func_name:ident( $( $t_param_name:ident : $t_param_ty:ty ),* ) -> $t_return_ty:ty ; $t_request_variant:ident;
		)+
	) => {
		specialize_requests!{
			fn $func_name( $( $param_name : $param_ty ),* ) -> $return_ty ; $request_variant ;
		}
		specialize_requests!{
			$(
				fn $t_func_name( $( $t_param_name : $t_param_ty ),* ) -> $t_return_ty ; $t_request_variant ;
			)+
		}
	};
}

specialize_requests! {
	fn request_authorities() -> Vec<AuthorityDiscoveryId>; Authorities;
	fn request_validators() -> Vec<ValidatorId>; Validators;
	fn request_validator_groups() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo); ValidatorGroups;
	fn request_availability_cores() -> Vec<CoreState>; AvailabilityCores;
	fn request_persisted_validation_data(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<PersistedValidationData>; PersistedValidationData;
	fn request_session_index_for_child() -> SessionIndex; SessionIndexForChild;
	fn request_validation_code(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<ValidationCode>; ValidationCode;
	fn request_candidate_pending_availability(para_id: ParaId) -> Option<CommittedCandidateReceipt>; CandidatePendingAvailability;
	fn request_candidate_events() -> Vec<CandidateEvent>; CandidateEvents;
	fn request_session_info(index: SessionIndex) -> Option<SessionInfo>; SessionInfo;
}

/// From the given set of validators, find the first key we can sign with, if any.
pub async fn signing_key(validators: &[ValidatorId], keystore: &SyncCryptoStorePtr)
	-> Option<ValidatorId>
{
	signing_key_and_index(validators, keystore).await.map(|(k, _)| k)
}

/// From the given set of validators, find the first key we can sign with, if any, and return it
/// along with the validator index.
pub async fn signing_key_and_index(validators: &[ValidatorId], keystore: &SyncCryptoStorePtr)
	-> Option<(ValidatorId, ValidatorIndex)>
{
	for (i, v) in validators.iter().enumerate() {
		if CryptoStore::has_keys(&**keystore, &[(v.to_raw_vec(), ValidatorId::ID)]).await {
			return Some((v.clone(), ValidatorIndex(i as _)));
		}
	}
	None
}

/// Find the validator group the given validator index belongs to.
pub fn find_validator_group(groups: &[Vec<ValidatorIndex>], index: ValidatorIndex)
	-> Option<GroupIndex>
{
	groups.iter().enumerate().find_map(|(i, g)| if g.contains(&index) {
		Some(GroupIndex(i as _))
	} else {
		None
	})
}

/// Chooses a random subset of sqrt(v.len()), but at least `min` elements.
pub fn choose_random_sqrt_subset<T>(mut v: Vec<T>, min: usize) -> Vec<T> {
	use rand::seq::SliceRandom as _;
	let mut rng = rand::thread_rng();
	v.shuffle(&mut rng);

	let len = max_of_min_and_sqrt_len(v.len(), min);
	v.truncate(len);
	v
}

/// Returns bool with a probability of `max(len.sqrt(), min) / len`
/// being true.
pub fn gen_ratio_sqrt_subset(len: usize, min: usize) -> bool {
	use rand::Rng as _;
	let mut rng = rand::thread_rng();
	let threshold = max_of_min_and_sqrt_len(len, min);
	let n = rng.gen_range(0..len);
	n < threshold
}

fn max_of_min_and_sqrt_len(len: usize, min: usize) -> usize {
	let len_sqrt = (len as f64).sqrt() as usize;
	std::cmp::max(min, len_sqrt)
}

/// Local validator information
///
/// It can be created if the local node is a validator in the context of a particular
/// relay chain block.
#[derive(Debug)]
pub struct Validator {
	signing_context: SigningContext,
	key: ValidatorId,
	index: ValidatorIndex,
}

impl Validator {
	/// Get a struct representing this node's validator if this node is in fact a validator in the context of the given block.
	pub async fn new(
		parent: Hash,
		keystore: SyncCryptoStorePtr,
		sender: &mut impl SubsystemSender,
	) -> Result<Self, Error> {
		// Note: request_validators and request_session_index_for_child do not and cannot
		// run concurrently: they both have a mutable handle to the same sender.
		// However, each of them returns a oneshot::Receiver, and those are resolved concurrently.
		let (validators, session_index) = futures::try_join!(
			request_validators(parent, sender).await,
			request_session_index_for_child(parent, sender).await,
		)?;

		let signing_context = SigningContext {
			session_index: session_index?,
			parent_hash: parent,
		};

		let validators = validators?;

		Self::construct(&validators, signing_context, keystore).await
	}

	/// Construct a validator instance without performing runtime fetches.
	///
	/// This can be useful if external code also needs the same data.
	pub async fn construct(
		validators: &[ValidatorId],
		signing_context: SigningContext,
		keystore: SyncCryptoStorePtr,
	) -> Result<Self, Error> {
		let (key, index) = signing_key_and_index(validators, &keystore)
			.await
			.ok_or(Error::NotAValidator)?;

		Ok(Validator {
			signing_context,
			key,
			index,
		})
	}

	/// Get this validator's id.
	pub fn id(&self) -> ValidatorId {
		self.key.clone()
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
	pub async fn sign<Payload: EncodeAs<RealPayload>, RealPayload: Encode>(
		&self,
		keystore: SyncCryptoStorePtr,
		payload: Payload,
	) -> Result<Option<Signed<Payload, RealPayload>>, KeystoreError> {
		Signed::sign(&keystore, payload, &self.signing_context, self.index, &self.key).await
	}
}

struct AbortOnDrop(future::AbortHandle);

impl Drop for AbortOnDrop {
	fn drop(&mut self) {
		self.0.abort();
	}
}

/// A JobHandle manages a particular job for a subsystem.
struct JobHandle<ToJob> {
	_abort_handle: AbortOnDrop,
	to_job: mpsc::Sender<ToJob>,
}

impl<ToJob> JobHandle<ToJob> {
	/// Send a message to the job.
	async fn send_msg(&mut self, msg: ToJob) -> Result<(), Error> {
		self.to_job.send(msg).await.map_err(Into::into)
	}
}

/// This module reexports Prometheus types and defines the [`Metrics`] trait.
pub mod metrics {
	/// Reexport Substrate Prometheus types.
	pub use substrate_prometheus_endpoint as prometheus;


	/// Subsystem- or job-specific Prometheus metrics.
	///
	/// Usually implemented as a wrapper for `Option<ActualMetrics>`
	/// to ensure `Default` bounds or as a dummy type ().
	/// Prometheus metrics internally hold an `Arc` reference, so cloning them is fine.
	pub trait Metrics: Default + Clone {
		/// Try to register metrics in the Prometheus registry.
		fn try_register(registry: &prometheus::Registry) -> Result<Self, prometheus::PrometheusError>;

		/// Convenience method to register metrics in the optional Promethius registry.
		///
		/// If no registry is provided, returns `Default::default()`. Otherwise, returns the same
		/// thing that `try_register` does.
		fn register(registry: Option<&prometheus::Registry>) -> Result<Self, prometheus::PrometheusError> {
			match registry {
				None => Ok(Self::default()),
				Some(registry) => Self::try_register(registry),
			}
		}
	}

	// dummy impl
	impl Metrics for () {
		fn try_register(_registry: &prometheus::Registry) -> Result<(), prometheus::PrometheusError> {
			Ok(())
		}
	}
}

/// Commands from a job to the broader subsystem.
pub enum FromJobCommand {
	/// Spawn a child task on the executor.
	Spawn(&'static str, Pin<Box<dyn Future<Output = ()> + Send>>),
	/// Spawn a blocking child task on the executor's dedicated thread pool.
	SpawnBlocking(&'static str, Pin<Box<dyn Future<Output = ()> + Send>>),
}

/// A sender for messages from jobs, as well as commands to the overseer.
#[derive(Clone)]
pub struct JobSender<S> {
	sender: S,
	from_job: mpsc::Sender<FromJobCommand>,
}

impl<S: SubsystemSender> JobSender<S> {
	/// Get access to the underlying subsystem sender.
	pub fn subsystem_sender(&mut self) -> &mut S {
		&mut self.sender
	}

	/// Send a direct message to some other `Subsystem`, routed based on message type.
	pub async fn send_message(&mut self, msg: AllMessages) {
		self.sender.send_message(msg).await
	}

	/// Send multiple direct messages to other `Subsystem`s, routed based on message type.
	pub async fn send_messages<T>(&mut self, msgs: T)
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send
	{
		self.sender.send_messages(msgs).await
	}


	/// Send a message onto the unbounded queue of some other `Subsystem`, routed based on message
	/// type.
	///
	/// This function should be used only when there is some other bounding factor on the messages
	/// sent with it. Otherwise, it risks a memory leak.
	pub fn send_unbounded_message(&mut self, msg: AllMessages) {
		self.sender.send_unbounded_message(msg)
	}

	/// Send a command to the subsystem, to be relayed onwards to the overseer.
	pub async fn send_command(&mut self, msg: FromJobCommand) -> Result<(), mpsc::SendError> {
		self.from_job.send(msg).await
	}
}

#[async_trait::async_trait]
impl<S: SubsystemSender> SubsystemSender for JobSender<S> {
	async fn send_message(&mut self, msg: AllMessages) {
		self.sender.send_message(msg).await
	}

	async fn send_messages<T>(&mut self, msgs: T)
		where T: IntoIterator<Item = AllMessages> + Send, T::IntoIter: Send
	{
		self.sender.send_messages(msgs).await
	}

	fn send_unbounded_message(&mut self, msg: AllMessages) {
		self.sender.send_unbounded_message(msg)
	}
}

impl fmt::Debug for FromJobCommand {
	fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
		match self {
			Self::Spawn(name, _) => write!(fmt, "FromJobCommand::Spawn({})", name),
			Self::SpawnBlocking(name, _) => write!(fmt, "FromJobCommand::SpawnBlocking({})", name),
		}
	}
}

/// This trait governs jobs.
///
/// Jobs are instantiated and killed automatically on appropriate overseer messages.
/// Other messages are passed along to and from the job via the overseer to other subsystems.
pub trait JobTrait: Unpin + Sized {
	/// Message type used to send messages to the job.
	type ToJob: 'static + BoundToRelayParent + Send;
	/// Job runtime error.
	type Error: 'static + std::error::Error + Send;
	/// Extra arguments this job needs to run properly.
	///
	/// If no extra information is needed, it is perfectly acceptable to set it to `()`.
	type RunArgs: 'static + Send;
	/// Subsystem-specific Prometheus metrics.
	///
	/// Jobs spawned by one subsystem should share the same
	/// instance of metrics (use `.clone()`).
	/// The `delegate_subsystem!` macro should take care of this.
	type Metrics: 'static + metrics::Metrics + Send;

	/// Name of the job, i.e. `CandidateBackingJob`
	const NAME: &'static str;

	/// Run a job for the given relay `parent`.
	///
	/// The job should be ended when `receiver` returns `None`.
	fn run<S: SubsystemSender>(
		parent: Hash,
		span: Arc<jaeger::Span>,
		run_args: Self::RunArgs,
		metrics: Self::Metrics,
		receiver: mpsc::Receiver<Self::ToJob>,
		sender: JobSender<S>,
	) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>>;
}

/// Error which can be returned by the jobs manager
///
/// Wraps the utility error type and the job-specific error
#[derive(Debug, Error)]
pub enum JobsError<JobError: std::fmt::Debug + std::error::Error + 'static> {
	/// utility error
	#[error("Utility")]
	Utility(#[source] Error),
	/// internal job error
	#[error("Internal")]
	Job(#[source] JobError),
}

/// Jobs manager for a subsystem
///
/// - Spawns new jobs for a given relay-parent on demand.
/// - Closes old jobs for a given relay-parent on demand.
/// - Dispatches messages to the appropriate job for a given relay-parent.
/// - When dropped, aborts all remaining jobs.
/// - implements `Stream<Item=FromJobCommand>`, collecting all messages from subordinate jobs.
#[pin_project]
struct Jobs<Spawner, ToJob> {
	spawner: Spawner,
	running: HashMap<Hash, JobHandle<ToJob>>,
	outgoing_msgs: StreamUnordered<mpsc::Receiver<FromJobCommand>>,
}

impl<Spawner: SpawnNamed, ToJob: Send + 'static> Jobs<Spawner, ToJob> {
	/// Create a new Jobs manager which handles spawning appropriate jobs.
	pub fn new(spawner: Spawner) -> Self {
		Self {
			spawner,
			running: HashMap::new(),
			outgoing_msgs: StreamUnordered::new(),
		}
	}

	/// Spawn a new job for this `parent_hash`, with whatever args are appropriate.
	fn spawn_job<Job, Sender>(
		&mut self,
		parent_hash: Hash,
		span: Arc<jaeger::Span>,
		run_args: Job::RunArgs,
		metrics: Job::Metrics,
		sender: Sender,
	)
		where Job: JobTrait<ToJob = ToJob>, Sender: SubsystemSender,
	{
		let (to_job_tx, to_job_rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);
		let (from_job_tx, from_job_rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);

		let (future, abort_handle) = future::abortable(async move {
			if let Err(e) = Job::run(
				parent_hash,
				span,
				run_args,
				metrics,
				to_job_rx,
				JobSender {
					sender,
					from_job: from_job_tx,
				},
			).await {
				tracing::error!(
					job = Job::NAME,
					parent_hash = %parent_hash,
					err = ?e,
					"job finished with an error",
				);

				return Err(e);
			}

			Ok(())
		});

		self.spawner.spawn(Job::NAME, future.map(drop).boxed());
		self.outgoing_msgs.push(from_job_rx);

		let handle = JobHandle {
			_abort_handle: AbortOnDrop(abort_handle),
			to_job: to_job_tx,
		};

		self.running.insert(parent_hash, handle);
	}

	/// Stop the job associated with this `parent_hash`.
	pub async fn stop_job(&mut self, parent_hash: Hash) {
		self.running.remove(&parent_hash);
	}

	/// Send a message to the appropriate job for this `parent_hash`.
	async fn send_msg(&mut self, parent_hash: Hash, msg: ToJob) {
		if let Entry::Occupied(mut job) = self.running.entry(parent_hash) {
			if job.get_mut().send_msg(msg).await.is_err() {
				job.remove();
			}
		}
	}
}

impl<Spawner, ToJob> Stream for Jobs<Spawner, ToJob>
where
	Spawner: SpawnNamed,
{
	type Item = FromJobCommand;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
		loop {
			match Pin::new(&mut self.outgoing_msgs).poll_next(cx) {
				Poll::Pending => return Poll::Pending,
				Poll::Ready(r) => match r.map(|v| v.0) {
					Some(StreamYield::Item(msg)) => return Poll::Ready(Some(msg)),
					// If a job is finished, rerun the loop
					Some(StreamYield::Finished(_)) => continue,
					// Don't end if there are no jobs running
					None => return Poll::Pending,
				}
			}
		}
	}
}

impl<Spawner, ToJob> stream::FusedStream for Jobs<Spawner, ToJob>
where
	Spawner: SpawnNamed,
{
	fn is_terminated(&self) -> bool {
		false
	}
}

/// Parameters to a job subsystem.
struct JobSubsystemParams<Spawner, RunArgs, Metrics> {
	/// A spawner for sub-tasks.
	spawner: Spawner,
	/// Arguments to each job.
	run_args: RunArgs,
	/// Metrics for the subsystem.
	metrics: Metrics,
}

/// A subsystem which wraps jobs.
///
/// Conceptually, this is very simple: it just loops forever.
///
/// - On incoming overseer messages, it starts or stops jobs as appropriate.
/// - On other incoming messages, if they can be converted into Job::ToJob and
///   include a hash, then they're forwarded to the appropriate individual job.
/// - On outgoing messages from the jobs, it forwards them to the overseer.
pub struct JobSubsystem<Job: JobTrait, Spawner> {
	params: JobSubsystemParams<Spawner, Job::RunArgs, Job::Metrics>,
	_marker: std::marker::PhantomData<Job>,
}

impl<Job: JobTrait, Spawner> JobSubsystem<Job, Spawner> {
	/// Create a new `JobSubsystem`.
	pub fn new(spawner: Spawner, run_args: Job::RunArgs, metrics: Job::Metrics) -> Self {
		JobSubsystem {
			params: JobSubsystemParams {
				spawner,
				run_args,
				metrics,
			},
			_marker: std::marker::PhantomData,
		}
	}

	/// Run the subsystem to completion.
	pub async fn run<Context>(self, mut ctx: Context)
		where
			Spawner: SpawnNamed + Send + Clone + Unpin + 'static,
			Context: SubsystemContext,
			Job: 'static + JobTrait + Send,
			Job::RunArgs: Clone + Sync,
			Job::ToJob: From<<Context as SubsystemContext>::Message> + Sync,
			Job::Metrics: Sync,
	{
		let JobSubsystem {
			params: JobSubsystemParams {
				spawner,
				run_args,
				metrics,
			},
			..
		} = self;

		let mut jobs = Jobs::new(spawner);

		loop {
			select! {
				incoming = ctx.recv().fuse() => {
					match incoming {
						Ok(FromOverseer::Signal(OverseerSignal::ActiveLeaves(ActiveLeavesUpdate {
							activated,
							deactivated,
						}))) => {
							for activated in activated {
								let sender: Context::Sender = ctx.sender().clone();
								jobs.spawn_job::<Job, _>(
									activated.hash,
									activated.span,
									run_args.clone(),
									metrics.clone(),
									sender,
								)
							}

							for hash in deactivated {
								jobs.stop_job(hash).await;
							}
						}
						Ok(FromOverseer::Signal(OverseerSignal::Conclude)) => {
							jobs.running.clear();
							break;
						}
						Ok(FromOverseer::Signal(OverseerSignal::BlockFinalized(..))) => {}
						Ok(FromOverseer::Communication { msg }) => {
							if let Ok(to_job) = <Job::ToJob>::try_from(msg) {
								jobs.send_msg(to_job.relay_parent(), to_job).await;
							}
						}
						Err(err) => {
							tracing::error!(
								job = Job::NAME,
								err = ?err,
								"error receiving message from subsystem context for job",
							);
							break;
						}
					}
				}
				outgoing = jobs.next() => {
					let res = match outgoing.expect("the Jobs stream never ends; qed") {
						FromJobCommand::Spawn(name, task) => ctx.spawn(name, task).await,
						FromJobCommand::SpawnBlocking(name, task)
							=> ctx.spawn_blocking(name, task).await,
					};

					if let Err(e) = res {
						tracing::warn!(err = ?e, "failed to handle command from job");
					}
				}
				complete => break,
			}
		}
	}
}

impl<Context, Job, Spawner> Subsystem<Context> for JobSubsystem<Job, Spawner>
where
	Spawner: SpawnNamed + Send + Clone + Unpin + 'static,
	Context: SubsystemContext,
	Job: 'static + JobTrait + Send,
	Job::RunArgs: Clone + Sync,
	Job::ToJob: From<<Context as SubsystemContext>::Message> + Sync,
	Job::Metrics: Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let future = Box::pin(async move {
			self.run(ctx).await;
			Ok(())
		});

		SpawnedSubsystem {
			name: Job::NAME.strip_suffix("Job").unwrap_or(Job::NAME),
			future,
		}
	}
}

/// A future that wraps another future with a `Delay` allowing for time-limited futures.
#[pin_project]
pub struct Timeout<F: Future> {
	#[pin]
	future: F,
	#[pin]
	delay: Delay,
}

/// Extends `Future` to allow time-limited futures.
pub trait TimeoutExt: Future {
	/// Adds a timeout of `duration` to the given `Future`.
	/// Returns a new `Future`.
	fn timeout(self, duration: Duration) -> Timeout<Self>
	where
		Self: Sized,
	{
		Timeout {
			future: self,
			delay: Delay::new(duration),
		}
	}
}

impl<F: Future> TimeoutExt for F {}

impl<F: Future> Future for Timeout<F> {
	type Output = Option<F::Output>;

	fn poll(self: Pin<&mut Self>, ctx: &mut Context) -> Poll<Self::Output> {
		let this = self.project();

		if this.delay.poll(ctx).is_ready() {
			return Poll::Ready(None);
		}

		if let Poll::Ready(output) = this.future.poll(ctx) {
			return Poll::Ready(Some(output));
		}

		Poll::Pending
	}
}


#[derive(Copy, Clone)]
enum MetronomeState {
	Snooze,
	SetAlarm,
}

/// Create a stream of ticks with a defined cycle duration.
pub struct Metronome {
	delay: Delay,
	period: Duration,
	state: MetronomeState,
}

impl Metronome
{
	/// Create a new metronome source with a defined cycle duration.
	pub fn new(cycle: Duration) -> Self {
		let period = cycle.into();
		Self {
			period,
			delay: Delay::new(period),
			state: MetronomeState::Snooze,
		}
	}
}

impl futures::Stream for Metronome
{
	type Item = ();
	fn poll_next(
		mut self: Pin<&mut Self>,
		cx: &mut Context<'_>
	) -> Poll<Option<Self::Item>> {
		loop {
			match self.state {
				MetronomeState::SetAlarm => {
					let val = self.period.clone();
					self.delay.reset(val);
					self.state = MetronomeState::Snooze;
				}
				MetronomeState::Snooze => {
					if !Pin::new(&mut self.delay).poll(cx).is_ready() {
						break
					}
					self.state = MetronomeState::SetAlarm;
					return Poll::Ready(Some(()));
				}
			}
		}
		Poll::Pending
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use executor::block_on;
	use thiserror::Error;
	use polkadot_node_jaeger as jaeger;
	use polkadot_node_subsystem::{
		messages::{AllMessages, CollatorProtocolMessage}, ActiveLeavesUpdate, FromOverseer, OverseerSignal,
		SpawnedSubsystem, ActivatedLeaf, LeafStatus,
	};
	use assert_matches::assert_matches;
	use futures::{channel::mpsc, executor, StreamExt, future, Future, FutureExt, SinkExt};
	use polkadot_primitives::v1::Hash;
	use polkadot_node_subsystem_test_helpers::{self as test_helpers, make_subsystem_context};
	use std::{pin::Pin, sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};

	// basic usage: in a nutshell, when you want to define a subsystem, just focus on what its jobs do;
	// you can leave the subsystem itself to the job manager.

	// for purposes of demonstration, we're going to whip up a fake subsystem.
	// this will 'select' candidates which are pre-loaded in the job

	// job structs are constructed within JobTrait::run
	// most will want to retain the sender and receiver, as well as whatever other data they like
	struct FakeCollatorProtocolJob {
		receiver: mpsc::Receiver<CollatorProtocolMessage>,
	}

	// Error will mostly be a wrapper to make the try operator more convenient;
	// deriving From implementations for most variants is recommended.
	// It must implement Debug for logging.
	#[derive(Debug, Error)]
	enum Error {
		#[error(transparent)]
		Sending(#[from]mpsc::SendError),
	}

	impl JobTrait for FakeCollatorProtocolJob {
		type ToJob = CollatorProtocolMessage;
		type Error = Error;
		type RunArgs = bool;
		type Metrics = ();

		const NAME: &'static str = "FakeCollatorProtocolJob";

		/// Run a job for the parent block indicated
		//
		// this function is in charge of creating and executing the job's main loop
		fn run<S: SubsystemSender>(
			_: Hash,
			_: Arc<jaeger::Span>,
			run_args: Self::RunArgs,
			_metrics: Self::Metrics,
			receiver: mpsc::Receiver<CollatorProtocolMessage>,
			mut sender: JobSender<S>,
		) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
			async move {
				let job = FakeCollatorProtocolJob { receiver };

				if run_args {
					sender.send_message(CollatorProtocolMessage::Invalid(
						Default::default(),
						Default::default(),
					).into()).await;
				}

				// it isn't necessary to break run_loop into its own function,
				// but it's convenient to separate the concerns in this way
				job.run_loop().await
			}
			.boxed()
		}
	}

	impl FakeCollatorProtocolJob {
		async fn run_loop(mut self) -> Result<(), Error> {
			loop {
				match self.receiver.next().await {
					Some(_csm) => {
						unimplemented!("we'd report the collator to the peer set manager here, but that's not implemented yet");
					}
					None => break,
				}
			}

			Ok(())
		}
	}

	// with the job defined, it's straightforward to get a subsystem implementation.
	type FakeCollatorProtocolSubsystem<Spawner> =
		JobSubsystem<FakeCollatorProtocolJob, Spawner>;

	// this type lets us pretend to be the overseer
	type OverseerHandle = test_helpers::TestSubsystemContextHandle<CollatorProtocolMessage>;

	fn test_harness<T: Future<Output = ()>>(
		run_args: bool,
		test: impl FnOnce(OverseerHandle) -> T,
	) {
		let _ = env_logger::builder()
			.is_test(true)
			.filter(
				None,
				log::LevelFilter::Trace,
			)
			.try_init();

		let pool = sp_core::testing::TaskExecutor::new();
		let (context, overseer_handle) = make_subsystem_context(pool.clone());

		let subsystem = FakeCollatorProtocolSubsystem::new(
			pool,
			run_args,
			(),
		).run(context);
		let test_future = test(overseer_handle);

		futures::pin_mut!(subsystem, test_future);

		executor::block_on(async move {
			future::join(subsystem, test_future)
				.timeout(Duration::from_secs(2))
				.await
				.expect("test timed out instead of completing")
		});
	}

	#[test]
	fn starting_and_stopping_job_works() {
		let relay_parent: Hash = [0; 32].into();

		test_harness(true, |mut overseer_handle| async move {
			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: relay_parent,
						number: 1,
						status: LeafStatus::Fresh,
						span: Arc::new(jaeger::Span::Disabled),
					}),
				)))
				.await;
			assert_matches!(
				overseer_handle.recv().await,
				AllMessages::CollatorProtocol(_)
			);
			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::stop_work(relay_parent),
				)))
				.await;

			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
				.await;
		});
	}

	#[test]
	fn sending_to_a_non_running_job_do_not_stop_the_subsystem() {
		let relay_parent = Hash::repeat_byte(0x01);

		test_harness(true, |mut overseer_handle| async move {
			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(ActivatedLeaf {
						hash: relay_parent,
						number: 1,
						status: LeafStatus::Fresh,
						span: Arc::new(jaeger::Span::Disabled),
					}),
				)))
				.await;

			// send to a non running job
			overseer_handle
				.send(FromOverseer::Communication {
					msg: Default::default(),
				})
				.await;

			// the subsystem is still alive
			assert_matches!(
				overseer_handle.recv().await,
				AllMessages::CollatorProtocol(_)
			);

			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
				.await;
		});
	}

	#[test]
	fn test_subsystem_impl_and_name_derivation() {
		let pool = sp_core::testing::TaskExecutor::new();
		let (context, _) = make_subsystem_context::<CollatorProtocolMessage, _>(pool.clone());

		let SpawnedSubsystem { name, .. } =
			FakeCollatorProtocolSubsystem::new(pool, false, ()).start(context);
		assert_eq!(name, "FakeCollatorProtocol");
	}


	#[test]
	fn tick_tack_metronome() {
		let n = Arc::new(AtomicUsize::default());

		let (tick, mut block) = mpsc::unbounded();

		let metronome = {
			let n = n.clone();
			let stream = Metronome::new(Duration::from_millis(137_u64));
			stream.for_each(move |_res| {
				let _ = n.fetch_add(1, Ordering::Relaxed);
				let mut tick = tick.clone();
				async move {
					tick.send(()).await.expect("Test helper channel works. qed");
				}
			}).fuse()
		};

		let f2 = async move {
			block.next().await;
			assert_eq!(n.load(Ordering::Relaxed), 1_usize);
			block.next().await;
			assert_eq!(n.load(Ordering::Relaxed), 2_usize);
			block.next().await;
			assert_eq!(n.load(Ordering::Relaxed), 3_usize);
			block.next().await;
			assert_eq!(n.load(Ordering::Relaxed), 4_usize);
		}.fuse();

		futures::pin_mut!(f2);
		futures::pin_mut!(metronome);

		block_on(async move {
			// futures::join!(metronome, f2)
			futures::select!(
				_ = metronome => unreachable!("Metronome never stops. qed"),
				_ = f2 => (),
			)
		});
	}
}
