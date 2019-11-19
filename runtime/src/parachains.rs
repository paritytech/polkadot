// Copyright 2017-2019 Parity Technologies (UK) Ltd.
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

//! Main parachains logic. For now this is just the determination of which validators do what.

use rstd::prelude::*;
use rstd::result;
use rstd::collections::btree_map::BTreeMap;
use codec::{Encode, Decode};

use sr_primitives::traits::{
	Hash as HashT, BlakeTwo256, Saturating, One, Zero, Dispatchable,
	AccountIdConversion,
};
use sr_primitives::weights::SimpleDispatchInfo;
use primitives::{
	Hash, Balance,
	parachain::{
		self, Id as ParaId, Chain, DutyRoster, AttestedCandidate, Statement, ParachainDispatchOrigin,
		UpwardMessage, BlockIngressRoots, ValidatorId, ActiveParas, CollatorId, Retriable
	},
};
use paint_support::{
	Parameter, dispatch::Result, decl_storage, decl_module, ensure,
	traits::{Currency, Get, WithdrawReason, ExistenceRequirement, Randomness},
};

use inherents::{ProvideInherent, InherentData, MakeFatalError, InherentIdentifier};

use system::ensure_none;
use crate::attestations::{self, IncludedBlocks};
use crate::registrar::Registrar;

// ranges for iteration of general block number don't work, so this
// is a utility to get around that.
struct BlockNumberRange<N> {
	low: N,
	high: N,
}

impl<N: Saturating + One + PartialOrd + PartialEq + Clone> Iterator for BlockNumberRange<N> {
	type Item = N;

	fn next(&mut self) -> Option<N> {
		if self.low >= self.high {
			return None
		}

		let item = self.low.clone();
		self.low = self.low.clone().saturating_add(One::one());
		Some(item)
	}
}

// creates a range iterator between `low` and `high`. `low` must be <= `high`.
fn number_range<N>(low: N, high: N) -> BlockNumberRange<N> {
	BlockNumberRange { low, high }
}

// wrapper trait because an associated type of `Currency<Self::AccountId,Balance=Balance>`
// doesn't work.`
pub trait ParachainCurrency<AccountId> {
	fn free_balance(para_id: ParaId) -> Balance;
	fn deduct(para_id: ParaId, amount: Balance) -> Result;
}

impl<AccountId, T: Currency<AccountId>> ParachainCurrency<AccountId> for T where
	T::Balance: From<Balance> + Into<Balance>,
	ParaId: AccountIdConversion<AccountId>,
{
	fn free_balance(para_id: ParaId) -> Balance {
		let para_account = para_id.into_account();
		T::free_balance(&para_account).into()
	}

	fn deduct(para_id: ParaId, amount: Balance) -> Result {
		let para_account = para_id.into_account();

		// burn the fee.
		let _ = T::withdraw(
			&para_account,
			amount.into(),
			WithdrawReason::Fee.into(),
			ExistenceRequirement::KeepAlive,
		)?;

		Ok(())
	}
}

/// Interface to the persistent (stash) identities of the current validators.
pub struct ValidatorIdentities<T>(rstd::marker::PhantomData<T>);

impl<T: session::Trait> Get<Vec<T::ValidatorId>> for ValidatorIdentities<T> {
	fn get() -> Vec<T::ValidatorId> {
		<session::Module<T>>::validators()
	}
}

pub trait Trait: attestations::Trait {
	/// The outer origin type.
	type Origin: From<Origin> + From<system::RawOrigin<Self::AccountId>>;

	/// The outer call dispatch type.
	type Call: Parameter + Dispatchable<Origin=<Self as Trait>::Origin>;

	/// Some way of interacting with balances for fees.
	type ParachainCurrency: ParachainCurrency<Self::AccountId>;

	/// Something that provides randomness in the runtime.
	type Randomness: Randomness<Self::Hash>;

	/// Means to determine what the current set of active parachains are.
	type ActiveParachains: ActiveParas;

	/// The way that we are able to register parachains.
	type Registrar: Registrar<Self::AccountId>;
}

/// Origin for the parachains module.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum Origin {
	/// It comes from a parachain.
	Parachain(ParaId),
}

// result of <NodeCodec<Blake2Hasher> as trie_db::NodeCodec<Blake2Hasher>>::hashed_null_node()
const EMPTY_TRIE_ROOT: [u8; 32] = [
	3, 23, 10, 46, 117, 151, 183, 183, 227, 216, 76, 5, 57, 29, 19, 154,
	98, 177, 87, 231, 135, 134, 216, 192, 130, 242, 157, 207, 76, 17, 19, 20
];

/// Total number of individual messages allowed in the parachain -> relay-chain message queue.
const MAX_QUEUE_COUNT: usize = 100;
/// Total size of messages allowed in the parachain -> relay-chain message queue before which no
/// further messages may be added to it. If it exceeds this then the queue may contain only a
/// single message.
const WATERMARK_QUEUE_SIZE: usize = 20000;

decl_storage! {
	trait Store for Module<T: Trait> as Parachains {
		/// All authorities' keys at the moment.
		pub Authorities get(authorities): Vec<ValidatorId>;
		/// The parachains registered at present.
		pub Code get(parachain_code): map ParaId => Option<Vec<u8>>;
		/// The heads of the parachains registered at present.
		pub Heads get(parachain_head): map ParaId => Option<Vec<u8>>;
		/// The watermark heights of the parachains registered at present.
		/// For every parachain, this is the block height from which all messages targeting
		/// that parachain have been processed. Can be `None` only if the parachain doesn't exist.
		pub Watermarks get(watermark): map ParaId => Option<T::BlockNumber>;

		/// Unrouted ingress. Maps (BlockNumber, to_chain) pairs to [(from_chain, egress_root)].
		///
		/// There may be an entry under (i, p) in this map for every i between the parachain's
		/// watermark and the current block.
		pub UnroutedIngress: map (T::BlockNumber, ParaId) => Option<Vec<(ParaId, Hash)>>;

		/// Messages ready to be dispatched onto the relay chain. It is subject to
		/// `MAX_MESSAGE_COUNT` and `WATERMARK_MESSAGE_SIZE`.
		pub RelayDispatchQueue: map ParaId => Vec<UpwardMessage>;
		/// Size of the dispatch queues. Separated from actual data in order to avoid costly
		/// decoding when checking receipt validity. First item in tuple is the count of messages
		///	second if the total length (in bytes) of the message payloads.
		pub RelayDispatchQueueSize: map ParaId => (u32, u32);
		/// The ordered list of ParaIds that have a `RelayDispatchQueue` entry.
		NeedsDispatch: Vec<ParaId>;

		/// Some if the parachain heads get updated in this block, along with the parachain IDs that
		/// did update. Ordered in the same way as `registrar::Active` (i.e. by ParaId).
		///
		/// None if not yet updated.
		pub DidUpdate: Option<Vec<ParaId>>;
	}
	add_extra_genesis {
		config(authorities): Vec<ValidatorId>;
		build(|config| Module::<T>::initialize_authorities(&config.authorities))
	}
}

decl_module! {
	/// Parachains module.
	pub struct Module<T: Trait> for enum Call where origin: <T as system::Trait>::Origin {
		/// Provide candidate receipts for parachains, in ascending order by id.
		#[weight = SimpleDispatchInfo::FixedNormal(1_000_000)]
		pub fn set_heads(origin, heads: Vec<AttestedCandidate>) -> Result {
			ensure_none(origin)?;
			ensure!(!<DidUpdate>::exists(), "Parachain heads must be updated only once in the block");

			let active_parachains = Self::active_parachains();

			let parachain_count = active_parachains.len();
			ensure!(heads.len() <= parachain_count, "Too many parachain candidates");

			let mut proceeded = Vec::with_capacity(heads.len());

			if !active_parachains.is_empty() {
				// perform integrity checks before writing to storage.
				{
					let mut last_id = None;

					let mut iter = active_parachains.iter();
					for head in &heads {
						let id = head.parachain_index();
						// proposed heads must be ascending order by parachain ID without duplicate.
						ensure!(
							last_id.as_ref().map_or(true, |x| x < &id),
							"candidate out of order"
						);

						// must be unknown since active parachains are always sorted.
						let (_, maybe_required_collator) = iter.find(|para| para.0 == id)
							.ok_or("candidate for unregistered parachain {}")?;

						if let Some((required_collator, _)) = maybe_required_collator {
							ensure!(required_collator == &head.candidate.collator, "invalid collator");
						}

						Self::check_upward_messages(
							id,
							&head.candidate.upward_messages,
							MAX_QUEUE_COUNT,
							WATERMARK_QUEUE_SIZE,
						)?;
						Self::check_egress_queue_roots(&head, &active_parachains)?;

						let id = head.parachain_index();
						proceeded.push(id);
						last_id = Some(id);
					}
				}

				let para_blocks = Self::check_candidates(&heads, &active_parachains)?;
				let current_number = <system::Module<T>>::block_number();

				<attestations::Module<T>>::note_included(&heads, para_blocks);

				Self::update_routing(
					current_number,
					&heads,
				);

				Self::dispatch_upward_messages(
					MAX_QUEUE_COUNT,
					WATERMARK_QUEUE_SIZE,
					Self::dispatch_message,
				);
			}

			DidUpdate::put(proceeded);

			Ok(())
		}

		fn on_initialize() {
			<Self as Store>::DidUpdate::kill();
		}

		fn on_finalize() {
			assert!(<Self as Store>::DidUpdate::exists(), "Parachain heads must be updated once in the block");
		}
	}
}

fn majority_of(list_len: usize) -> usize {
	list_len / 2 + list_len % 2
}

fn localized_payload(statement: Statement, parent_hash: ::primitives::Hash) -> Vec<u8> {
	let mut encoded = statement.encode();
	encoded.extend(parent_hash.as_ref());
	encoded
}

impl<T: Trait> Module<T> {
	/// Initialize the state of a new parachain/parathread.
	pub fn initialize_para(
		id: ParaId,
		code: Vec<u8>,
		initial_head_data: Vec<u8>,
	) {
		<Code>::insert(id, code);
		<Heads>::insert(id, initial_head_data);

		// Because there are no ordering guarantees that inherents
		// are applied before regular transactions, a parachain candidate could
		// be registered before the `UpdateHeads` inherent is processed. If so, messages
		// could be sent to a parachain in the block it is registered.
		<Watermarks<T>>::insert(id, <system::Module<T>>::block_number().saturating_sub(One::one()));
	}

	pub fn cleanup_para(
		id: ParaId,
	) {
		<Code>::remove(id);
		<Heads>::remove(id);

		let watermark = <Watermarks<T>>::take(id);

		// clear all routing entries _to_. But not those _from_.
		if let Some(watermark) = watermark {
			let now = <system::Module<T>>::block_number();

			// iterate over all blocks between watermark and now + 1 (since messages might
			// have already been sent to `id` in this block.
			for unrouted_block in number_range(watermark, now).map(|n| n.saturating_add(One::one())) {
				<UnroutedIngress<T>>::remove(&(unrouted_block, id));
			}
		}
	}

	/// Dispatch some messages from a parachain.
	fn dispatch_message(
		id: ParaId,
		origin: ParachainDispatchOrigin,
		data: &[u8],
	) {
		if let Ok(message_call) = <T as Trait>::Call::decode(&mut &data[..]) {
			let origin: <T as Trait>::Origin = match origin {
				ParachainDispatchOrigin::Signed =>
					system::RawOrigin::Signed(id.into_account()).into(),
				ParachainDispatchOrigin::Parachain =>
					Origin::Parachain(id).into(),
				ParachainDispatchOrigin::Root =>
					system::RawOrigin::Root.into(),
			};
			let _ok = message_call.dispatch(origin).is_ok();
			// Not much to do with the result as it is. It's up to the parachain to ensure that the
			// message makes sense.
		}
	}

	/// Ensure all is well with the upward messages.
	fn check_upward_messages(
		id: ParaId,
		upward_messages: &[UpwardMessage],
		max_queue_count: usize,
		watermark_queue_size: usize,
	) -> Result {
		// Either there are no more messages to add...
		if !upward_messages.is_empty() {
			let (count, size) = <RelayDispatchQueueSize>::get(id);
			ensure!(
				// ...or we are appending one message onto an empty queue...
				upward_messages.len() + count as usize == 1
				// ...or...
				|| (
					// ...the total messages in the queue ends up being no greater than the
					// limit...
					upward_messages.len() + count as usize <= max_queue_count
				&&
					// ...and the total size of the payloads in the queue ends up being no
					// greater than the limit.
					upward_messages.iter()
						.fold(size as usize, |a, x| a + x.data.len())
					<= watermark_queue_size
				),
				"Messages added when queue full"
			);
			if !id.is_system() {
				for m in upward_messages.iter() {
					ensure!(m.origin != ParachainDispatchOrigin::Root, "bad message origin");
				}
			}
		}
		Ok(())
	}

	/// Update routing information from the parachain heads. This queues upwards
	/// messages to the relay chain as well.
	fn update_routing(
		now: T::BlockNumber,
		heads: &[AttestedCandidate],
	) {
		// TODO: per-chain watermark
		// https://github.com/paritytech/polkadot/issues/286
		let watermark = now.saturating_sub(One::one());

		let mut ingress_update = BTreeMap::new();

		// we sort them in order to provide a fast lookup to ensure we can avoid duplicates in the
		// needs_dispatch queue.
		let mut ordered_needs_dispatch = NeedsDispatch::get();

		for head in heads.iter() {
			let id = head.parachain_index();
			<Heads>::insert(id, &head.candidate.head_data.0);

			let last_watermark = <Watermarks<T>>::mutate(id, |mark| {
				rstd::mem::replace(mark, Some(watermark))
			});

			if let Some(last_watermark) = last_watermark {
				// Discard routed ingress.
				for routed_height in number_range(last_watermark, watermark) {
					<UnroutedIngress<T>>::remove(&(routed_height, id));
				}
			}

			// place our egress root to `to` into the ingress table for (now, `to`).
			for &(to, root) in &head.candidate.egress_queue_roots {
				ingress_update.entry(to).or_insert_with(Vec::new).push((id, root));
			}

			// Queue up upwards messages (from parachains to relay chain).
			Self::queue_upward_messages(
				id,
				&head.candidate.upward_messages,
				&mut ordered_needs_dispatch,
			);
		}

		NeedsDispatch::put(ordered_needs_dispatch);

		// apply the ingress update.
		for (to, ingress_roots) in ingress_update {
			<UnroutedIngress<T>>::insert((now, to), ingress_roots);
		}
	}

	/// Place any new upward messages into our queue for later dispatch.
	///
	/// `ordered_needs_dispatch` is mutated to ensure it reflects the new value of
	/// `RelayDispatchQueueSize`. It is up to the caller to guarantee that it gets written into
	/// storage after this call.
	fn queue_upward_messages(
		id: ParaId,
		upward_messages: &[UpwardMessage],
		ordered_needs_dispatch: &mut Vec<ParaId>,
	) {
		if !upward_messages.is_empty() {
			RelayDispatchQueueSize::mutate(id, |&mut(ref mut count, ref mut len)| {
				*count += upward_messages.len() as u32;
				*len += upward_messages.iter()
					.fold(0, |a, x| a + x.data.len()) as u32;
			});
			// Should never be able to fail assuming our state is uncorrupted, but best not
			// to panic, even if it does.
			let _ = RelayDispatchQueue::append(id, upward_messages);
			if let Err(i) = ordered_needs_dispatch.binary_search(&id) {
				// same.
				ordered_needs_dispatch.insert(i, id);
			} else {
				sr_primitives::print("ordered_needs_dispatch contains id?!");
			}
		}
	}

	/// Simple FIFO dispatcher.
	fn dispatch_upward_messages(
		max_queue_count: usize,
		watermark_queue_size: usize,
		mut dispatch_message: impl FnMut(ParaId, ParachainDispatchOrigin, &[u8]),
	) {
		let queueds = NeedsDispatch::get();
		let mut drained_count = 0usize;
		let mut dispatched_count = 0usize;
		let mut dispatched_size = 0usize;
		for id in queueds.iter() {
			drained_count += 1;

			let (count, size) = <RelayDispatchQueueSize>::get(id);
			let count = count as usize;
			let size = size as usize;
			if dispatched_count == 0 || (
				dispatched_count + count <= max_queue_count
					&& dispatched_size + size <= watermark_queue_size
			) {
				if count > 0 {
					// still dispatching messages...
					RelayDispatchQueueSize::remove(id);
					let messages = RelayDispatchQueue::take(id);
					for UpwardMessage { origin, data } in messages.into_iter() {
						dispatch_message(*id, origin, &data);
					}
					dispatched_count += count;
					dispatched_size += size;
					if dispatched_count >= max_queue_count
						|| dispatched_size >= watermark_queue_size
					{
						break
					}
				}
			}
		}
		NeedsDispatch::put(&queueds[drained_count..]);
	}

	/// Calculate the current block's duty roster using system's random seed.
	/// Returns the duty roster along with the random seed.
	pub fn calculate_duty_roster() -> (DutyRoster, [u8; 32]) {
		let parachains = Self::active_parachains();
		let parachain_count = parachains.len();

		// TODO: use decode length. substrate #2794
		let validator_count = Self::authorities().len();
		let validators_per_parachain =
			if parachain_count == 0 {
				0
			} else {
				(validator_count - 1) / parachain_count
			};

		let mut roles_val = (0..validator_count).map(|i| match i {
			i if i < parachain_count * validators_per_parachain => {
				let idx = i / validators_per_parachain;
				Chain::Parachain(parachains[idx].0.clone())
			}
			_ => Chain::Relay,
		}).collect::<Vec<_>>();

		let mut seed = {
			let phrase = b"validator_role_pairs";
			let seed = T::Randomness::random(&phrase[..]);
			let seed_len = seed.as_ref().len();
			let needed_bytes = validator_count * 4;

			// hash only the needed bits of the random seed.
			// if earlier bits are influencable, they will not factor into
			// the seed used here.
			let seed_off = if needed_bytes >= seed_len {
				0
			} else {
				seed_len - needed_bytes
			};

			BlakeTwo256::hash(&seed.as_ref()[seed_off..])
		};

		let orig_seed = seed.clone().to_fixed_bytes();

		// shuffle
		for i in 0..(validator_count.saturating_sub(1)) {
			// 4 bytes of entropy used per cycle, 32 bytes entropy per hash
			let offset = (i * 4 % 32) as usize;

			// number of roles remaining to select from.
			let remaining = rstd::cmp::max(1, (validator_count - i) as usize);

			// 8 32-bit ints per 256-bit seed.
			let val_index = u32::decode(&mut &seed[offset..offset + 4])
				.expect("using 4 bytes for a 32-bit quantity") as usize % remaining;

			if offset == 28 {
				// into the last 4 bytes - rehash to gather new entropy
				seed = BlakeTwo256::hash(seed.as_ref());
			}

			// exchange last item with randomly chosen first.
			roles_val.swap(remaining - 1, val_index);
		}

		(DutyRoster { validator_duty: roles_val, }, orig_seed)
	}

	/// Calculate the ingress to a specific parachain.
	/// If `since` is provided, only messages since (including those in) that block
	/// will be included.
	/// Complexity: O(n) in the number of blocks since the supplied block.
	/// invoked off-chain.
	///
	/// Yields a structure containing all unrouted ingress to the parachain.
	pub fn ingress(to: ParaId, since: Option<T::BlockNumber>) -> Option<Vec<(T::BlockNumber, BlockIngressRoots)>> {
		let watermark = <Watermarks<T>>::get(to)?;
		let now = <system::Module<T>>::block_number();

		let watermark_since = watermark.saturating_add(One::one());
		let since = rstd::cmp::max(since.unwrap_or(Zero::zero()), watermark_since);
		if since > now {
			return Some(Vec::new());
		}

		Some(number_range(since, now)
			.filter_map(|unrouted_height| {
				<UnroutedIngress<T>>::get(&(unrouted_height, to)).map(|roots| {
					(unrouted_height, BlockIngressRoots(roots))
				})
			})
			.collect())
	}

	/// Get the parachain status necessary for validation.
	pub fn parachain_status(id: &parachain::Id) -> Option<parachain::Status> {
		let balance = T::ParachainCurrency::free_balance(*id);
		Self::parachain_head(id).map(|head_data| parachain::Status {
			head_data: parachain::HeadData(head_data),
			balance,
			// TODO: https://github.com/paritytech/polkadot/issues/92
			// plug in some real values here. most likely governable.
			fee_schedule: parachain::FeeSchedule {
				base: 0,
				per_byte: 0,
			}
		})
	}

	/// Get the currently active set of parachains.
	pub fn active_parachains() -> Vec<(ParaId, Option<(CollatorId, Retriable)>)> {
		T::ActiveParachains::active_paras()
	}

	fn check_egress_queue_roots(
		head: &AttestedCandidate,
		active_parachains: &[(ParaId, Option<(CollatorId, Retriable)>)]
	) -> Result {
		let mut last_egress_id = None;
		let mut iter = active_parachains.iter().map(|x| x.0);
		for (egress_para_id, root) in &head.candidate.egress_queue_roots {
			// egress routes should be ascending order by parachain ID without duplicate.
			ensure!(
				last_egress_id.as_ref().map_or(true, |x| x < &egress_para_id),
				"Egress routes out of order by ID"
			);

			// a parachain can't route to self
			ensure!(
				*egress_para_id != head.candidate.parachain_index,
				"Parachain routing to self"
			);

			// no empty trie roots
			ensure!(
				*root != EMPTY_TRIE_ROOT.into(),
				"Empty trie root included"
			);

			// can't route to a parachain which doesn't exist
			ensure!(
				iter.find(|x| x == egress_para_id).is_some(),
				"Routing to non-existent parachain"
			);

			last_egress_id = Some(egress_para_id)
		}
		Ok(())
	}

	// check the attestations on these candidates. The candidates should have been checked
	// that each candidates' chain ID is valid.
	fn check_candidates(
		attested_candidates: &[AttestedCandidate],
		active_parachains: &[(ParaId, Option<(CollatorId, Retriable)>)]
	) -> rstd::result::Result<IncludedBlocks<T>, &'static str>
	{
		use primitives::parachain::ValidityAttestation;
		use sr_primitives::traits::AppVerify;

		// returns groups of slices that have the same chain ID.
		// assumes the inner slice is sorted by id.
		struct GroupedDutyIter<'a> {
			next_idx: usize,
			inner: &'a [(usize, ParaId)],
		}

		impl<'a> GroupedDutyIter<'a> {
			fn new(inner: &'a [(usize, ParaId)]) -> Self {
				GroupedDutyIter { next_idx: 0, inner }
			}

			fn group_for(&mut self, wanted_id: ParaId) -> Option<&'a [(usize, ParaId)]> {
				while let Some((id, keys)) = self.next() {
					if wanted_id == id {
						return Some(keys)
					}
				}

				None
			}
		}

		impl<'a> Iterator for GroupedDutyIter<'a> {
			type Item = (ParaId, &'a [(usize, ParaId)]);

			fn next(&mut self) -> Option<Self::Item> {
				if self.next_idx == self.inner.len() { return None }
				let start_idx = self.next_idx;
				self.next_idx += 1;
				let start_id = self.inner[start_idx].1;

				while self.inner.get(self.next_idx).map_or(false, |&(_, ref id)| id == &start_id) {
					self.next_idx += 1;
				}

				Some((start_id, &self.inner[start_idx..self.next_idx]))
			}
		}

		let authorities = Self::authorities();
		let (duty_roster, random_seed) = Self::calculate_duty_roster();

		// convert a duty roster, which is originally a Vec<Chain>, where each
		// item corresponds to the same position in the session keys, into
		// a list containing (index, parachain duty) where indices are into the session keys.
		// this list is sorted ascending by parachain duty, just like the
		// parachain candidates are.
		let make_sorted_duties = |duty: &[Chain]| {
			let mut sorted_duties = Vec::with_capacity(duty.len());
			for (val_idx, duty) in duty.iter().enumerate() {
				let id = match duty {
					Chain::Relay => continue,
					Chain::Parachain(id) => id,
				};

				let idx = sorted_duties.binary_search_by_key(&id, |&(_, ref id)| id)
					.unwrap_or_else(|idx| idx);

				sorted_duties.insert(idx, (val_idx, *id));
			}

			sorted_duties
		};

		let sorted_validators = make_sorted_duties(&duty_roster.validator_duty);

		let parent_hash = super::System::parent_hash();
		let localized_payload = |statement: Statement| localized_payload(statement, parent_hash);

		let mut validator_groups = GroupedDutyIter::new(&sorted_validators[..]);

		let mut para_block_hashes = Vec::new();
		for candidate in attested_candidates {
			let para_id = candidate.parachain_index();
			let validator_group = validator_groups.group_for(para_id)
				.ok_or("no validator group for parachain")?;

			ensure!(
				candidate.validity_votes.len() >= majority_of(validator_group.len()),
				"Not enough validity attestations"
			);

			ensure!(
				candidate.validity_votes.len() <= authorities.len(),
				"The number of attestations exceeds the number of authorities"
			);

			let fees = candidate.candidate().fees;
			T::ParachainCurrency::deduct(para_id, fees)?;

			let mut candidate_hash = None;
			let mut encoded_implicit = None;
			let mut encoded_explicit = None;

			let mut expected_votes_len = 0;
			for (vote_index, (auth_index, _)) in candidate.validator_indices
				.iter()
				.enumerate()
				.filter(|(_, bit)| *bit)
				.enumerate()
			{
				let validity_attestation = match candidate.validity_votes.get(vote_index) {
					None => return Err("Not enough validity votes"),
					Some(v) => {
						expected_votes_len = vote_index + 1;
						v
					}
				};

				if validator_group.iter().find(|&(idx, _)| *idx == auth_index).is_none() {
					return Err("Attesting validator not on this chain's validation duty.");
				}

				let (payload, sig) = match validity_attestation {
					ValidityAttestation::Implicit(sig) => {
						let payload = encoded_implicit.get_or_insert_with(|| localized_payload(
							Statement::Candidate(candidate.candidate.clone()),
						));

						(payload, sig)
					}
					ValidityAttestation::Explicit(sig) => {
						let hash = candidate_hash
							.get_or_insert_with(|| candidate.candidate.hash())
							.clone();

						let payload = encoded_explicit.get_or_insert_with(|| localized_payload(
							Statement::Valid(hash),
						));

						(payload, sig)
					}
				};

				ensure!(
					sig.verify(&payload[..], &authorities[auth_index]),
					"Candidate validity attestation signature is bad."
				);
			}

			para_block_hashes.push(candidate_hash.unwrap_or_else(|| candidate.candidate().hash()));

		ensure!(
				candidate.validity_votes.len() == expected_votes_len,
				"Extra untagged validity votes along with candidate"
			);
		}

		Ok(IncludedBlocks {
			actual_number: <system::Module<T>>::block_number(),
			session: <session::Module<T>>::current_index(),
			random_seed,
			active_parachains: active_parachains.iter().map(|x| x.0).collect(),
			para_blocks: para_block_hashes,
		})
	}

	fn initialize_authorities(authorities: &[ValidatorId]) {
		if !authorities.is_empty() {
			assert!(Authorities::get().is_empty(), "Authorities are already initialized!");
			Authorities::put(authorities);
		}
	}

/*
	// TODO: Consider integrating if needed. (https://github.com/paritytech/polkadot/issues/223)
	/// Extract the parachain heads from the block.
	pub fn parachain_heads(&self) -> &[CandidateReceipt] {
		let x = self.inner.extrinsics.get(PARACHAINS_SET_POSITION as usize).and_then(|xt| match xt.function {
			Call::Parachains(ParachainsCall::set_heads(ref x)) => Some(&x[..]),
			_ => None
		});

		match x {
			Some(x) => x,
			None => panic!("Invalid polkadot block asserted at {:?}", self.file_line),
		}
	}
*/
}

impl<T: Trait> sr_primitives::BoundToRuntimeAppPublic for Module<T> {
	type Public = ValidatorId;
}

impl<T: Trait> session::OneSessionHandler<T::AccountId> for Module<T> {
	type Key = ValidatorId;

	fn on_genesis_session<'a, I: 'a>(validators: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		Self::initialize_authorities(&validators.map(|(_, key)| key).collect::<Vec<_>>());
	}

	fn on_new_session<'a, I: 'a>(changed: bool, validators: I, _queued: I)
		where I: Iterator<Item=(&'a T::AccountId, Self::Key)>
	{
		if changed {
			<Self as Store>::Authorities::put(validators.map(|(_, key)| key).collect::<Vec<_>>());
		}
	}

	fn on_disabled(_i: usize) { }
}

/// An identifier for inherent data that provides new minimally-attested
/// parachain heads.
pub const NEW_HEADS_IDENTIFIER: InherentIdentifier = *b"newheads";

pub type InherentType = Vec<AttestedCandidate>;

impl<T: Trait> ProvideInherent for Module<T> {
	type Call = Call<T>;
	type Error = MakeFatalError<inherents::Error>;
	const INHERENT_IDENTIFIER: InherentIdentifier = NEW_HEADS_IDENTIFIER;

	fn create_inherent(data: &InherentData) -> Option<Self::Call> {
		let data = data.get_data::<InherentType>(&NEW_HEADS_IDENTIFIER)
			.expect("Parachain heads could not be decoded.")
			.expect("No parachain heads found in inherent data.");

		Some(Call::set_heads(data))
	}
}

/// Ensure that the origin `o` represents a parachain.
/// Returns `Ok` with the parachain ID that effected the extrinsic or an `Err` otherwise.
pub fn ensure_parachain<OuterOrigin>(o: OuterOrigin) -> result::Result<ParaId, &'static str>
	where OuterOrigin: Into<result::Result<Origin, OuterOrigin>>
{
	match o.into() {
		Ok(Origin::Parachain(id)) => Ok(id),
		_ => Err("bad origin: expected to be a parachain origin"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use super::Call as ParachainsCall;
	use bitvec::{bitvec, vec::BitVec};
	use sr_io::TestExternalities;
	use substrate_primitives::{H256, Blake2Hasher};
	use substrate_trie::NodeCodec;
	use sr_primitives::{
		Perbill, curve::PiecewiseLinear, testing::{UintAuthorityId, Header},
		traits::{BlakeTwo256, IdentityLookup, OnInitialize, OnFinalize},
	};
	use primitives::{
		parachain::{
			CandidateReceipt, HeadData, ValidityAttestation, ValidatorId, Info as ParaInfo,
			Scheduling,
		},
		BlockNumber,
	};
	use crate::constants::time::*;
	use keyring::Sr25519Keyring;
	use paint_support::{
		impl_outer_origin, impl_outer_dispatch, assert_ok, assert_err, parameter_types,
	};
	use crate::parachains;
	use crate::registrar;
	use crate::slots;

	impl_outer_origin! {
		pub enum Origin for Test {
			parachains
		}
	}

	impl_outer_dispatch! {
		pub enum Call for Test where origin: Origin {
			parachains::Parachains,
		}
	}

	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u32 = 250;
		pub const MaximumBlockWeight: u32 = 4 * 1024 * 1024;
		pub const MaximumBlockLength: u32 = 4 * 1024 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::from_percent(75);
	}

	impl system::Trait for Test {
		type Origin = Origin;
		type Call = Call;
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<u64>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
	}

	parameter_types! {
		pub const Period: BlockNumber = 1;
		pub const Offset: BlockNumber = 0;
		pub const DisabledValidatorsThreshold: Perbill = Perbill::from_percent(17);
	}

	impl session::Trait for Test {
		type OnSessionEnding = ();
		type Keys = UintAuthorityId;
		type ShouldEndSession = session::PeriodicSessions<Period, Offset>;
		type SessionHandler = session::TestSessionHandler;
		type Event = ();
		type SelectInitialValidators = staking::Module<Self>;
		type ValidatorId = u64;
		type ValidatorIdOf = staking::StashOf<Self>;
		type DisabledValidatorsThreshold = DisabledValidatorsThreshold;
	}

	impl session::historical::Trait for Test {
		type FullIdentification = staking::Exposure<u64, Balance>;
		type FullIdentificationOf = staking::ExposureOf<Self>;
	}

	parameter_types! {
		pub const MinimumPeriod: u64 = 3;
	}
	impl timestamp::Trait for Test {
		type Moment = u64;
		type OnTimestampSet = ();
		type MinimumPeriod = MinimumPeriod;
	}

	parameter_types! {
		pub const EpochDuration: u64 = EPOCH_DURATION_IN_BLOCKS as u64;
		pub const ExpectedBlockTime: u64 = MILLISECS_PER_BLOCK;
	}

	impl babe::Trait for Test {
		type EpochDuration = EpochDuration;
		type ExpectedBlockTime = ExpectedBlockTime;

		// session module is the trigger
		type EpochChangeTrigger = babe::ExternalTrigger;
	}

	parameter_types! {
		pub const ExistentialDeposit: Balance = 0;
		pub const TransferFee: Balance = 0;
		pub const CreationFee: Balance = 0;
	}

	impl balances::Trait for Test {
		type Balance = Balance;
		type OnFreeBalanceZero = ();
		type OnNewAccount = ();
		type Event = ();
		type DustRemoval = ();
		type TransferPayment = ();
		type ExistentialDeposit = ExistentialDeposit;
		type TransferFee = TransferFee;
		type CreationFee = CreationFee;
	}

	paint_staking_reward_curve::build! {
		const REWARD_CURVE: PiecewiseLinear<'static> = curve!(
			min_inflation: 0_025_000,
			max_inflation: 0_100_000,
			ideal_stake: 0_500_000,
			falloff: 0_050_000,
			max_piece_count: 40,
			test_precision: 0_005_000,
		);
	}

	parameter_types! {
		pub const SessionsPerEra: sr_staking_primitives::SessionIndex = 6;
		pub const BondingDuration: staking::EraIndex = 28;
		pub const AttestationPeriod: BlockNumber = 100;
		pub const RewardCurve: &'static PiecewiseLinear<'static> = &REWARD_CURVE;
	}

	impl staking::Trait for Test {
		type RewardRemainder = ();
		type CurrencyToVote = ();
		type Event = ();
		type Currency = balances::Module<Test>;
		type Slash = ();
		type Reward = ();
		type SessionsPerEra = SessionsPerEra;
		type BondingDuration = BondingDuration;
		type SessionInterface = Self;
		type Time = timestamp::Module<Test>;
		type RewardCurve = RewardCurve;
	}

	impl attestations::Trait for Test {
		type AttestationPeriod = AttestationPeriod;
		type ValidatorIdentities = ValidatorIdentities<Test>;
		type RewardAttestation = ();
	}

	parameter_types!{
		pub const LeasePeriod: u64 = 10;
		pub const EndingPeriod: u64 = 3;
	}

	impl slots::Trait for Test {
		type Event = ();
		type Currency = balances::Module<Test>;
		type Parachains = registrar::Module<Test>;
		type EndingPeriod = EndingPeriod;
		type LeasePeriod = LeasePeriod;
		type Randomness = RandomnessCollectiveFlip;
	}

	parameter_types! {
		pub const ParathreadDeposit: Balance = 10;
		pub const QueueSize: usize = 2;
		pub const MaxRetries: u32 = 3;
	}

	impl registrar::Trait for Test {
		type Event = ();
		type Origin = Origin;
		type Currency = balances::Module<Test>;
		type ParathreadDeposit = ParathreadDeposit;
		type SwapAux = slots::Module<Test>;
		type QueueSize = QueueSize;
		type MaxRetries = MaxRetries;
	}

	impl Trait for Test {
		type Origin = Origin;
		type Call = Call;
		type ParachainCurrency = balances::Module<Test>;
		type Randomness = RandomnessCollectiveFlip;
		type ActiveParachains = registrar::Module<Test>;
		type Registrar = registrar::Module<Test>;
	}

	type Parachains = Module<Test>;
	type System = system::Module<Test>;
	type RandomnessCollectiveFlip = randomness_collective_flip::Module<Test>;
	type Registrar = registrar::Module<Test>;

	fn new_test_ext(parachains: Vec<(ParaId, Vec<u8>, Vec<u8>)>) -> TestExternalities {
		use staking::StakerStatus;
		use babe::AuthorityId as BabeAuthorityId;

		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();

		let authority_keys = [
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Eve,
			Sr25519Keyring::Ferdie,
			Sr25519Keyring::One,
			Sr25519Keyring::Two,
		];

		// stashes are the index.
		let session_keys: Vec<_> = authority_keys.iter().enumerate()
			.map(|(i, _k)| (i as u64, UintAuthorityId(i as u64)))
			.collect();

		let authorities: Vec<_> = authority_keys.iter().map(|k| ValidatorId::from(k.public())).collect();
		let babe_authorities: Vec<_> = authority_keys.iter()
			.map(|k| BabeAuthorityId::from(k.public()))
			.map(|k| (k, 1))
			.collect();

		// controllers are the index + 1000
		let stakers: Vec<_> = (0..authority_keys.len()).map(|i| (
			i as u64,
			i as u64 + 1000,
			10_000,
			StakerStatus::<u64>::Validator,
		)).collect();

		let balances: Vec<_> = (0..authority_keys.len()).map(|i| (i as u64, 10_000_000)).collect();

		GenesisConfig {
			authorities: authorities.clone(),
		}.assimilate_storage::<Test>(&mut t).unwrap();

		registrar::GenesisConfig::<Test> {
			parachains,
			_phdata: Default::default(),
		}.assimilate_storage(&mut t).unwrap();

		session::GenesisConfig::<Test> {
			keys: session_keys,
		}.assimilate_storage(&mut t).unwrap();

		babe::GenesisConfig {
			authorities: babe_authorities,
		}.assimilate_storage::<Test>(&mut t).unwrap();

		balances::GenesisConfig::<Test> {
			balances,
			vesting: vec![],
		}.assimilate_storage(&mut t).unwrap();

		staking::GenesisConfig::<Test> {
			current_era: 0,
			stakers,
			validator_count: 10,
			minimum_validator_count: 8,
			invulnerables: vec![],
			.. Default::default()
		}.assimilate_storage(&mut t).unwrap();

		t.into()
	}

	fn set_heads(v: Vec<AttestedCandidate>) -> ParachainsCall<Test> {
		ParachainsCall::set_heads(v)
	}

	fn make_attestations(candidate: &mut AttestedCandidate) {
		let mut vote_implicit = false;
		let parent_hash = crate::System::parent_hash();

		let (duty_roster, _) = Parachains::calculate_duty_roster();
		let candidate_hash = candidate.candidate.hash();

		let authorities = Parachains::authorities();
		let extract_key = |public: ValidatorId| {
			let mut raw_public = [0; 32];
			raw_public.copy_from_slice(public.as_ref());
			Sr25519Keyring::from_raw_public(raw_public).unwrap()
		};

		let validation_entries = duty_roster.validator_duty.iter()
			.enumerate();

		let mut validator_indices = BitVec::new();
		for (idx, &duty) in validation_entries {
			if duty != Chain::Parachain(candidate.parachain_index()) { continue }
			vote_implicit = !vote_implicit;

			let key = extract_key(authorities[idx].clone());

			let statement = if vote_implicit {
				Statement::Candidate(candidate.candidate.clone())
			} else {
				Statement::Valid(candidate_hash.clone())
			};

			let payload = localized_payload(statement, parent_hash);
			let signature = key.sign(&payload[..]).into();

			candidate.validity_votes.push(if vote_implicit {
				ValidityAttestation::Implicit(signature)
			} else {
				ValidityAttestation::Explicit(signature)
			});

			if validator_indices.len() <= idx {
				validator_indices.resize(idx + 1, false);
			}
			validator_indices.set(idx, true);
		}
		candidate.validator_indices = validator_indices;
	}

	fn new_candidate_with_egress_roots(egress_queue_roots: Vec<(ParaId, H256)>) -> AttestedCandidate {
		AttestedCandidate {
			validity_votes: vec![],
			validator_indices: BitVec::new(),
			candidate: CandidateReceipt {
				parachain_index: 0.into(),
				collator: Default::default(),
				signature: Default::default(),
				head_data: HeadData(vec![1, 2, 3]),
				egress_queue_roots,
				fees: 0,
				block_data_hash: Default::default(),
				upward_messages: vec![],
			}
		}
	}

	fn new_candidate_with_upward_messages(
		id: u32,
		upward_messages: Vec<(ParachainDispatchOrigin, Vec<u8>)>
	) -> AttestedCandidate {
		AttestedCandidate {
			validity_votes: vec![],
			validator_indices: BitVec::new(),
			candidate: CandidateReceipt {
				parachain_index: id.into(),
				collator: Default::default(),
				signature: Default::default(),
				head_data: HeadData(vec![1, 2, 3]),
				egress_queue_roots: vec![],
				fees: 0,
				block_data_hash: Default::default(),
				upward_messages: upward_messages.into_iter()
					.map(|x| UpwardMessage { origin: x.0, data: x.1 })
					.collect(),
			}
		}
	}

	fn init_block() {
		println!("Initializing {}", System::block_number());
		System::on_initialize(System::block_number());
		Registrar::on_initialize(System::block_number());
		Parachains::on_initialize(System::block_number());
	}
	fn run_to_block(n: u64) {
		println!("Running until block {}", n);
		while System::block_number() < n {
			if System::block_number() > 1 {
				println!("Finalizing {}", System::block_number());
				Parachains::on_finalize(System::block_number());
				Registrar::on_finalize(System::block_number());
				System::on_finalize(System::block_number());
			}
			System::set_block_number(System::block_number() + 1);
			init_block();
		}
	}

	fn queue_upward_messages(id: ParaId, upward_messages: &[UpwardMessage]) {
		NeedsDispatch::mutate(|nd|
			Parachains::queue_upward_messages(id, upward_messages, nd)
		);
	}

	#[test]
	fn check_dispatch_upward_works() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
			(2u32.into(), vec![], vec![]),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			init_block();
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![0; 4] }
			]);
			queue_upward_messages(1.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1; 4] }
			]);
			let mut dispatched: Vec<(ParaId, ParachainDispatchOrigin, Vec<u8>)> = vec![];
			let dummy = |id, origin, data: &[u8]| dispatched.push((id, origin, data.to_vec()));
			Parachains::dispatch_upward_messages(2, 3, dummy);
			assert_eq!(dispatched, vec![
				(0.into(), ParachainDispatchOrigin::Parachain, vec![0; 4])
			]);
			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(1)).len(), 1);
		});
		new_test_ext(parachains.clone()).execute_with(|| {
			init_block();
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![0; 2] }
			]);
			queue_upward_messages(1.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1; 2] }
			]);
			queue_upward_messages(2.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![2] }
			]);
			let mut dispatched: Vec<(ParaId, ParachainDispatchOrigin, Vec<u8>)> = vec![];
			let dummy = |id, origin, data: &[u8]| dispatched.push((id, origin, data.to_vec()));
			Parachains::dispatch_upward_messages(2, 3, dummy);
			assert_eq!(dispatched, vec![
				(0.into(), ParachainDispatchOrigin::Parachain, vec![0; 2]),
				(2.into(), ParachainDispatchOrigin::Parachain, vec![2])
			]);
			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(1)).len(), 1);
			assert!(<RelayDispatchQueue>::get(ParaId::from(2)).is_empty());
		});
		new_test_ext(parachains.clone()).execute_with(|| {
			init_block();
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![0; 2] }
			]);
			queue_upward_messages(1.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1; 2] }
			]);
			queue_upward_messages(2.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![2] }
			]);
			let mut dispatched: Vec<(ParaId, ParachainDispatchOrigin, Vec<u8>)> = vec![];
			let dummy = |id, origin, data: &[u8]| dispatched.push((id, origin, data.to_vec()));
			Parachains::dispatch_upward_messages(2, 3, dummy);
			assert_eq!(dispatched, vec![
				(0.into(), ParachainDispatchOrigin::Parachain, vec![0; 2]),
				(2.into(), ParachainDispatchOrigin::Parachain, vec![2])
			]);
			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(1)).len(), 1);
			assert!(<RelayDispatchQueue>::get(ParaId::from(2)).is_empty());
		});
		new_test_ext(parachains.clone()).execute_with(|| {
			init_block();
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![0; 2] }
			]);
			queue_upward_messages(1.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1; 2] }
			]);
			queue_upward_messages(2.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![2] }
			]);
			let mut dispatched: Vec<(ParaId, ParachainDispatchOrigin, Vec<u8>)> = vec![];
			let dummy = |id, origin, data: &[u8]| dispatched.push((id, origin, data.to_vec()));
			Parachains::dispatch_upward_messages(2, 3, dummy);
			assert_eq!(dispatched, vec![
				(0.into(), ParachainDispatchOrigin::Parachain, vec![0; 2]),
				(2.into(), ParachainDispatchOrigin::Parachain, vec![2]),
			]);
			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(1)).len(), 1);
			assert!(<RelayDispatchQueue>::get(ParaId::from(2)).is_empty());
		});
	}

	#[test]
	fn check_queue_upward_messages_works() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] }
			];
			assert_ok!(Parachains::check_upward_messages(0.into(), &messages, 2, 3));

			// all good.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1, 2] }
			];
			assert_ok!(Parachains::check_upward_messages(0.into(), &messages, 2, 3));
			queue_upward_messages(0.into(), &messages);
			assert_eq!(<RelayDispatchQueue>::get(ParaId::from(0)), vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
				UpwardMessage { origin: ParachainDispatchOrigin::Parachain, data: vec![1, 2] },
			]);
		});
	}

	#[test]
	fn check_queue_full_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// oversize, but ok since it's just one and the queue is empty.
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0; 4] },
			];
			assert_ok!(Parachains::check_upward_messages(0.into(), &messages, 2, 3));

			// oversize and bad since it's not just one.
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0; 4] },
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				"Messages added when queue full"
			);

			// too many messages.
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![1] },
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![2] },
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				"Messages added when queue full"
			);
		});
	}

	#[test]
	fn check_queued_too_many_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// too many messages.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![1] },
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![2] },
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				"Messages added when queue full"
			);
		});
	}

	#[test]
	fn check_queued_total_oversize_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// too much data.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0, 1] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![2, 3] },
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				"Messages added when queue full"
			);
		});
	}

	#[test]
	fn check_queued_pre_jumbo_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// bad - already an oversize messages queued.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0; 4] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] }
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				"Messages added when queue full"
			);
		});
	}

	#[test]
	fn check_queued_post_jumbo_upward_messages_fails() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
		];
		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// bad - oversized and already a message queued.
			queue_upward_messages(0.into(), &vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0] },
			]);
			let messages = vec![
				UpwardMessage { origin: ParachainDispatchOrigin::Signed, data: vec![0; 4] }
			];
			assert_err!(
				Parachains::check_upward_messages(0.into(), &messages, 2, 3),
				"Messages added when queue full"
			);
		});
	}

	#[test]
	fn upward_queuing_works() {
		// That the list of egress queue roots is in ascending order by `ParaId`.
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// parachain 0 is self
			let mut candidates = vec![
				new_candidate_with_upward_messages(0, vec![
					(ParachainDispatchOrigin::Signed, vec![1]),
				]),
				new_candidate_with_upward_messages(1, vec![
					(ParachainDispatchOrigin::Parachain, vec![2]),
				])
			];
			candidates.iter_mut().for_each(make_attestations);

			assert_ok!(Parachains::dispatch(
				set_heads(candidates),
				Origin::NONE,
			));

			assert!(<RelayDispatchQueue>::get(ParaId::from(0)).is_empty());
			assert!(<RelayDispatchQueue>::get(ParaId::from(1)).is_empty());
		});
	}

	#[test]
	fn active_parachains_should_work() {
		let parachains = vec![
			(5u32.into(), vec![1,2,3], vec![1]),
			(100u32.into(), vec![4,5,6], vec![2]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			assert_eq!(Parachains::active_parachains(), vec![(5u32.into(), None), (100u32.into(), None)]);
			assert_eq!(Parachains::parachain_code(ParaId::from(5u32)), Some(vec![1, 2, 3]));
			assert_eq!(Parachains::parachain_code(ParaId::from(100u32)), Some(vec![4, 5, 6]));
		});
	}

	#[test]
	fn register_deregister() {
		let parachains = vec![
			(5u32.into(), vec![1,2,3], vec![1]),
			(100u32.into(), vec![4,5,6], vec![2,]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			assert_eq!(Parachains::active_parachains(), vec![(5u32.into(), None), (100u32.into(), None)]);

			assert_eq!(Parachains::parachain_code(ParaId::from(5u32)), Some(vec![1,2,3]));
			assert_eq!(Parachains::parachain_code(ParaId::from(100u32)), Some(vec![4,5,6]));

			assert_ok!(Registrar::register_para(Origin::ROOT, 99u32.into(), ParaInfo{scheduling: Scheduling::Always}, vec![7,8,9], vec![1, 1, 1]));
			assert_ok!(Parachains::set_heads(Origin::NONE, vec![]));

			run_to_block(3);

			assert_eq!(Parachains::active_parachains(), vec![(5u32.into(), None), (99u32.into(), None), (100u32.into(), None)]);
			assert_eq!(Parachains::parachain_code(&ParaId::from(99u32)), Some(vec![7,8,9]));

			assert_ok!(Registrar::deregister_para(Origin::ROOT, 5u32.into()));
			assert_ok!(Parachains::set_heads(Origin::NONE, vec![]));

			// parachain still active this block. another block must pass before it's inactive.
			run_to_block(4);

			assert_eq!(Parachains::active_parachains(), vec![(99u32.into(), None), (100u32.into(), None)]);
			assert_eq!(Parachains::parachain_code(&ParaId::from(5u32)), None);
		});
	}

	#[test]
	fn duty_roster_works() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			let check_roster = |duty_roster: &DutyRoster| {
				assert_eq!(duty_roster.validator_duty.len(), 8);
				for i in (0..2).map(ParaId::from) {
					assert_eq!(duty_roster.validator_duty.iter().filter(|&&j| j == Chain::Parachain(i)).count(), 3);
				}
				assert_eq!(duty_roster.validator_duty.iter().filter(|&&j| j == Chain::Relay).count(), 2);
			};

			let duty_roster_0 = Parachains::calculate_duty_roster().0;
			check_roster(&duty_roster_0);

			System::initialize(&1, &H256::from([1; 32]), &Default::default(), &Default::default());
			RandomnessCollectiveFlip::on_initialize(1);
			let duty_roster_1 = Parachains::calculate_duty_roster().0;
			check_roster(&duty_roster_1);
			assert_ne!(duty_roster_0, duty_roster_1);


			System::initialize(&2, &H256::from([2; 32]), &Default::default(), &Default::default());
			RandomnessCollectiveFlip::on_initialize(2);
			let duty_roster_2 = Parachains::calculate_duty_roster().0;
			check_roster(&duty_roster_2);
			assert_ne!(duty_roster_0, duty_roster_2);
			assert_ne!(duty_roster_1, duty_roster_2);
		});
	}

	#[test]
	fn unattested_candidate_is_rejected() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			let candidate = AttestedCandidate {
				validity_votes: vec![],
				validator_indices: BitVec::new(),
				candidate: CandidateReceipt {
					parachain_index: 0.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
					upward_messages: vec![],
				},

			};

			assert!(Parachains::dispatch(set_heads(vec![candidate]), Origin::NONE).is_err());
		})
	}

	#[test]
	fn attested_candidates_accepted_in_order() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			assert_eq!(Parachains::active_parachains().len(), 2);

			let mut candidate_a = AttestedCandidate {
				validity_votes: vec![],
				validator_indices: BitVec::new(),
				candidate: CandidateReceipt {
					parachain_index: 0.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
					upward_messages: vec![],
				}
			};

			let mut candidate_b = AttestedCandidate {
				validity_votes: vec![],
				validator_indices: BitVec::new(),
				candidate: CandidateReceipt {
					parachain_index: 1.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![2, 3, 4]),
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
					upward_messages: vec![],
				}
			};

			make_attestations(&mut candidate_a);
			make_attestations(&mut candidate_b);

			assert!(Parachains::dispatch(
				set_heads(vec![candidate_b.clone(), candidate_a.clone()]),
				Origin::NONE,
			).is_err());

			assert_ok!(Parachains::dispatch(
				set_heads(vec![candidate_a.clone(), candidate_b.clone()]),
				Origin::NONE,
			));
		});
	}

	#[test]
	fn duplicate_vote_is_rejected() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			let mut candidate = AttestedCandidate {
				validity_votes: vec![],
				validator_indices: BitVec::new(),
				candidate: CandidateReceipt {
					parachain_index: 0.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
					upward_messages: vec![],
				}
			};

			make_attestations(&mut candidate);

			let mut double_validity = candidate.clone();
			double_validity.validity_votes.push(candidate.validity_votes[0].clone());
			double_validity.validator_indices.push(true);

			assert!(Parachains::dispatch(
				set_heads(vec![double_validity]),
				Origin::NONE,
			).is_err());
		});
	}

	#[test]
	fn validators_not_from_group_is_rejected() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			let mut candidate = AttestedCandidate {
				validity_votes: vec![],
				validator_indices: BitVec::new(),
				candidate: CandidateReceipt {
					parachain_index: 0.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					egress_queue_roots: vec![],
					fees: 0,
					block_data_hash: Default::default(),
					upward_messages: vec![],
				}
			};

			make_attestations(&mut candidate);

			// Change the last vote index to make it not corresponding to the assigned group.
			assert!(candidate.validator_indices.pop().is_some());
			candidate.validator_indices.append(&mut bitvec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

			assert!(Parachains::dispatch(
				set_heads(vec![candidate]),
				Origin::NONE,
			).is_err());
		});
	}

	#[test]
	fn ingress_works() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
			(99u32.into(), vec![1, 2, 3], vec![4, 5, 6]),
		];

		new_test_ext(parachains).execute_with(|| {
			assert_eq!(Parachains::ingress(ParaId::from(1), None), Some(Vec::new()));
			assert_eq!(Parachains::ingress(ParaId::from(99), None), Some(Vec::new()));

			init_block();
			for i in 1..10 {
				run_to_block(i);

				let from_a = vec![(1.into(), [i as u8; 32].into())];
				let mut candidate_a = AttestedCandidate {
					validity_votes: vec![],
					validator_indices: BitVec::new(),
					candidate: CandidateReceipt {
						parachain_index: 0.into(),
						collator: Default::default(),
						signature: Default::default(),
						head_data: HeadData(vec![1, 2, 3]),
						egress_queue_roots: from_a.clone(),
						fees: 0,
						block_data_hash: Default::default(),
						upward_messages: vec![],
					}
				};

				let from_b = vec![(99.into(), [i as u8; 32].into())];
				let mut candidate_b = AttestedCandidate {
					validity_votes: vec![],
					validator_indices: BitVec::new(),
					candidate: CandidateReceipt {
						parachain_index: 1.into(),
						collator: Default::default(),
						signature: Default::default(),
						head_data: HeadData(vec![1, 2, 3]),
						egress_queue_roots: from_b.clone(),
						fees: 0,
						block_data_hash: Default::default(),
						upward_messages: vec![],
					}
				};

				make_attestations(&mut candidate_a);
				make_attestations(&mut candidate_b);

				assert_ok!(Parachains::dispatch(
					set_heads(vec![candidate_a, candidate_b]),
					Origin::NONE,
				));
			}

			run_to_block(10);
			assert_ok!(Parachains::dispatch(
				set_heads(vec![]),
				Origin::NONE,
			));

			// parachain 1 has had a bunch of parachain candidates included,
			// which raises the watermark.
			assert_eq!(
				Parachains::ingress(ParaId::from(1), None),
				Some(vec![
					(9, BlockIngressRoots(vec![
						(0.into(), [9; 32].into())
					]))
				]),
			);

			// parachain 99 hasn't had any candidates included, so the
			// ingress is piling up.
			assert_eq!(
				Parachains::ingress(ParaId::from(99), None),
				Some((1..10).map(|i| (i, BlockIngressRoots(
					vec![(1.into(), [i as u8; 32].into())]
				))).collect::<Vec<_>>()),
			);

			assert_ok!(Registrar::deregister_para(Origin::ROOT, 1u32.into()));

			// after deregistering, there is no ingress to 1, but unrouted messages
			// from 1 stick around.
			assert_eq!(Parachains::ingress(ParaId::from(1), None), None);
			assert_eq!(Parachains::ingress(ParaId::from(99), None), Some((1..10).map(|i| (i, BlockIngressRoots(
				vec![(1.into(), [i as u8; 32].into())]
			))).collect::<Vec<_>>()));

			run_to_block(11);

			let mut candidate_c = AttestedCandidate {
				validity_votes: vec![],
				validator_indices: BitVec::new(),
				candidate: CandidateReceipt {
					parachain_index: 99.into(),
					collator: Default::default(),
					signature: Default::default(),
					head_data: HeadData(vec![1, 2, 3]),
					egress_queue_roots: Vec::new(),
					fees: 0,
					block_data_hash: Default::default(),
					upward_messages: vec![],
				}
			};
			make_attestations(&mut candidate_c);

			assert_ok!(Parachains::dispatch(
				set_heads(vec![candidate_c]),
				Origin::NONE,
			));

			run_to_block(12);

			// at the next block, ingress to 99 should be empty.
			assert_eq!(Parachains::ingress(ParaId::from(99), None), Some(Vec::new()));
		});
	}

	#[test]
	fn egress_routed_to_non_existent_parachain_is_rejected() {
		// That no parachain is routed to which doesn't exist
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// parachain 99 does not exist
			let non_existent = vec![(99.into(), [1; 32].into())];
			let mut candidate = new_candidate_with_egress_roots(non_existent);

			make_attestations(&mut candidate);

			let result = Parachains::dispatch(
				set_heads(vec![candidate.clone()]),
				Origin::NONE,
			);

			assert_eq!(Err("Routing to non-existent parachain"), result);
		});
	}

	#[test]
	fn egress_routed_to_self_is_rejected() {
		// That the parachain doesn't route to self
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// parachain 0 is self
			let to_self = vec![(0.into(), [1; 32].into())];
			let mut candidate = new_candidate_with_egress_roots(to_self);

			make_attestations(&mut candidate);

			let result = Parachains::dispatch(
				set_heads(vec![candidate.clone()]),
				Origin::NONE,
			);

			assert_eq!(Err("Parachain routing to self"), result);
		});
	}

	#[test]
	fn egress_queue_roots_out_of_order_rejected() {
		// That the list of egress queue roots is in ascending order by `ParaId`.
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// parachain 0 is self
			let out_of_order = vec![(1.into(), [1; 32].into()), ((0.into(), [1; 32].into()))];
			let mut candidate = new_candidate_with_egress_roots(out_of_order);

			make_attestations(&mut candidate);

			let result = Parachains::dispatch(
				set_heads(vec![candidate.clone()]),
				Origin::NONE,
			);

			assert_eq!(Err("Egress routes out of order by ID"), result);
		});
	}

	#[test]
	fn egress_queue_roots_empty_trie_roots_rejected() {
		let parachains = vec![
			(0u32.into(), vec![], vec![]),
			(1u32.into(), vec![], vec![]),
			(2u32.into(), vec![], vec![]),
		];

		new_test_ext(parachains.clone()).execute_with(|| {
			run_to_block(2);
			// parachain 0 is self
			let contains_empty_trie_root = vec![(1.into(), [1; 32].into()), ((2.into(), EMPTY_TRIE_ROOT.into()))];
			let mut candidate = new_candidate_with_egress_roots(contains_empty_trie_root);

			make_attestations(&mut candidate);

			let result = Parachains::dispatch(
				set_heads(vec![candidate.clone()]),
				Origin::NONE,
			);

			assert_eq!(Err("Empty trie root included"), result);
		});
	}

	#[test]
	fn empty_trie_root_const_is_blake2_hashed_null_node() {
		let hashed_null_node =  <NodeCodec<Blake2Hasher> as trie_db::NodeCodec<Blake2Hasher>>::hashed_null_node();
		assert_eq!(hashed_null_node, EMPTY_TRIE_ROOT.into())
	}
}
