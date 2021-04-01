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

use futures::{FutureExt, channel::{mpsc, oneshot}, future::BoxFuture};
use lru::LruCache;

use polkadot_subsystem::jaeger;
use polkadot_node_network_protocol::{
	PeerId, peer_set::PeerSet,
	request_response::{OutgoingRequest, Recipient, request::{RequestError, Requests},
	v1::{PoVFetchingRequest, PoVFetchingResponse}}
};
use polkadot_primitives::v1::{
	AuthorityDiscoveryId, CandidateHash, Hash, SessionIndex, ValidatorIndex
};
use polkadot_node_primitives::PoV;
use polkadot_subsystem::{
	ActiveLeavesUpdate, SubsystemContext, ActivatedLeaf,
	messages::{AllMessages, NetworkBridgeMessage, IfDisconnected}
};

use crate::{error::{Error, log_error}, runtime::{Runtime, ValidatorInfo}};

/// Number of sessions we want to keep in the LRU.
const NUM_SESSIONS: usize = 2;

pub struct PoVRequester {
	/// We only ever care about being connected to validators of at most two sessions.
	///
	/// So we keep an LRU for managing connection requests of size 2.
	/// Cache will contain `None` if we are not a validator in that session.
	connected_validators: LruCache<SessionIndex, Option<mpsc::Receiver<(AuthorityDiscoveryId, PeerId)>>>,
}

impl PoVRequester {
	/// Create a new requester for PoVs.
	pub fn new() -> Self {
		Self {
			connected_validators: LruCache::new(NUM_SESSIONS),
		}
	}

	/// Make sure we are connected to the right set of validators.
	///
	/// On every `ActiveLeavesUpdate`, we check whether we are connected properly to our current
	/// validator group.
	pub async fn update_connected_validators<Context>(
		&mut self,
		ctx: &mut Context,
		runtime: &mut Runtime,
		update: &ActiveLeavesUpdate,
	) -> super::Result<()>
	where
		Context: SubsystemContext,
	{
		let activated = update.activated.iter().map(|ActivatedLeaf { hash: h, .. }| h);
		let activated_sessions =
			get_activated_sessions(ctx, runtime, activated).await?;

		for (parent, session_index) in activated_sessions {
			if self.connected_validators.contains(&session_index) {
				continue
			}
			let rx = connect_to_relevant_validators(ctx, runtime, parent, session_index).await?;
			self.connected_validators.put(session_index, rx);
		}
		Ok(())
	}

	/// Start background worker for taking care of fetching the requested `PoV` from the network.
	pub async fn fetch_pov<Context>(
		&self,
		ctx: &mut Context,
		runtime: &mut Runtime,
		parent: Hash,
		from_validator: ValidatorIndex,
		candidate_hash: CandidateHash,
		pov_hash: Hash,
		tx: oneshot::Sender<PoV>
	) -> super::Result<()>
	where
		Context: SubsystemContext,
	{
		let info = &runtime.get_session_info(ctx, parent).await?.session_info;
		let authority_id = info.discovery_keys.get(from_validator.0 as usize)
			.ok_or(Error::InvalidValidatorIndex)?
			.clone();
		let (req, pending_response) = OutgoingRequest::new(
			Recipient::Authority(authority_id),
			PoVFetchingRequest {
				candidate_hash,
			},
		);
		let full_req = Requests::PoVFetching(req);

		ctx.send_message(
			AllMessages::NetworkBridge(
				NetworkBridgeMessage::SendRequests(
					vec![full_req],
					// We are supposed to be connected to validators of our group via `PeerSet`,
					// but at session boundaries that is kind of racy, in case a connection takes
					// longer to get established, so we try to connect in any case.
					IfDisconnected::TryConnect
				)
		)).await;

		let span = jaeger::Span::new(candidate_hash, "fetch-pov")
			.with_validator_index(from_validator)
			.with_relay_parent(parent);
		ctx.spawn("pov-fetcher", fetch_pov_job(pov_hash, pending_response.boxed(), span, tx).boxed())
			.await
			.map_err(|e| Error::SpawnTask(e))
	}
}

/// Future to be spawned for taking care of handling reception and sending of PoV.
async fn fetch_pov_job(
	pov_hash: Hash,
	pending_response: BoxFuture<'static, Result<PoVFetchingResponse, RequestError>>,
	span: jaeger::Span,
	tx: oneshot::Sender<PoV>,
) {
	log_error(
		do_fetch_pov(pov_hash, pending_response, span, tx).await,
		"fetch_pov_job",
	)
}

/// Do the actual work of waiting for the response.
async fn do_fetch_pov(
	pov_hash: Hash,
	pending_response: BoxFuture<'static, Result<PoVFetchingResponse, RequestError>>,
	_span: jaeger::Span,
	tx: oneshot::Sender<PoV>,
)
	-> super::Result<()>
{
	let response = pending_response.await.map_err(Error::FetchPoV)?;
	let pov = match response {
		PoVFetchingResponse::PoV(compressed) => {
			compressed.decompress().map_err(Error::PoVDecompression)?
		}
		PoVFetchingResponse::NoSuchPoV => {
			return Err(Error::NoSuchPoV)
		}
	};
	if pov.hash() == pov_hash {
		tx.send(pov).map_err(|_| Error::SendResponse)
	} else {
		Err(Error::UnexpectedPoV)
	}
}

/// Get the session indeces for the given relay chain parents.
async fn get_activated_sessions<Context>(ctx: &mut Context, runtime: &mut Runtime, new_heads: impl Iterator<Item = &Hash>)
	-> super::Result<impl Iterator<Item = (Hash, SessionIndex)>>
where
	Context: SubsystemContext,
{
	let mut sessions = Vec::new();
	for parent in new_heads {
		sessions.push((*parent, runtime.get_session_index(ctx, *parent).await?));
	}
	Ok(sessions.into_iter())
}

/// Connect to validators of our validator group.
async fn connect_to_relevant_validators<Context>(
	ctx: &mut Context,
	runtime: &mut Runtime,
	parent: Hash,
	session: SessionIndex
)
	-> super::Result<Option<mpsc::Receiver<(AuthorityDiscoveryId, PeerId)>>>
where
	Context: SubsystemContext,
{
	if let Some(validator_ids) = determine_relevant_validators(ctx, runtime, parent, session).await? {
		// We don't actually care about `PeerId`s, just keeping receiver so we stay connected:
		let (tx, rx) = mpsc::channel(0);
		ctx.send_message(AllMessages::NetworkBridge(NetworkBridgeMessage::ConnectToValidators {
			validator_ids, peer_set: PeerSet::Validation, connected: tx
		})).await;
		Ok(Some(rx))
	} else {
		Ok(None)
	}
}

/// Get the validators in our validator group.
///
/// Return: `None` if not a validator.
async fn determine_relevant_validators<Context>(
	ctx: &mut Context,
	runtime: &mut Runtime,
	parent: Hash,
	session: SessionIndex,
)
	-> super::Result<Option<Vec<AuthorityDiscoveryId>>>
where
	Context: SubsystemContext,
{
	let info = runtime.get_session_info_by_index(ctx, parent, session).await?;
	if let ValidatorInfo {
			our_index: Some(our_index),
			our_group: Some(our_group)
	} = &info.validator_info {

		let indeces = info.session_info.validator_groups.get(our_group.0 as usize)
			.expect("Our group got retrieved from that session info, it must exist. qed.")
			.clone();
		Ok(Some(
			indeces.into_iter()
			   .filter(|i| *i != *our_index)
			   .map(|i| info.session_info.discovery_keys[i.0 as usize].clone())
			   .collect()
	   ))
	} else {
		Ok(None)
	}
}

#[cfg(test)]
mod tests {
	use assert_matches::assert_matches;
	use futures::{executor, future};

	use parity_scale_codec::Encode;
	use sp_core::testing::TaskExecutor;

	use polkadot_primitives::v1::{CandidateHash, Hash, ValidatorIndex};
	use polkadot_node_primitives::{BlockData, CompressedPoV};
	use polkadot_subsystem_testhelpers as test_helpers;
	use polkadot_subsystem::messages::{AvailabilityDistributionMessage, RuntimeApiMessage, RuntimeApiRequest};

	use super::*;
	use crate::LOG_TARGET;
	use crate::tests::mock::{make_session_info, make_ferdie_keystore};

	#[test]
	fn rejects_invalid_pov() {
		sp_tracing::try_init_simple();
		let pov = PoV {
			block_data: BlockData(vec![1,2,3,4,5,6]),
		};
		test_run(Hash::default(), pov);
	}

	#[test]
	fn accepts_valid_pov() {
		sp_tracing::try_init_simple();
		let pov = PoV {
			block_data: BlockData(vec![1,2,3,4,5,6]),
		};
		test_run(pov.hash(), pov);
	}

	fn test_run(pov_hash: Hash, pov: PoV) {
		let requester = PoVRequester::new();
		let pool = TaskExecutor::new();
		let (mut context, mut virtual_overseer) =
			test_helpers::make_subsystem_context::<AvailabilityDistributionMessage, TaskExecutor>(pool.clone());
		let keystore = make_ferdie_keystore();
		let mut runtime = crate::runtime::Runtime::new(keystore);

		let (tx, rx) = oneshot::channel();
		let testee = async {
			requester.fetch_pov(
				&mut context,
				&mut runtime,
				Hash::default(),
				ValidatorIndex(0),
				CandidateHash::default(),
				pov_hash,
				tx,
			).await.expect("Should succeed");
		};

		let tester = async move {
			loop {
				match virtual_overseer.recv().await {
					AllMessages::RuntimeApi(
						RuntimeApiMessage::Request(
							_,
							RuntimeApiRequest::SessionIndexForChild(tx)
							)
						) => {
						tx.send(Ok(0)).unwrap();
					}
					AllMessages::RuntimeApi(
						RuntimeApiMessage::Request(
							_,
							RuntimeApiRequest::SessionInfo(_, tx)
							)
						) => {
						tx.send(Ok(Some(make_session_info()))).unwrap();
					}
					AllMessages::NetworkBridge(NetworkBridgeMessage::SendRequests(mut reqs, _)) => {
						let req = assert_matches!(
							reqs.pop(),
							Some(Requests::PoVFetching(outgoing)) => {outgoing}
						);
						req.pending_response.send(Ok(PoVFetchingResponse::PoV(
							CompressedPoV::compress(&pov).unwrap()).encode()
						)).unwrap();
						break
					},
					msg => tracing::debug!(target: LOG_TARGET, msg = ?msg, "Received msg"),
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
