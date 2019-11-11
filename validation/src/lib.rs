// Copyright 2017 Parity Technologies (UK) Ltd.
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

use std::{collections::{HashMap, HashSet}, pin::Pin, sync::Arc, time::{self, Duration, Instant}};

use babe_primitives::BabeApi;
use client::{BlockchainEvents, BlockBody};
use client::blockchain::HeaderBackend;
use client::block_builder::api::BlockBuilder as BlockBuilderApi;
use client::error as client_error;
use codec::Encode;
use consensus::SelectChain;
use availability_store::Store as AvailabilityStore;
use parking_lot::Mutex;
use polkadot_primitives::{Hash, Block, BlockId, BlockNumber, Header};
use polkadot_primitives::parachain::{
	Id as ParaId, Chain, DutyRoster, OutgoingMessages, CandidateReceipt,
	ParachainHost, AttestedCandidate, Statement as PrimitiveStatement, Message,
	Collation, PoVBlock, ValidatorSignature, ValidatorPair, ValidatorId
};
use primitives::Pair;
use runtime_primitives::traits::{ProvideRuntimeApi, DigestFor};
use futures_timer::{Delay, Interval};
use transaction_pool::txpool::{Pool, ChainApi as PoolChainApi};

use attestation_service::ServiceHandle;
use futures::prelude::*;
use futures03::{future::{self, Either, FutureExt}, task::Context, stream::StreamExt};
use collation::CollationFetch;
use dynamic_inclusion::DynamicInclusion;
use inherents::InherentData;
use runtime_babe::timestamp::TimestampInherentData;
use log::{info, debug, warn, trace, error};
use keystore::KeyStorePtr;

type TaskExecutor = Arc<dyn futures::future::Executor<Box<dyn Future<Item = (), Error = ()> + Send>> + Send + Sync>;

pub use self::collation::{
	validate_collation, validate_incoming, message_queue_root, egress_roots, Collators,
};
pub use self::error::Error;
pub use self::shared_table::{
	SharedTable, ParachainWork, PrimedParachainWork, Validated, Statement, SignedStatement,
	GenericStatement,
};
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
	type FetchValidationProof: IntoFuture<Item=PoVBlock,Error=Self::Error>;

	/// Call with local candidate data. This will make the data available on the network,
	/// and sign, import, and broadcast a statement about the candidate.
	fn local_collation(&self, collation: Collation, outgoing: OutgoingMessages);

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
	type BuildTableRouter: IntoFuture<Item=Self::TableRouter,Error=Self::Error>;

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
pub fn check_statement(statement: &Statement, signature: &ValidatorSignature, signer: ValidatorId, parent_hash: &Hash) -> bool {
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
	let mut map = HashMap::new();

	let duty_iter = authorities.iter().zip(&roster.validator_duty);
	for (authority, v_duty) in duty_iter {
		if Some(authority) == local_id.as_ref() {
			local_validation = Some(v_duty.clone());
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
		validation: v
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
	C: Collators + Send + 'static,
	N: Network,
	P: ProvideRuntimeApi + HeaderBackend<Block> + BlockBody<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
	<C::Collation as IntoFuture>::Future: Send + 'static,
	N::TableRouter: Send + 'static,
	<N::BuildTableRouter as IntoFuture>::Future: Send + 'static,
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

		if let Some(Chain::Parachain(id)) = local_duty.as_ref().map(|d| d.validation) {
			self.launch_work(parent_hash, id, router, max_block_data_size, exit);
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
		exit: exit_future::Exit,
	) {
		use availability_store::Data;

		let (collators, client) = (self.collators.clone(), self.client.clone());
		let availability_store = self.availability_store.clone();

		let with_router = move |router: N::TableRouter| {
			// fetch a local collation from connected collators.
			let collation_work = CollationFetch::new(
				validation_para,
				relay_parent,
				collators,
				client,
				max_block_data_size,
			);

			collation_work.then(move |result| match result {
				Ok((collation, outgoing_targeted)) => {
					let outgoing_queues = crate::outgoing_queues(&outgoing_targeted)
						.map(|(_target, root, data)| (root, data))
						.collect();

					let res = availability_store.make_available(Data {
						relay_parent,
						parachain_id: collation.receipt.parachain_index,
						candidate_hash: collation.receipt.hash(),
						block_data: collation.pov.block_data.clone(),
						outgoing_queues: Some(outgoing_queues),
					});

					match res {
						Ok(()) => {
							// TODO TODO: here we gossip the collation with router
							// TODO: https://github.com/paritytech/polkadot/issues/51
							// Erasure-code and provide merkle branches.
							router.local_collation(collation, outgoing_targeted);
						}
						Err(e) => warn!(
							target: "validation",
							"Failed to make collation data available: {:?}",
							e,
						),
					}

					Ok(())
				}
				Err(e) => {
					warn!(target: "validation", "Failed to collate candidate: {:?}", e);
					Ok(())
				}
			})
		};

		let cancellable_work = build_router
			.into_future()
			.map_err(|e| {
				warn!(target: "validation" , "Failed to build table router: {:?}", e);
			})
			.and_then(with_router)
			.select(exit)
			.then(|_| Ok(()));

		// spawn onto thread pool.
		if self.handle.execute(Box::new(cancellable_work)).is_err() {
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
pub struct ProposerFactory<C, N, P, SC, TxApi: PoolChainApi> {
	parachain_validation: Arc<ParachainValidation<C, N, P>>,
	transaction_pool: Arc<Pool<TxApi>>,
	keystore: KeyStorePtr,
	_service_handle: ServiceHandle,
	babe_slot_duration: u64,
	_select_chain: SC,
	max_block_data_size: Option<u64>,
}

impl<C, N, P, SC, TxApi> ProposerFactory<C, N, P, SC, TxApi> where
	C: Collators + Send + Sync + 'static,
	<C::Collation as IntoFuture>::Future: Send + 'static,
	P: BlockchainEvents<Block> + BlockBody<Block>,
	P: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + BlockBuilderApi<Block> + BabeApi<Block>,
	N: Network + Send + Sync + 'static,
	N::TableRouter: Send + 'static,
	<N::BuildTableRouter as IntoFuture>::Future: Send + 'static,
	TxApi: PoolChainApi,
	SC: SelectChain<Block> + 'static,
{
	/// Create a new proposer factory.
	pub fn new(
		client: Arc<P>,
		_select_chain: SC,
		network: N,
		collators: C,
		transaction_pool: Arc<Pool<TxApi>>,
		thread_pool: TaskExecutor,
		keystore: KeyStorePtr,
		availability_store: AvailabilityStore,
		babe_slot_duration: u64,
		max_block_data_size: Option<u64>,
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
			availability_store,
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
		}
	}
}

impl<C, N, P, SC, TxApi> consensus::Environment<Block> for ProposerFactory<C, N, P, SC, TxApi> where
	C: Collators + Send + 'static,
	N: Network,
	TxApi: PoolChainApi<Block=Block> + 'static,
	P: ProvideRuntimeApi + HeaderBackend<Block> + BlockBody<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + BlockBuilderApi<Block> + BabeApi<Block>,
	<C::Collation as IntoFuture>::Future: Send + 'static,
	N::TableRouter: Send + 'static,
	<N::BuildTableRouter as IntoFuture>::Future: Send + 'static,
	SC: SelectChain<Block>,
{
	type Proposer = Proposer<P, TxApi>;
	type Error = Error;

	fn init(
		&mut self,
		parent_header: &Header,
	) -> Result<Self::Proposer, Error> {
		let parent_hash = parent_header.hash();
		let parent_id = BlockId::hash(parent_hash);

		let tracker = self.parachain_validation.get_or_instantiate(
			parent_hash,
			&self.keystore,
			self.max_block_data_size,
		)?;

		Ok(Proposer {
			client: self.parachain_validation.client.clone(),
			tracker,
			parent_hash,
			parent_id,
			parent_number: parent_header.number,
			transaction_pool: self.transaction_pool.clone(),
			slot_duration: self.babe_slot_duration,
		})
	}
}

/// The local duty of a validator.
#[derive(Debug)]
pub struct LocalDuty {
	validation: Chain,
}

/// The Polkadot proposer logic.
pub struct Proposer<C: Send + Sync, TxApi: PoolChainApi> where
	C: ProvideRuntimeApi + HeaderBackend<Block>,
{
	client: Arc<C>,
	parent_hash: Hash,
	parent_id: BlockId,
	parent_number: BlockNumber,
	tracker: Arc<AttestationTracker>,
	transaction_pool: Arc<Pool<TxApi>>,
	slot_duration: u64,
}

type Proof = Vec<Vec<u8>>;

impl<C, TxApi> consensus::Proposer<Block> for Proposer<C, TxApi> where
	TxApi: PoolChainApi<Block=Block> + 'static,
	C: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync + 'static,
	C::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
{
	type Error = Error;
	type Proposal = Either<
		CreateProposal<C, TxApi>, future::Ready<Result<(Block, Option<Proof>), Error>>
	>;

	fn propose(&mut self,
		inherent_data: InherentData,
		inherent_digests: DigestFor<Block>,
		max_duration: Duration,
		record_proof: bool,
	) -> Self::Proposal {
		const ATTEMPT_PROPOSE_EVERY: Duration = Duration::from_millis(100);
		const SLOT_DURATION_DENOMINATOR: u64 = 3; // wait up to 1/3 of the slot for candidates.

		let initial_included = self.tracker.table.includable_count();
		let now = Instant::now();

		let dynamic_inclusion = DynamicInclusion::new(
			self.tracker.table.num_parachains(),
			self.tracker.started,
			Duration::from_millis(self.slot_duration / SLOT_DURATION_DENOMINATOR),
		);

		let enough_candidates = dynamic_inclusion.acceptable_in(
			now,
			initial_included,
		).unwrap_or_else(|| Duration::from_millis(1));

		let believed_timestamp = match inherent_data.timestamp_inherent_data() {
			Ok(timestamp) => timestamp,
			Err(e) => return Either::Right(future::err(Error::InherentError(e))),
		};

		// set up delay until next allowed timestamp.
		let current_timestamp = current_timestamp();
		let delay_future = if current_timestamp >= believed_timestamp {
			None
		} else {
			Some(Delay::new(Duration::from_millis (current_timestamp - believed_timestamp)))
		};

		let timing = ProposalTiming {
			minimum: delay_future,
			attempt_propose: Interval::new(ATTEMPT_PROPOSE_EVERY),
			enough_candidates: Delay::new(enough_candidates),
			dynamic_inclusion,
			last_included: initial_included,
		};

		let deadline_diff = max_duration - max_duration / 3;
		let deadline = match Instant::now().checked_add(deadline_diff) {
			None => return Either::Right(
				future::err(Error::DeadlineComputeFailure(deadline_diff)),
			),
			Some(d) => d,
		};

		Either::Left(CreateProposal {
			parent_hash: self.parent_hash.clone(),
			parent_number: self.parent_number.clone(),
			parent_id: self.parent_id.clone(),
			client: self.client.clone(),
			transaction_pool: self.transaction_pool.clone(),
			table: self.tracker.table.clone(),
			believed_minimum_timestamp: believed_timestamp,
			timing,
			inherent_data: Some(inherent_data),
			inherent_digests,
			// leave some time for the proposal finalisation
			deadline,
			record_proof,
		})
	}
}

fn current_timestamp() -> u64 {
	time::SystemTime::now().duration_since(time::UNIX_EPOCH)
		.expect("now always later than unix epoch; qed")
		.as_millis() as u64
}

struct ProposalTiming {
	minimum: Option<Delay>,
	attempt_propose: Interval,
	dynamic_inclusion: DynamicInclusion,
	enough_candidates: Delay,
	last_included: usize,
}

impl ProposalTiming {
	// whether it's time to attempt a proposal.
	// shouldn't be called outside of the context of a task.
	fn poll(&mut self, cx: &mut Context, included: usize) -> futures03::Poll<Result<(), Error>> {
		// first drain from the interval so when the minimum delay is up
		// we don't have any notifications built up.
		//
		// this interval is just meant to produce periodic task wakeups
		// that lead to the `dynamic_inclusion` getting updated as necessary.
		while let futures03::Poll::Ready(x) = self.attempt_propose.poll_next_unpin(cx) {
			x.expect("timer still alive; intervals never end; qed");
		}

		// wait until the minimum time has passed.
		if let Some(mut minimum) = self.minimum.take() {
			if let futures03::Poll::Pending = minimum.poll_unpin(cx) {
				self.minimum = Some(minimum);
				return futures03::Poll::Pending;
			}
		}

		if included == self.last_included {
			return self.enough_candidates.poll_unpin(cx).map_err(Error::Timer);
		}

		// the amount of includable candidates has changed. schedule a wakeup
		// if it's not sufficient anymore.
		match self.dynamic_inclusion.acceptable_in(Instant::now(), included) {
			Some(instant) => {
				self.last_included = included;
				self.enough_candidates.reset(instant);
				self.enough_candidates.poll_unpin(cx).map_err(Error::Timer)
			}
			None => futures03::Poll::Ready(Ok(())),
		}
	}
}

/// Future which resolves upon the creation of a proposal.
pub struct CreateProposal<C: Send + Sync, TxApi: PoolChainApi> {
	parent_hash: Hash,
	parent_number: BlockNumber,
	parent_id: BlockId,
	client: Arc<C>,
	transaction_pool: Arc<Pool<TxApi>>,
	table: Arc<SharedTable>,
	timing: ProposalTiming,
	believed_minimum_timestamp: u64,
	inherent_data: Option<InherentData>,
	inherent_digests: DigestFor<Block>,
	deadline: Instant,
	record_proof: bool,
}

impl<C, TxApi> CreateProposal<C, TxApi> where
	TxApi: PoolChainApi<Block=Block>,
	C: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync,
	C::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
{
	fn propose_with(
		&mut self,
		candidates: Vec<AttestedCandidate>,
	) -> Result<(Block, Option<Proof>), Error> {
		use client::block_builder::BlockBuilder;
		use runtime_primitives::traits::{Hash as HashT, BlakeTwo256};

		const MAX_TRANSACTIONS: usize = 40;

		let mut inherent_data = self.inherent_data
			.take()
			.expect("CreateProposal is not polled after finishing; qed");
		inherent_data.put_data(
			polkadot_runtime::NEW_HEADS_IDENTIFIER,
			&candidates,
		).map_err(Error::InherentError)?;

		let runtime_api = self.client.runtime_api();

		let mut block_builder = BlockBuilder::at_block(
			&self.parent_id,
			&*self.client,
			self.record_proof,
			self.inherent_digests.clone(),
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
				let encoded_size = ready.data.encode().len();
				if pending_size + encoded_size >= MAX_TRANSACTIONS_SIZE {
					break;
				}
				if Instant::now() > self.deadline {
					debug!("Consensus deadline reached when pushing block transactions, proceeding with proposing.");
					break;
				}

				match block_builder.push(ready.data.clone()) {
					Ok(()) => {
						debug!("[{:?}] Pushed to the block.", ready.hash);
						pending_size += encoded_size;
					}
					Err(client_error::Error::ApplyExtrinsicFailed(e)) if e.exhausted_resources() => {
						debug!("Block is full, proceed with proposing.");
						break;
					}
					Err(e) => {
						trace!(target: "transaction-pool", "Invalid transaction: {}", e);
						unqueue_invalid.push(ready.hash.clone());
					}
				}
			}

			self.transaction_pool.remove_invalid(&unqueue_invalid);
		}

		let (new_block, proof) = block_builder.bake_and_extract_proof()?;

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

		Ok((new_block, proof))
	}
}

impl<C, TxApi> futures03::Future for CreateProposal<C, TxApi> where
	TxApi: PoolChainApi<Block=Block>,
	C: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync,
	C::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
{
	type Output = Result<(Block, Option<Proof>), Error>;

	fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> futures03::Poll<Self::Output> {
		// 1. try to propose if we have enough includable candidates and other
		// delays have concluded.
		let included = self.table.includable_count();
		futures03::ready!(self.timing.poll(cx, included))?;

		// 2. propose
		let proposed_candidates = self.table.proposed_set();

		futures03::Poll::Ready(self.propose_with(proposed_candidates))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use substrate_keyring::Sr25519Keyring;

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
