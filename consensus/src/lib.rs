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

extern crate parking_lot;
extern crate polkadot_availability_store as extrinsic_store;
extern crate polkadot_statement_table as table;
extern crate polkadot_parachain as parachain;
extern crate polkadot_runtime;
extern crate polkadot_primitives;

extern crate parity_codec as codec;
extern crate substrate_inherents as inherents;
extern crate substrate_primitives as primitives;
extern crate srml_aura as runtime_aura;
extern crate srml_support as runtime_support;
extern crate sr_primitives as runtime_primitives;
extern crate substrate_client as client;

extern crate exit_future;
extern crate tokio;
extern crate substrate_consensus_common as consensus;
extern crate substrate_consensus_aura as aura;
extern crate substrate_consensus_aura_primitives as aura_primitives;
extern crate substrate_finality_grandpa as grandpa;
extern crate substrate_transaction_pool as transaction_pool;
extern crate substrate_consensus_authorities as consensus_authorities;

#[macro_use]
extern crate error_chain;

#[macro_use]
extern crate futures;

#[macro_use]
extern crate log;

#[cfg(test)]
extern crate substrate_keyring;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{self, Duration, Instant};

use aura::SlotDuration;
use client::{BlockchainEvents, ChainHead, BlockBody};
use client::blockchain::HeaderBackend;
use client::block_builder::api::BlockBuilder as BlockBuilderApi;
use client::runtime_api::Core;
use codec::Encode;
use extrinsic_store::Store as ExtrinsicStore;
use parking_lot::Mutex;
use polkadot_primitives::{
	Hash, Block, BlockId, BlockNumber, Header, SessionKey
};
use polkadot_primitives::parachain::{Id as ParaId, Chain, DutyRoster, BlockData, Extrinsic as ParachainExtrinsic, CandidateReceipt, CollatorSignature};
use polkadot_primitives::parachain::{AttestedCandidate, ParachainHost, Statement as PrimitiveStatement};
use primitives::{Pair, ed25519::{self, Public as AuthorityId}};
use runtime_primitives::traits::ProvideRuntimeApi;
use tokio::runtime::TaskExecutor;
use tokio::timer::{Delay, Interval};
use transaction_pool::txpool::{Pool, ChainApi as PoolChainApi};

use attestation_service::ServiceHandle;
use futures::prelude::*;
use futures::future::{self, Either};
use collation::CollationFetch;
use dynamic_inclusion::DynamicInclusion;
use inherents::InherentData;
use runtime_aura::timestamp::TimestampInherentData;
use consensus_authorities::AuthoritiesApi;

pub use self::collation::{validate_collation, Collators};
pub use self::error::{ErrorKind, Error};
pub use self::shared_table::{SharedTable, StatementProducer, ProducedStatements, Statement, SignedStatement, GenericStatement};

mod attestation_service;
mod dynamic_inclusion;
mod evaluation;
mod error;
mod shared_table;

pub mod collation;

// block size limit.
const MAX_TRANSACTIONS_SIZE: usize = 4 * 1024 * 1024;

/// A handle to a statement table router.
///
/// This is expected to be a lightweight, shared type like an `Arc`.
pub trait TableRouter: Clone {
	/// Errors when fetching data from the network.
	type Error;
	/// Future that resolves when candidate data is fetched.
	type FetchCandidate: IntoFuture<Item=BlockData,Error=Self::Error>;
	/// Future that resolves when extrinsic candidate data is fetched.
	type FetchExtrinsic: IntoFuture<Item=ParachainExtrinsic,Error=Self::Error>;

	/// Call with local candidate data. This will make the data available on the network,
	/// and sign, import, and broadcast a statement about the candidate.
	fn local_candidate(&self, candidate: CandidateReceipt, block_data: BlockData, extrinsic: ParachainExtrinsic);

	/// Fetch block data for a specific candidate.
	fn fetch_block_data(&self, candidate: &CandidateReceipt) -> Self::FetchCandidate;

	/// Fetch extrinsic data for a specific candidate.
	fn fetch_extrinsic_data(&self, candidate: &CandidateReceipt) -> Self::FetchExtrinsic;
}

/// A long-lived network which can create parachain statement and BFT message routing processes on demand.
pub trait Network {
	/// The table router type. This should handle importing of any statements,
	/// routing statements to peers, and driving completion of any `StatementProducers`.
	type TableRouter: TableRouter;

	/// Instantiate a table router using the given shared table and task executor.
	fn communication_for(
		&self,
		validators: &[SessionKey],
		table: Arc<SharedTable>,
		task_executor: TaskExecutor
	) -> Self::TableRouter;
}

/// Information about a specific group.
#[derive(Debug, Clone, Default)]
pub struct GroupInfo {
	/// Authorities meant to check validity of candidates.
	pub validity_guarantors: HashSet<SessionKey>,
	/// Authorities meant to check availability of candidate data.
	pub availability_guarantors: HashSet<SessionKey>,
	/// Number of votes needed for validity.
	pub needed_validity: usize,
	/// Number of votes needed for availability.
	pub needed_availability: usize,
}

/// Sign a table statement against a parent hash.
/// The actual message signed is the encoded statement concatenated with the
/// parent hash.
pub fn sign_table_statement(statement: &Statement, key: &ed25519::Pair, parent_hash: &Hash) -> CollatorSignature {
	// we sign using the primitive statement type because that's what the runtime
	// expects. These types probably encode the same way so this clone could be optimized
	// out in the future.
	let mut encoded = PrimitiveStatement::from(statement.clone()).encode();
	encoded.extend(parent_hash.as_ref());

	key.sign(&encoded).into()
}

/// Check signature on table statement.
pub fn check_statement(statement: &Statement, signature: &CollatorSignature, signer: SessionKey, parent_hash: &Hash) -> bool {
	use runtime_primitives::traits::Verify;

	let mut encoded = PrimitiveStatement::from(statement.clone()).encode();
	encoded.extend(parent_hash.as_ref());

	signature.verify(&encoded[..], &signer.into())
}

fn make_group_info(roster: DutyRoster, authorities: &[AuthorityId], local_id: AuthorityId) -> Result<(HashMap<ParaId, GroupInfo>, LocalDuty), Error> {
	if roster.validator_duty.len() != authorities.len() {
		bail!(ErrorKind::InvalidDutyRosterLength(authorities.len(), roster.validator_duty.len()))
	}

	if roster.guarantor_duty.len() != authorities.len() {
		bail!(ErrorKind::InvalidDutyRosterLength(authorities.len(), roster.guarantor_duty.len()))
	}

	let mut local_validation = None;
	let mut map = HashMap::new();

	let duty_iter = authorities.iter().zip(&roster.validator_duty).zip(&roster.guarantor_duty);
	for ((authority, v_duty), a_duty) in duty_iter {
		if authority == &local_id {
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

		match *a_duty {
			Chain::Relay => {}, // does nothing for now.
			Chain::Parachain(ref id) => {
				map.entry(id.clone()).or_insert_with(GroupInfo::default)
					.availability_guarantors
					.insert(authority.clone());
			}
		}
	}

	for live_group in map.values_mut() {
		let validity_len = live_group.validity_guarantors.len();
		let availability_len = live_group.availability_guarantors.len();

		live_group.needed_validity = validity_len / 2 + validity_len % 2;
		live_group.needed_availability = availability_len / 2 + availability_len % 2;
	}

	match local_validation {
		Some(local_validation) => {
			let local_duty = LocalDuty {
				validation: local_validation,
			};

			Ok((map, local_duty))
		}
		None => bail!(ErrorKind::NotValidator(local_id)),
	}
}

/// Constructs parachain-agreement instances.
struct ParachainConsensus<C, N, P> {
	/// The client instance.
	client: Arc<P>,
	/// The backing network handle.
	network: N,
	/// Parachain collators.
	collators: C,
	/// handle to remote task executor
	handle: TaskExecutor,
	/// Store for extrinsic data.
	extrinsic_store: ExtrinsicStore,
	/// Live agreements.
	live_instances: Mutex<HashMap<Hash, Arc<AttestationTracker>>>,

}

impl<C, N, P> ParachainConsensus<C, N, P> where
	C: Collators + Send + 'static,
	N: Network,
	P: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
	<C::Collation as IntoFuture>::Future: Send + 'static,
	N::TableRouter: Send + 'static,
{
	/// Get an attestation table for given parent hash.
	///
	/// This starts a parachain agreement process for given parent hash if
	/// one has not already started.
	fn get_or_instantiate(
		&self,
		parent_hash: Hash,
		authorities: &[AuthorityId],
		sign_with: Arc<ed25519::Pair>,
	)
		-> Result<Arc<AttestationTracker>, Error>
	{
		let mut live_instances = self.live_instances.lock();
		if let Some(tracker) = live_instances.get(&parent_hash) {
			return Ok(tracker.clone());
		}

		let id = BlockId::hash(parent_hash);
		let duty_roster = self.client.runtime_api().duty_roster(&id)?;

		let (group_info, local_duty) = make_group_info(
			duty_roster,
			authorities,
			sign_with.public().into(),
		)?;

		info!("Starting parachain attestation session on top of parent {:?}. Local parachain duty is {:?}",
			parent_hash, local_duty.validation);

		let active_parachains = self.client.runtime_api().active_parachains(&id)?;

		debug!(target: "consensus", "Active parachains: {:?}", active_parachains);

		let table = Arc::new(SharedTable::new(group_info, sign_with.clone(), parent_hash, self.extrinsic_store.clone()));
		let router = self.network.communication_for(
			authorities,
			table.clone(),
			self.handle.clone()
		);

		let validation_para = match local_duty.validation {
			Chain::Relay => None,
			Chain::Parachain(id) => Some(id),
		};

		let collation_work = validation_para.map(|para| CollationFetch::new(
			para,
			id.clone(),
			parent_hash.clone(),
			self.collators.clone(),
			self.client.clone(),
		));

		let drop_signal = dispatch_collation_work(
			router.clone(),
			&self.handle,
			collation_work,
			self.extrinsic_store.clone(),
		);

		let tracker = Arc::new(AttestationTracker {
			table,
			started: Instant::now(),
			_drop_signal: drop_signal
		});

		live_instances.insert(parent_hash, tracker.clone());

		Ok(tracker)
	}

	/// Retain consensus sessions matching predicate.
	fn retain<F: FnMut(&Hash) -> bool>(&self, mut pred: F) {
		self.live_instances.lock().retain(|k, _| pred(k))
	}
}

/// Parachain consensus for a single block.
struct AttestationTracker {
	_drop_signal: exit_future::Signal,
	table: Arc<SharedTable>,
	started: Instant,
}

/// Polkadot proposer factory.
pub struct ProposerFactory<C, N, P, TxApi: PoolChainApi> {
	parachain_consensus: Arc<ParachainConsensus<C, N, P>>,
	transaction_pool: Arc<Pool<TxApi>>,
	key: Arc<ed25519::Pair>,
	_service_handle: ServiceHandle,
	aura_slot_duration: SlotDuration,
}

impl<C, N, P, TxApi> ProposerFactory<C, N, P, TxApi> where
	C: Collators + Send + Sync + 'static,
	<C::Collation as IntoFuture>::Future: Send + 'static,
	P: BlockchainEvents<Block> + ChainHead<Block> + BlockBody<Block>,
	P: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + Core<Block> + BlockBuilderApi<Block> + AuthoritiesApi<Block>,
	N: Network + Send + Sync + 'static,
	N::TableRouter: Send + 'static,
	TxApi: PoolChainApi,
{
	/// Create a new proposer factory.
	pub fn new(
		client: Arc<P>,
		network: N,
		collators: C,
		transaction_pool: Arc<Pool<TxApi>>,
		thread_pool: TaskExecutor,
		key: Arc<ed25519::Pair>,
		extrinsic_store: ExtrinsicStore,
		aura_slot_duration: SlotDuration,
	) -> Self {
		let parachain_consensus = Arc::new(ParachainConsensus {
			client: client.clone(),
			network,
			collators,
			handle: thread_pool.clone(),
			extrinsic_store: extrinsic_store.clone(),
			live_instances: Mutex::new(HashMap::new()),
		});

		let service_handle = ::attestation_service::start(
			client,
			parachain_consensus.clone(),
			thread_pool,
			key.clone(),
			extrinsic_store,
		);

		ProposerFactory {
			parachain_consensus,
			transaction_pool,
			key,
			_service_handle: service_handle,
			aura_slot_duration,
		}
	}
}

impl<C, N, P, TxApi> consensus::Environment<Block> for ProposerFactory<C, N, P, TxApi> where
	C: Collators + Send + 'static,
	N: Network,
	TxApi: PoolChainApi<Block=Block>,
	P: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
	<C::Collation as IntoFuture>::Future: Send + 'static,
	N::TableRouter: Send + 'static,
{
	type Proposer = Proposer<P, TxApi>;
	type Error = Error;

	fn init(
		&self,
		parent_header: &Header,
		authorities: &[AuthorityId],
	) -> Result<Self::Proposer, Error> {
		let parent_hash = parent_header.hash();
		let parent_id = BlockId::hash(parent_hash);
		let sign_with = self.key.clone();
		let tracker = self.parachain_consensus.get_or_instantiate(
			parent_hash,
			authorities,
			sign_with,
		)?;

		Ok(Proposer {
			client: self.parachain_consensus.client.clone(),
			tracker,
			parent_hash,
			parent_id,
			parent_number: parent_header.number,
			transaction_pool: self.transaction_pool.clone(),
			slot_duration: self.aura_slot_duration,
		})
	}
}

// dispatch collation work to be done in the background. returns a signal object
// that should fire when the collation work is no longer necessary (e.g. when the proposer object is dropped)
fn dispatch_collation_work<R, C, P>(
	router: R,
	handle: &TaskExecutor,
	work: Option<CollationFetch<C, P>>,
	extrinsic_store: ExtrinsicStore,
) -> exit_future::Signal where
	C: Collators + Send + 'static,
	P: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync + 'static,
	P::Api: ParachainHost<Block>,
	<C::Collation as IntoFuture>::Future: Send + 'static,
	R: TableRouter + Send + 'static,
{
	use extrinsic_store::Data;

	let (signal, exit) = exit_future::signal();

	let work = match work {
		Some(w) => w,
		None => return signal,
	};

	let relay_parent = work.relay_parent();
	let handled_work = work.then(move |result| match result {
		Ok((collation, extrinsic)) => {
			let res = extrinsic_store.make_available(Data {
				relay_parent,
				parachain_id: collation.receipt.parachain_index,
				candidate_hash: collation.receipt.hash(),
				block_data: collation.block_data.clone(),
				extrinsic: Some(extrinsic.clone()),
			});

			match res {
				Ok(()) =>
					router.local_candidate(collation.receipt, collation.block_data, extrinsic),
				Err(e) =>
					warn!(target: "consensus", "Failed to make collation data available: {:?}", e),
			}

			Ok(())
		}
		Err(_e) => {
			warn!(target: "consensus", "Failed to collate candidate");
			Ok(())
		}
	});

	let cancellable_work = handled_work.select(exit).then(|_| Ok(()));

	// spawn onto thread pool.
	handle.spawn(cancellable_work);
	signal
}

struct LocalDuty {
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
	slot_duration: SlotDuration,
}

impl<C, TxApi> consensus::Proposer<Block> for Proposer<C, TxApi> where
	TxApi: PoolChainApi<Block=Block>,
	C: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync,
	C::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
{
	type Error = Error;
	type Create = Either<
		CreateProposal<C, TxApi>,
		future::FutureResult<Block, Error>,
	>;

	fn propose(&self, inherent_data: InherentData, max_duration: Duration) -> Self::Create {
		const ATTEMPT_PROPOSE_EVERY: Duration = Duration::from_millis(100);
		const SLOT_DURATION_DENOMINATOR: u64 = 3; // wait up to 1/3 of the slot for candidates.

		let initial_included = self.tracker.table.includable_count();
		let now = Instant::now();

		let dynamic_inclusion = DynamicInclusion::new(
			self.tracker.table.num_parachains(),
			self.tracker.started,
			Duration::from_secs(self.slot_duration.get() / SLOT_DURATION_DENOMINATOR),
		);

		let enough_candidates = dynamic_inclusion.acceptable_in(
			now,
			initial_included,
		).unwrap_or_else(|| now + Duration::from_millis(1));

		let believed_timestamp = match inherent_data.timestamp_inherent_data() {
			Ok(timestamp) => timestamp,
			Err(e) => return Either::B(future::err(ErrorKind::InherentError(e).into())),
		};

		// set up delay until next allowed timestamp.
		let current_timestamp = current_timestamp();
		let delay_future = if current_timestamp >= believed_timestamp {
			None
		} else {
			Some(Delay::new(
				Instant::now() + Duration::from_secs(current_timestamp - believed_timestamp)
			))
		};

		let timing = ProposalTiming {
			minimum: delay_future,
			attempt_propose: Interval::new(now + ATTEMPT_PROPOSE_EVERY, ATTEMPT_PROPOSE_EVERY),
			enough_candidates: Delay::new(enough_candidates),
			dynamic_inclusion,
			last_included: initial_included,
		};

		Either::A(CreateProposal {
			parent_hash: self.parent_hash.clone(),
			parent_number: self.parent_number.clone(),
			parent_id: self.parent_id.clone(),
			client: self.client.clone(),
			transaction_pool: self.transaction_pool.clone(),
			table: self.tracker.table.clone(),
			believed_minimum_timestamp: believed_timestamp,
			timing,
			inherent_data: Some(inherent_data),
			// leave some time for the proposal finalisation
			deadline: Instant::now() + max_duration - max_duration / 3,
		})
	}
}

fn current_timestamp() -> u64 {
	time::SystemTime::now().duration_since(time::UNIX_EPOCH)
		.expect("now always later than unix epoch; qed")
		.as_secs()
		.into()
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
	fn poll(&mut self, included: usize) -> Poll<(), ErrorKind> {
		// first drain from the interval so when the minimum delay is up
		// we don't have any notifications built up.
		//
		// this interval is just meant to produce periodic task wakeups
		// that lead to the `dynamic_inclusion` getting updated as necessary.
		while let Async::Ready(x) = self.attempt_propose.poll().map_err(ErrorKind::Timer)? {
			x.expect("timer still alive; intervals never end; qed");
		}

		// wait until the minimum time has passed.
		if let Some(mut minimum) = self.minimum.take() {
			if let Async::NotReady = minimum.poll().map_err(ErrorKind::Timer)? {
				self.minimum = Some(minimum);
				return Ok(Async::NotReady);
			}
		}

		if included == self.last_included {
			return self.enough_candidates.poll().map_err(ErrorKind::Timer);
		}

		// the amount of includable candidates has changed. schedule a wakeup
		// if it's not sufficient anymore.
		match self.dynamic_inclusion.acceptable_in(Instant::now(), included) {
			Some(instant) => {
				self.last_included = included;
				self.enough_candidates.reset(instant);
				self.enough_candidates.poll().map_err(ErrorKind::Timer)
			}
			None => Ok(Async::Ready(())),
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
	deadline: Instant,
}

impl<C, TxApi> CreateProposal<C, TxApi> where
	TxApi: PoolChainApi<Block=Block>,
	C: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync,
	C::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
{
	fn propose_with(&mut self, candidates: Vec<AttestedCandidate>) -> Result<Block, Error> {
		use client::block_builder::BlockBuilder;
		use runtime_primitives::traits::{Hash as HashT, BlakeTwo256};

		const MAX_TRANSACTIONS: usize = 40;

		let mut inherent_data = self.inherent_data
			.take()
			.expect("CreateProposal is not polled after finishing; qed");
		inherent_data.put_data(polkadot_runtime::PARACHAIN_INHERENT_IDENTIFIER, &candidates).map_err(ErrorKind::InherentError)?;

		let runtime_api = self.client.runtime_api();

		let mut block_builder = BlockBuilder::at_block(&self.parent_id, &*self.client)?;

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
					break
				}
				if Instant::now() > self.deadline {
					debug!("Consensus deadline reached when pushing block transactions, proceeding with proposing.");
					break;
				}

				match block_builder.push(ready.data.clone()) {
					Ok(()) => {
						pending_size += encoded_size;
					}
					Err(e) => {
						trace!(target: "transaction-pool", "Invalid transaction: {}", e);
						unqueue_invalid.push(ready.hash.clone());
					}
				}
			}

			self.transaction_pool.remove_invalid(&unqueue_invalid);
		}

		let new_block = block_builder.bake()?;

		info!("Prepared block for proposing at {} [hash: {:?}; parent_hash: {}; extrinsics: [{}]]",
			new_block.header.number,
			Hash::from(new_block.header.hash()),
			new_block.header.parent_hash,
			new_block.extrinsics.iter()
				.map(|xt| format!("{}", BlakeTwo256::hash_of(xt)))
				.collect::<Vec<_>>()
				.join(", ")
		);

		// TODO: full re-evaluation
		let active_parachains = runtime_api.active_parachains(&self.parent_id)?;
		assert!(evaluation::evaluate_initial(
			&new_block,
			self.believed_minimum_timestamp,
			&self.parent_hash,
			self.parent_number,
			&active_parachains,
		).is_ok());

		Ok(new_block)
	}
}

impl<C, TxApi> Future for CreateProposal<C, TxApi> where
	TxApi: PoolChainApi<Block=Block>,
	C: ProvideRuntimeApi + HeaderBackend<Block> + Send + Sync,
	C::Api: ParachainHost<Block> + BlockBuilderApi<Block>,
{
	type Item = Block;
	type Error = Error;

	fn poll(&mut self) -> Poll<Block, Error> {
		// 1. try to propose if we have enough includable candidates and other
		// delays have concluded.
		let included = self.table.includable_count();
		try_ready!(self.timing.poll(included));

		// 2. propose
		let proposed_candidates = self.table.proposed_set();

		self.propose_with(proposed_candidates).map(Async::Ready)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use substrate_keyring::AuthorityKeyring;

	#[test]
	fn sign_and_check_statement() {
		let statement: Statement = GenericStatement::Valid([1; 32].into());
		let parent_hash = [2; 32].into();

		let sig = sign_table_statement(&statement, &AuthorityKeyring::Alice.pair(), &parent_hash);

		assert!(check_statement(&statement, &sig, AuthorityKeyring::Alice.into(), &parent_hash));
		assert!(!check_statement(&statement, &sig, AuthorityKeyring::Alice.into(), &[0xff; 32].into()));
		assert!(!check_statement(&statement, &sig, AuthorityKeyring::Bob.into(), &parent_hash));
	}
}
