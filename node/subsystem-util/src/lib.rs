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

#![deny(unused_results)]
// #![deny(unused_crate_dependencies] causes false positives
// https://github.com/rust-lang/rust/issues/57274
#![warn(missing_docs)]

use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest, RuntimeApiSender},
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
use parity_scale_codec::Encode;
use pin_project::{pin_project, pinned_drop};
use polkadot_primitives::v1::{
	CandidateEvent, CommittedCandidateReceipt, CoreState, EncodeAs, PersistedValidationData,
	GroupRotationInfo, Hash, Id as ParaId, ValidationData, OccupiedCoreAssumption,
	SessionIndex, Signed, SigningContext, ValidationCode, ValidatorId, ValidatorIndex,
};
use sp_core::{
	traits::SpawnNamed,
	Public
};
use sp_application_crypto::AppKey;
use sp_keystore::{
	CryptoStore,
	SyncCryptoStorePtr,
	Error as KeystoreError,
};
use std::{
	collections::HashMap,
	convert::{TryFrom, TryInto},
	marker::Unpin,
	pin::Pin,
	task::{Poll, Context},
	time::Duration,
};
use streamunordered::{StreamUnordered, StreamYield};
use thiserror::Error;

pub mod validator_discovery;

/// These reexports are required so that external crates can use the `delegated_subsystem` macro properly.
pub mod reexports {
	pub use sp_core::traits::SpawnNamed;
	pub use polkadot_node_subsystem::{
		SpawnedSubsystem,
		Subsystem,
		SubsystemContext,
	};
}


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
	/// An error in the Chain API.
	#[error(transparent)]
	ChainApi(#[from] ChainApiError),
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
	/// The desired job is not present in the jobs list.
	#[error("Relay parent {0} not of interest")]
	JobNotFound(Hash),
	/// Already forwarding errors to another sender
	#[error("AlreadyForwarding")]
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
		pub async fn $func_name<FromJob>(
			parent: Hash,
			$(
				$param_name: $param_ty,
			)*
			sender: &mut mpsc::Sender<FromJob>,
		) -> Result<RuntimeApiReceiver<$return_ty>, Error>
		where
			FromJob: TryFrom<AllMessages>,
			<FromJob as TryFrom<AllMessages>>::Error: std::fmt::Debug,
		{
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
	fn request_validators() -> Vec<ValidatorId>; Validators;
	fn request_validator_groups() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo); ValidatorGroups;
	fn request_availability_cores() -> Vec<CoreState>; AvailabilityCores;
	fn request_full_validation_data(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<ValidationData>; FullValidationData;
	fn request_persisted_validation_data(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<PersistedValidationData>; PersistedValidationData;
	fn request_session_index_for_child() -> SessionIndex; SessionIndexForChild;
	fn request_validation_code(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<ValidationCode>; ValidationCode;
	fn request_candidate_pending_availability(para_id: ParaId) -> Option<CommittedCandidateReceipt>; CandidatePendingAvailability;
	fn request_candidate_events() -> Vec<CandidateEvent>; CandidateEvents;
}

/// Request some data from the `RuntimeApi` via a SubsystemContext.
async fn request_from_runtime_ctx<RequestBuilder, Context, Response>(
	parent: Hash,
	ctx: &mut Context,
	request_builder: RequestBuilder,
) -> Result<RuntimeApiReceiver<Response>, Error>
where
	RequestBuilder: FnOnce(RuntimeApiSender<Response>) -> RuntimeApiRequest,
	Context: SubsystemContext,
{
	let (tx, rx) = oneshot::channel();

	ctx
		.send_message(
			AllMessages::RuntimeApi(RuntimeApiMessage::Request(parent, request_builder(tx)))
				.try_into()
				.map_err(|err| Error::SenderConversion(format!("{:?}", err)))?,
		)
		.await?;

	Ok(rx)
}


/// Construct specialized request functions for the runtime.
///
/// These would otherwise get pretty repetitive.
macro_rules! specialize_requests_ctx {
	// expand return type name for documentation purposes
	(fn $func_name:ident( $( $param_name:ident : $param_ty:ty ),* ) -> $return_ty:ty ; $request_variant:ident;) => {
		specialize_requests_ctx!{
			named stringify!($request_variant) ; fn $func_name( $( $param_name : $param_ty ),* ) -> $return_ty ; $request_variant;
		}
	};

	// create a single specialized request function
	(named $doc_name:expr ; fn $func_name:ident( $( $param_name:ident : $param_ty:ty ),* ) -> $return_ty:ty ; $request_variant:ident;) => {
		#[doc = "Request `"]
		#[doc = $doc_name]
		#[doc = "` from the runtime via a `SubsystemContext`"]
		pub async fn $func_name<Context: SubsystemContext>(
			parent: Hash,
			$(
				$param_name: $param_ty,
			)*
			ctx: &mut Context,
		) -> Result<RuntimeApiReceiver<$return_ty>, Error> {
			request_from_runtime_ctx(parent, ctx, |tx| RuntimeApiRequest::$request_variant(
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
		specialize_requests_ctx!{
			fn $func_name( $( $param_name : $param_ty ),* ) -> $return_ty ; $request_variant ;
		}
		specialize_requests_ctx!{
			$(
				fn $t_func_name( $( $t_param_name : $t_param_ty ),* ) -> $t_return_ty ; $t_request_variant ;
			)+
		}
	};
}

specialize_requests_ctx! {
	fn request_validators_ctx() -> Vec<ValidatorId>; Validators;
	fn request_validator_groups_ctx() -> (Vec<Vec<ValidatorIndex>>, GroupRotationInfo); ValidatorGroups;
	fn request_availability_cores_ctx() -> Vec<CoreState>; AvailabilityCores;
	fn request_full_validation_data_ctx(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<ValidationData>; FullValidationData;
	fn request_persisted_validation_data_ctx(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<PersistedValidationData>; PersistedValidationData;
	fn request_session_index_for_child_ctx() -> SessionIndex; SessionIndexForChild;
	fn request_validation_code_ctx(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<ValidationCode>; ValidationCode;
	fn request_candidate_pending_availability_ctx(para_id: ParaId) -> Option<CommittedCandidateReceipt>; CandidatePendingAvailability;
	fn request_candidate_events_ctx() -> Vec<CandidateEvent>; CandidateEvents;
}

/// From the given set of validators, find the first key we can sign with, if any.
pub async fn signing_key(validators: &[ValidatorId], keystore: SyncCryptoStorePtr) -> Option<ValidatorId> {
	for v in validators.iter() {
		if CryptoStore::has_keys(&*keystore, &[(v.to_raw_vec(), ValidatorId::ID)]).await {
			return Some(v.clone());
		}
	}
	None
}

/// Local validator information
///
/// It can be created if the local node is a validator in the context of a particular
/// relay chain block.
pub struct Validator {
	signing_context: SigningContext,
	key: ValidatorId,
	index: ValidatorIndex,
}

impl Validator {
	/// Get a struct representing this node's validator if this node is in fact a validator in the context of the given block.
	pub async fn new<FromJob>(
		parent: Hash,
		keystore: SyncCryptoStorePtr,
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
		let key = signing_key(validators, keystore).await.ok_or(Error::NotAValidator)?;
		let index = validators
			.iter()
			.enumerate()
			.find(|(_, k)| k == &&key)
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
	) -> Result<Signed<Payload, RealPayload>, KeystoreError> {
		Signed::sign(&keystore, payload, &self.signing_context, self.index, &self.key).await
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

/// This module reexports Prometheus types and defines the [`Metrics`] trait.
pub mod metrics {
	/// Reexport Prometheus types.
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

	/// Run a job for the parent block indicated
	fn run(
		parent: Hash,
		run_args: Self::RunArgs,
		metrics: Self::Metrics,
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
#[derive(Debug, Error)]
pub enum JobsError<JobError: 'static + std::error::Error> {
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
	pub fn forward_errors(
		&mut self,
		tx: mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>,
	) -> Result<(), Error> {
		if self.errors.is_some() {
			return Err(Error::AlreadyForwarding);
		}
		self.errors = Some(tx);
		Ok(())
	}

	/// Spawn a new job for this `parent_hash`, with whatever args are appropriate.
	fn spawn_job(&mut self, parent_hash: Hash, run_args: Job::RunArgs, metrics: Job::Metrics) -> Result<(), Error> {
		let (to_job_tx, to_job_rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);
		let (from_job_tx, from_job_rx) = mpsc::channel(JOB_CHANNEL_CAPACITY);
		let (finished_tx, finished) = oneshot::channel();

		// clone the error transmitter to move into the future
		let err_tx = self.errors.clone();

		let (future, abort_handle) = future::abortable(async move {
			if let Err(e) = Job::run(parent_hash, run_args, metrics, to_job_rx, from_job_tx).await {
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

		let _ = self.running.insert(parent_hash, handle);

		Ok(())
	}

	/// Stop the job associated with this `parent_hash`.
	pub async fn stop_job(&mut self, parent_hash: Hash) -> Result<(), Error> {
		match self.running.remove(&parent_hash) {
			Some(handle) => {
				let _ = Pin::new(&mut self.outgoing_msgs).remove(handle.outgoing_msgs_handle);
				handle.stop().await;
				Ok(())
			}
			None => Err(Error::JobNotFound(parent_hash)),
		}
	}

	/// Send a message to the appropriate job for this `parent_hash`.
	/// Will not return an error if the job is not running.
	async fn send_msg(&mut self, parent_hash: Hash, msg: Job::ToJob) -> Result<(), Error> {
		match self.running.get_mut(&parent_hash) {
			Some(job) => job.send_msg(msg).await?,
			None => {
				// don't bring down the subsystem, this can happen to due a race condition
			},
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
		let result = self.project().outgoing_msgs.poll_next(cx).map(|opt| {
			opt.and_then(|(stream_yield, _)| match stream_yield {
				StreamYield::Item(msg) => Some(msg),
				StreamYield::Finished(_) => None,
			})
		});
		// we don't want the stream to end if the jobs are empty at some point
		match result {
			task::Poll::Ready(None) => task::Poll::Pending,
			otherwise => otherwise,
		}
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
	metrics: Job::Metrics,
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
	pub fn new(spawner: Spawner, run_args: Job::RunArgs, metrics: Job::Metrics) -> Self {
		Self {
			spawner,
			run_args,
			metrics,
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
	pub fn forward_errors(
		&mut self,
		tx: mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>,
	) -> Result<(), Error> {
		if self.errors.is_some() {
			return Err(Error::AlreadyForwarding);
		}
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
	pub async fn run(
		mut ctx: Context,
		run_args: Job::RunArgs,
		metrics: Job::Metrics,
		spawner: Spawner,
		mut err_tx: Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>,
	) {
		let mut jobs = Jobs::new(spawner.clone());
		if let Some(ref err_tx) = err_tx {
			jobs.forward_errors(err_tx.clone())
				.expect("we never call this twice in this context; qed");
		}

		loop {
			select! {
				incoming = ctx.recv().fuse() => if Self::handle_incoming(incoming, &mut jobs, &run_args, &metrics, &mut err_tx).await { break },
				outgoing = jobs.next().fuse() => Self::handle_outgoing(outgoing, &mut ctx, &mut err_tx).await,
				complete => break,
			}
		}
	}

	// if we have a channel on which to forward errors, do so
	async fn fwd_err(
		hash: Option<Hash>,
		err: JobsError<Job::Error>,
		err_tx: &mut Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>,
	) {
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
		metrics: &Job::Metrics,
		err_tx: &mut Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>,
	) -> bool {
		use polkadot_node_subsystem::ActiveLeavesUpdate;
		use polkadot_node_subsystem::FromOverseer::{Communication, Signal};
		use polkadot_node_subsystem::OverseerSignal::{ActiveLeaves, BlockFinalized, Conclude};

		match incoming {
			Ok(Signal(ActiveLeaves(ActiveLeavesUpdate {
				activated,
				deactivated,
			}))) => {
				for hash in activated {
					let metrics = metrics.clone();
					if let Err(e) = jobs.spawn_job(hash, run_args.clone(), metrics) {
						log::error!("Failed to spawn a job: {:?}", e);
						let e = JobsError::Utility(e);
						Self::fwd_err(Some(hash), e, err_tx).await;
						return true;
					}
				}

				for hash in deactivated {
					if let Err(e) = jobs.stop_job(hash).await {
						log::error!("Failed to stop a job: {:?}", e);
						let e = JobsError::Utility(e);
						Self::fwd_err(Some(hash), e, err_tx).await;
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
				use futures::stream::FuturesUnordered;
				use futures::stream::StreamExt;

				if let Err(e) = jobs
					.running
					.drain()
					.map(|(_, handle)| handle.stop())
					.collect::<FuturesUnordered<_>>()
					.map(Ok)
					.forward(drain())
					.await
				{
					log::error!("failed to stop all jobs on conclude signal: {:?}", e);
					let e = Error::from(e);
					Self::fwd_err(None, JobsError::Utility(e), err_tx).await;
				}

				return true;
			}
			Ok(Communication { msg }) => {
				if let Ok(to_job) = <Job::ToJob>::try_from(msg) {
					match to_job.relay_parent() {
						Some(hash) => {
							if let Err(err) = jobs.send_msg(hash, to_job).await {
								log::error!("Failed to send a message to a job: {:?}", err);
								let e = JobsError::Utility(err);
								Self::fwd_err(Some(hash), e, err_tx).await;
								return true;
							}
						}
						None => {
							if let Err(err) = Job::handle_unanchored_msg(to_job) {
								log::error!("Failed to handle unhashed message: {:?}", err);
								let e = JobsError::Job(err);
								Self::fwd_err(None, e, err_tx).await;
								return true;
							}
						}
					}
				}
			}
			Ok(Signal(BlockFinalized(_))) => {}
			Err(err) => {
				log::error!("error receiving message from subsystem context: {:?}", err);
				let e = JobsError::Utility(Error::from(err));
				Self::fwd_err(None, e, err_tx).await;
				return true;
			}
		}
		false
	}

	// handle an outgoing message.
	async fn handle_outgoing(
		outgoing: Option<Job::FromJob>,
		ctx: &mut Context,
		err_tx: &mut Option<mpsc::Sender<(Option<Hash>, JobsError<Job::Error>)>>,
	) {
		let msg = outgoing.expect("the Jobs stream never ends; qed");
		if let Err(e) = ctx.send_message(msg.into()).await {
			let e = JobsError::Utility(e.into());
			Self::fwd_err(None, e, err_tx).await;
		}
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
	Job::Metrics: Sync,
{
	fn start(self, ctx: Context) -> SpawnedSubsystem {
		let spawner = self.spawner.clone();
		let run_args = self.run_args.clone();
		let metrics = self.metrics.clone();
		let errors = self.errors;

		let future = Box::pin(async move {
			Self::run(ctx, run_args, metrics, spawner, errors).await;
			Ok(())
		});

		SpawnedSubsystem {
			name: Job::NAME.strip_suffix("Job").unwrap_or(Job::NAME),
			future,
		}
	}
}

/// Create a delegated subsystem
///
/// It is possible to create a type which implements `Subsystem` by simply doing:
///
/// ```ignore
/// pub type ExampleSubsystem<Spawner, Context> = JobManager<Spawner, Context, ExampleJob>;
/// ```
///
/// However, doing this requires that job itself and all types which comprise it (i.e. `ToJob`, `FromJob`, `Error`, `RunArgs`)
/// are public, to avoid exposing private types in public interfaces. It's possible to delegate instead, which
/// can reduce the total number of public types exposed, i.e.
///
/// ```ignore
/// type Manager<Spawner, Context> = JobManager<Spawner, Context, ExampleJob>;
/// pub struct ExampleSubsystem {
/// 	manager: Manager<Spawner, Context>,
/// }
///
/// impl<Spawner, Context> Subsystem<Context> for ExampleSubsystem<Spawner, Context> { ... }
/// ```
///
/// This dramatically reduces the number of public types in the crate; the only things which must be public are now
///
/// - `struct ExampleSubsystem` (defined by this macro)
/// - `type ToJob` (because it appears in a trait bound)
/// - `type RunArgs` (because it appears in a function signature)
///
/// Implementing this all manually is of course possible, but it's tedious; why bother? This macro exists for
/// the purpose of doing it automatically:
///
/// ```ignore
/// delegated_subsystem!(ExampleJob(ExampleRunArgs) <- ExampleToJob as ExampleSubsystem);
/// ```
#[macro_export]
macro_rules! delegated_subsystem {
	($job:ident($run_args:ty, $metrics:ty) <- $to_job:ty as $subsystem:ident) => {
		delegated_subsystem!($job($run_args, $metrics) <- $to_job as $subsystem; stringify!($subsystem));
	};

	($job:ident($run_args:ty, $metrics:ty) <- $to_job:ty as $subsystem:ident; $subsystem_name:expr) => {
		#[doc = "Manager type for the "]
		#[doc = $subsystem_name]
		type Manager<Spawner, Context> = $crate::JobManager<Spawner, Context, $job>;

		#[doc = "An implementation of the "]
		#[doc = $subsystem_name]
		pub struct $subsystem<Spawner, Context> {
			manager: Manager<Spawner, Context>,
		}

		impl<Spawner, Context> $subsystem<Spawner, Context>
		where
			Spawner: Clone + $crate::reexports::SpawnNamed + Send + Unpin,
			Context: $crate::reexports::SubsystemContext,
			<Context as $crate::reexports::SubsystemContext>::Message: Into<$to_job>,
		{
			#[doc = "Creates a new "]
			#[doc = $subsystem_name]
			pub fn new(spawner: Spawner, run_args: $run_args, metrics: $metrics) -> Self {
				$subsystem {
					manager: $crate::JobManager::new(spawner, run_args, metrics)
				}
			}

			/// Run this subsystem
			pub async fn run(ctx: Context, run_args: $run_args, metrics: $metrics, spawner: Spawner) {
				<Manager<Spawner, Context>>::run(ctx, run_args, metrics, spawner, None).await
			}
		}

		impl<Spawner, Context> $crate::reexports::Subsystem<Context> for $subsystem<Spawner, Context>
		where
			Spawner: $crate::reexports::SpawnNamed + Send + Clone + Unpin + 'static,
			Context: $crate::reexports::SubsystemContext,
			<Context as $crate::reexports::SubsystemContext>::Message: Into<$to_job>,
		{
			fn start(self, ctx: Context) -> $crate::reexports::SpawnedSubsystem {
				self.manager.start(ctx)
			}
		}
	};
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

#[cfg(test)]
mod tests {
	use super::{Error as UtilError, JobManager, JobTrait, JobsError, TimeoutExt, ToJobTrait};
	use thiserror::Error;
	use polkadot_node_subsystem::{
		messages::{AllMessages, CandidateSelectionMessage},
		ActiveLeavesUpdate, FromOverseer, OverseerSignal, SpawnedSubsystem, Subsystem,
	};
	use assert_matches::assert_matches;
	use futures::{
		channel::mpsc,
		executor,
		stream::{self, StreamExt},
		future, Future, FutureExt, SinkExt,
	};
	use polkadot_primitives::v1::Hash;
	use polkadot_node_subsystem_test_helpers::{self as test_helpers, make_subsystem_context};
	use std::{collections::HashMap, convert::TryFrom, pin::Pin, time::Duration};

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
				_ => Err(()),
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
		Test,
	}

	impl From<FromJob> for AllMessages {
		fn from(from_job: FromJob) -> AllMessages {
			match from_job {
				FromJob::Test => AllMessages::CandidateSelection(CandidateSelectionMessage::default()),
			}
		}
	}

	// Error will mostly be a wrapper to make the try operator more convenient;
	// deriving From implementations for most variants is recommended.
	// It must implement Debug for logging.
	#[derive(Debug, Error)]
	enum Error {
		#[error(transparent)]
		Sending(#[from]mpsc::SendError),
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
		type Metrics = ();

		const NAME: &'static str = "FakeCandidateSelectionJob";

		/// Run a job for the parent block indicated
		//
		// this function is in charge of creating and executing the job's main loop
		fn run(
			parent: Hash,
			mut run_args: Self::RunArgs,
			_metrics: Self::Metrics,
			receiver: mpsc::Receiver<ToJob>,
			mut sender: mpsc::Sender<FromJob>,
		) -> Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>> {
			async move {
				let job = FakeCandidateSelectionJob { receiver };

				// most jobs will have a request-response cycle at the heart of their run loop.
				// however, in this case, we never receive valid messages, so we may as well
				// just send all of our (mock) output messages now
				let mock_output = run_args.remove(&parent).unwrap_or_default();
				let mut stream = stream::iter(mock_output.into_iter().map(Ok));
				sender.send_all(&mut stream).await?;

				// it isn't necessary to break run_loop into its own function,
				// but it's convenient to separate the concerns in this way
				job.run_loop().await
			}
			.boxed()
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
	type FakeCandidateSelectionSubsystem<Spawner, Context> =
		JobManager<Spawner, Context, FakeCandidateSelectionJob>;

	// this type lets us pretend to be the overseer
	type OverseerHandle = test_helpers::TestSubsystemContextHandle<CandidateSelectionMessage>;

	fn test_harness<T: Future<Output = ()>>(
		run_args: HashMap<Hash, Vec<FromJob>>,
		test: impl FnOnce(OverseerHandle, mpsc::Receiver<(Option<Hash>, JobsError<Error>)>) -> T,
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
		let (err_tx, err_rx) = mpsc::channel(16);

		let subsystem = FakeCandidateSelectionSubsystem::run(context, run_args, (), pool, Some(err_tx));
		let test_future = test(overseer_handle, err_rx);

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
		let mut run_args = HashMap::new();
		let _ = run_args.insert(
			relay_parent.clone(),
			vec![FromJob::Test],
		);

		test_harness(run_args, |mut overseer_handle, err_rx| async move {
			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(relay_parent),
				)))
				.await;
			assert_matches!(
				overseer_handle.recv().await,
				AllMessages::CandidateSelection(_)
			);
			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::stop_work(relay_parent),
				)))
				.await;

			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
				.await;

			let errs: Vec<_> = err_rx.collect().await;
			assert_eq!(errs.len(), 0);
		});
	}

	#[test]
	fn stopping_non_running_job_fails() {
		let relay_parent: Hash = [0; 32].into();
		let run_args = HashMap::new();

		test_harness(run_args, |mut overseer_handle, err_rx| async move {
			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::stop_work(relay_parent),
				)))
				.await;

			let errs: Vec<_> = err_rx.collect().await;
			assert_eq!(errs.len(), 1);
			assert_eq!(errs[0].0, Some(relay_parent));
			assert_matches!(
				errs[0].1,
				JobsError::Utility(UtilError::JobNotFound(match_relay_parent)) if relay_parent == match_relay_parent
			);
		});
	}

	#[test]
	fn sending_to_a_non_running_job_do_not_stop_the_subsystem() {
		let relay_parent = Hash::repeat_byte(0x01);
		let mut run_args = HashMap::new();
		let _ = run_args.insert(
			relay_parent.clone(),
			vec![FromJob::Test],
		);

		test_harness(run_args, |mut overseer_handle, err_rx| async move {
			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::ActiveLeaves(
					ActiveLeavesUpdate::start_work(relay_parent),
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
				AllMessages::CandidateSelection(_)
			);

			overseer_handle
				.send(FromOverseer::Signal(OverseerSignal::Conclude))
				.await;

			let errs: Vec<_> = err_rx.collect().await;
			assert_eq!(errs.len(), 0);
		});
	}

	#[test]
	fn test_subsystem_impl_and_name_derivation() {
		let pool = sp_core::testing::TaskExecutor::new();
		let (context, _) = make_subsystem_context::<CandidateSelectionMessage, _>(pool.clone());

		let SpawnedSubsystem { name, .. } =
			FakeCandidateSelectionSubsystem::new(pool, HashMap::new(), ()).start(context);
		assert_eq!(name, "FakeCandidateSelection");
	}
}
