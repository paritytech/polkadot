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
use polkadot_primitives::{
	parachain::{
		GlobalValidationSchedule, HeadData, Id as ParaId, LocalValidationData, SigningContext,
		ValidatorId,
	},
	Hash,
};
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
