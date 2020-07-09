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
	future::AbortHandle,
	prelude::*,
	task::SpawnError,
};
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
	SenderConversion(String),
	/// The local node is not a validator.
	NotAValidator,
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
