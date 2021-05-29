#![warn(dead_code)]
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

use super::{CollatorId, CollatorProtocolMessage, PeerId, SubsystemContext, LOG_TARGET};
use std::time::{Duration, Instant};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ops::{Index, IndexMut};

use lru::LruCache;
use rand::{thread_rng, Rng};

use sp_keystore::SyncCryptoStorePtr;

use polkadot_node_network_protocol::{UnifiedReputationChange as Rep, View};
use polkadot_primitives::v1::{Hash, Id as ParaId};
use polkadot_subsystem::{messages::{AllMessages, NetworkBridgeMessage}, SubsystemSender};

// CACHE_SIZE determines the size (in number of entries) of in-memory cache maintained for the
// purposes of determining a collator's fitness. Metrics for collators that do not fit in the
// Cache are stored in a database.
const CACHE_SIZE: usize = 1_000;
// RESERVOIR SIZE determines the upper bound on the number of collator connections
pub(super) const RESERVOIR_SIZE: usize = 128;

/// Message was went out-of-order
const COST_UNEXPECTED_MESSAGE: Rep = Rep::CostMinor("An unexpected message");
/// Message could not be decoded properly.
const COST_CORRUPTED_MESSAGE: Rep = Rep::CostMinor("Message was corrupt");
/// Network errors that originated at the remote host should have same cost as timeout.
const COST_NETWORK_ERROR: Rep = Rep::CostMinor("Some network error");
const COST_REQUEST_TIMED_OUT: Rep =
	Rep::CostMinor("A collation request has timed out");
pub(super) const COST_INVALID_SIGNATURE: Rep =
	Rep::Malicious("Invalid network message signature");
const COST_WRONG_PARA: Rep =
	Rep::Malicious("A collator provided a collation for the wrong para");
pub(super) const COST_UNNEEDED_COLLATOR: Rep = Rep::CostMinor("An unneeded collator connected");

pub(super) const COST_REPORT_BAD: Rep =
	Rep::Malicious("A collator was reported by another subsystem");
pub(super) const BENEFIT_NOTIFY_GOOD: Rep =
	Rep::BenefitMinor("A collator was noted good by another subsystem");

const COLLATOR_METRICS: [Rep; 9] = [
	COST_UNEXPECTED_MESSAGE,
	COST_CORRUPTED_MESSAGE,
	COST_NETWORK_ERROR,
	COST_REQUEST_TIMED_OUT,
	COST_INVALID_SIGNATURE,
	COST_WRONG_PARA,
	COST_UNNEEDED_COLLATOR,
	COST_REPORT_BAD,
	BENEFIT_NOTIFY_GOOD,
];

#[derive(Eq, PartialEq, Copy, Clone)]
#[repr(usize)]
pub enum FitnessEvent {
	Unexpected = 0,
	Corrupted = 1,
	NetworkError = 2,
	Timeout = 3,
	SigError = 4,
	ParaError = 5,
	Superfluous = 6,
	ReportBad = 7,
	NotifyGood = 8,
}

#[derive(Copy, Clone)]
pub struct FitnessMetric {
	last: std::time::Instant,
	cumulative: Duration,
	total: u64,
}

impl PartialEq for FitnessMetric {
	fn eq(&self, other: &Self) -> bool {
		(self.fitness() - other.fitness()).abs() < f64::EPSILON
	}
}

impl Eq for FitnessMetric {}

impl PartialOrd for FitnessMetric {
	fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
		Some(self.cmp(other))
	}
}

impl Ord for FitnessMetric {
	fn cmp(&self, other: &Self) -> std::cmp::Ordering {
		if (self.fitness() - other.fitness()).abs() < f64::EPSILON {
			std::cmp::Ordering::Less
		} else {
			std::cmp::Ordering::Greater
		}
	}
}

impl FitnessMetric {
	pub fn new() -> Self {
		Self {
			last: std::time::Instant::now(),
			cumulative: Duration::new(0, 0),
			total: 0u64,
		}
	}

	pub fn _insert_event(&mut self, elapsed: Duration) {
		self.total += 1;
		self.cumulative += elapsed;
		self.last = std::time::Instant::now();
	}

	pub fn insert_event(&mut self) {
		self._insert_event(self.last.elapsed());
	}

	pub fn fitness(&self) -> f64 {
		if self.total != 0 {
			self.total as f64 / self.cumulative.as_nanos() as f64
		} else {
			0f64
		}
	}
}

pub struct CollatorFitnessMetric([FitnessMetric; 8]);

impl Index<FitnessEvent> for CollatorFitnessMetric {
	type Output = FitnessMetric;
	fn index(&self, index: FitnessEvent) -> &FitnessMetric {
		&self.0[index as usize]
	}
}

impl IndexMut<FitnessEvent> for CollatorFitnessMetric {
	fn index_mut(&mut self, index: FitnessEvent) -> &mut FitnessMetric {
		&mut self.0[index as usize]
	}
}

impl CollatorFitnessMetric {
	pub fn _insert_event(&mut self, event: FitnessEvent, elapsed: Duration) {
		(&mut self[event])._insert_event(elapsed)
	}

	pub fn insert_event(&mut self, event: FitnessEvent) {
		(&mut self[event]).insert_event()
	}

	// Compute collator's fitness value.
	// Note that this is a naive sum of all metric averages.
	// In the future we will expand on this functionality.
	pub fn compute_fitness(&self) -> f64 {
		self.0.iter()
			.map(|a| a.fitness())
			.sum::<f64>()
	}
}

impl Default for CollatorFitnessMetric {
	fn default() -> Self {
		Self([FitnessMetric::new(); COLLATOR_METRICS.len() - 1])
	}
}

#[derive(Debug)]
pub enum AdvertisementError {
	Duplicate,
	OutOfOurView,
	UndeclaredCollator,
}

#[derive(Debug)]
struct CollatingPeerState {
	collator_id: CollatorId,
	para_id: ParaId,
	// Advertised relay parents.
	advertisements: HashSet<Hash>,
	last_active: Instant,
}

#[derive(Debug)]
enum PeerState {
	// The peer has connected at the given instant.
	Connected(Instant),
	// Thepe
	Collating(CollatingPeerState),
}

#[derive(Debug)]
pub(super) struct PeerData {
	view: View,
	state: PeerState,
	pub(super) slot_idx: usize,
}

impl PeerData {
	pub fn new(view: View, slot_idx: usize) -> Self {
		PeerData {
			view,
			state: PeerState::Connected(Instant::now()),
			slot_idx,
		}
	}

	/// Update the view, clearing all advertisements that are no longer in the
	/// current view.
	pub fn update_view(&mut self, new_view: View) {
		let old_view = std::mem::replace(&mut self.view, new_view);
		if let PeerState::Collating(ref mut peer_state) = self.state {
			for removed in old_view.difference(&self.view) {
				let _ = peer_state.advertisements.remove(&removed);
			}
		}
	}

	/// Prune old advertisements relative to our view.
	pub fn prune_old_advertisements(&mut self, our_view: &View) {
		if let PeerState::Collating(ref mut peer_state) = self.state {
			peer_state.advertisements.retain(|a| our_view.contains(a));
		}
	}

	/// Note an advertisement by the collator. Returns `true` if the advertisement was imported
	/// successfully. Fails if the advertisement is duplicate, out of view, or the peer has not
	/// declared itself a collator.
	pub fn insert_advertisement(
		&mut self,
		on_relay_parent: Hash,
		our_view: &View,
	)
		-> std::result::Result<(CollatorId, ParaId), AdvertisementError>
	{
		match self.state {
			PeerState::Connected(_) => Err(AdvertisementError::UndeclaredCollator),
			_ if !our_view.contains(&on_relay_parent) => Err(AdvertisementError::OutOfOurView),
			PeerState::Collating(ref mut state) => {
				if state.advertisements.insert(on_relay_parent) {
					state.last_active = Instant::now();
					Ok((state.collator_id.clone(), state.para_id.clone()))
				} else {
					Err(AdvertisementError::Duplicate)
				}
			}
		}
	}

	/// Whether a peer is collating.
	pub fn is_collating(&self) -> bool {
		match self.state {
			PeerState::Connected(_) => false,
			PeerState::Collating(_) => true,
		}
	}

	/// Note that a peer is now collating with the given collator and para ids.
	///
	/// This will overwrite any previous call to `set_collating` and should only be called
	/// if `is_collating` is false.
	pub fn set_collating(&mut self, collator_id: &CollatorId, para_id: &ParaId) {
		self.state = PeerState::Collating(CollatingPeerState {
			collator_id: collator_id.clone(),
			para_id: *para_id,
			advertisements: HashSet::new(),
			last_active: Instant::now(),
		});
	}

	pub fn collator_id(&self) -> Option<&CollatorId> {
		match self.state {
			PeerState::Connected(_) => None,
			PeerState::Collating(ref state) => Some(&state.collator_id),
		}
	}

	pub fn collating_para(&self) -> Option<ParaId> {
		match self.state {
			PeerState::Connected(_) => None,
			PeerState::Collating(ref state) => Some(state.para_id),
		}
	}

	/// Whether the peer has advertised the given collation.
	pub fn has_advertised(&self, relay_parent: &Hash) -> bool {
		match self.state {
			PeerState::Connected(_) => false,
			PeerState::Collating(ref state) => state.advertisements.contains(relay_parent),
		}
	}

	/// Whether the peer is now inactive according to the current instant and the eviction policy.
	pub fn is_inactive(&self, now: Instant, policy: &crate::CollatorEvictionPolicy) -> bool {
		match self.state {
			PeerState::Connected(connected_at) => connected_at + policy.undeclared < now,
			PeerState::Collating(ref state) => state.last_active + policy.inactive_collator < now,
		}
	}
}

struct GroupAssignments {
	current: Option<ParaId>,
	next: Option<ParaId>,
}

#[derive(Default)]
pub(super) struct ActiveParas {
	relay_parent_assignments: HashMap<Hash, GroupAssignments>,
	current_assignments: HashMap<ParaId, usize>,
	next_assignments: HashMap<ParaId, usize>
}

impl ActiveParas {
	pub(super) async fn assign_incoming(
		&mut self,
		sender: &mut impl SubsystemSender,
		keystore: &SyncCryptoStorePtr,
		new_relay_parents: impl IntoIterator<Item = Hash>,
	) {
		for relay_parent in new_relay_parents {
			let mv = polkadot_node_subsystem_util::request_validators(relay_parent, sender)
				.await
				.await
				.ok()
				.map(|x| x.ok())
				.flatten();

			let mg = polkadot_node_subsystem_util::request_validator_groups(relay_parent, sender)
				.await
				.await
				.ok()
				.map(|x| x.ok())
				.flatten();


			let mc = polkadot_node_subsystem_util::request_availability_cores(relay_parent, sender)
				.await
				.await
				.ok()
				.map(|x| x.ok())
				.flatten();

			let (validators, groups, rotation_info, cores) = match (mv, mg, mc) {
				(Some(v), Some((g, r)), Some(c)) => (v, g, r, c),
				_ => {
					tracing::debug!(
						target: LOG_TARGET,
						relay_parent = ?relay_parent,
						"Failed to query runtime API for relay-parent",
					);

					continue
				}
			};

			let (para_now, para_next) = match polkadot_node_subsystem_util
				::signing_key_and_index(&validators, keystore)
				.await
				.and_then(|(_, index)| polkadot_node_subsystem_util::find_validator_group(
					&groups,
					index,
				))
			{
				Some(group) => {
					let next_rotation_info = rotation_info.bump_rotation();

					let core_now = rotation_info.core_for_group(group, cores.len());
					let core_next = next_rotation_info.core_for_group(group, cores.len());

					(
						cores.get(core_now.0 as usize).and_then(|c| c.para_id()),
						cores.get(core_next.0 as usize).and_then(|c| c.para_id()),
					)
				}
				None => {
					tracing::trace!(
						target: LOG_TARGET,
						relay_parent = ?relay_parent,
						"Not a validator",
					);

					continue
				}
			};

			// This code won't work well, if at all for parathreads. For parathreads we'll
			// have to be aware of which core the parathread claim is going to be multiplexed
			// onto. The parathread claim will also have a known collator, and we should always
			// allow an incoming connection from that collator. If not even connecting to them
			// directly.
			//
			// However, this'll work fine for parachains, as each parachain gets a dedicated
			// core.
			if let Some(para_now) = para_now {
				*self.current_assignments.entry(para_now).or_default() += 1;
			}

			if let Some(para_next) = para_next {
				*self.next_assignments.entry(para_next).or_default() += 1;
			}

			self.relay_parent_assignments.insert(
				relay_parent,
				GroupAssignments { current: para_now, next: para_next },
			);
		}
	}

	pub(super) fn remove_outgoing(
		&mut self,
		old_relay_parents: impl IntoIterator<Item = Hash>,
	) {
		for old_relay_parent in old_relay_parents {
			if let Some(assignments) = self.relay_parent_assignments.remove(&old_relay_parent) {
				let GroupAssignments { current, next } = assignments;

				if let Some(cur) = current {
					if let Entry::Occupied(mut occupied) = self.current_assignments.entry(cur) {
						*occupied.get_mut() -= 1;
						if *occupied.get() == 0 {
							occupied.remove_entry();
						}
					}
				}

				if let Some(next) = next {
					if let Entry::Occupied(mut occupied) = self.next_assignments.entry(next) {
						*occupied.get_mut() -= 1;
						if *occupied.get() == 0 {
							occupied.remove_entry();
						}
					}
				}
			}
		}
	}

	pub(super) fn is_current_or_next(&self, id: ParaId) -> bool {
		self.current_assignments.contains_key(&id) || self.next_assignments.contains_key(&id)
	}
}

pub(super) type CollatorFitness = LruCache<CollatorId, CollatorFitnessMetric>;

pub(super) type Reservoir = HashMap<PeerId, PeerData>;

pub struct PeerSlots {
	seq_nr: u64,
	pub(super) fitness: CollatorFitness,
	pub(super) peer_data: Reservoir,
	pub(super) active_paras: ActiveParas,
	pub(super) peers: Vec<PeerId>,
}

impl Default for PeerSlots {
	fn default() -> Self {
		Self {
			seq_nr: 0u64,
			fitness: LruCache::new(CACHE_SIZE),
			peer_data: HashMap::new(),
			active_paras: ActiveParas::default(),
			peers: Vec::new(),
		}
	}
}

impl std::fmt::Debug for PeerSlots {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "{:?}", self.peer_data)
	}
}

impl PeerSlots {
	fn reserve_peer(&mut self, peer_id: &PeerId) {
		if let Some(peer_data) = self.peer_data.get_mut(peer_id) {
			peer_data.slot_idx = self.peers.len();
			self.peers.push(*peer_id);
		}
	}

	pub fn _insert_event(&mut self, collator_id: &CollatorId, event: FitnessEvent, elapsed: Duration) {
		if let Some(metric) = self.fitness.peek_mut(collator_id) {
			metric._insert_event(event, elapsed);
		} else {
			let mut metric = CollatorFitnessMetric::default();
			metric._insert_event(event, elapsed);
			self.fitness.put(collator_id.clone(), metric);
		}
	}

	fn insert_event(&mut self, collator_id: &CollatorId, event: FitnessEvent) {
		if let Some(metric) = self.fitness.peek_mut(collator_id) {
			metric.insert_event(event);
		} else {
			let mut metric = CollatorFitnessMetric::default();
			metric.insert_event(event);
			self.fitness.put(collator_id.clone(), metric);
		}
	}

	fn _update(&mut self, start: usize, end: usize, reverse: bool) {
		let (first, last) = if reverse {
			(end, start)
		} else {
			(start, end)
		};
		if let Some(peer_id) = self.peers.get_mut(first) {
			if let Some(peer_data) = self.peer_data.get_mut(peer_id) {
				peer_data.slot_idx = last;
			}
		}
		for idx in start..end {
			if let Some(peer_id) = self.peers.get_mut(idx) {
				if let Some(peer_data) = self.peer_data.get_mut(peer_id) {
					if reverse {
						peer_data.slot_idx = std::cmp::min(peer_data.slot_idx + 1, self.peers.len() - 1);
					} else {
						peer_data.slot_idx -= 1;
					}
				}
			}
		}
	}

	pub fn update(&mut self, start: usize, end: usize, reverse: bool) {
		self._update(start, end, reverse);

		for idx in start..end.saturating_sub(1) {
			self.peers.swap(idx, idx + 1);
		}
	}

	pub fn insert_collator(&mut self, collator_id: &CollatorId) {
		if self.fitness.peek(collator_id).is_none() {
			self.fitness.put(collator_id.clone(), CollatorFitnessMetric::default());
		} 
	}

	fn _compute_fitness(&self, peer_data: &PeerData) -> f64 {
		let collator_id = peer_data.collator_id();

		if collator_id.is_none() {
			return 0f64;
		}

		let peer_fitness_metric = self.fitness.peek(collator_id.expect("None case handled above qed"));
		
		if peer_fitness_metric.is_none() {
			return 0f64;
		}
		
		peer_fitness_metric.expect("None case handled above qed").compute_fitness()
	}

	pub fn compute_peer_fitness(&self, peer_id: &PeerId) -> f64 {
		let temp = self.peer_data.get(peer_id);
		if temp.is_none() {
			return 0f64;
		}

		self._compute_fitness(temp.expect("None case handled above"))
	}

	fn find_key(
		&self,
		peer_fitness: f64,
		it: &mut dyn std::iter::Iterator<Item = (usize, &PeerId)>,
		reverse: bool
	) -> Option<usize> {
		while let Some((idx, peer_id)) = it.next() {
			// Note that temp_slot_idx is guaratneed to be > 1
			if (peer_fitness + f64::EPSILON > self.compute_peer_fitness(peer_id)) == reverse {
				return Some(idx);
			}
		}
		None
	}

	fn peer_slot(&self, peer_id: &PeerId) -> Option<usize> {
		self.peer_data.get(peer_id).map(|v| v.slot_idx)
	}

	pub fn reprioritize(&mut self, peer_id: &PeerId, reverse: bool) -> (Option<usize>, Option<usize>) {
		println!("\n Total number of peers {}", self.peer_data.len());
		for peer_id in self.peers.iter() {
			println!("{:?} \t {}", peer_id, self.compute_peer_fitness(peer_id));
		}
		let temp = self.peer_slot(peer_id);

		if temp.is_none() {
			return (None, None);
		}

		let slot_idx = temp.expect("None cast handled above");

		let out = if reverse {
			(
				Some(slot_idx),
				self.find_key(
					self.compute_peer_fitness(peer_id),
					&mut self.peers[..slot_idx].iter().enumerate().rev(),
					reverse,
				)
			)
		} else {
			(
				Some(slot_idx),
				self.find_key(
					self.compute_peer_fitness(peer_id),
					&mut self.peers[slot_idx..].iter().enumerate(),
					reverse,
				)
			)
		};
		println!("Total number of peers after {}", self.peer_data.len());
		for peer_id in self.peers.iter() {
			println!("{:?} \t {}", peer_id, self.compute_peer_fitness(peer_id));
		}

		println!("DONE\n\n");
		out
	}

	pub fn reset_sample(&mut self) {
		self.seq_nr = 0;
	}

	fn evict_peers(&mut self) -> Vec<PeerId> {
		// Total number of peers
		let total_peers = self.peers.len();
		// Determine how many peer we need to evict
		let surplus_peers = total_peers.saturating_sub(RESERVOIR_SIZE);
		// Evict the worst peers
		let mut out = Vec::new();
		if total_peers > RESERVOIR_SIZE {
			out = self.peers.drain(total_peers - surplus_peers..).collect();
		}
		let n = out.len();
		if n > 1 {
			out.swap(0, n-1);
		}

		out
	}

	// Reservoir sampling is a randomized algorithm that allows to maintain a reservoir of size k
	// along with a sequence number in order to iteratively fill the reservoir with elements over an
	// incoming stream such that at any given point in time the elements in the reservoir were
	// k-choose-seq_nr-uniformly randomly selected among the events accepted off the stream.
	// By incrementing the sequence number after each sample, we ensure the the reservoir of size k
	// has been chosen k-choose-seq_nr-uniformly at random among all viable collators since the seq_nr
	// was reset in the previous parachain assignment.
	//
	// That is, for each collator we generate a random number between 0 and the sampler's seq_nr.
	//
	// If that random number is lower than the size of our reservoir we evict a random other peer.
	// If that random number is greater than the size of our reservoir, we evict the declaring
	// peer.
	//
	// We must reset this seq_nr after every view change in order to ensure that we enforce a
	// uniform random distribution over connecting and declaring collators for the parachains that
	// are relevant at the time of the random sample.
	pub fn sample_connection(&mut self, peer_id: &PeerId) -> (Vec<PeerId>, Option<PeerId>) {
		// Get the worst performing peers to kick out of the reservoir
		let mut out = self.evict_peers();
		// Increment seq_nr in order to ensure that the sample probability for peer_id to be
		// included in this reservoir is k/seq_nr.
		self.seq_nr += 1;

		let peer = if let Some(available_slot) = out.pop() {
			// TODO LADI - Make this generic
			let mut rng = thread_rng();

			// If there is at least one element in the output, i.e. we must evict at least one peer,
			// then we must sample the declaring peer into our reservoir with probability
			// RESERVOIR_SIZE/peer_slots.seq_nr.
			let sample_idx = rng.gen_range(0..self.seq_nr);
			// This ensures that after we reset the seq_nr, we will always collate at least
			// RESERVOIR_SIZE new collators for the current and next parachain
			if (sample_idx as usize) < RESERVOIR_SIZE {
				// We are sampling peer_id into our reservoir
				// Therefore, we must compute CollatorId to determine the peer's fitness
				out.push(available_slot);
				Some(*peer_id)
			} else {
				// Since we are evicting peer_id, we needn't determine their fitness
				// Therefore, we needn't compute the CollatorId
				out.push(*peer_id);
				Some(available_slot)
			}
		} else {
			Some(*peer_id)
		};
		(out, peer)
	}
}

pub(super) fn handle_view_change(peer_slots: &mut PeerSlots, view: &View) -> Vec<PeerId> {
	let mut out = Vec::new();
	for (peer_id, peer_data) in peer_slots.peer_data.iter_mut() {
		peer_data.prune_old_advertisements(view);

		if let Some(para_id) = peer_data.collating_para() {
			if !peer_slots.active_paras.is_current_or_next(para_id) {
				out.push(*peer_id);
			}
		}
	}
	out
}

pub(super) async fn cycle_para(
	sender: &mut impl SubsystemSender,
	active_paras: &mut ActiveParas,
	keystore: &SyncCryptoStorePtr,
	new_relay_parents: impl IntoIterator<Item = Hash>,
	old_relay_parents: impl IntoIterator<Item = Hash>,
) {
	active_paras.assign_incoming(sender, keystore, new_relay_parents).await;
	active_paras.remove_outgoing(old_relay_parents);
}

pub fn handle_connection(
	peer_slots: &mut PeerSlots,
	peer_id: PeerId,
) {
	peer_slots
		.peer_data
		.insert(
			peer_id.clone(),
			PeerData::new(Default::default(), 0),
		);
}

pub fn reprioritize(
	peer_slots: &mut PeerSlots,
	peer_id: &PeerId,
	reverse: bool
) {
	let n = peer_slots.peers.len();
	let (start, end) = match peer_slots.reprioritize(peer_id, reverse) {
		(Some(slot_idx), Some(key)) => (slot_idx, std::cmp::min(slot_idx+key, n)),
		(Some(slot_idx), None) => {
			if reverse {
				(slot_idx, 0)
			} else {
				(slot_idx, n)
			}
		}
		_ => (0, peer_slots.peers.len()),
	};
	peer_slots.update(start, end, reverse);
}

pub fn sample_connection(
	peer_slots: &mut PeerSlots,
	peer_id: &PeerId,
) -> Vec<PeerId> {
	let (peers_to_evict, peer) = peer_slots.sample_connection(peer_id);

	if let Some(peer_to_reserve) = peer {
		peer_slots.reserve_peer(&peer_to_reserve);
		reprioritize(peer_slots, &peer_to_reserve, true);
	}

	peers_to_evict
}

pub fn _insert_event(
	peer_slots: &mut PeerSlots,
	peer_id: &PeerId,
	collator_id: &CollatorId,
	event: FitnessEvent,
	elapsed: Duration,
) {
	peer_slots._insert_event(collator_id, event, elapsed);

	reprioritize(peer_slots, peer_id, false);
}

pub async fn insert_event<Context>(
	ctx: &mut Context,
	peer_slots: &mut PeerSlots,
	peer_id: &PeerId,
	collator_id: &CollatorId,
	event: FitnessEvent,
) where
	Context: SubsystemContext<Message = CollatorProtocolMessage>,
{
	modify_reputation(ctx, *peer_id, COLLATOR_METRICS[event as usize]).await;
	if event != FitnessEvent::NotifyGood {
		peer_slots.insert_event(collator_id, event);	

		reprioritize(peer_slots, peer_id, false);
	}
}

/// Modify the reputation of a peer based on its behavior.
#[tracing::instrument(level = "trace", skip(ctx), fields(subsystem = LOG_TARGET))]
async fn modify_reputation<Context>(ctx: &mut Context, peer: PeerId, rep: Rep)
where
	Context: SubsystemContext,
{
	tracing::trace!(
		target: LOG_TARGET,
		rep = ?rep,
		peer_id = %peer,
		"reputation change for peer",
	);

	ctx.send_message(AllMessages::NetworkBridge(
		NetworkBridgeMessage::ReportPeer(peer, rep),
	)).await;
}

#[cfg(test)]
mod tests {
	use super::*;
	use polkadot_primitives::v1::CollatorPair;
	use sp_core::Pair;

	fn full_reservoir_setup() -> (PeerSlots, Vec<(PeerId, CollatorId)>) {
		let mut peer_slots = PeerSlots::default();
		let collators: Vec<CollatorPair> = std::iter::repeat(())
				.map(|_| CollatorPair::generate().0)
				.take(RESERVOIR_SIZE)
				.collect();
		let mut ids = Vec::new();

		// Connect all collators into peer slots
		for collator in collators.iter() {
			let peer_id = PeerId::random();
			handle_connection(&mut peer_slots, peer_id.clone());
			let collator_id = collator.public();
			ids.push((peer_id, collator_id));
		}

		// Traverse list of ids and register Collator Id
		for id in ids.iter() {
			let peer_data = peer_slots.peer_data.get_mut(&id.0).unwrap();
			peer_data.set_collating(&id.1, &ParaId::from(1));
			peer_slots.insert_collator(&id.1);

			// Ensure that peers are added whenever the reservoir is deemed to be full
			let peers_to_evict = sample_connection(&mut peer_slots, &id.0);
			assert_eq!(peers_to_evict, vec![]);
		}

		(peer_slots, ids)
	}

	#[test]
	fn test_find_key() {
		let (mut peer_slots, ids) = full_reservoir_setup();
		for i in 0..RESERVOIR_SIZE {
			let (target_peer_id, target_collator_id) = &ids[i];
			_insert_event(&mut peer_slots, target_peer_id, target_collator_id, FitnessEvent::Unexpected, Duration::new(i as u64, RESERVOIR_SIZE as u32));
		}

		for i in 0..RESERVOIR_SIZE {
			let (target_peer_id, _) = &ids[i];
			let peer_fitness = peer_slots.compute_peer_fitness(target_peer_id);
			assert_eq!(
				peer_slots.find_key(
					peer_fitness, &mut peer_slots.peers[..].iter().enumerate(),
					true
				),
				Some(0),
			);
		}

		for i in 0..RESERVOIR_SIZE {
			let (target_peer_id, _) = &ids[i];
			let peer_fitness = peer_slots.compute_peer_fitness(target_peer_id);
			let key = peer_slots.find_key(
				peer_fitness,
				&mut peer_slots.peers[..].iter().enumerate(),
				false
			);
			if i == 0 {
				assert_eq!(key, None);
			} else {
				assert_eq!(key, Some(RESERVOIR_SIZE - i));
			}
		}

		for i in 0..RESERVOIR_SIZE {
			let (target_peer_id, _) = &ids[i];
			let peer_fitness = peer_slots.compute_peer_fitness(target_peer_id);
			let key = peer_slots.find_key(
				peer_fitness,
				&mut peer_slots.peers[..].iter().enumerate().rev(),
				false
			);
			if i == 0 {
				assert_eq!(key, None);
			} else {
				assert_eq!(key, Some(RESERVOIR_SIZE-1));
			}
		}

		for i in 0..RESERVOIR_SIZE {
			let (target_peer_id, _) = &ids[i];
			let peer_fitness = peer_slots.compute_peer_fitness(target_peer_id);
			let key = peer_slots.find_key(
				peer_fitness,
				&mut peer_slots.peers[..].iter().enumerate().rev(),
				true
			);
			assert_eq!(key, Some(RESERVOIR_SIZE - i -  1));
		}
	}

	#[test]
	fn test_reprioritize_consistency_single() {
		let (mut peer_slots, ids) = full_reservoir_setup();

		for i in 1..RESERVOIR_SIZE {
			let (target_peer_id, target_collator_id) = &ids[i];
			_insert_event(
				&mut peer_slots,
				target_peer_id,
				target_collator_id,
				FitnessEvent::Unexpected,
				Duration::new(i as u64, 0),
			);
			assert_eq!(peer_slots.peer_slot(target_peer_id), Some(RESERVOIR_SIZE - i));
			for peer in peer_slots.peers.iter() {
				if let Some(slot_idx) = peer_slots.peer_slot(peer) {
					assert_eq!(&peer_slots.peers[slot_idx], peer);
				}
			}
		}
	}

	#[test]
	fn test_reprioritize_consistency_combined() {
		let (mut peer_slots, ids) = full_reservoir_setup();

		for i in 1..RESERVOIR_SIZE/4 {
			let (target_peer_id, target_collator_id) = &ids[i];
			_insert_event(
				&mut peer_slots,
				target_peer_id,
				target_collator_id,
				FitnessEvent::Unexpected,
				Duration::new(i as u64, 0),
			);
			let key = peer_slots.peer_slot(target_peer_id);
			assert_eq!(key, Some(RESERVOIR_SIZE - i));
			for peer in peer_slots.peers.iter() {
				if let Some(slot_idx) = peer_slots.peer_slot(peer) {
					assert_eq!(&peer_slots.peers[slot_idx], peer);
				}
			}
		}

		for i in RESERVOIR_SIZE - RESERVOIR_SIZE/4..RESERVOIR_SIZE - 1 {
			let (target_peer_id, target_collator_id) = &ids[i];
			assert!(
				peer_slots.peer_slot(target_peer_id) != Some(i)
			);
			_insert_event(
				&mut peer_slots,
				target_peer_id,
				target_collator_id,
				FitnessEvent::Unexpected,
				Duration::new(i as u64, 0),
			);
			let key = peer_slots.peer_slot(target_peer_id);
			assert_eq!(key, Some(RESERVOIR_SIZE + RESERVOIR_SIZE/2 + 1 - i - 1));
			for peer in peer_slots.peers.iter() {
				if let Some(slot_idx) = peer_slots.peer_slot(peer) {
					assert_eq!(&peer_slots.peers[slot_idx], peer);
				}
			}
		}

		for i in RESERVOIR_SIZE/4..RESERVOIR_SIZE/2 {
			let (target_peer_id, target_collator_id) = &ids[i];
			assert_eq!(peer_slots.peer_slot(target_peer_id), Some(1));
			_insert_event(
				&mut peer_slots,
				target_peer_id,
				target_collator_id,
				FitnessEvent::Unexpected,
				Duration::new(i as u64, 0),
			);
			assert_eq!(peer_slots.peer_slot(target_peer_id), Some(RESERVOIR_SIZE - i));
			for peer in peer_slots.peers.iter() {
				if let Some(slot_idx) = peer_slots.peer_slot(peer) {
					assert_eq!(&peer_slots.peers[slot_idx], peer);
				}
			}
		}

		for i in 1..RESERVOIR_SIZE/4 {
			let (target_peer_id, target_collator_id) = &ids[i];
			_insert_event(
				&mut peer_slots,
				target_peer_id,
				target_collator_id,
				FitnessEvent::Unexpected,
				Duration::new(i as u64, 0),

			);
			let key = peer_slots.peer_slot(target_peer_id);
			if i == 0 {
				assert_eq!(key, Some(0));
			} else {
				assert_eq!(key, Some(RESERVOIR_SIZE - i));
			}
			for peer in peer_slots.peers.iter() {
				if let Some(slot_idx) = peer_slots.peer_slot(peer) {
					assert_eq!(&peer_slots.peers[slot_idx], peer);
				}
			}
		}
	}
	
	#[test]
	fn test_broad_consistency() {
		let (mut peer_slots, ids) = full_reservoir_setup();
		for i in 0..10 {
			for (j, id) in ids.iter().enumerate() {
				let (target_peer_id, target_collator_id) = &id;
				_insert_event(
					&mut peer_slots,
					target_peer_id,
					target_collator_id,
					FitnessEvent::Unexpected,
					Duration::new(i+j as u64, 0),
				);
			}
		}
	}
}
