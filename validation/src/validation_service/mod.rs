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

use std::{time::{Duration, Instant}, sync::Arc, pin::Pin, collections::HashMap};

use crate::pipeline::FullOutput;
use sc_client_api::{BlockchainEvents, BlockBackend};
use consensus::SelectChain;
use futures::prelude::*;
use polkadot_primitives::v0::{
	Block, Hash, BlockId,
	Chain, ParachainHost, Id as ParaId, ValidatorIndex, ValidatorId, ValidatorPair,
	CollationInfo, SigningContext,
};
use keystore::KeyStorePtr;
use sp_api::{ProvideRuntimeApi, ApiExt};
use runtime_primitives::traits::HashFor;
use availability_store::Store as AvailabilityStore;
use primitives::traits::SpawnNamed;

use ansi_term::Colour;
use log::{warn, info, debug, trace};

use super::{Network, Collators, SharedTable, TableRouter};
use crate::Error;
use crate::pipeline::ValidationPool;

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
	P: BlockchainEvents<Block> + BlockBackend<Block>,
	P: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	N: Network + Send + Sync + 'static,
	N::TableRouter: Send + 'static + Sync,
	N::BuildTableRouter: Send + Unpin + 'static,
	<N::TableRouter as TableRouter>::SendLocalCollation: Send,
	SC: SelectChain<Block> + 'static,
	SP: SpawnNamed + Clone + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HashFor<Block>>,
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

		let validation_pool = Some(ValidationPool::new());
		let mut parachain_validation = ParachainValidationInstances {
			client: self.client.clone(),
			network: self.network,
			spawner: self.spawner.clone(),
			availability_store: self.availability_store,
			live_instances: HashMap::new(),
			validation_pool: validation_pool.clone(),
			collation_fetch: DefaultCollationFetch {
				collators: self.collators,
				validation_pool,
				spawner: self.spawner,
			},
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
						).await);
					}
					Message::NotifyImport(notification) => {
						let relay_parent = notification.hash;
						if notification.is_new_best {
							let res = parachain_validation.get_or_instantiate(
								relay_parent,
								&keystore,
								max_block_data_size,
							).await;

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

/// Abstraction over `collation_fetch`.
pub(crate) trait CollationFetch {
	/// Error type used by `collation_fetch`.
	type Error: std::fmt::Debug;

	/// Fetch a collation for the given `parachain`.
	fn collation_fetch<P>(
		self,
		parachain: ParaId,
		relay_parent: Hash,
		client: Arc<P>,
		max_block_data_size: Option<u64>,
		n_validators: usize,
	) -> Pin<Box<dyn Future<Output = Result<(CollationInfo, FullOutput), Self::Error>> + Send>>
		where
			P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
			P: ProvideRuntimeApi<Block> + Send + Sync + 'static;
}

#[derive(Clone)]
struct DefaultCollationFetch<C, S> {
	collators: C,
	validation_pool: Option<ValidationPool>,
	spawner: S,
}

impl<C, S> CollationFetch for DefaultCollationFetch<C, S>
	where
		C: Collators + Send + Sync + Unpin + 'static,
		C::Collation: Send + Unpin + 'static,
		S: SpawnNamed + Clone + 'static,
{
	type Error = C::Error;

	fn collation_fetch<P>(
		self,
		parachain: ParaId,
		relay_parent: Hash,
		client: Arc<P>,
		max_block_data_size: Option<u64>,
		n_validators: usize,
	) -> Pin<Box<dyn Future<Output = Result<(CollationInfo, FullOutput), Self::Error>> + Send>>
		where
			P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
			P: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	{
		crate::collation::collation_fetch(
			self.validation_pool,
			parachain,
			relay_parent,
			self.collators,
			client,
			max_block_data_size,
			n_validators,
			self.spawner,
		).boxed()
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

/// A live instance that is related to a relay chain validation round.
///
/// It stores the `instance_handle` and the `_table_router`.
struct LiveInstance<TR> {
	instance_handle: ValidationInstanceHandle,
	/// Make sure we keep the table router alive, to respond/receive consensus messages.
	_table_router: TR,
}

/// Constructs parachain-agreement instances.
pub(crate) struct ParachainValidationInstances<N: Network, P, SP, CF> {
	/// The client instance.
	client: Arc<P>,
	/// The backing network handle.
	network: N,
	/// handle to spawner
	spawner: SP,
	/// Store for extrinsic data.
	availability_store: AvailabilityStore,
	/// Live agreements. Maps relay chain parent hashes to attestation
	/// instances.
	live_instances: HashMap<Hash, LiveInstance<N::TableRouter>>,
	/// The underlying validation pool of processes to use.
	/// Only `None` in tests.
	validation_pool: Option<ValidationPool>,
	/// Used to fetch a collation.
	collation_fetch: CF,
}

impl<N, P, SP, CF> ParachainValidationInstances<N, P, SP, CF> where
	N: Network,
	N::Error: 'static,
	P: ProvideRuntimeApi<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block, Error = sp_blockchain::Error>,
	N::TableRouter: Send + 'static + Sync,
	<N::TableRouter as TableRouter>::SendLocalCollation: Send,
	N::BuildTableRouter: Unpin + Send + 'static,
	SP: SpawnNamed + Send + 'static,
	CF: CollationFetch + Clone + Send + Sync + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HashFor<Block>>,
{
	/// Get an attestation table for given parent hash.
	///
	/// This starts a parachain agreement process on top of the parent hash if
	/// one has not already started.
	///
	/// Additionally, this will trigger broadcast of data to the new block's duty
	/// roster.
	async fn get_or_instantiate(
		&mut self,
		parent_hash: Hash,
		keystore: &KeyStorePtr,
		max_block_data_size: Option<u64>,
	) -> Result<ValidationInstanceHandle, Error> {
		use primitives::Pair;

		if let Some(instance) = self.live_instances.get(&parent_hash) {
			return Ok(instance.instance_handle.clone());
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

		if let Some(ref duty) = local_duty {
			info!(
				"✍️ Starting parachain attestation session (parent: {}) with active duty {}",
				parent_hash,
				Colour::Red.bold().paint(format!("{:?}", duty)),
			);
		} else {
			debug!(
				"✍️ Starting parachain attestation session (parent: {}). No local duty..",
				parent_hash,
			);
		}
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

		let api = self.client.runtime_api();

		let signing_context = if api.has_api_with::<dyn ParachainHost<Block, Error = ()>, _>(
			&BlockId::hash(parent_hash),
			|version| version >= 3,
		)? {
			api.signing_context(&id)?
		} else {
			trace!(
				target: "validation",
				"Expected runtime with ParachainHost version >= 3",
			);
			SigningContext {
				session_index: 0,
				parent_hash,
			}
		};

		let table = Arc::new(SharedTable::new(
			validators.clone(),
			group_info,
			sign_with,
			signing_context,
			self.availability_store.clone(),
			max_block_data_size,
			self.validation_pool.clone(),
		));

		// The router will join the consensus gossip network. This is important
		// to receive messages sent for the current round.
		let router = match self.network.build_table_router(
			table.clone(),
			&validators,
		).await {
			Ok(res) => res,
			Err(e) => {
				warn!(target: "validation", "Failed to build router: {:?}", e);
				return Err(Error::CouldNotBuildTableRouter(format!("{:?}", e)))
			}
		};

		if let Some((Chain::Parachain(id), index)) = local_duty.map(|d| (d.validation, d.index)) {
			let n_validators = validators.len();
			let availability_store = self.availability_store.clone();
			let client = self.client.clone();
			let collation_fetch = self.collation_fetch.clone();
			let router = router.clone();

			self.spawner.spawn(
				"polkadot-parachain-validation-work",
				launch_work(
					move || collation_fetch.collation_fetch(id, parent_hash, client, max_block_data_size, n_validators),
					availability_store,
					router,
					n_validators,
					index,
				).boxed(),
			);
		}

		let tracker = ValidationInstanceHandle {
			table,
			started: Instant::now(),
		};

		let live_instance = LiveInstance {
			instance_handle: tracker.clone(),
			_table_router: router,
		};
		self.live_instances.insert(parent_hash, live_instance);

		Ok(tracker)
	}

	/// Retain validation sessions matching predicate.
	fn retain<F: FnMut(&Hash) -> bool>(&mut self, mut pred: F) {
		self.live_instances.retain(|k, _| pred(k))
	}
}

// launch parachain work asynchronously.
async fn launch_work<CFF, E>(
	collation_fetch: impl FnOnce() -> CFF,
	availability_store: AvailabilityStore,
	router: impl TableRouter,
	n_validators: usize,
	local_id: ValidatorIndex,
) where
	E: std::fmt::Debug,
	CFF: Future<Output = Result<(CollationInfo, FullOutput), E>> + Send,
{
	// fetch a local collation from connected collators.
	let (collation_info, full_output) = match collation_fetch().await {
		Ok(res) => res,
		Err(e) => {
			warn!(target: "validation", "Failed to collate candidate: {:?}", e);
			return
		}
	};

	let crate::pipeline::FullOutput {
		commitments,
		erasure_chunks,
		available_data,
		..
	} = full_output;

	let receipt = collation_info.into_receipt(commitments);
	let pov_block = available_data.pov_block.clone();

	if let Err(e) = availability_store.make_available(
		receipt.hash(),
		available_data,
	).await {
		warn!(
			target: "validation",
			"Failed to make parachain block data available: {}",
			e,
		);
	}

	if let Err(e) = availability_store.clone().add_erasure_chunks(
		receipt.clone(),
		n_validators as _,
		erasure_chunks.clone(),
	).await {
		warn!(target: "validation", "Failed to add erasure chunks: {}", e);
	}

	if let Err(e) = router.local_collation(
		receipt,
		pov_block,
		(local_id, &erasure_chunks),
	).await {
		warn!(target: "validation", "Failed to send local collation: {:?}", e);
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::{executor, future::ready, channel::mpsc};
	use availability_store::ErasureNetworking;
	use polkadot_primitives::v0::{
		PoVBlock, AbridgedCandidateReceipt, ErasureChunk, ValidatorIndex,
		CollationInfo, DutyRoster, GlobalValidationData, LocalValidationData,
		Retriable, CollatorId, BlockData, Chain, AvailableData, SigningContext, ValidationCode,
	};
	use runtime_primitives::traits::Block as BlockT;
	use std::pin::Pin;
	use sp_keyring::sr25519::Keyring;
	use primitives::testing::TaskExecutor;

	/// Events fired while running mock implementations to follow execution.
	enum Events {
		BuildTableRouter,
		CollationFetch,
		LocalCollation,
	}

	#[derive(Clone)]
	struct MockNetwork(mpsc::UnboundedSender<Events>);

	impl Network for MockNetwork {
		type Error = String;
		type TableRouter = MockTableRouter;
		type BuildTableRouter = Pin<Box<dyn Future<Output = Result<MockTableRouter, String>> + Send>>;

		fn build_table_router(
			&self,
			_: Arc<SharedTable>,
			_: &[ValidatorId],
		) -> Self::BuildTableRouter {
			let event_sender = self.0.clone();
			async move {
				event_sender.unbounded_send(Events::BuildTableRouter).expect("Send `BuildTableRouter`");

				Ok(MockTableRouter(event_sender))
			}.boxed()
		}
	}

	#[derive(Clone)]
	struct MockTableRouter(mpsc::UnboundedSender<Events>);

	impl TableRouter for MockTableRouter {
		type Error = String;
		type SendLocalCollation = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
		type FetchValidationProof = Box<dyn Future<Output = Result<PoVBlock, String>> + Unpin>;

		fn local_collation(
			&self,
			_: AbridgedCandidateReceipt,
			_: PoVBlock,
			_: (ValidatorIndex, &[ErasureChunk]),
		) -> Self::SendLocalCollation {
			let sender = self.0.clone();

			async move {
				sender.unbounded_send(Events::LocalCollation).expect("Send `LocalCollation`");

				Ok(())
			}.boxed()
		}

		fn fetch_pov_block(&self, _: &AbridgedCandidateReceipt) -> Self::FetchValidationProof {
			unimplemented!("Not required in tests")
		}
	}

	#[derive(Clone)]
	struct MockErasureNetworking;

	impl ErasureNetworking for MockErasureNetworking {
		type Error = String;

		fn fetch_erasure_chunk(
			&self,
			_: &Hash,
			_: u32,
		) -> Pin<Box<dyn Future<Output = Result<ErasureChunk, Self::Error>> + Send>> {
			ready(Err("Not required in tests".to_string())).boxed()
		}

		fn distribute_erasure_chunk(&self, _: Hash, _: ErasureChunk) {
			unimplemented!("Not required in tests")
		}
	}

	#[derive(Clone)]
	struct MockCollationFetch(mpsc::UnboundedSender<Events>);

	impl CollationFetch for MockCollationFetch {
		type Error = ();

		fn collation_fetch<P>(
			self,
			parachain: ParaId,
			relay_parent: Hash,
			_: Arc<P>,
			_: Option<u64>,
			n_validators: usize,
		) -> Pin<Box<dyn Future<Output = Result<(CollationInfo, FullOutput), ()>> + Send>> {
			let info = CollationInfo {
				parachain_index: parachain,
				relay_parent,
				collator: Default::default(),
				signature: Default::default(),
				head_data: Default::default(),
				pov_block_hash: Default::default(),
			};

			let available_data = AvailableData {
				pov_block: PoVBlock { block_data: BlockData(Vec::new()) },
				omitted_validation: Default::default(),
			};

			let full_output = FullOutput {
				available_data,
				commitments: Default::default(),
				erasure_chunks: Default::default(),
				n_validators,
			};

			let sender = self.0;

			async move {
				sender.unbounded_send(Events::CollationFetch).expect("`CollationFetch` event send");

				Ok((info, full_output))
			}.boxed()
		}
	}

	#[derive(Clone)]
	struct MockRuntimeApi {
		validators: Vec<ValidatorId>,
		duty_roster: DutyRoster,
	}

	impl ProvideRuntimeApi<Block> for MockRuntimeApi {
		type Api = Self;

		fn runtime_api<'a>(&'a self) -> sp_api::ApiRef<'a, Self::Api> {
			self.clone().into()
		}
	}

	sp_api::mock_impl_runtime_apis! {
		impl ParachainHost<Block> for MockRuntimeApi {
			type Error = sp_blockchain::Error;

			fn validators(&self) -> Vec<ValidatorId> { self.validators.clone() }
			fn duty_roster(&self) -> DutyRoster { self.duty_roster.clone() }
			fn active_parachains() -> Vec<(ParaId, Option<(CollatorId, Retriable)>)> { vec![(ParaId::from(1), None)] }
			fn global_validation_data() -> GlobalValidationData { Default::default() }
			fn local_validation_data(_: ParaId) -> Option<LocalValidationData> { None }
			fn parachain_code(_: ParaId) -> Option<ValidationCode> { None }
			fn get_heads(_: Vec<<Block as BlockT>::Extrinsic>) -> Option<Vec<AbridgedCandidateReceipt>> {
				None
			}
			fn signing_context() -> SigningContext {
				Default::default()
			}
			fn downward_messages(_: ParaId) -> Vec<Vec<u8>> {
				Vec::new()
			}
		}
	}

	#[test]
	fn launch_work_is_executed_properly() {
		let executor = TaskExecutor::new();
		let keystore = keystore::Store::new_in_memory();

		// Make sure `Bob` key is in the keystore, so this mocked node will be a parachain validator.
		keystore.write().insert_ephemeral_from_seed::<ValidatorPair>(&Keyring::Bob.to_seed())
			.expect("Insert key into keystore");

		let validators = vec![ValidatorId::from(Keyring::Alice.public()), ValidatorId::from(Keyring::Bob.public())];
		let validator_duty = vec![Chain::Relay, Chain::Parachain(1.into())];
		let duty_roster = DutyRoster { validator_duty };

		let (events_sender, events) = mpsc::unbounded();

		let mut parachain_validation = ParachainValidationInstances {
			client: Arc::new(MockRuntimeApi { validators, duty_roster }),
			network: MockNetwork(events_sender.clone()),
			collation_fetch: MockCollationFetch(events_sender.clone()),
			spawner: executor.clone(),
			availability_store: AvailabilityStore::new_in_memory(MockErasureNetworking),
			live_instances: HashMap::new(),
			validation_pool: None,
		};

		executor::block_on(parachain_validation.get_or_instantiate(Default::default(), &keystore, None))
			.expect("Creates new validation round");
		assert!(parachain_validation.live_instances.contains_key(&Default::default()));

		let mut events = executor::block_on_stream(events);

		assert!(matches!(events.next().unwrap(), Events::BuildTableRouter));
		assert!(matches!(events.next().unwrap(), Events::CollationFetch));
		assert!(matches!(events.next().unwrap(), Events::LocalCollation));

		drop(events_sender);
		drop(parachain_validation);
		assert!(events.next().is_none());
	}

	#[test]
	fn router_is_built_on_relay_chain_validator() {
		let executor = TaskExecutor::new();
		let keystore = keystore::Store::new_in_memory();

		// Make sure `Alice` key is in the keystore, so this mocked node will be a relay-chain validator.
		keystore.write().insert_ephemeral_from_seed::<ValidatorPair>(&Keyring::Alice.to_seed())
			.expect("Insert key into keystore");

		let validators = vec![ValidatorId::from(Keyring::Alice.public()), ValidatorId::from(Keyring::Bob.public())];
		let validator_duty = vec![Chain::Relay, Chain::Parachain(1.into())];
		let duty_roster = DutyRoster { validator_duty };

		let (events_sender, events) = mpsc::unbounded();

		let mut parachain_validation = ParachainValidationInstances {
			client: Arc::new(MockRuntimeApi { validators, duty_roster }),
			network: MockNetwork(events_sender.clone()),
			collation_fetch: MockCollationFetch(events_sender.clone()),
			spawner: executor.clone(),
			availability_store: AvailabilityStore::new_in_memory(MockErasureNetworking),
			live_instances: HashMap::new(),
			validation_pool: None,
		};

		executor::block_on(parachain_validation.get_or_instantiate(Default::default(), &keystore, None))
			.expect("Creates new validation round");
		assert!(parachain_validation.live_instances.contains_key(&Default::default()));

		let mut events = executor::block_on_stream(events);

		assert!(matches!(events.next().unwrap(), Events::BuildTableRouter));

		drop(events_sender);
		drop(parachain_validation);
		assert!(events.next().is_none());
	}
}
