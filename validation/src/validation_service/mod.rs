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

//! The validation service is a long-running future that creates and manages parachain attestation
//! instances.
//!
//! As soon as we import a new chain head, we start a parachain attestation session on top of it.
//! The block authorship service may want access to the attestation session, and for that reason
//! we expose a `ServiceHandle` which can be used to request a copy of it.
//!
//! In fact, the import notification and request from the block production pipeline may race to be
//! the first one to create the instant, but the import notification will usually win.
//!
//! These attestation sessions are kept live until they are periodically garbage-collected.

use std::{time::{Duration, Instant}, sync::Arc};
use std::collections::HashMap;

use sc_client_api::{BlockchainEvents, BlockBody};
use sp_blockchain::HeaderBackend;
use block_builder::BlockBuilderApi;
use consensus::SelectChain;
use futures::{future::ready, prelude::*, task::{Spawn, SpawnExt}};
use polkadot_primitives::{Block, Hash, BlockId};
use polkadot_primitives::parachain::{
	Chain, ParachainHost, Id as ParaId, ValidatorIndex, ValidatorId, ValidatorPair,
};
use babe_primitives::BabeApi;
use keystore::KeyStorePtr;
use sp_api::{ApiExt, ProvideRuntimeApi};
use runtime_primitives::traits::HasherFor;
use availability_store::Store as AvailabilityStore;

use log::{warn, error, info, debug};

use super::{Network, Collators, SharedTable, TableRouter};
use crate::Error;

/// A handle to spawn background tasks onto.
pub type TaskExecutor = Arc<dyn Spawn + Send + Sync>;

// Remote processes may request for a validation instance to be cloned or instantiated.
// They send a oneshot channel.
type ValidationInstanceRequest = (
	Hash,
	futures::channel::oneshot::Sender<Result<ValidationInstanceHandle, Error>>,
);

/// A handle to a single instance of parachain validation, which is pinned to
/// a specific relay-chain block. This is the instance that should be used when
/// constructing any
#[derive(Clone)]
pub(crate) struct ValidationInstanceHandle {
	table: Arc<SharedTable>,
	started: Instant,
}

impl ValidationInstanceHandle {
	/// Access the underlying table of attestations on parachain candidates.
	pub(crate) fn table(&self) -> &Arc<SharedTable> {
		&self.table
	}

	/// The moment we started this validation instance.
	pub(crate) fn started(&self) -> Instant {
		self.started.clone()
	}
}

/// A handle to the service. This can be used to create a block-production environment.
#[derive(Clone)]
pub struct ServiceHandle {
	sender: futures::channel::mpsc::Sender<ValidationInstanceRequest>,
}

impl ServiceHandle {
	/// Requests instantiation or cloning of a validation instance from the service.
	///
	/// This can fail if the service task has shut down for some reason.
	pub(crate) async fn get_validation_instance(self, relay_parent: Hash)
		-> Result<ValidationInstanceHandle, Error>
	{
		let mut sender = self.sender;
		let instance_rx = loop {
			let (instance_tx, instance_rx) = futures::channel::oneshot::channel();
			match sender.send((relay_parent, instance_tx)).await {
				Ok(()) => break instance_rx,
				Err(e) => if !e.is_full() {
					// Sink::send should be doing `poll_ready` before start-send,
					// so this should only happen when there is a race.
					return Err(Error::ValidationServiceDown)
				},
			}
		};

		instance_rx.map_err(|_| Error::ValidationServiceDown).await.and_then(|x| x)
	}
}

fn interval(duration: Duration) -> impl Stream<Item=()> + Send + Unpin {
	stream::unfold((), move |_| {
		futures_timer::Delay::new(duration).map(|_| Some(((), ())))
	}).map(drop)
}

/// A builder for the validation service.
pub struct ServiceBuilder<C, N, P, SC, SP> {
	/// The underlying blockchain client.
	pub client: Arc<P>,
	/// A handle to the network object used to communicate.
	pub network: N,
	/// A handle to the collator pool we are using.
	pub collators: C,
	/// A handle to a background executor.
	pub spawner: SP,
	/// A handle to the availability store.
	pub availability_store: AvailabilityStore,
	/// A chain selector for determining active leaves in the block-DAG.
	pub select_chain: SC,
	/// The keystore which holds the signing keys.
	pub keystore: KeyStorePtr,
	/// The maximum block-data size in bytes.
	pub max_block_data_size: Option<u64>,
}

impl<C, N, P, SC, SP> ServiceBuilder<C, N, P, SC, SP> where
	C: Collators + Send + Sync + Unpin + 'static,
	C::Collation: Send + Unpin + 'static,
	P: BlockchainEvents<Block> + BlockBody<Block>,
	P: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> +
		BlockBuilderApi<Block> +
		BabeApi<Block> +
		ApiExt<Block, Error = sp_blockchain::Error>,
	N: Network + Send + Sync + 'static,
	N::TableRouter: Send + 'static,
	N::BuildTableRouter: Send + Unpin + 'static,
	<N::TableRouter as TableRouter>::SendLocalCollation: Send,
	SC: SelectChain<Block> + 'static,
	SP: Spawn + Send + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HasherFor<Block>>,
{
	/// Build the service - this consists of a handle to it, as well as a background
	/// future to be run to completion.
	pub fn build(self) -> (ServiceHandle, impl Future<Output = ()> + Send + 'static) {
		const TIMER_INTERVAL: Duration = Duration::from_secs(30);
		const CHAN_BUFFER: usize = 10;

		enum Message {
			CollectGarbage,
			// relay-parent, receiver for instance.
			RequestInstance(ValidationInstanceRequest),
			// new chain heads - import notification.
			NotifyImport(sc_client_api::BlockImportNotification<Block>),
		}

		let mut parachain_validation = ParachainValidationInstances {
			client: self.client.clone(),
			network: self.network,
			collators: self.collators,
			spawner: self.spawner,
			availability_store: self.availability_store,
			live_instances: HashMap::new(),
		};

		let client = self.client;
		let select_chain = self.select_chain;
		let keystore = self.keystore;
		let max_block_data_size = self.max_block_data_size;

		let (tx, rx) = futures::channel::mpsc::channel(CHAN_BUFFER);
		let interval = interval(TIMER_INTERVAL).map(|_| Message::CollectGarbage);
		let import_notifications = client.import_notification_stream().map(Message::NotifyImport);
		let instance_requests = rx.map(Message::RequestInstance);
		let service = ServiceHandle { sender: tx };

		let background_work = async move {
			let message_stream = futures::stream::select(interval, instance_requests);
			let mut message_stream = futures::stream::select(import_notifications, message_stream);
			while let Some(message) = message_stream.next().await {
				match message {
					Message::CollectGarbage => {
						match select_chain.leaves() {
							Ok(leaves) => {
								parachain_validation.retain(|h| leaves.contains(h));
							}
							Err(e) => {
								warn!("Error fetching leaves from client: {:?}", e);
							}
						}
					}
					Message::RequestInstance((relay_parent, sender)) => {
						// Upstream will handle the failure case.
						let _ = sender.send(parachain_validation.get_or_instantiate(
							relay_parent,
							&keystore,
							max_block_data_size,
						));
					}
					Message::NotifyImport(notification) => {
						let relay_parent = notification.hash;
						if notification.is_new_best {
							let res = parachain_validation.get_or_instantiate(
								relay_parent,
								&keystore,
								max_block_data_size,
							);

							if let Err(e) = res {
								warn!(
									"Unable to start parachain validation on top of {:?}: {}",
									relay_parent, e
								);
							}
						}
					}
				}
			}
		};

		(service, background_work)
	}
}

// finds the first key we are capable of signing with out of the given set of validators,
// if any.
fn signing_key(validators: &[ValidatorId], keystore: &KeyStorePtr) -> Option<Arc<ValidatorPair>> {
	let keystore = keystore.read();
	validators.iter()
		.find_map(|v| {
			keystore.key_pair::<ValidatorPair>(&v).ok()
		})
		.map(|pair| Arc::new(pair))
}

/// Constructs parachain-agreement instances.
pub(crate) struct ParachainValidationInstances<C, N, P, SP> {
	/// The client instance.
	client: Arc<P>,
	/// The backing network handle.
	network: N,
	/// Parachain collators.
	collators: C,
	/// handle to spawner
	spawner: SP,
	/// Store for extrinsic data.
	availability_store: AvailabilityStore,
	/// Live agreements. Maps relay chain parent hashes to attestation
	/// instances.
	live_instances: HashMap<Hash, ValidationInstanceHandle>,
}

impl<C, N, P, SP> ParachainValidationInstances<C, N, P, SP> where
	C: Collators + Send + Unpin + 'static + Sync,
	N: Network,
	P: ProvideRuntimeApi<Block> + HeaderBackend<Block> + BlockBody<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	C::Collation: Send + Unpin + 'static,
	N::TableRouter: Send + 'static,
	<N::TableRouter as TableRouter>::SendLocalCollation: Send,
	N::BuildTableRouter: Unpin + Send + 'static,
	SP: Spawn + Send + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HasherFor<Block>>,
{
	/// Get an attestation table for given parent hash.
	///
	/// This starts a parachain agreement process on top of the parent hash if
	/// one has not already started.
	///
	/// Additionally, this will trigger broadcast of data to the new block's duty
	/// roster.
	fn get_or_instantiate(
		&mut self,
		parent_hash: Hash,
		keystore: &KeyStorePtr,
		max_block_data_size: Option<u64>,
	) -> Result<ValidationInstanceHandle, Error> {
		use primitives::Pair;

		if let Some(tracker) = self.live_instances.get(&parent_hash) {
			return Ok(tracker.clone());
		}

		let id = BlockId::hash(parent_hash);

		let validators = self.client.runtime_api().validators(&id)?;
		let sign_with = signing_key(&validators[..], keystore);

		let duty_roster = self.client.runtime_api().duty_roster(&id)?;

		let (group_info, local_duty) = crate::make_group_info(
			duty_roster,
			&validators,
			sign_with.as_ref().map(|k| k.public()),
		)?;

		info!(
			"Starting parachain attestation session on top of parent {:?}. Local parachain duty is {:?}",
			parent_hash,
			local_duty,
		);

		let active_parachains = self.client.runtime_api().active_parachains(&id)?;

		debug!(target: "validation", "Active parachains: {:?}", active_parachains);

		// If we are a validator, we need to store our index in this round in availability store.
		// This will tell which erasure chunk we should store.
		if let Some(ref local_duty) = local_duty {
			if let Err(e) = self.availability_store.note_validator_index_and_n_validators(
				&parent_hash,
				local_duty.index,
				validators.len() as u32,
			) {
				warn!(
					target: "validation",
					"Failed to add validator index and n_validators to the availability-store: {:?}", e
				)
			}
		}

		let table = Arc::new(SharedTable::new(
			validators.clone(),
			group_info,
			sign_with,
			parent_hash,
			self.availability_store.clone(),
			max_block_data_size,
		));

		let router = self.network.build_table_router(
			table.clone(),
			&validators,
		);

		if let Some((Chain::Parachain(id), index)) = local_duty.as_ref().map(|d| (d.validation, d.index)) {
			self.launch_work(parent_hash, id, router, max_block_data_size, validators.len(), index);
		}

		let tracker = ValidationInstanceHandle {
			table,
			started: Instant::now(),
		};

		self.live_instances.insert(parent_hash, tracker.clone());

		Ok(tracker)
	}

	/// Retain validation sessions matching predicate.
	fn retain<F: FnMut(&Hash) -> bool>(&mut self, mut pred: F) {
		self.live_instances.retain(|k, _| pred(k))
	}

	// launch parachain work asynchronously.
	fn launch_work(
		&self,
		relay_parent: Hash,
		validation_para: ParaId,
		build_router: N::BuildTableRouter,
		max_block_data_size: Option<u64>,
		n_validators: usize,
		local_id: ValidatorIndex,
	) {
		let (collators, client) = (self.collators.clone(), self.client.clone());
		let availability_store = self.availability_store.clone();

		let with_router = move |router: N::TableRouter| {
			// fetch a local collation from connected collators.
			let collation_work = crate::collation::collation_fetch(
				validation_para,
				relay_parent,
				collators,
				client.clone(),
				max_block_data_size,
				n_validators,
			);

			collation_work.then(move |result| match result {
				Ok((collation_info, full_output)) => {
					let crate::pipeline::FullOutput {
						commitments,
						erasure_chunks,
						available_data,
					} = full_output;

					let receipt = collation_info.into_receipt(commitments);

					// Apparently the `async move` block is the only way to convince
					// the compiler that we are not moving values out of borrowed context.
					let av_clone = availability_store.clone();
					let receipt_clone = receipt.clone();
					let erasure_chunks_clone = erasure_chunks.clone();
					let pov_block = available_data.pov_block.clone();

					let res = async move {
						if let Err(e) = av_clone.make_available(
							receipt_clone.hash(),
							available_data,
						).await {
							warn!(
								target: "validation",
								"Failed to make parachain block data available: {}",
								e,
							);
						}
						if let Err(e) = av_clone.clone().add_erasure_chunks(
							receipt_clone,
							erasure_chunks_clone,
						).await {
							warn!(target: "validation", "Failed to add erasure chunks: {}", e);
						}
					}
					.unit_error()
					.then(move |_| {
						router.local_collation(
							receipt,
							pov_block,
							(local_id, &erasure_chunks),
						).map_err(|e| warn!(target: "validation", "Failed to send local collation: {:?}", e))
					});

					res.boxed()
				}
				Err(e) => {
					warn!(target: "validation", "Failed to collate candidate: {:?}", e);
					Box::pin(ready(Ok(())))
				}
			})
		};

		let router_work = build_router
			.map_ok(with_router)
			.map_err(|e| {
				warn!(target: "validation" , "Failed to build table router: {:?}", e);
			})
			.and_then(|r| r)
			.map(|_| ());


		// spawn onto thread pool.
		if self.spawner.spawn(router_work).is_err() {
			error!("Failed to spawn router work task");
		}
	}
}
