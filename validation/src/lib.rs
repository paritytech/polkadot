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

//! Propagation and agreement of candidates.
//!
//! Authorities are split into groups by parachain, and each authority might come
//! up its own candidate for their parachain. Within groups, authorities pass around
//! their candidates and produce statements of validity.
//!
//! Any candidate that receives majority approval by the authorities in a group
//! may be subject to inclusion, unless any authorities flag that candidate as invalid.
//!
//! Wrongly flagging as invalid should be strongly disincentivized, so that in the
//! equilibrium state it is not expected to happen. Likewise with the submission
//! of invalid blocks.
//!
//! Groups themselves may be compromised by malicious authorities.

use std::{
	collections::{HashMap, HashSet},
	pin::Pin,
	sync::Arc,
	time::{self, Duration, Instant},
};

use babe_primitives::BabeApi;
use sc_client_api::{Backend, BlockchainEvents, BlockBody};
use sp_blockchain::HeaderBackend;
use block_builder::BlockBuilderApi;
use codec::Encode;
use consensus::{SelectChain, Proposal, RecordProof};
use availability_store::Store as AvailabilityStore;
use parking_lot::Mutex;
use polkadot_primitives::{Hash, Block, BlockId, BlockNumber, Header};
use polkadot_primitives::parachain::{
	Id as ParaId, Chain, DutyRoster, CandidateReceipt,
	ParachainHost, AttestedCandidate, Statement as PrimitiveStatement, Message, OutgoingMessages,
	Collation, PoVBlock, ErasureChunk, ValidatorSignature, ValidatorIndex,
	ValidatorPair, ValidatorId, NEW_HEADS_IDENTIFIER,
};
use primitives::Pair;
use runtime_primitives::traits::{DigestFor, HasherFor};
use futures_timer::Delay;
use txpool_api::{TransactionPool, InPoolTransaction};

use attestation_service::ServiceHandle;
use futures::prelude::*;
use futures::{future::{select, ready}, stream::unfold, task::{Spawn, SpawnExt}};
use collation::collation_fetch;
use dynamic_inclusion::DynamicInclusion;
use inherents::InherentData;
use sp_timestamp::TimestampInherentData;
use log::{info, debug, warn, trace, error};
use keystore::KeyStorePtr;
use sp_api::{ApiExt, ProvideRuntimeApi};

type TaskExecutor = Arc<dyn Spawn + Send + Sync>;

fn interval(duration: Duration) -> impl Stream<Item=()> + Send + Unpin {
	unfold((), move |_| {
		futures_timer::Delay::new(duration).map(|_| Some(((), ())))
	}).map(drop)
}

pub use self::collation::{
	validate_collation, validate_incoming, message_queue_root, egress_roots, Collators,
	produce_receipt_and_chunks,
};
pub use self::error::Error;
pub use self::shared_table::{
	SharedTable, ParachainWork, PrimedParachainWork, Validated, Statement, SignedStatement,
	GenericStatement,
};

#[cfg(not(target_os = "unknown"))]
pub use parachain::wasm_executor::{run_worker as run_validation_worker};

mod attestation_service;
mod dynamic_inclusion;
mod evaluation;
mod error;
mod shared_table;

pub mod collation;

// block size limit.
const MAX_TRANSACTIONS_SIZE: usize = 4 * 1024 * 1024;

/// Incoming messages; a series of sorted (ParaId, Message) pairs.
pub type Incoming = Vec<(ParaId, Vec<Message>)>;

/// A handle to a statement table router.
///
/// This is expected to be a lightweight, shared type like an `Arc`.
pub trait TableRouter: Clone {
	/// Errors when fetching data from the network.
	type Error: std::fmt::Debug;
	/// Future that resolves when candidate data is fetched.
	type FetchValidationProof: Future<Output=Result<PoVBlock, Self::Error>>;

	/// Call with local candidate data. This will make the data available on the network,
	/// and sign, import, and broadcast a statement about the candidate.
	fn local_collation(
		&self,
		collation: Collation,
		receipt: CandidateReceipt,
		outgoing: OutgoingMessages,
		chunks: (ValidatorIndex, &[ErasureChunk]),
	);

	/// Fetch validation proof for a specific candidate.
	fn fetch_pov_block(&self, candidate: &CandidateReceipt) -> Self::FetchValidationProof;
}

/// A long-lived network which can create parachain statement and BFT message routing processes on demand.
pub trait Network {
	/// The error type of asynchronously building the table router.
	type Error: std::fmt::Debug;

	/// The table router type. This should handle importing of any statements,
	/// routing statements to peers, and driving completion of any `StatementProducers`.
	type TableRouter: TableRouter;

	/// The future used for asynchronously building the table router.
	/// This should not fail.
	type BuildTableRouter: Future<Output=Result<Self::TableRouter,Self::Error>>;

	/// Instantiate a table router using the given shared table.
	/// Also pass through any outgoing messages to be broadcast to peers.
	fn communication_for(
		&self,
		table: Arc<SharedTable>,
		authorities: &[ValidatorId],
		exit: exit_future::Exit,
	) -> Self::BuildTableRouter;
}

/// Information about a specific group.
#[derive(Debug, Clone, Default)]
pub struct GroupInfo {
	/// Authorities meant to check validity of candidates.
	validity_guarantors: HashSet<ValidatorId>,
	/// Number of votes needed for validity.
	needed_validity: usize,
}

/// Sign a table statement against a parent hash.
/// The actual message signed is the encoded statement concatenated with the
/// parent hash.
pub fn sign_table_statement(statement: &Statement, key: &ValidatorPair, parent_hash: &Hash) -> ValidatorSignature {
	// we sign using the primitive statement type because that's what the runtime
	// expects. These types probably encode the same way so this clone could be optimized
	// out in the future.
	let mut encoded = PrimitiveStatement::from(statement.clone()).encode();
	encoded.extend(parent_hash.as_ref());

	key.sign(&encoded)
}

/// Check signature on table statement.
pub fn check_statement(
	statement: &Statement,
	signature: &ValidatorSignature,
	signer: ValidatorId,
	parent_hash: &Hash,
) -> bool {
	use runtime_primitives::traits::AppVerify;

	let mut encoded = PrimitiveStatement::from(statement.clone()).encode();
	encoded.extend(parent_hash.as_ref());

	signature.verify(&encoded[..], &signer)
}

/// Compute group info out of a duty roster and a local authority set.
pub fn make_group_info(
	roster: DutyRoster,
	authorities: &[ValidatorId],
	local_id: Option<ValidatorId>,
) -> Result<(HashMap<ParaId, GroupInfo>, Option<LocalDuty>), Error> {
	if roster.validator_duty.len() != authorities.len() {
		return Err(Error::InvalidDutyRosterLength {
			expected: authorities.len(),
			got: roster.validator_duty.len()
		});
	}

	let mut local_validation = None;
	let mut local_index = 0;
	let mut map = HashMap::new();

	let duty_iter = authorities.iter().zip(&roster.validator_duty);
	for (i, (authority, v_duty)) in duty_iter.enumerate() {
		if Some(authority) == local_id.as_ref() {
			local_validation = Some(v_duty.clone());
			local_index = i;
		}

		match *v_duty {
			Chain::Relay => {}, // does nothing for now.
			Chain::Parachain(ref id) => {
				map.entry(id.clone()).or_insert_with(GroupInfo::default)
					.validity_guarantors
					.insert(authority.clone());
			}
		}
	}

	for live_group in map.values_mut() {
		let validity_len = live_group.validity_guarantors.len();
		live_group.needed_validity = validity_len / 2 + validity_len % 2;
	}


	let local_duty = local_validation.map(|v| LocalDuty {
		validation: v,
		index: local_index as u32,
	});

	Ok((map, local_duty))

}

/// Compute the (target, root, messages) of all outgoing queues.
pub fn outgoing_queues(outgoing_targeted: &'_ OutgoingMessages)
	-> impl Iterator<Item=(ParaId, Hash, Vec<Message>)> + '_
{
	outgoing_targeted.message_queues().filter_map(|queue| {
		let target = queue.get(0)?.target;
		let queue_root = message_queue_root(queue);
		let queue_data = queue.iter().map(|msg| msg.clone().into()).collect();
		Some((target, queue_root, queue_data))
	})
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
struct ParachainValidation<C, N, P> {
	/// The client instance.
	client: Arc<P>,
	/// The backing network handle.
	network: N,
	/// Parachain collators.
	collators: C,
	/// handle to remote task executor
	handle: TaskExecutor,
	/// Store for extrinsic data.
	availability_store: AvailabilityStore,
	/// Live agreements. Maps relay chain parent hashes to attestation
	/// instances.
	live_instances: Mutex<HashMap<Hash, Arc<AttestationTracker>>>,
}

impl<C, N, P> ParachainValidation<C, N, P> where
	C: Collators + Send + Unpin + 'static + Sync,
	N: Network,
	P: ProvideRuntimeApi<Block> + HeaderBackend<Block> + BlockBody<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	C::Collation: Send + Unpin + 'static,
	N::TableRouter: Send + 'static,
	N::BuildTableRouter: Unpin + Send + 'static,
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
		&self,
		parent_hash: Hash,
		keystore: &KeyStorePtr,
		max_block_data_size: Option<u64>,
	)
		-> Result<Arc<AttestationTracker>, Error>
	{
		let mut live_instances = self.live_instances.lock();
		if let Some(tracker) = live_instances.get(&parent_hash) {
			return Ok(tracker.clone());
		}

		let id = BlockId::hash(parent_hash);

		let validators = self.client.runtime_api().validators(&id)?;
		let sign_with = signing_key(&validators[..], keystore);

		let duty_roster = self.client.runtime_api().duty_roster(&id)?;

		let (group_info, local_duty) = make_group_info(
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
			if let Err(e) = self.availability_store.add_validator_index_and_n_validators(
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

		let (_drop_signal, exit) = exit_future::signal();

		let router = self.network.communication_for(
			table.clone(),
			&validators,
			exit.clone(),
		);

		if let Some((Chain::Parachain(id), index)) = local_duty.as_ref().map(|d| (d.validation, d.index)) {
			self.launch_work(parent_hash, id, router, max_block_data_size, validators.len(), index, exit);
		}

		let tracker = Arc::new(AttestationTracker {
			table,
			started: Instant::now(),
			_drop_signal,
		});

		live_instances.insert(parent_hash, tracker.clone());

		Ok(tracker)
	}

	/// Retain validation sessions matching predicate.
	fn retain<F: FnMut(&Hash) -> bool>(&self, mut pred: F) {
		self.live_instances.lock().retain(|k, _| pred(k))
	}

	// launch parachain work asynchronously.
	fn launch_work(
		&self,
		relay_parent: Hash,
		validation_para: ParaId,
		build_router: N::BuildTableRouter,
		max_block_data_size: Option<u64>,
		authorities_num: usize,
		local_id: ValidatorIndex,
		exit: exit_future::Exit,
	) {
		let (collators, client) = (self.collators.clone(), self.client.clone());
		let availability_store = self.availability_store.clone();

		let with_router = move |router: N::TableRouter| {
			// fetch a local collation from connected collators.
			let collation_work = collation_fetch(
				validation_para,
				relay_parent,
				collators,
				client.clone(),
				max_block_data_size,
			);

			collation_work.map(move |result| match result {
				Ok((collation, outgoing_targeted, fees_charged)) => {
					match produce_receipt_and_chunks(
						authorities_num,
						&collation.pov,
						&outgoing_targeted,
						fees_charged,
						&collation.info,
					) {
						Ok((receipt, chunks)) => {
							// Apparently the `async move` block is the only way to convince
							// the compiler that we are not moving values out of borrowed context.
							let av_clone = availability_store.clone();
							let chunks_clone = chunks.clone();
							let receipt_clone = receipt.clone();

							let res = async move {
								if let Err(e) = av_clone.clone().add_erasure_chunks(
									relay_parent.clone(),
									receipt_clone,
									chunks_clone,
								).await {
									warn!(target: "validation", "Failed to add erasure chunks: {}", e);
								}
							}
							.unit_error()
							.boxed()
							.then(move |_| {
								router.local_collation(collation, receipt, outgoing_targeted, (local_id, &chunks));
								ready(())
							});

							Ok(Some(res))
						}
						Err(e) => {
							warn!(target: "validation", "Failed to produce a receipt: {:?}", e);
							Ok(None)
						}
					}
				}
				Err(e) => {
					warn!(target: "validation", "Failed to collate candidate: {:?}", e);
					Ok(None)
				}
			})
		};

		let router = build_router
			.map_ok(with_router)
			.map_err(|e| {
				warn!(target: "validation" , "Failed to build table router: {:?}", e);
			})
			.and_then(|f| f)
			.and_then(|f| match f {
				Some(f) => f.map(Ok).boxed(),
				None => ready(Ok(())).boxed(),
			}).boxed();

		let cancellable_work = select(exit, router).map(drop);

		// spawn onto thread pool.
		if self.handle.spawn(cancellable_work).is_err() {
			error!("Failed to spawn cancellable work task");
		}
	}
}

/// Parachain validation for a single block.
struct AttestationTracker {
	_drop_signal: exit_future::Signal,
	table: Arc<SharedTable>,
	started: Instant,
}

/// Polkadot proposer factory.
pub struct ProposerFactory<C, N, P, SC, TxPool, B> {
	parachain_validation: Arc<ParachainValidation<C, N, P>>,
	transaction_pool: Arc<TxPool>,
	keystore: KeyStorePtr,
	_service_handle: ServiceHandle,
	babe_slot_duration: u64,
	_select_chain: SC,
	max_block_data_size: Option<u64>,
	backend: Arc<B>,
}

impl<C, N, P, SC, TxPool, B> ProposerFactory<C, N, P, SC, TxPool, B> where
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
	TxPool: TransactionPool,
	SC: SelectChain<Block> + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HasherFor<Block>>,
{
	/// Create a new proposer factory.
	pub fn new(
		client: Arc<P>,
		_select_chain: SC,
		network: N,
		collators: C,
		transaction_pool: Arc<TxPool>,
		thread_pool: TaskExecutor,
		keystore: KeyStorePtr,
		availability_store: AvailabilityStore,
		babe_slot_duration: u64,
		max_block_data_size: Option<u64>,
		backend: Arc<B>,
	) -> Self {
		let parachain_validation = Arc::new(ParachainValidation {
			client: client.clone(),
			network,
			collators,
			handle: thread_pool.clone(),
			availability_store: availability_store.clone(),
			live_instances: Mutex::new(HashMap::new()),
		});

		let service_handle = crate::attestation_service::start(
			client,
			_select_chain.clone(),
			parachain_validation.clone(),
			thread_pool,
			keystore.clone(),
			max_block_data_size,
		);

		ProposerFactory {
			parachain_validation,
			transaction_pool,
			keystore,
			_service_handle: service_handle,
			babe_slot_duration,
			_select_chain,
			max_block_data_size,
			backend,
		}
	}
}

impl<C, N, P, SC, TxPool, B> consensus::Environment<Block> for ProposerFactory<C, N, P, SC, TxPool, B> where
	C: Collators + Send + Unpin + 'static + Sync,
	N: Network,
	TxPool: TransactionPool<Block=Block> + 'static,
	P: ProvideRuntimeApi<Block> + HeaderBackend<Block> + BlockBody<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> +
		BlockBuilderApi<Block> +
		BabeApi<Block> +
		ApiExt<Block, Error = sp_blockchain::Error>,
	C::Collation: Send + Unpin + 'static,
	N::TableRouter: Send + 'static,
	N::BuildTableRouter: Send + Unpin + 'static,
	SC: SelectChain<Block>,
	B: Backend<Block, State = sp_api::StateBackendFor<P, Block>> + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<P, Block>: sp_api::StateBackend<HasherFor<Block>> + Send,
{
	type CreateProposer = Pin<Box<
		dyn Future<Output = Result<Self::Proposer, Self::Error>> + Send + Unpin + 'static
	>>;
	type Proposer = Proposer<P, TxPool, B>;
	type Error = Error;

	fn init(
		&mut self,
		parent_header: &Header,
	) -> Self::CreateProposer {
		let parent_hash = parent_header.hash();
		let parent_id = BlockId::hash(parent_hash);

		let maybe_proposer = self.parachain_validation.get_or_instantiate(
			parent_hash,
			&self.keystore,
			self.max_block_data_size,
		).map(|tracker| Proposer {
			client: self.parachain_validation.client.clone(),
			tracker,
			parent_hash,
			parent_id,
			parent_number: parent_header.number,
			transaction_pool: self.transaction_pool.clone(),
			slot_duration: self.babe_slot_duration,
			backend: self.backend.clone(),
		});

		Box::pin(future::ready(maybe_proposer))
	}
}

/// The local duty of a validator.
#[derive(Debug)]
pub struct LocalDuty {
	validation: Chain,
	index: ValidatorIndex,
}

/// The Polkadot proposer logic.
pub struct Proposer<Client, TxPool, Backend> {
	client: Arc<Client>,
	parent_hash: Hash,
	parent_id: BlockId,
	parent_number: BlockNumber,
	tracker: Arc<AttestationTracker>,
	transaction_pool: Arc<TxPool>,
	slot_duration: u64,
	backend: Arc<Backend>,
}

impl<Client, TxPool, Backend> consensus::Proposer<Block> for Proposer<Client, TxPool, Backend> where
	TxPool: TransactionPool<Block=Block> + 'static,
	Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync + 'static,
	Client::Api: ParachainHost<Block> + BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	Backend: sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>> + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HasherFor<Block>> + Send,
{
	type Error = Error;
	type Transaction = sp_api::TransactionFor<Client, Block>;
	type Proposal = Pin<
		Box<
			dyn Future<Output = Result<Proposal<Block, sp_api::TransactionFor<Client, Block>>, Error>>
				+ Send
		>
	>;

	fn propose(&mut self,
		inherent_data: InherentData,
		inherent_digests: DigestFor<Block>,
		max_duration: Duration,
		record_proof: RecordProof,
	) -> Self::Proposal {
		const SLOT_DURATION_DENOMINATOR: u64 = 3; // wait up to 1/3 of the slot for candidates.

		let initial_included = self.tracker.table.includable_count();
		let now = Instant::now();

		let dynamic_inclusion = DynamicInclusion::new(
			self.tracker.table.num_parachains(),
			self.tracker.started,
			Duration::from_millis(self.slot_duration / SLOT_DURATION_DENOMINATOR),
		);

		let parent_hash = self.parent_hash.clone();
		let parent_number = self.parent_number.clone();
		let parent_id = self.parent_id.clone();
		let client = self.client.clone();
		let transaction_pool = self.transaction_pool.clone();
		let table = self.tracker.table.clone();
		let backend = self.backend.clone();

		async move {
			let enough_candidates = dynamic_inclusion.acceptable_in(
				now,
				initial_included,
			).unwrap_or_else(|| Duration::from_millis(1));

			let believed_timestamp = match inherent_data.timestamp_inherent_data() {
				Ok(timestamp) => timestamp,
				Err(e) => return Err(Error::InherentError(e)),
			};

			let deadline_diff = max_duration - max_duration / 3;
			let deadline = match Instant::now().checked_add(deadline_diff) {
				None => return Err(Error::DeadlineComputeFailure(deadline_diff)),
				Some(d) => d,
			};

			let data = CreateProposalData {
				parent_hash,
				parent_number,
				parent_id,
				client,
				transaction_pool,
				table,
				believed_minimum_timestamp: believed_timestamp,
				inherent_data: Some(inherent_data),
				inherent_digests,
				// leave some time for the proposal finalisation
				deadline,
				record_proof,
				backend,
			};

			// set up delay until next allowed timestamp.
			let current_timestamp = current_timestamp();
			if current_timestamp < believed_timestamp {
				Delay::new(Duration::from_millis(current_timestamp - believed_timestamp))
					.await;
			}

			Delay::new(enough_candidates).await;

			tokio_executor::blocking::run(move || {
				let proposed_candidates = data.table.proposed_set();
				data.propose_with(proposed_candidates)
			})
				.await
		}.boxed()
	}
}

fn current_timestamp() -> u64 {
	time::SystemTime::now().duration_since(time::UNIX_EPOCH)
		.expect("now always later than unix epoch; qed")
		.as_millis() as u64
}

/// Inner data of the create proposal.
struct CreateProposalData<Client, TxPool, Backend> {
	parent_hash: Hash,
	parent_number: BlockNumber,
	parent_id: BlockId,
	client: Arc<Client>,
	transaction_pool: Arc<TxPool>,
	table: Arc<SharedTable>,
	believed_minimum_timestamp: u64,
	inherent_data: Option<InherentData>,
	inherent_digests: DigestFor<Block>,
	deadline: Instant,
	record_proof: RecordProof,
	backend: Arc<Backend>,
}

impl<Client, TxPool, Backend> CreateProposalData<Client, TxPool, Backend> where
	TxPool: TransactionPool<Block=Block>,
	Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
	Client::Api: ParachainHost<Block> + BlockBuilderApi<Block> + ApiExt<Block, Error = sp_blockchain::Error>,
	Backend: sc_client_api::Backend<Block, State = sp_api::StateBackendFor<Client, Block>> + 'static,
	// Rust bug: https://github.com/rust-lang/rust/issues/24159
	sp_api::StateBackendFor<Client, Block>: sp_api::StateBackend<HasherFor<Block>> + Send,
{
	fn propose_with(
		mut self,
		candidates: Vec<AttestedCandidate>,
	) -> Result<Proposal<Block, sp_api::TransactionFor<Client, Block>>, Error> {
		use runtime_primitives::traits::{Hash as HashT, BlakeTwo256};

		const MAX_TRANSACTIONS: usize = 40;

		let mut inherent_data = self.inherent_data
			.take()
			.expect("CreateProposal is not polled after finishing; qed");
		inherent_data.put_data(NEW_HEADS_IDENTIFIER, &candidates)
			.map_err(Error::InherentError)?;

		let runtime_api = self.client.runtime_api();

		let mut block_builder = block_builder::BlockBuilder::new(
			&*self.client,
			self.client.expect_block_hash_from_id(&self.parent_id)?,
			self.client.expect_block_number_from_id(&self.parent_id)?,
			self.record_proof,
			self.inherent_digests.clone(),
			&*self.backend,
		)?;

		{
			let inherents = runtime_api.inherent_extrinsics(&self.parent_id, inherent_data)?;
			for inherent in inherents {
				block_builder.push(inherent)?;
			}

			let mut unqueue_invalid = Vec::new();
			let mut pending_size = 0;

			let ready_iter = self.transaction_pool.ready();
			for ready in ready_iter.take(MAX_TRANSACTIONS) {
				let encoded_size = ready.data().encode().len();
				if pending_size + encoded_size >= MAX_TRANSACTIONS_SIZE {
					break;
				}
				if Instant::now() > self.deadline {
					debug!("Consensus deadline reached when pushing block transactions, proceeding with proposing.");
					break;
				}

				match block_builder.push(ready.data().clone()) {
					Ok(()) => {
						debug!("[{:?}] Pushed to the block.", ready.hash());
						pending_size += encoded_size;
					}
					Err(sp_blockchain::Error::ApplyExtrinsicFailed(sp_blockchain::ApplyExtrinsicFailed::Validity(e)))
						if e.exhausted_resources() =>
					{
						debug!("Block is full, proceed with proposing.");
						break;
					}
					Err(e) => {
						trace!(target: "transaction-pool", "Invalid transaction: {}", e);
						unqueue_invalid.push(ready.hash().clone());
					}
				}
			}

			self.transaction_pool.remove_invalid(&unqueue_invalid);
		}

		let (new_block, storage_changes, proof) = block_builder.build()?.into_inner();

		info!("Prepared block for proposing at {} [hash: {:?}; parent_hash: {}; extrinsics: [{}]]",
			new_block.header.number,
			Hash::from(new_block.header.hash()),
			new_block.header.parent_hash,
			new_block.extrinsics.iter()
				.map(|xt| format!("{}", BlakeTwo256::hash_of(xt)))
				.collect::<Vec<_>>()
				.join(", ")
		);

		// TODO: full re-evaluation (https://github.com/paritytech/polkadot/issues/216)
		let active_parachains = runtime_api.active_parachains(&self.parent_id)?;
		assert!(evaluation::evaluate_initial(
			&new_block,
			self.believed_minimum_timestamp,
			&self.parent_hash,
			self.parent_number,
			&active_parachains[..],
		).is_ok());

		Ok(Proposal { block: new_block, storage_changes, proof })
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_keyring::Sr25519Keyring;

	#[test]
	fn sign_and_check_statement() {
		let statement: Statement = GenericStatement::Valid([1; 32].into());
		let parent_hash = [2; 32].into();

		let sig = sign_table_statement(&statement, &Sr25519Keyring::Alice.pair().into(), &parent_hash);

		assert!(check_statement(&statement, &sig, Sr25519Keyring::Alice.public().into(), &parent_hash));
		assert!(!check_statement(&statement, &sig, Sr25519Keyring::Alice.public().into(), &[0xff; 32].into()));
		assert!(!check_statement(&statement, &sig, Sr25519Keyring::Bob.public().into(), &parent_hash));
	}
}
