// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

//! Polkadot-specific base networking protocol.
//!
//! This is implemented using the sc-network APIs for futures-based
//! notifications protocols. In some cases, we emulate request/response on top
//! of the notifications machinery, which is slightly less efficient but not
//! meaningfully so.
//!
//! We handle events from `sc-network` in a thin wrapper that forwards to a
//! background worker which also handles commands from other parts of the node.

use codec::{Decode, Encode};
use futures::channel::{mpsc, oneshot};
use futures::future::Either;
use futures::prelude::*;
use futures::task::{Spawn, SpawnExt};
use log::{debug, trace};

use av_store::Store as AvailabilityStore;
use polkadot_primitives::{
	Hash, Block,
	parachain::{
		PoVBlock, ValidatorId, ValidatorIndex, Collation, CandidateReceipt, OutgoingMessages,
		ErasureChunk, ParachainHost,
	},
};
use polkadot_validation::{
	SharedTable, TableRouter, Network as ParachainNetwork, Validated, GenericStatement,
};
use sc_network::{config::Roles, Event, PeerId, RequestId};
use sp_api::ProvideRuntimeApi;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Weak};

use super::{cost, benefit, PolkadotNetworkService};

/// The current protocol version.
pub const VERSION: u32 = 1;
/// The minimum supported protocol version.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// The engine ID of the polkadot network protocol.
pub const POLKADOT_ENGINE_ID: sp_runtime::ConsensusEngineId = *b"dot2";

pub use crate::legacy::gossip::ChainContext;

/// The role of the collator. Whether they're the primary or backup for this parachain.
// TODO [now]: extract CollatorPool to here.
#[derive(PartialEq, Debug, Clone, Copy, Encode, Decode)]
pub enum Role {
	/// Primary collators should send collations whenever it's time.
	Primary = 0,
	/// Backup collators should not.
	Backup = 1,
}

// Messages from the service API or network adapter.
enum ServiceToWorkerMsg {
	// basic peer messages.
	PeerConnected(PeerId, Roles),
	PeerMessage(PeerId, Vec<bytes::Bytes>),
	PeerDisconnected(PeerId),

	// service messages.
	BuildConsensusNetworking(Arc<SharedTable>, Vec<ValidatorId>, oneshot::Sender<Router>),
	DropConsensusNetworking(Hash),
	LocalCollation(
		Hash, // relay-parent
		Collation,
		CandidateReceipt,
		(ValidatorIndex, Vec<ErasureChunk>),
	),
	FetchPoVBlock(
		Hash, // relay-parent
		CandidateReceipt,
		oneshot::Sender<PoVBlock>,
	),
}

/// An async handle to the network service.
#[derive(Clone)]
pub struct Service {
	sender: mpsc::Sender<ServiceToWorkerMsg>,
	network_service: Arc<PolkadotNetworkService>,
}

/// Registers the protocol.
///
/// You are very strongly encouraged to call this method very early on. Any connection open
/// will retain the protocols that were registered then, and not any new one.
pub fn start<C, Api, SP>(
	service: Arc<PolkadotNetworkService>,
	availability_store: AvailabilityStore,
	chain_context: C,
	api: Arc<Api>,
	executor: SP,
) -> Result<Service, futures::task::SpawnError> where
	C: ChainContext + 'static,
	Api: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	Api::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	SP: Spawn + Clone + Send + 'static,
{
	const SERVICE_TO_WORKER_BUF: usize = 256;

	let mut event_stream = service.event_stream();
	let (mut worker_sender, worker_receiver) = mpsc::channel(SERVICE_TO_WORKER_BUF);

	let gossip_validator = crate::legacy::gossip::register_validator(
		service.clone(),
		chain_context,
		&executor,
	);
	executor.spawn(worker_loop(
		service.clone(),
		availability_store,
		gossip_validator,
		worker_sender.clone(),
		api,
		worker_receiver,
		executor.clone(),
	))?;

	let polkadot_service = Service {
		sender: worker_sender.clone(),
		network_service: service.clone(),
	};

	executor.spawn(async move {
		while let Some(event) = event_stream.next().await {
			let res = match event {
				Event::Dht(_) => continue,
				Event::NotificationStreamOpened {
					remote,
					engine_id,
					roles,
				} => {
					if engine_id != POLKADOT_ENGINE_ID { continue }

					worker_sender.send(ServiceToWorkerMsg::PeerConnected(remote, roles)).await
				},
				Event::NotificationsStreamClosed {
					remote,
					engine_id,
				} => {
					if engine_id != POLKADOT_ENGINE_ID { continue }

					worker_sender.send(ServiceToWorkerMsg::PeerDisconnected(remote)).await
				},
				Event::NotificationsReceived {
					remote,
					messages,
				} => {
					let our_notifications = messages.into_iter()
						.filter_map(|(engine, message)| if engine == POLKADOT_ENGINE_ID {
							Some(message)
						} else {
							None
						})
						.collect();

					worker_sender.send(
						ServiceToWorkerMsg::PeerMessage(remote, our_notifications)
					).await
				}
			};

			if let Err(e) = res {
				// full is impossible here, as we've `await`ed the value being sent.
				if e.is_disconnected() {
					break
				}
			}
		}
	})?;

	Ok(polkadot_service)
}

/// Polkadot-specific messages.
#[derive(Debug, Encode, Decode)]
pub enum Message {
	/// As a validator, tell the peer your current session key.
	// TODO: do this with a cryptographic proof of some kind
	// https://github.com/paritytech/polkadot/issues/47
	ValidatorId(ValidatorId),
	/// Requesting parachain proof-of-validation block (relay_parent, candidate_hash).
	RequestPovBlock(RequestId, Hash, Hash),
	/// Provide requested proof-of-validation block data by candidate hash or nothing if unknown.
	PovBlock(RequestId, Option<PoVBlock>),
	/// Tell a collator their role.
	CollatorRole(Role),
	/// A collation provided by a peer. Relay parent and collation.
	Collation(Hash, Collation),
}

struct PeerData {
	roles: Roles,
}

struct ConsensusNetworkingInstance {
	statement_table: Arc<SharedTable>,
	relay_parent: Hash,
	attestation_topic: Hash,
	exit: exit_future::Exit,
	_drop_signal: exit_future::Signal,
}

type RegisteredMessageValidator = crate::legacy::gossip::RegisteredMessageValidator<crate::PolkadotProtocol>;

async fn worker_loop<Api, Sp>(
	service: Arc<PolkadotNetworkService>,
	availability_store: AvailabilityStore,
	gossip_handle: RegisteredMessageValidator,
	sender: mpsc::Sender<ServiceToWorkerMsg>,
	api: Arc<Api>,
	mut receiver: mpsc::Receiver<ServiceToWorkerMsg>,
	executor: Sp,
) where
	Api: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	Api::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	Sp: Spawn + Clone + Send + 'static,
{
	let mut peers = HashMap::new();
	let mut consensus_instances = HashMap::new();

	while let Some(message) = receiver.next().await {
		match message {
			ServiceToWorkerMsg::PeerConnected(remote, roles) => {
				peers.insert(remote, PeerData { roles });
			}
			ServiceToWorkerMsg::PeerDisconnected(remote) => {
				peers.remove(&remote);
			}
			ServiceToWorkerMsg::PeerMessage(remote, messages) => {
				for raw_message in messages {
					match Message::decode(&mut raw_message.as_ref()) {
						Ok(message) => {
							service.report_peer(remote.clone(), benefit::VALID_FORMAT);
							unimplemented!()
						},
						Err(_) => service.report_peer(remote.clone(), cost::INVALID_FORMAT),
					}
				}
			}

			ServiceToWorkerMsg::BuildConsensusNetworking(table, authorities, router_sender) => {
				// glue: let gossip know about our new local leaf.
				let relay_parent = table.consensus_parent_hash().clone();
				let (signal, exit) = exit_future::signal();

				let router = Router {
					inner: Arc::new(RouterInner { relay_parent, sender: sender.clone() }),
				};

				let new_leaf_actions = gossip_handle.new_local_leaf(
					relay_parent,
					crate::legacy::gossip::MessageValidationData { authorities },
					|queue_root| availability_store.queue_by_root(queue_root),
				);

				new_leaf_actions.perform(&gossip_handle);

				let instance = ConsensusNetworkingInstance {
					statement_table: table,
					relay_parent,
					attestation_topic: crate::legacy::router::attestation_topic(relay_parent),
					exit: exit.clone(),
					_drop_signal: signal,
				};

				let weak_router = Arc::downgrade(&router.inner);

				consensus_instances.insert(relay_parent, instance);
				let consensus_instance = consensus_instances.get(&relay_parent)
					.expect("just inserted into map; has not been removed; qed");

				// glue the incoming messages, shared table, and validation
				// work together.
				let _ = executor.spawn(statement_import_loop(
					relay_parent,
					consensus_instance.statement_table.clone(),
					api.clone(),
					weak_router,
					gossip_handle.clone(),
					exit,
					executor.clone(),
				));

			}
			ServiceToWorkerMsg::DropConsensusNetworking(relay_parent) => {
				consensus_instances.remove(&relay_parent);
			}
			ServiceToWorkerMsg::LocalCollation(relay_parent, collation, receipt, chunks) => {
				let instance = match consensus_instances.get(&relay_parent) {
					None => continue,
					Some(instance) => instance,
				};

				distribute_local_collation(
					instance,
					collation,
					receipt,
					chunks,
					&gossip_handle,
				);
			}
			ServiceToWorkerMsg::FetchPoVBlock(relay_parent, candidate, sender) => {
				// create a filter on gossip for it and send to sender.
			}
		}
	}
}

// the internal loop of waiting for messages and spawning validation work
// as a result of those messages.
async fn statement_import_loop<Api>(
	relay_parent: Hash,
	table: Arc<SharedTable>,
	api: Arc<Api>,
	temp_router: Weak<RouterInner>,
	validator: RegisteredMessageValidator,
	mut exit: exit_future::Exit,
	executor: impl Spawn,
) where
	Api: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	Api::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
{
	let topic = crate::legacy::router::attestation_topic(relay_parent);
	let mut checked_messages = validator.gossip_messages_for(topic)
		.filter_map(|msg| match msg.0 {
			crate::legacy::gossip::GossipMessage::Statement(s) => future::ready(Some(s.signed_statement)),
			_ => future::ready(None),
		});

	let mut deferred_statements = crate::legacy::router::DeferredStatements::new();

	loop {
		let statement = match future::select(exit, checked_messages.next()).await {
			Either::Left(_) | Either::Right((None, _)) => return,
			Either::Right((Some(statement), e)) => {
				exit = e;
				statement
			}
		};

		// defer any statements for which we haven't imported the candidate yet
		let c_hash = {
			let candidate_data = match statement.statement {
				GenericStatement::Candidate(ref c) => Some(c.hash()),
				GenericStatement::Valid(ref hash)
					| GenericStatement::Invalid(ref hash)
					=> table.with_candidate(hash, |c| c.map(|_| *hash)),
			};
			match candidate_data {
				Some(x) => x,
				None => {
					deferred_statements.push(statement);
					continue;
				}
			}
		};

		// import all statements pending on this candidate
		let (mut statements, _traces) = if let GenericStatement::Candidate(_) = statement.statement {
			deferred_statements.take_deferred(&c_hash)
		} else {
			(Vec::new(), Vec::new())
		};

		// prepend the candidate statement.
		debug!(target: "validation", "Importing statements about candidate {:?}", c_hash);
		statements.insert(0, statement);

		let producers: Vec<_> = {
			// create a temporary router handle for importing all of these statements
			let temp_router = match temp_router.upgrade() {
				None => break,
				Some(inner) => Router { inner },
			};

			table.import_remote_statements(
				&temp_router,
				statements.iter().cloned(),
			)
		};

		// dispatch future work as necessary.
		for (producer, statement) in producers.into_iter().zip(statements) {
			if let Some(sender) = table.index_to_id(statement.sender) {
				if let Some(producer) = producer {
					trace!(target: "validation", "driving statement work to completion");

					let work = producer.prime(api.clone()).validate();

					let work = future::select(work.boxed(), exit.clone()).map(drop);
					let _ = executor.spawn(work);
				}
			}
		}
	}
}

fn distribute_local_collation(
	instance: &ConsensusNetworkingInstance,
	collation: Collation,
	receipt: CandidateReceipt,
	chunks: (ValidatorIndex, Vec<ErasureChunk>),
	gossip_handle: &RegisteredMessageValidator,
) {
	// produce a signed statement.
	let hash = receipt.hash();
	let erasure_root = receipt.erasure_root;
	let validated = Validated::collated_local(
		receipt,
		collation.pov.clone(),
		OutgoingMessages { outgoing_messages: Vec::new() },
	);

	let statement = crate::legacy::gossip::GossipStatement::new(
		instance.relay_parent,
		match instance.statement_table.import_validated(validated) {
			None => return,
			Some(s) => s,
		}
	);

	gossip_handle.gossip_message(instance.attestation_topic, statement.into());

	for chunk in chunks.1 {
		let index = chunk.index;
		let message = crate::legacy::gossip::ErasureChunkMessage {
			chunk,
			relay_parent: instance.relay_parent,
			candidate_hash: hash,
		};

		gossip_handle.gossip_message(
			av_store::erasure_coding_topic(instance.relay_parent, erasure_root, index),
			message.into(),
		);
	}
}

// TODO [now]: implement `Collators`.

/// Routing logic for a particular attestation session.
#[derive(Clone)]
pub struct Router {
	inner: Arc<RouterInner>,
}

// note: do _not_ make this `Clone`: the drop implementation needs to _uniquely_
// send the `DropConsensusNetworking` message.
struct RouterInner {
	relay_parent: Hash,
	sender: mpsc::Sender<ServiceToWorkerMsg>,
}

impl Drop for RouterInner  {
	fn drop(&mut self) {
		let res = self.sender.try_send(
			ServiceToWorkerMsg::DropConsensusNetworking(self.relay_parent)
		);

		if let Err(e) = res {
			assert!(
				!e.is_full(),
				"futures 0.3 guarantees at least one free slot in the capacity \
				per sender; this is the first message sent via this sender; \
				therefore we will not have to wait for capacity; qed"
			);
			// other error variants (disconnection) are fine here.
		}
	}
}

impl ParachainNetwork for Service {
	type Error = future::Either<mpsc::SendError, oneshot::Canceled>;
	type TableRouter = Router;
	type BuildTableRouter = Pin<Box<dyn Future<Output=Result<Router,Self::Error>>>>;

	fn build_table_router(
		&self,
		table: Arc<SharedTable>,
		authorities: &[ValidatorId],
	) -> Self::BuildTableRouter {
		let authorities = authorities.to_vec();
		let mut sender = self.sender.clone();

		let (tx, rx) = oneshot::channel();
		Box::pin(async move {
			sender.send(
				ServiceToWorkerMsg::BuildConsensusNetworking(table, authorities, tx)
			).map_err(future::Either::Left).await?;

			rx.map_err(future::Either::Right).await
		})
	}
}

/// Errors when interacting with the statement router.
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum RouterError {
	#[display(fmt = "Encountered unexpected I/O error: {}", _0)]
	Io(std::io::Error),
	#[display(fmt = "Worker hung up while answering request.")]
	Canceled(oneshot::Canceled),
	#[display(fmt = "Could not reach worker with request: {}", _0)]
	SendError(mpsc::SendError),
}

impl TableRouter for Router {
	type Error = RouterError;
	type SendLocalCollation = Pin<Box<dyn Future<Output = Result<(), Self::Error>> + Send>>;
	type FetchValidationProof = Pin<Box<dyn Future<Output = Result<PoVBlock, Self::Error>> + Send>>;

	fn local_collation(
		&self,
		collation: Collation,
		receipt: CandidateReceipt,
		_outgoing: OutgoingMessages,
		chunks: (ValidatorIndex, &[ErasureChunk]),
	) -> Self::SendLocalCollation {
		let message = ServiceToWorkerMsg::LocalCollation(
			self.inner.relay_parent.clone(),
			collation,
			receipt,
			(chunks.0, chunks.1.to_vec()),
		);
		let mut sender = self.inner.sender.clone();
		Box::pin(async move {
			sender.send(message).map_err(Into::into).await
		})
	}

	fn fetch_pov_block(&self, candidate: &CandidateReceipt) -> Self::FetchValidationProof {
		let (tx, rx) = oneshot::channel();
		let message = ServiceToWorkerMsg::FetchPoVBlock(
			self.inner.relay_parent.clone(),
			candidate.clone(),
			tx,
		);

		let mut sender = self.inner.sender.clone();
		Box::pin(async move {
			sender.send(message).await?;
			rx.map_err(Into::into).await
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn router_inner_drop_sends_worker_message() {
		let parent = [1; 32].into();

		let (sender, mut receiver) = mpsc::channel(0);
		drop(RouterInner {
			relay_parent: parent,
			sender,
		});

		match receiver.try_next() {
			Ok(Some(ServiceToWorkerMsg::DropConsensusNetworking(x))) => assert_eq!(parent, x),
			_ => panic!("message not sent"),
		}
	}
}
