// Copyright 2019-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

#![cfg_attr(not(feature = "std"), no_std)]
// Runtime-generated enums
#![allow(clippy::large_enum_variant)]

use crate::finality::{CachedFinalityVotes, FinalityVotes};
use bp_eth_poa::{Address, AuraHeader, HeaderId, RawTransaction, RawTransactionReceipt, Receipt, H256, U256};
use codec::{Decode, Encode};
use frame_support::{decl_module, decl_storage, traits::Get};
use sp_runtime::{
	transaction_validity::{
		InvalidTransaction, TransactionLongevity, TransactionPriority, TransactionSource, TransactionValidity,
		UnknownTransaction, ValidTransaction,
	},
	RuntimeDebug,
};
use sp_std::{cmp::Ord, collections::btree_map::BTreeMap, prelude::*};

pub use validators::{ValidatorsConfiguration, ValidatorsSource};

mod error;
mod finality;
mod import;
mod validators;
mod verification;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;

#[cfg(test)]
mod mock;

#[cfg(any(feature = "runtime-benchmarks", test))]
pub mod test_utils;

/// Maximal number of blocks we're pruning in single import call.
const MAX_BLOCKS_TO_PRUNE_IN_SINGLE_IMPORT: u64 = 8;

/// Authority round engine configuration parameters.
#[derive(Clone, Encode, Decode, PartialEq, RuntimeDebug)]
pub struct AuraConfiguration {
	/// Empty step messages transition block.
	pub empty_steps_transition: u64,
	/// Transition block to strict empty steps validation.
	pub strict_empty_steps_transition: u64,
	/// Monotonic step validation transition block.
	pub validate_step_transition: u64,
	/// Chain score validation transition block.
	pub validate_score_transition: u64,
	/// First block for which a 2/3 quorum (instead of 1/2) is required.
	pub two_thirds_majority_transition: u64,
	/// Minimum gas limit.
	pub min_gas_limit: U256,
	/// Maximum gas limit.
	pub max_gas_limit: U256,
	/// Maximum size of extra data.
	pub maximum_extra_data_size: u64,
}

/// Transaction pool configuration.
///
/// This is used to limit number of unsigned headers transactions in
/// the pool. We never use it to verify signed transactions.
pub struct PoolConfiguration {
	/// Maximal difference between number of header from unsigned transaction
	/// and current best block. This must be selected with caution - the more
	/// is the difference, the more (potentially invalid) transactions could be
	/// accepted to the pool and mined later (filling blocks with spam).
	pub max_future_number_difference: u64,
}

/// Block header as it is stored in the runtime storage.
#[derive(Clone, Encode, Decode, PartialEq, RuntimeDebug)]
pub struct StoredHeader<Submitter> {
	/// Submitter of this header. May be `None` if header has been submitted
	/// using unsigned transaction.
	pub submitter: Option<Submitter>,
	/// The block header itself.
	pub header: AuraHeader,
	/// Total difficulty of the chain.
	pub total_difficulty: U256,
	/// The ID of set of validators that is expected to produce direct descendants of
	/// this block. If header enacts new set, this would be the new set. Otherwise
	/// this is the set that has produced the block itself.
	/// The hash is the hash of block where validators set has been enacted.
	pub next_validators_set_id: u64,
	/// Hash of the last block which has **SCHEDULED** validators set change.
	/// Note that signal doesn't mean that the set has been (or ever will be) enacted.
	/// Note that the header may already be pruned.
	pub last_signal_block: Option<HeaderId>,
}

/// Validators set as it is stored in the runtime storage.
#[derive(Encode, Decode, PartialEq, RuntimeDebug)]
#[cfg_attr(test, derive(Clone))]
pub struct ValidatorsSet {
	/// Validators of this set.
	pub validators: Vec<Address>,
	/// Hash of the block where this set has been signalled. None if this is the first set.
	pub signal_block: Option<HeaderId>,
	/// Hash of the block where this set has been enacted.
	pub enact_block: HeaderId,
}

/// Validators set change as it is stored in the runtime storage.
#[derive(Encode, Decode, PartialEq, RuntimeDebug)]
#[cfg_attr(test, derive(Clone))]
pub struct AuraScheduledChange {
	/// Validators of this set.
	pub validators: Vec<Address>,
	/// Hash of the block which has emitted previous validators change signal.
	pub prev_signal_block: Option<HeaderId>,
}

/// Header that we're importing.
#[derive(RuntimeDebug)]
#[cfg_attr(test, derive(Clone, PartialEq))]
pub struct HeaderToImport<Submitter> {
	/// Header import context,
	pub context: ImportContext<Submitter>,
	/// Should we consider this header as best?
	pub is_best: bool,
	/// The id of the header.
	pub id: HeaderId,
	/// The header itself.
	pub header: AuraHeader,
	/// Total chain difficulty at the header.
	pub total_difficulty: U256,
	/// New validators set and the hash of block where it has been scheduled (if applicable).
	/// Some if set is is enacted by this header.
	pub enacted_change: Option<ChangeToEnact>,
	/// Validators set scheduled change, if happened at the header.
	pub scheduled_change: Option<Vec<Address>>,
	/// Finality votes at this header.
	pub finality_votes: FinalityVotes<Submitter>,
}

/// Header that we're importing.
#[derive(RuntimeDebug)]
#[cfg_attr(test, derive(Clone, PartialEq))]
pub struct ChangeToEnact {
	/// The id of the header where change has been scheduled.
	/// None if it is a first set within current `ValidatorsSource`.
	pub signal_block: Option<HeaderId>,
	/// Validators set that is enacted.
	pub validators: Vec<Address>,
}

/// Blocks range that we want to prune.
#[derive(Encode, Decode, Default, RuntimeDebug, Clone, PartialEq)]
struct PruningRange {
	/// Number of the oldest unpruned block(s). This might be the block that we do not
	/// want to prune now (then it is equal to `oldest_block_to_keep`), or block that we
	/// were unable to prune for whatever reason (i.e. if it isn't finalized yet and has
	/// scheduled validators set change).
	pub oldest_unpruned_block: u64,
	/// Number of oldest block(s) that we want to keep. We want to prune blocks in range
	/// [`oldest_unpruned_block`; `oldest_block_to_keep`).
	pub oldest_block_to_keep: u64,
}

/// Header import context.
///
/// The import context contains information needed by the header verification
/// pipeline which is not directly part of the header being imported. This includes
/// information relating to its parent, and the current validator set (which
/// provide _context_ for the current header).
#[derive(RuntimeDebug)]
#[cfg_attr(test, derive(Clone, PartialEq))]
pub struct ImportContext<Submitter> {
	submitter: Option<Submitter>,
	parent_hash: H256,
	parent_header: AuraHeader,
	parent_total_difficulty: U256,
	parent_scheduled_change: Option<AuraScheduledChange>,
	validators_set_id: u64,
	validators_set: ValidatorsSet,
	last_signal_block: Option<HeaderId>,
}

impl<Submitter> ImportContext<Submitter> {
	/// Returns reference to header submitter (if known).
	pub fn submitter(&self) -> Option<&Submitter> {
		self.submitter.as_ref()
	}

	/// Returns reference to parent header.
	pub fn parent_header(&self) -> &AuraHeader {
		&self.parent_header
	}

	/// Returns total chain difficulty at parent block.
	pub fn total_difficulty(&self) -> &U256 {
		&self.parent_total_difficulty
	}

	/// Returns the validator set change if the parent header has signaled a change.
	pub fn parent_scheduled_change(&self) -> Option<&AuraScheduledChange> {
		self.parent_scheduled_change.as_ref()
	}

	/// Returns id of the set of validators.
	pub fn validators_set_id(&self) -> u64 {
		self.validators_set_id
	}

	/// Returns reference to validators set for the block we're going to import.
	pub fn validators_set(&self) -> &ValidatorsSet {
		&self.validators_set
	}

	/// Returns reference to the latest block which has signalled change of validators set.
	/// This may point to parent if parent has signalled change.
	pub fn last_signal_block(&self) -> Option<HeaderId> {
		match self.parent_scheduled_change {
			Some(_) => Some(HeaderId {
				number: self.parent_header.number,
				hash: self.parent_hash,
			}),
			None => self.last_signal_block,
		}
	}

	/// Converts import context into header we're going to import.
	#[allow(clippy::too_many_arguments)]
	pub fn into_import_header(
		self,
		is_best: bool,
		id: HeaderId,
		header: AuraHeader,
		total_difficulty: U256,
		enacted_change: Option<ChangeToEnact>,
		scheduled_change: Option<Vec<Address>>,
		finality_votes: FinalityVotes<Submitter>,
	) -> HeaderToImport<Submitter> {
		HeaderToImport {
			context: self,
			is_best,
			id,
			header,
			total_difficulty,
			enacted_change,
			scheduled_change,
			finality_votes,
		}
	}
}

/// The storage that is used by the client.
///
/// Storage modification must be discarded if block import has failed.
pub trait Storage {
	/// Header submitter identifier.
	type Submitter: Clone + Ord;

	/// Get best known block and total chain difficulty.
	fn best_block(&self) -> (HeaderId, U256);
	/// Get last finalized block.
	fn finalized_block(&self) -> HeaderId;
	/// Get imported header by its hash.
	///
	/// Returns header and its submitter (if known).
	fn header(&self, hash: &H256) -> Option<(AuraHeader, Option<Self::Submitter>)>;
	/// Returns latest cached finality votes (if any) for block ancestors, starting
	/// from `parent_hash` block and stopping at genesis block, best finalized block
	/// or block where `stop_at` returns true.
	fn cached_finality_votes(
		&self,
		parent: &HeaderId,
		best_finalized: &HeaderId,
		stop_at: impl Fn(&H256) -> bool,
	) -> CachedFinalityVotes<Self::Submitter>;
	/// Get header import context by parent header hash.
	fn import_context(
		&self,
		submitter: Option<Self::Submitter>,
		parent_hash: &H256,
	) -> Option<ImportContext<Self::Submitter>>;
	/// Get new validators that are scheduled by given header and hash of the previous
	/// block that has scheduled change.
	fn scheduled_change(&self, hash: &H256) -> Option<AuraScheduledChange>;
	/// Insert imported header.
	fn insert_header(&mut self, header: HeaderToImport<Self::Submitter>);
	/// Finalize given block and schedules pruning of all headers
	/// with number < prune_end.
	///
	/// The headers in the pruning range could be either finalized, or not.
	/// It is the storage duty to ensure that unfinalized headers that have
	/// scheduled changes won't be pruned until they or their competitors
	/// are finalized.
	fn finalize_and_prune_headers(&mut self, finalized: Option<HeaderId>, prune_end: u64);
}

/// Headers pruning strategy.
pub trait PruningStrategy: Default {
	/// Return upper bound (exclusive) of headers pruning range.
	///
	/// Every value that is returned from this function, must be greater or equal to the
	/// previous value. Otherwise it will be ignored (we can't revert pruning).
	///
	/// Module may prune both finalized and unfinalized blocks. But it can't give any
	/// guarantees on when it will happen. Example: if some unfinalized block at height N
	/// has scheduled validators set change, then the module won't prune any blocks with
	/// number >= N even if strategy allows that.
	///
	/// If your strategy allows pruning unfinalized blocks, this could lead to switch
	/// between finalized forks (only if authorities are misbehaving). But since 50%+1 (or 2/3)
	/// authorities are able to do whatever they want with the chain, this isn't considered
	/// fatal. If your strategy only prunes finalized blocks, we'll never be able to finalize
	/// header that isn't descendant of current best finalized block.
	fn pruning_upper_bound(&mut self, best_number: u64, best_finalized_number: u64) -> u64;
}

/// ChainTime represents the runtime on-chain time
pub trait ChainTime: Default {
	/// Is a header timestamp ahead of the current on-chain time.
	///
	/// Check whether `timestamp` is ahead (i.e greater than) the current on-chain
	/// time. If so, return `true`, `false` otherwise.
	fn is_timestamp_ahead(&self, timestamp: u64) -> bool;
}

/// ChainTime implementation for the empty type.
///
/// This implementation will allow a runtime without the timestamp pallet to use
/// the empty type as its ChainTime associated type.
impl ChainTime for () {
	fn is_timestamp_ahead(&self, _: u64) -> bool {
		false
	}
}

/// Callbacks for header submission rewards/penalties.
pub trait OnHeadersSubmitted<AccountId> {
	/// Called when valid headers have been submitted.
	///
	/// The submitter **must not** be rewarded for submitting valid headers, because greedy authority
	/// could produce and submit multiple valid headers (without relaying them to other peers) and
	/// get rewarded. Instead, the provider could track submitters and stop rewarding if too many
	/// headers have been submitted without finalization.
	fn on_valid_headers_submitted(submitter: AccountId, useful: u64, useless: u64);
	/// Called when invalid headers have been submitted.
	fn on_invalid_headers_submitted(submitter: AccountId);
	/// Called when earlier submitted headers have been finalized.
	///
	/// finalized is the number of headers that submitter has submitted and which
	/// have been finalized.
	fn on_valid_headers_finalized(submitter: AccountId, finalized: u64);
}

impl<AccountId> OnHeadersSubmitted<AccountId> for () {
	fn on_valid_headers_submitted(_submitter: AccountId, _useful: u64, _useless: u64) {}
	fn on_invalid_headers_submitted(_submitter: AccountId) {}
	fn on_valid_headers_finalized(_submitter: AccountId, _finalized: u64) {}
}

/// The module configuration trait.
pub trait Config<I = DefaultInstance>: frame_system::Config {
	/// Aura configuration.
	type AuraConfiguration: Get<AuraConfiguration>;
	/// Validators configuration.
	type ValidatorsConfiguration: Get<validators::ValidatorsConfiguration>;

	/// Interval (in blocks) for for finality votes caching.
	/// If None, cache is disabled.
	///
	/// Ideally, this should either be None (when we are sure that there won't
	/// be any significant finalization delays), or something that is bit larger
	/// than average finalization delay.
	type FinalityVotesCachingInterval: Get<Option<u64>>;
	/// Headers pruning strategy.
	type PruningStrategy: PruningStrategy;
	/// Header timestamp verification against current on-chain time.
	type ChainTime: ChainTime;

	/// Handler for headers submission result.
	type OnHeadersSubmitted: OnHeadersSubmitted<Self::AccountId>;
}

decl_module! {
	pub struct Module<T: Config<I>, I: Instance = DefaultInstance> for enum Call where origin: T::Origin {
		/// Import single Aura header. Requires transaction to be **UNSIGNED**.
		#[weight = 0] // TODO: update me (https://github.com/paritytech/parity-bridges-common/issues/78)
		pub fn import_unsigned_header(origin, header: AuraHeader, receipts: Option<Vec<Receipt>>) {
			frame_system::ensure_none(origin)?;

			import::import_header(
				&mut BridgeStorage::<T, I>::new(),
				&mut T::PruningStrategy::default(),
				&T::AuraConfiguration::get(),
				&T::ValidatorsConfiguration::get(),
				None,
				header,
				&T::ChainTime::default(),
				receipts,
			).map_err(|e| e.msg())?;
		}

		/// Import Aura chain headers in a single **SIGNED** transaction.
		/// Ignores non-fatal errors (like when known header is provided), rewards
		/// for successful headers import and penalizes for fatal errors.
		///
		/// This should be used with caution - passing too many headers could lead to
		/// enormous block production/import time.
		#[weight = 0] // TODO: update me (https://github.com/paritytech/parity-bridges-common/issues/78)
		pub fn import_signed_headers(origin, headers_with_receipts: Vec<(AuraHeader, Option<Vec<Receipt>>)>) {
			let submitter = frame_system::ensure_signed(origin)?;
			let mut finalized_headers = BTreeMap::new();
			let import_result = import::import_headers(
				&mut BridgeStorage::<T, I>::new(),
				&mut T::PruningStrategy::default(),
				&T::AuraConfiguration::get(),
				&T::ValidatorsConfiguration::get(),
				Some(submitter.clone()),
				headers_with_receipts,
				&T::ChainTime::default(),
				&mut finalized_headers,
			);

			// if we have finalized some headers, we will reward their submitters even
			// if current submitter has provided some invalid headers
			for (f_submitter, f_count) in finalized_headers {
				T::OnHeadersSubmitted::on_valid_headers_finalized(
					f_submitter,
					f_count,
				);
			}

			// now track/penalize current submitter for providing new headers
			match import_result {
				Ok((useful, useless)) =>
					T::OnHeadersSubmitted::on_valid_headers_submitted(submitter, useful, useless),
				Err(error) => {
					// even though we may have accept some headers, we do not want to reward someone
					// who provides invalid headers
					T::OnHeadersSubmitted::on_invalid_headers_submitted(submitter);
					return Err(error.msg().into());
				},
			}
		}
	}
}

decl_storage! {
	trait Store for Module<T: Config<I>, I: Instance = DefaultInstance> as Bridge {
		/// Best known block.
		BestBlock: (HeaderId, U256);
		/// Best finalized block.
		FinalizedBlock: HeaderId;
		/// Range of blocks that we want to prune.
		BlocksToPrune: PruningRange;
		/// Map of imported headers by hash.
		Headers: map hasher(identity) H256 => Option<StoredHeader<T::AccountId>>;
		/// Map of imported header hashes by number.
		HeadersByNumber: map hasher(blake2_128_concat) u64 => Option<Vec<H256>>;
		/// Map of cached finality data by header hash.
		FinalityCache: map hasher(identity) H256 => Option<FinalityVotes<T::AccountId>>;
		/// The ID of next validator set.
		NextValidatorsSetId: u64;
		/// Map of validators sets by their id.
		ValidatorsSets: map hasher(twox_64_concat) u64 => Option<ValidatorsSet>;
		/// Validators sets reference count. Each header that is authored by this set increases
		/// the reference count. When we prune this header, we decrease the reference count.
		/// When it reaches zero, we are free to prune validator set as well.
		ValidatorsSetsRc: map hasher(twox_64_concat) u64 => Option<u64>;
		/// Map of validators set changes scheduled by given header.
		ScheduledChanges: map hasher(identity) H256 => Option<AuraScheduledChange>;
	}
	add_extra_genesis {
		config(initial_header): AuraHeader;
		config(initial_difficulty): U256;
		config(initial_validators): Vec<Address>;
		build(|config| {
			// the initial blocks should be selected so that:
			// 1) it doesn't signal validators changes;
			// 2) there are no scheduled validators changes from previous blocks;
			// 3) (implied) all direct children of initial block are authored by the same validators set.

			assert!(
				!config.initial_validators.is_empty(),
				"Initial validators set can't be empty",
			);

			initialize_storage::<T, I>(
				&config.initial_header,
				config.initial_difficulty,
				&config.initial_validators,
			);
		})
	}
}

impl<T: Config<I>, I: Instance> Module<T, I> {
	/// Returns number and hash of the best block known to the bridge module.
	/// The caller should only submit `import_header` transaction that makes
	/// (or leads to making) other header the best one.
	pub fn best_block() -> HeaderId {
		BridgeStorage::<T, I>::new().best_block().0
	}

	/// Returns number and hash of the best finalized block known to the bridge module.
	pub fn finalized_block() -> HeaderId {
		BridgeStorage::<T, I>::new().finalized_block()
	}

	/// Returns true if the import of given block requires transactions receipts.
	pub fn is_import_requires_receipts(header: AuraHeader) -> bool {
		import::header_import_requires_receipts(
			&BridgeStorage::<T, I>::new(),
			&T::ValidatorsConfiguration::get(),
			&header,
		)
	}

	/// Returns true if header is known to the runtime.
	pub fn is_known_block(hash: H256) -> bool {
		BridgeStorage::<T, I>::new().header(&hash).is_some()
	}

	/// Verify that transaction is included into given finalized block.
	pub fn verify_transaction_finalized(
		block: H256,
		tx_index: u64,
		proof: &[(RawTransaction, RawTransactionReceipt)],
	) -> bool {
		crate::verify_transaction_finalized(&BridgeStorage::<T, I>::new(), block, tx_index, proof)
	}
}

impl<T: Config<I>, I: Instance> frame_support::unsigned::ValidateUnsigned for Module<T, I> {
	type Call = Call<T, I>;

	fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
		match *call {
			Self::Call::import_unsigned_header(ref header, ref receipts) => {
				let accept_result = verification::accept_aura_header_into_pool(
					&BridgeStorage::<T, I>::new(),
					&T::AuraConfiguration::get(),
					&T::ValidatorsConfiguration::get(),
					&pool_configuration(),
					header,
					&T::ChainTime::default(),
					receipts.as_ref(),
				);

				match accept_result {
					Ok((requires, provides)) => Ok(ValidTransaction {
						priority: TransactionPriority::max_value(),
						requires,
						provides,
						longevity: TransactionLongevity::max_value(),
						propagate: true,
					}),
					// UnsignedTooFarInTheFuture is the special error code used to limit
					// number of transactions in the pool - we do not want to ban transaction
					// in this case (see verification.rs for details)
					Err(error::Error::UnsignedTooFarInTheFuture) => {
						UnknownTransaction::Custom(error::Error::UnsignedTooFarInTheFuture.code()).into()
					}
					Err(error) => InvalidTransaction::Custom(error.code()).into(),
				}
			}
			_ => InvalidTransaction::Call.into(),
		}
	}
}

/// Runtime bridge storage.
#[derive(Default)]
pub struct BridgeStorage<T, I = DefaultInstance>(sp_std::marker::PhantomData<(T, I)>);

impl<T: Config<I>, I: Instance> BridgeStorage<T, I> {
	/// Create new BridgeStorage.
	pub fn new() -> Self {
		BridgeStorage(sp_std::marker::PhantomData::<(T, I)>::default())
	}

	/// Prune old blocks.
	fn prune_blocks(&self, mut max_blocks_to_prune: u64, finalized_number: u64, prune_end: u64) {
		let pruning_range = BlocksToPrune::<I>::get();
		let mut new_pruning_range = pruning_range.clone();

		// update oldest block we want to keep
		if prune_end > new_pruning_range.oldest_block_to_keep {
			new_pruning_range.oldest_block_to_keep = prune_end;
		}

		// start pruning blocks
		let begin = new_pruning_range.oldest_unpruned_block;
		let end = new_pruning_range.oldest_block_to_keep;
		frame_support::debug::trace!(target: "runtime", "Pruning blocks in range [{}..{})", begin, end);
		for number in begin..end {
			// if we can't prune anything => break
			if max_blocks_to_prune == 0 {
				break;
			}

			// read hashes of blocks with given number and try to prune these blocks
			let blocks_at_number = HeadersByNumber::<I>::take(number);
			if let Some(mut blocks_at_number) = blocks_at_number {
				self.prune_blocks_by_hashes(
					&mut max_blocks_to_prune,
					finalized_number,
					number,
					&mut blocks_at_number,
				);

				// if we haven't pruned all blocks, remember unpruned
				if !blocks_at_number.is_empty() {
					HeadersByNumber::<I>::insert(number, blocks_at_number);
					break;
				}
			}

			// we have pruned all headers at number
			new_pruning_range.oldest_unpruned_block = number + 1;
			frame_support::debug::trace!(
				target: "runtime",
				"Oldest unpruned PoA header is now: {}",
				new_pruning_range.oldest_unpruned_block,
			);
		}

		// update pruning range in storage
		if pruning_range != new_pruning_range {
			BlocksToPrune::<I>::put(new_pruning_range);
		}
	}

	/// Prune old blocks with given hashes.
	fn prune_blocks_by_hashes(
		&self,
		max_blocks_to_prune: &mut u64,
		finalized_number: u64,
		number: u64,
		blocks_at_number: &mut Vec<H256>,
	) {
		// ensure that unfinalized headers we want to prune do not have scheduled changes
		if number > finalized_number && blocks_at_number.iter().any(ScheduledChanges::<I>::contains_key) {
			return;
		}

		// physically remove headers and (probably) obsolete validators sets
		while let Some(hash) = blocks_at_number.pop() {
			let header = Headers::<T, I>::take(&hash);
			frame_support::debug::trace!(
				target: "runtime",
				"Pruning PoA header: ({}, {})",
				number,
				hash,
			);

			ScheduledChanges::<I>::remove(hash);
			FinalityCache::<T, I>::remove(hash);
			if let Some(header) = header {
				ValidatorsSetsRc::<I>::mutate(header.next_validators_set_id, |rc| match *rc {
					Some(rc) if rc > 1 => Some(rc - 1),
					_ => None,
				});
			}

			// check if we have already pruned too much headers in this call
			*max_blocks_to_prune -= 1;
			if *max_blocks_to_prune == 0 {
				return;
			}
		}
	}
}

impl<T: Config<I>, I: Instance> Storage for BridgeStorage<T, I> {
	type Submitter = T::AccountId;

	fn best_block(&self) -> (HeaderId, U256) {
		BestBlock::<I>::get()
	}

	fn finalized_block(&self) -> HeaderId {
		FinalizedBlock::<I>::get()
	}

	fn header(&self, hash: &H256) -> Option<(AuraHeader, Option<Self::Submitter>)> {
		Headers::<T, I>::get(hash).map(|header| (header.header, header.submitter))
	}

	fn cached_finality_votes(
		&self,
		parent: &HeaderId,
		best_finalized: &HeaderId,
		stop_at: impl Fn(&H256) -> bool,
	) -> CachedFinalityVotes<Self::Submitter> {
		let mut votes = CachedFinalityVotes::default();
		let mut current_id = *parent;
		loop {
			// if we have reached finalized block's sibling => stop with special signal
			if current_id.number == best_finalized.number && current_id.hash != best_finalized.hash {
				votes.stopped_at_finalized_sibling = true;
				return votes;
			}

			// if we have reached target header => stop
			if stop_at(&current_id.hash) {
				return votes;
			}

			// if we have found cached votes => stop
			let cached_votes = FinalityCache::<T, I>::get(&current_id.hash);
			if let Some(cached_votes) = cached_votes {
				votes.votes = Some(cached_votes);
				return votes;
			}

			// read next parent header id
			let header = match Headers::<T, I>::get(&current_id.hash) {
				Some(header) if header.header.number != 0 => header,
				_ => return votes,
			};
			let parent_id = header.header.parent_id().expect(
				"only returns None at genesis header;\
					the header is proved to have number > 0;\
					qed",
			);

			votes
				.unaccounted_ancestry
				.push_back((current_id, header.submitter, header.header));

			current_id = parent_id;
		}
	}

	fn import_context(
		&self,
		submitter: Option<Self::Submitter>,
		parent_hash: &H256,
	) -> Option<ImportContext<Self::Submitter>> {
		Headers::<T, I>::get(parent_hash).map(|parent_header| {
			let validators_set = ValidatorsSets::<I>::get(parent_header.next_validators_set_id)
				.expect("validators set is only pruned when last ref is pruned; there is a ref; qed");
			let parent_scheduled_change = ScheduledChanges::<I>::get(parent_hash);
			ImportContext {
				submitter,
				parent_hash: *parent_hash,
				parent_header: parent_header.header,
				parent_total_difficulty: parent_header.total_difficulty,
				parent_scheduled_change,
				validators_set_id: parent_header.next_validators_set_id,
				validators_set,
				last_signal_block: parent_header.last_signal_block,
			}
		})
	}

	fn scheduled_change(&self, hash: &H256) -> Option<AuraScheduledChange> {
		ScheduledChanges::<I>::get(hash)
	}

	fn insert_header(&mut self, header: HeaderToImport<Self::Submitter>) {
		if header.is_best {
			BestBlock::<I>::put((header.id, header.total_difficulty));
		}
		if let Some(scheduled_change) = header.scheduled_change {
			ScheduledChanges::<I>::insert(
				&header.id.hash,
				AuraScheduledChange {
					validators: scheduled_change,
					prev_signal_block: header.context.last_signal_block,
				},
			);
		}
		let next_validators_set_id = match header.enacted_change {
			Some(enacted_change) => {
				let next_validators_set_id = NextValidatorsSetId::<I>::mutate(|set_id| {
					let next_set_id = *set_id;
					*set_id += 1;
					next_set_id
				});
				ValidatorsSets::<I>::insert(
					next_validators_set_id,
					ValidatorsSet {
						validators: enacted_change.validators,
						enact_block: header.id,
						signal_block: enacted_change.signal_block,
					},
				);
				ValidatorsSetsRc::<I>::insert(next_validators_set_id, 1);
				next_validators_set_id
			}
			None => {
				ValidatorsSetsRc::<I>::mutate(header.context.validators_set_id, |rc| {
					*rc = Some(rc.map(|rc| rc + 1).unwrap_or(1));
					*rc
				});
				header.context.validators_set_id
			}
		};

		let finality_votes_caching_interval = T::FinalityVotesCachingInterval::get();
		if let Some(finality_votes_caching_interval) = finality_votes_caching_interval {
			let cache_entry_required = header.id.number != 0 && header.id.number % finality_votes_caching_interval == 0;
			if cache_entry_required {
				FinalityCache::<T, I>::insert(header.id.hash, header.finality_votes);
			}
		}

		frame_support::debug::trace!(
			target: "runtime",
			"Inserting PoA header: ({}, {})",
			header.header.number,
			header.id.hash,
		);

		let last_signal_block = header.context.last_signal_block();
		HeadersByNumber::<I>::append(header.id.number, header.id.hash);
		Headers::<T, I>::insert(
			&header.id.hash,
			StoredHeader {
				submitter: header.context.submitter,
				header: header.header,
				total_difficulty: header.total_difficulty,
				next_validators_set_id,
				last_signal_block,
			},
		);
	}

	fn finalize_and_prune_headers(&mut self, finalized: Option<HeaderId>, prune_end: u64) {
		// remember just finalized block
		let finalized_number = finalized
			.as_ref()
			.map(|f| f.number)
			.unwrap_or_else(|| FinalizedBlock::<I>::get().number);
		if let Some(finalized) = finalized {
			frame_support::debug::trace!(
				target: "runtime",
				"Finalizing PoA header: ({}, {})",
				finalized.number,
				finalized.hash,
			);

			FinalizedBlock::<I>::put(finalized);
		}

		// and now prune headers if we need to
		self.prune_blocks(MAX_BLOCKS_TO_PRUNE_IN_SINGLE_IMPORT, finalized_number, prune_end);
	}
}

/// Initialize storage.
#[cfg(any(feature = "std", feature = "runtime-benchmarks"))]
pub(crate) fn initialize_storage<T: Config<I>, I: Instance>(
	initial_header: &AuraHeader,
	initial_difficulty: U256,
	initial_validators: &[Address],
) {
	let initial_hash = initial_header.compute_hash();
	frame_support::debug::trace!(
		target: "runtime",
		"Initializing bridge with PoA header: ({}, {})",
		initial_header.number,
		initial_hash,
	);

	let initial_id = HeaderId {
		number: initial_header.number,
		hash: initial_hash,
	};
	BestBlock::<I>::put((initial_id, initial_difficulty));
	FinalizedBlock::<I>::put(initial_id);
	BlocksToPrune::<I>::put(PruningRange {
		oldest_unpruned_block: initial_header.number,
		oldest_block_to_keep: initial_header.number,
	});
	HeadersByNumber::<I>::insert(initial_header.number, vec![initial_hash]);
	Headers::<T, I>::insert(
		initial_hash,
		StoredHeader {
			submitter: None,
			header: initial_header.clone(),
			total_difficulty: initial_difficulty,
			next_validators_set_id: 0,
			last_signal_block: None,
		},
	);
	NextValidatorsSetId::<I>::put(1);
	ValidatorsSets::<I>::insert(
		0,
		ValidatorsSet {
			validators: initial_validators.to_vec(),
			signal_block: None,
			enact_block: initial_id,
		},
	);
	ValidatorsSetsRc::<I>::insert(0, 1);
}

/// Verify that transaction is included into given finalized block.
pub fn verify_transaction_finalized<S: Storage>(
	storage: &S,
	block: H256,
	tx_index: u64,
	proof: &[(RawTransaction, RawTransactionReceipt)],
) -> bool {
	if tx_index >= proof.len() as _ {
		frame_support::debug::trace!(
			target: "runtime",
			"Tx finality check failed: transaction index ({}) is larger than number of transactions ({})",
			tx_index,
			proof.len(),
		);

		return false;
	}

	let header = match storage.header(&block) {
		Some((header, _)) => header,
		None => {
			frame_support::debug::trace!(
				target: "runtime",
				"Tx finality check failed: can't find header in the storage: {}",
				block,
			);

			return false;
		}
	};
	let finalized = storage.finalized_block();

	// if header is not yet finalized => return
	if header.number > finalized.number {
		frame_support::debug::trace!(
			target: "runtime",
			"Tx finality check failed: header {}/{} is not finalized. Best finalized: {}",
			header.number,
			block,
			finalized.number,
		);

		return false;
	}

	// check if header is actually finalized
	let is_finalized = match header.number < finalized.number {
		true => ancestry(storage, finalized.hash)
			.skip_while(|(_, ancestor)| ancestor.number > header.number)
			.any(|(ancestor_hash, _)| ancestor_hash == block),
		false => block == finalized.hash,
	};
	if !is_finalized {
		frame_support::debug::trace!(
			target: "runtime",
			"Tx finality check failed: header {} is not finalized: no canonical path to best finalized block {}",
			block,
			finalized.hash,
		);

		return false;
	}

	// verify that transaction is included in the block
	if let Err(computed_root) = header.check_transactions_root(proof.iter().map(|(tx, _)| tx)) {
		frame_support::debug::trace!(
			target: "runtime",
			"Tx finality check failed: transactions root mismatch. Expected: {}, computed: {}",
			header.transactions_root,
			computed_root,
		);

		return false;
	}

	// verify that transaction receipt is included in the block
	if let Err(computed_root) = header.check_raw_receipts_root(proof.iter().map(|(_, r)| r)) {
		frame_support::debug::trace!(
			target: "runtime",
			"Tx finality check failed: receipts root mismatch. Expected: {}, computed: {}",
			header.receipts_root,
			computed_root,
		);

		return false;
	}

	// check that transaction has completed successfully
	let is_successful_raw_receipt = Receipt::is_successful_raw_receipt(&proof[tx_index as usize].1);
	match is_successful_raw_receipt {
		Ok(true) => true,
		Ok(false) => {
			frame_support::debug::trace!(
				target: "runtime",
				"Tx finality check failed: receipt shows that transaction has failed",
			);

			false
		}
		Err(err) => {
			frame_support::debug::trace!(
				target: "runtime",
				"Tx finality check failed: receipt check has failed: {}",
				err,
			);

			false
		}
	}
}

/// Transaction pool configuration.
fn pool_configuration() -> PoolConfiguration {
	PoolConfiguration {
		max_future_number_difference: 10,
	}
}

/// Return iterator of given header ancestors.
fn ancestry<S: Storage>(storage: &'_ S, mut parent_hash: H256) -> impl Iterator<Item = (H256, AuraHeader)> + '_ {
	sp_std::iter::from_fn(move || {
		let (header, _) = storage.header(&parent_hash)?;
		if header.number == 0 {
			return None;
		}

		let hash = parent_hash;
		parent_hash = header.parent_hash;
		Some((hash, header))
	})
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use crate::finality::FinalityAncestor;
	use crate::mock::{
		genesis, insert_header, run_test, run_test_with_genesis, validators_addresses, HeaderBuilder, TestRuntime,
		GAS_LIMIT,
	};
	use crate::test_utils::validator_utils::*;
	use bp_eth_poa::compute_merkle_root;

	const TOTAL_VALIDATORS: usize = 3;

	fn example_tx() -> Vec<u8> {
		vec![42]
	}

	fn example_tx_receipt(success: bool) -> Vec<u8> {
		Receipt {
			// the only thing that we care of:
			outcome: bp_eth_poa::TransactionOutcome::StatusCode(if success { 1 } else { 0 }),
			gas_used: Default::default(),
			log_bloom: Default::default(),
			logs: Vec::new(),
		}
		.rlp()
	}

	fn example_header_with_failed_receipt() -> AuraHeader {
		HeaderBuilder::with_parent(&example_header())
			.transactions_root(compute_merkle_root(vec![example_tx()].into_iter()))
			.receipts_root(compute_merkle_root(vec![example_tx_receipt(false)].into_iter()))
			.sign_by(&validator(0))
	}

	fn example_header() -> AuraHeader {
		HeaderBuilder::with_parent(&example_header_parent())
			.transactions_root(compute_merkle_root(vec![example_tx()].into_iter()))
			.receipts_root(compute_merkle_root(vec![example_tx_receipt(true)].into_iter()))
			.sign_by(&validator(0))
	}

	fn example_header_parent() -> AuraHeader {
		HeaderBuilder::with_parent(&genesis())
			.transactions_root(compute_merkle_root(vec![example_tx()].into_iter()))
			.receipts_root(compute_merkle_root(vec![example_tx_receipt(true)].into_iter()))
			.sign_by(&validator(0))
	}

	fn with_headers_to_prune<T>(f: impl Fn(BridgeStorage<TestRuntime>) -> T) -> T {
		run_test(TOTAL_VALIDATORS, |ctx| {
			for i in 1..10 {
				let mut headers_by_number = Vec::with_capacity(5);
				for j in 0..5 {
					let header = HeaderBuilder::with_parent_number(i - 1)
						.gas_limit((GAS_LIMIT + j).into())
						.sign_by_set(&ctx.validators);
					let hash = header.compute_hash();
					headers_by_number.push(hash);
					Headers::<TestRuntime>::insert(
						hash,
						StoredHeader {
							submitter: None,
							header,
							total_difficulty: 0.into(),
							next_validators_set_id: 0,
							last_signal_block: None,
						},
					);

					if i == 7 && j == 1 {
						ScheduledChanges::<DefaultInstance>::insert(
							hash,
							AuraScheduledChange {
								validators: validators_addresses(5),
								prev_signal_block: None,
							},
						);
					}
				}
				HeadersByNumber::<DefaultInstance>::insert(i, headers_by_number);
			}

			f(BridgeStorage::new())
		})
	}

	#[test]
	fn blocks_are_not_pruned_if_range_is_empty() {
		with_headers_to_prune(|storage| {
			BlocksToPrune::<DefaultInstance>::put(PruningRange {
				oldest_unpruned_block: 5,
				oldest_block_to_keep: 5,
			});

			// try to prune blocks [5; 10)
			storage.prune_blocks(0xFFFF, 10, 5);
			assert_eq!(HeadersByNumber::<DefaultInstance>::get(&5).unwrap().len(), 5);
			assert_eq!(
				BlocksToPrune::<DefaultInstance>::get(),
				PruningRange {
					oldest_unpruned_block: 5,
					oldest_block_to_keep: 5,
				},
			);
		});
	}

	#[test]
	fn blocks_to_prune_never_shrinks_from_the_end() {
		with_headers_to_prune(|storage| {
			BlocksToPrune::<DefaultInstance>::put(PruningRange {
				oldest_unpruned_block: 0,
				oldest_block_to_keep: 5,
			});

			// try to prune blocks [5; 10)
			storage.prune_blocks(0xFFFF, 10, 3);
			assert_eq!(
				BlocksToPrune::<DefaultInstance>::get(),
				PruningRange {
					oldest_unpruned_block: 5,
					oldest_block_to_keep: 5,
				},
			);
		});
	}

	#[test]
	fn blocks_are_not_pruned_if_limit_is_zero() {
		with_headers_to_prune(|storage| {
			// try to prune blocks [0; 10)
			storage.prune_blocks(0, 10, 10);
			assert!(HeadersByNumber::<DefaultInstance>::get(&0).is_some());
			assert!(HeadersByNumber::<DefaultInstance>::get(&1).is_some());
			assert!(HeadersByNumber::<DefaultInstance>::get(&2).is_some());
			assert!(HeadersByNumber::<DefaultInstance>::get(&3).is_some());
			assert_eq!(
				BlocksToPrune::<DefaultInstance>::get(),
				PruningRange {
					oldest_unpruned_block: 0,
					oldest_block_to_keep: 10,
				},
			);
		});
	}

	#[test]
	fn blocks_are_pruned_if_limit_is_non_zero() {
		with_headers_to_prune(|storage| {
			// try to prune blocks [0; 10)
			storage.prune_blocks(7, 10, 10);
			// 1 headers with number = 0 is pruned (1 total)
			assert!(HeadersByNumber::<DefaultInstance>::get(&0).is_none());
			// 5 headers with number = 1 are pruned (6 total)
			assert!(HeadersByNumber::<DefaultInstance>::get(&1).is_none());
			// 1 header with number = 2 are pruned (7 total)
			assert_eq!(HeadersByNumber::<DefaultInstance>::get(&2).unwrap().len(), 4);
			assert_eq!(
				BlocksToPrune::<DefaultInstance>::get(),
				PruningRange {
					oldest_unpruned_block: 2,
					oldest_block_to_keep: 10,
				},
			);

			// try to prune blocks [2; 10)
			storage.prune_blocks(11, 10, 10);
			// 4 headers with number = 2 are pruned (4 total)
			assert!(HeadersByNumber::<DefaultInstance>::get(&2).is_none());
			// 5 headers with number = 3 are pruned (9 total)
			assert!(HeadersByNumber::<DefaultInstance>::get(&3).is_none());
			// 2 headers with number = 4 are pruned (11 total)
			assert_eq!(HeadersByNumber::<DefaultInstance>::get(&4).unwrap().len(), 3);
			assert_eq!(
				BlocksToPrune::<DefaultInstance>::get(),
				PruningRange {
					oldest_unpruned_block: 4,
					oldest_block_to_keep: 10,
				},
			);
		});
	}

	#[test]
	fn pruning_stops_on_unfainalized_block_with_scheduled_change() {
		with_headers_to_prune(|storage| {
			// try to prune blocks [0; 10)
			// last finalized block is 5
			// and one of blocks#7 has scheduled change
			// => we won't prune any block#7 at all
			storage.prune_blocks(0xFFFF, 5, 10);
			assert!(HeadersByNumber::<DefaultInstance>::get(&0).is_none());
			assert!(HeadersByNumber::<DefaultInstance>::get(&1).is_none());
			assert!(HeadersByNumber::<DefaultInstance>::get(&2).is_none());
			assert!(HeadersByNumber::<DefaultInstance>::get(&3).is_none());
			assert!(HeadersByNumber::<DefaultInstance>::get(&4).is_none());
			assert!(HeadersByNumber::<DefaultInstance>::get(&5).is_none());
			assert!(HeadersByNumber::<DefaultInstance>::get(&6).is_none());
			assert_eq!(HeadersByNumber::<DefaultInstance>::get(&7).unwrap().len(), 5);
			assert_eq!(
				BlocksToPrune::<DefaultInstance>::get(),
				PruningRange {
					oldest_unpruned_block: 7,
					oldest_block_to_keep: 10,
				},
			);
		});
	}

	#[test]
	fn finality_votes_are_cached() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();
			let interval = <TestRuntime as Config>::FinalityVotesCachingInterval::get().unwrap();

			// for all headers with number < interval, cache entry is not created
			for i in 1..interval {
				let header = HeaderBuilder::with_parent_number(i - 1).sign_by_set(&ctx.validators);
				let id = header.compute_id();
				insert_header(&mut storage, header);
				assert_eq!(FinalityCache::<TestRuntime>::get(&id.hash), None);
			}

			// for header with number = interval, cache entry is created
			let header_with_entry = HeaderBuilder::with_parent_number(interval - 1).sign_by_set(&ctx.validators);
			let header_with_entry_hash = header_with_entry.compute_hash();
			insert_header(&mut storage, header_with_entry);
			assert!(FinalityCache::<TestRuntime>::get(&header_with_entry_hash).is_some());

			// when we later prune this header, cache entry is removed
			BlocksToPrune::<DefaultInstance>::put(PruningRange {
				oldest_unpruned_block: interval - 1,
				oldest_block_to_keep: interval - 1,
			});
			storage.finalize_and_prune_headers(None, interval + 1);
			assert_eq!(FinalityCache::<TestRuntime>::get(&header_with_entry_hash), None);
		});
	}

	#[test]
	fn cached_finality_votes_finds_entry() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			// insert 5 headers
			let mut storage = BridgeStorage::<TestRuntime>::new();
			let mut headers = Vec::new();
			for i in 1..5 {
				let header = HeaderBuilder::with_parent_number(i - 1).sign_by_set(&ctx.validators);
				headers.push(header.clone());
				insert_header(&mut storage, header);
			}

			// when inserting header#6, entry isn't found
			let id5 = headers.last().unwrap().compute_id();
			assert_eq!(
				storage.cached_finality_votes(&id5, &genesis().compute_id(), |_| false),
				CachedFinalityVotes {
					stopped_at_finalized_sibling: false,
					unaccounted_ancestry: headers
						.iter()
						.map(|header| (header.compute_id(), None, header.clone(),))
						.rev()
						.collect(),
					votes: None,
				},
			);

			// let's now create entry at #3
			let hash3 = headers[2].compute_hash();
			let votes_at_3 = FinalityVotes {
				votes: vec![([42; 20].into(), 21)].into_iter().collect(),
				ancestry: vec![FinalityAncestor {
					id: HeaderId {
						number: 100,
						hash: Default::default(),
					},
					..Default::default()
				}]
				.into_iter()
				.collect(),
			};
			FinalityCache::<TestRuntime>::insert(hash3, votes_at_3.clone());

			// searching at #6 again => entry is found
			assert_eq!(
				storage.cached_finality_votes(&id5, &genesis().compute_id(), |_| false),
				CachedFinalityVotes {
					stopped_at_finalized_sibling: false,
					unaccounted_ancestry: headers
						.iter()
						.skip(3)
						.map(|header| (header.compute_id(), None, header.clone(),))
						.rev()
						.collect(),
					votes: Some(votes_at_3),
				},
			);
		});
	}

	#[test]
	fn cached_finality_votes_stops_at_finalized_sibling() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();

			// insert header1
			let header1 = HeaderBuilder::with_parent_number(0).sign_by_set(&ctx.validators);
			let header1_id = header1.compute_id();
			insert_header(&mut storage, header1);

			// insert header1' - sibling of header1
			let header1s = HeaderBuilder::with_parent_number(0)
				.gas_limit((GAS_LIMIT + 1).into())
				.sign_by_set(&ctx.validators);
			let header1s_id = header1s.compute_id();
			insert_header(&mut storage, header1s);

			// header1 is finalized
			FinalizedBlock::<DefaultInstance>::put(header1_id);

			// trying to get finality votes when importing header2 -> header1 succeeds
			assert!(
				!storage
					.cached_finality_votes(&header1_id, &genesis().compute_id(), |_| false)
					.stopped_at_finalized_sibling
			);

			// trying to get finality votes when importing header2s -> header1s fails
			assert!(
				storage
					.cached_finality_votes(&header1s_id, &header1_id, |_| false)
					.stopped_at_finalized_sibling
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_works_for_best_finalized_header() {
		run_test_with_genesis(example_header(), TOTAL_VALIDATORS, |_| {
			let storage = BridgeStorage::<TestRuntime>::new();
			assert_eq!(
				verify_transaction_finalized(
					&storage,
					example_header().compute_hash(),
					0,
					&[(example_tx(), example_tx_receipt(true))],
				),
				true,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_works_for_best_finalized_header_ancestor() {
		run_test(TOTAL_VALIDATORS, |_| {
			let mut storage = BridgeStorage::<TestRuntime>::new();
			insert_header(&mut storage, example_header_parent());
			insert_header(&mut storage, example_header());
			storage.finalize_and_prune_headers(Some(example_header().compute_id()), 0);
			assert_eq!(
				verify_transaction_finalized(
					&storage,
					example_header_parent().compute_hash(),
					0,
					&[(example_tx(), example_tx_receipt(true))],
				),
				true,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_rejects_proof_with_missing_tx() {
		run_test_with_genesis(example_header(), TOTAL_VALIDATORS, |_| {
			let storage = BridgeStorage::<TestRuntime>::new();
			assert_eq!(
				verify_transaction_finalized(&storage, example_header().compute_hash(), 1, &[],),
				false,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_rejects_unknown_header() {
		run_test(TOTAL_VALIDATORS, |_| {
			let storage = BridgeStorage::<TestRuntime>::new();
			assert_eq!(
				verify_transaction_finalized(&storage, example_header().compute_hash(), 1, &[],),
				false,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_rejects_unfinalized_header() {
		run_test(TOTAL_VALIDATORS, |_| {
			let mut storage = BridgeStorage::<TestRuntime>::new();
			insert_header(&mut storage, example_header_parent());
			insert_header(&mut storage, example_header());
			assert_eq!(
				verify_transaction_finalized(
					&storage,
					example_header().compute_hash(),
					0,
					&[(example_tx(), example_tx_receipt(true))],
				),
				false,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_rejects_finalized_header_sibling() {
		run_test(TOTAL_VALIDATORS, |_| {
			let mut finalized_header_sibling = example_header();
			finalized_header_sibling.timestamp = 1;
			let finalized_header_sibling_hash = finalized_header_sibling.compute_hash();

			let mut storage = BridgeStorage::<TestRuntime>::new();
			insert_header(&mut storage, example_header_parent());
			insert_header(&mut storage, example_header());
			insert_header(&mut storage, finalized_header_sibling);
			storage.finalize_and_prune_headers(Some(example_header().compute_id()), 0);
			assert_eq!(
				verify_transaction_finalized(
					&storage,
					finalized_header_sibling_hash,
					0,
					&[(example_tx(), example_tx_receipt(true))],
				),
				false,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_rejects_finalized_header_uncle() {
		run_test(TOTAL_VALIDATORS, |_| {
			let mut finalized_header_uncle = example_header_parent();
			finalized_header_uncle.timestamp = 1;
			let finalized_header_uncle_hash = finalized_header_uncle.compute_hash();

			let mut storage = BridgeStorage::<TestRuntime>::new();
			insert_header(&mut storage, example_header_parent());
			insert_header(&mut storage, finalized_header_uncle);
			insert_header(&mut storage, example_header());
			storage.finalize_and_prune_headers(Some(example_header().compute_id()), 0);
			assert_eq!(
				verify_transaction_finalized(
					&storage,
					finalized_header_uncle_hash,
					0,
					&[(example_tx(), example_tx_receipt(true))],
				),
				false,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_rejects_invalid_transactions_in_proof() {
		run_test_with_genesis(example_header(), TOTAL_VALIDATORS, |_| {
			let storage = BridgeStorage::<TestRuntime>::new();
			assert_eq!(
				verify_transaction_finalized(
					&storage,
					example_header().compute_hash(),
					0,
					&[
						(example_tx(), example_tx_receipt(true)),
						(example_tx(), example_tx_receipt(true))
					],
				),
				false,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_rejects_invalid_receipts_in_proof() {
		run_test_with_genesis(example_header(), TOTAL_VALIDATORS, |_| {
			let storage = BridgeStorage::<TestRuntime>::new();
			assert_eq!(
				verify_transaction_finalized(
					&storage,
					example_header().compute_hash(),
					0,
					&[(example_tx(), vec![42])],
				),
				false,
			);
		});
	}

	#[test]
	fn verify_transaction_finalized_rejects_failed_transaction() {
		run_test_with_genesis(example_header_with_failed_receipt(), TOTAL_VALIDATORS, |_| {
			let storage = BridgeStorage::<TestRuntime>::new();
			assert_eq!(
				verify_transaction_finalized(
					&storage,
					example_header_with_failed_receipt().compute_hash(),
					0,
					&[(example_tx(), example_tx_receipt(false))],
				),
				false,
			);
		});
	}
}
