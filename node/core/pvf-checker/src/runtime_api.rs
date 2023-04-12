// Copyright (C) Parity Technologies (UK) Ltd.
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

use crate::LOG_TARGET;
use futures::channel::oneshot;
use polkadot_node_subsystem::{
	errors::RuntimeApiError as RuntimeApiSubsystemError,
	messages::{RuntimeApiMessage, RuntimeApiRequest},
	SubsystemSender,
};
use polkadot_primitives::{
	Hash, PvfCheckStatement, SessionIndex, ValidationCodeHash, ValidatorId, ValidatorSignature,
};

pub(crate) async fn session_index_for_child(
	sender: &mut impl SubsystemSender<RuntimeApiMessage>,
	relay_parent: Hash,
) -> Result<SessionIndex, RuntimeRequestError> {
	let (tx, rx) = oneshot::channel();
	runtime_api_request(sender, relay_parent, RuntimeApiRequest::SessionIndexForChild(tx), rx).await
}

pub(crate) async fn validators(
	sender: &mut impl SubsystemSender<RuntimeApiMessage>,
	relay_parent: Hash,
) -> Result<Vec<ValidatorId>, RuntimeRequestError> {
	let (tx, rx) = oneshot::channel();
	runtime_api_request(sender, relay_parent, RuntimeApiRequest::Validators(tx), rx).await
}

pub(crate) async fn submit_pvf_check_statement(
	sender: &mut impl SubsystemSender<RuntimeApiMessage>,
	relay_parent: Hash,
	stmt: PvfCheckStatement,
	signature: ValidatorSignature,
) -> Result<(), RuntimeRequestError> {
	let (tx, rx) = oneshot::channel();
	runtime_api_request(
		sender,
		relay_parent,
		RuntimeApiRequest::SubmitPvfCheckStatement(stmt, signature, tx),
		rx,
	)
	.await
}

pub(crate) async fn pvfs_require_precheck(
	sender: &mut impl SubsystemSender<RuntimeApiMessage>,
	relay_parent: Hash,
) -> Result<Vec<ValidationCodeHash>, RuntimeRequestError> {
	let (tx, rx) = oneshot::channel();
	runtime_api_request(sender, relay_parent, RuntimeApiRequest::PvfsRequirePrecheck(tx), rx).await
}

#[derive(Debug)]
pub(crate) enum RuntimeRequestError {
	NotSupported,
	ApiError,
	CommunicationError,
}

pub(crate) async fn runtime_api_request<T>(
	sender: &mut impl SubsystemSender<RuntimeApiMessage>,
	relay_parent: Hash,
	request: RuntimeApiRequest,
	receiver: oneshot::Receiver<Result<T, RuntimeApiSubsystemError>>,
) -> Result<T, RuntimeRequestError> {
	sender
		.send_message(RuntimeApiMessage::Request(relay_parent, request).into())
		.await;

	receiver
		.await
		.map_err(|_| {
			gum::debug!(target: LOG_TARGET, ?relay_parent, "Runtime API request dropped");
			RuntimeRequestError::CommunicationError
		})
		.and_then(|res| {
			res.map_err(|e| {
				use RuntimeApiSubsystemError::*;
				match e {
					Execution { .. } => {
						gum::debug!(
							target: LOG_TARGET,
							?relay_parent,
							err = ?e,
							"Runtime API request internal error"
						);
						RuntimeRequestError::ApiError
					},
					NotSupported { .. } => RuntimeRequestError::NotSupported,
				}
			})
		})
}
