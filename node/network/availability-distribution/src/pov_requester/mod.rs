// Copyright 2021 Parity Technologies (UK) Ltd.
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

//! PoV requester takes care of requesting PoVs from validators of a backing group.

use futures::{channel::oneshot, future::BoxFuture, FutureExt};

use polkadot_node_network_protocol::request_response::{
	outgoing::{RequestError, Requests},
	v1::{PoVFetchingRequest, PoVFetchingResponse},
	OutgoingRequest, Recipient,
};
use polkadot_node_primitives::PoV;
use polkadot_node_subsystem::{
	jaeger,
	messages::{IfDisconnected, NetworkBridgeTxMessage},
	overseer,
};
use polkadot_node_subsystem_util::runtime::RuntimeInfo;
use polkadot_primitives::v2::{
	AuthorityDiscoveryId, CandidateHash, Hash, Id as ParaId, ValidatorIndex,
};

use crate::{
	error::{Error, FatalError, JfyiError, Result},
	metrics::{FAILED, NOT_FOUND, SUCCEEDED},
	Metrics, LOG_TARGET,
};

/// Start background worker for taking care of fetching the requested `PoV` from the network.
#[overseer::contextbounds(AvailabilityDistribution, prefix = self::overseer)]
pub async fn fetch_pov<Context>(
	ctx: &mut Context,
	runtime: &mut RuntimeInfo,
	parent: Hash,
	from_validator: ValidatorIndex,
	para_id: ParaId,
	candidate_hash: CandidateHash,
	pov_hash: Hash,
	tx: oneshot::Sender<PoV>,
	metrics: Metrics,
) -> Result<()> {
	let info = &runtime.get_session_info(ctx.sender(), parent).await?.session_info;
	let authority_id = info
		.discovery_keys
		.get(from_validator.0 as usize)
		.ok_or(JfyiError::InvalidValidatorIndex)?
		.clone();
	let (req, pending_response) = OutgoingRequest::new(
		Recipient::Authority(authority_id.clone()),
		PoVFetchingRequest { candidate_hash },
	);
	let full_req = Requests::PoVFetchingV1(req);

	ctx.send_message(NetworkBridgeTxMessage::SendRequests(
		vec![full_req],
		IfDisconnected::ImmediateError,
	))
	.await;

	let span = jaeger::Span::new(candidate_hash, "fetch-pov")
		.with_validator_index(from_validator)
		.with_relay_parent(parent)
		.with_para_id(para_id);
	ctx.spawn(
		"pov-fetcher",
		fetch_pov_job(para_id, pov_hash, authority_id, pending_response.boxed(), span, tx, metrics)
			.boxed(),
	)
	.map_err(|e| FatalError::SpawnTask(e))?;
	Ok(())
}

/// Future to be spawned for taking care of handling reception and sending of PoV.
async fn fetch_pov_job(
	para_id: ParaId,
	pov_hash: Hash,
	authority_id: AuthorityDiscoveryId,
	pending_response: BoxFuture<'static, std::result::Result<PoVFetchingResponse, RequestError>>,
	span: jaeger::Span,
	tx: oneshot::Sender<PoV>,
	metrics: Metrics,
) {
	if let Err(err) = do_fetch_pov(pov_hash, pending_response, span, tx, metrics).await {
		gum::warn!(target: LOG_TARGET, ?err, ?para_id, ?pov_hash, ?authority_id, "fetch_pov_job");
	}
}

/// Do the actual work of waiting for the response.
async fn do_fetch_pov(
	pov_hash: Hash,
	pending_response: BoxFuture<'static, std::result::Result<PoVFetchingResponse, RequestError>>,
	_span: jaeger::Span,
	tx: oneshot::Sender<PoV>,
	metrics: Metrics,
) -> Result<()> {
	let response = pending_response.await.map_err(Error::FetchPoV);
	let pov = match response {
		Ok(PoVFetchingResponse::PoV(pov)) => pov,
		Ok(PoVFetchingResponse::NoSuchPoV) => {
			metrics.on_fetched_pov(NOT_FOUND);
			return Err(Error::NoSuchPoV)
		},
		Err(err) => {
			metrics.on_fetched_pov(FAILED);
			return Err(err)
		},
	};
	if pov.hash() == pov_hash {
		metrics.on_fetched_pov(SUCCEEDED);
		tx.send(pov).map_err(|_| Error::SendResponse)
	} else {
		metrics.on_fetched_pov(FAILED);
		Err(Error::UnexpectedPoV)
	}
}

#[cfg(test)]
mod tests {
	use assert_matches::assert_matches;
	use futures::{executor, future};

	use parity_scale_codec::Encode;
	use sp_core::testing::TaskExecutor;

	use polkadot_node_primitives::BlockData;
	use polkadot_node_subsystem::messages::{
		AllMessages, AvailabilityDistributionMessage, RuntimeApiMessage, RuntimeApiRequest,
	};
	use polkadot_node_subsystem_test_helpers as test_helpers;
	use polkadot_primitives::v2::{CandidateHash, Hash, ValidatorIndex};
	use test_helpers::mock::make_ferdie_keystore;

	use super::*;
	use crate::{tests::mock::make_session_info, LOG_TARGET};

	#[test]
	fn rejects_invalid_pov() {
		sp_tracing::try_init_simple();
		let pov = PoV { block_data: BlockData(vec![1, 2, 3, 4, 5, 6]) };
		test_run(Hash::default(), pov);
	}

	#[test]
	fn accepts_valid_pov() {
		sp_tracing::try_init_simple();
		let pov = PoV { block_data: BlockData(vec![1, 2, 3, 4, 5, 6]) };
		test_run(pov.hash(), pov);
	}

	fn test_run(pov_hash: Hash, pov: PoV) {
		let pool = TaskExecutor::new();
		let (mut context, mut virtual_overseer) = test_helpers::make_subsystem_context::<
			AvailabilityDistributionMessage,
			TaskExecutor,
		>(pool.clone());
		let keystore = make_ferdie_keystore();
		let mut runtime = polkadot_node_subsystem_util::runtime::RuntimeInfo::new(Some(keystore));

		let (tx, rx) = oneshot::channel();
		let testee = async {
			fetch_pov(
				&mut context,
				&mut runtime,
				Hash::default(),
				ValidatorIndex(0),
				ParaId::default(),
				CandidateHash::default(),
				pov_hash,
				tx,
				Metrics::new_dummy(),
			)
			.await
			.expect("Should succeed");
		};

		let tester = async move {
			loop {
				match virtual_overseer.recv().await {
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						_,
						RuntimeApiRequest::SessionIndexForChild(tx),
					)) => {
						tx.send(Ok(0)).unwrap();
					},
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						_,
						RuntimeApiRequest::SessionInfo(_, tx),
					)) => {
						tx.send(Ok(Some(make_session_info()))).unwrap();
					},
					AllMessages::NetworkBridgeTx(NetworkBridgeTxMessage::SendRequests(
						mut reqs,
						_,
					)) => {
						let req = assert_matches!(
							reqs.pop(),
							Some(Requests::PoVFetchingV1(outgoing)) => {outgoing}
						);
						req.pending_response
							.send(Ok(PoVFetchingResponse::PoV(pov.clone()).encode()))
							.unwrap();
						break
					},
					msg => gum::debug!(target: LOG_TARGET, msg = ?msg, "Received msg"),
				}
			}
			if pov.hash() == pov_hash {
				assert_eq!(rx.await, Ok(pov));
			} else {
				assert_eq!(rx.await, Err(oneshot::Canceled));
			}
		};
		futures::pin_mut!(testee);
		futures::pin_mut!(tester);
		executor::block_on(future::join(testee, tester));
	}
}
