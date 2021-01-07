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
//! This crate also reexports Prometheus metric types which are expected to be implemented by subsystems.

#![warn(missing_docs)]

use pnu_subsystem::{
	errors::RuntimeApiError,
	messages::{AllMessages, RuntimeApiMessage, RuntimeApiRequest, RuntimeApiSender},
	SubsystemContext,
};
use futures::{channel::{mpsc, oneshot}, prelude::*};
use futures_timer::Delay;
use parity_scale_codec::Encode;
use pin_project::pin_project;
use pdot_primitives::v1::{
	CandidateEvent, CommittedCandidateReceipt, CoreState, EncodeAs, PersistedValidationData,
	GroupRotationInfo, Hash, Id as ParaId, OccupiedCoreAssumption,
	SessionIndex, Signed, SigningContext, ValidationCode, ValidatorId, ValidatorIndex, SessionInfo,
};
use sp_core::Public;
use sp_application_crypto::AppKey;
use sp_keystore::{CryptoStore, SyncCryptoStorePtr, Error as KeystoreError};
use std::{
	convert::TryInto,
	pin::Pin,
	task::{Poll, Context},
	time::Duration,
};
use thiserror::Error;

pub use pnu_metered_channel as metered;

/// Utility errors
#[derive(Debug, Error)]
pub enum Error {
	/// Attempted to send or receive on a oneshot channel which had been canceled
	#[error(transparent)]
	Oneshot(#[from] oneshot::Canceled),
	/// Attempted to send on a MPSC channel which has been canceled
	#[error(transparent)]
	Mpsc(#[from] mpsc::SendError),
	/// An error in the Runtime API.
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),
	/// Attempted to convert from an AllMessages to a FromJob, and failed.
	#[error("AllMessage not relevant to Job")]
	SenderConversion(String),
	/// The local node is not a validator.
	#[error("Node is not a validator")]
	NotAValidator,
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
	FromJob: From<AllMessages>,
{
	let (tx, rx) = oneshot::channel();

	sender.send(AllMessages::RuntimeApi(RuntimeApiMessage::Request(parent, request_builder(tx))).into()).await?;

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
			FromJob: From<AllMessages>,
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
	fn request_persisted_validation_data(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<PersistedValidationData>; PersistedValidationData;
	fn request_session_index_for_child() -> SessionIndex; SessionIndexForChild;
	fn request_validation_code(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<ValidationCode>; ValidationCode;
	fn request_candidate_pending_availability(para_id: ParaId) -> Option<CommittedCandidateReceipt>; CandidatePendingAvailability;
	fn request_candidate_events() -> Vec<CandidateEvent>; CandidateEvents;
	fn request_session_info(index: SessionIndex) -> Option<SessionInfo>; SessionInfo;
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

	ctx.send_message(
		AllMessages::RuntimeApi(RuntimeApiMessage::Request(parent, request_builder(tx)))
			.try_into()
			.map_err(|err| Error::SenderConversion(format!("{:?}", err)))?,
	).await;

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
	fn request_persisted_validation_data_ctx(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<PersistedValidationData>; PersistedValidationData;
	fn request_session_index_for_child_ctx() -> SessionIndex; SessionIndexForChild;
	fn request_validation_code_ctx(para_id: ParaId, assumption: OccupiedCoreAssumption) -> Option<ValidationCode>; ValidationCode;
	fn request_candidate_pending_availability_ctx(para_id: ParaId) -> Option<CommittedCandidateReceipt>; CandidatePendingAvailability;
	fn request_candidate_events_ctx() -> Vec<CandidateEvent>; CandidateEvents;
	fn request_session_info_ctx(index: SessionIndex) -> Option<SessionInfo>; SessionInfo;
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
#[derive(Debug)]
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
		FromJob: From<AllMessages>,
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
