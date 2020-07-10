// Copyright 2018-2020 Parity Technologies (UK) Ltd.
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

//! Bridge between the network and consensus service for getting collations to it.

use codec::{Encode, Decode};
use polkadot_primitives::v0::{Hash, CollatorId, Id as ParaId, Collation};
use sc_network::PeerId;
use futures::channel::oneshot;

use std::collections::hash_map::{HashMap, Entry};
use std::time::Duration;
use wasm_timer::Instant;

const COLLATION_LIFETIME: Duration = Duration::from_secs(60 * 5);

/// The role of the collator. Whether they're the primary or backup for this parachain.
#[derive(PartialEq, Debug, Clone, Copy, Encode, Decode)]
pub enum Role {
	/// Primary collators should send collations whenever it's time.
	Primary = 0,
	/// Backup collators should not.
	Backup = 1,
}

/// A maintenance action for the collator set.
#[derive(PartialEq, Debug)]
#[allow(dead_code)]
pub enum Action {
	/// Disconnect the given collator.
	Disconnect(CollatorId),
	/// Give the collator a new role.
	NewRole(CollatorId, Role),
}

struct CollationSlot {
	live_at: Instant,
	entries: SlotEntries,
}

impl CollationSlot {
	fn blank_now() -> Self {
		CollationSlot {
			live_at: Instant::now(),
			entries: SlotEntries::Blank,
		}
	}

	fn stay_alive(&self, now: Instant) -> bool {
		self.live_at + COLLATION_LIFETIME > now
	}
}

#[derive(Debug)]
enum SlotEntries {
	Blank,
	// not queried yet
	Pending(Vec<Collation>),
	// waiting for next to arrive.
	Awaiting(Vec<oneshot::Sender<Collation>>),
}

impl SlotEntries {
	fn received_collation(&mut self, collation: Collation) {
		*self = match std::mem::replace(self, SlotEntries::Blank) {
			SlotEntries::Blank => SlotEntries::Pending(vec![collation]),
			SlotEntries::Pending(mut cs) => {
				cs.push(collation);
				SlotEntries::Pending(cs)
			}
			SlotEntries::Awaiting(senders) => {
				for sender in senders {
					let _ = sender.send(collation.clone());
				}

				SlotEntries::Blank
			}
		};
	}

	fn await_with(&mut self, sender: oneshot::Sender<Collation>) {
		*self = match ::std::mem::replace(self, SlotEntries::Blank) {
			SlotEntries::Blank => SlotEntries::Awaiting(vec![sender]),
			SlotEntries::Awaiting(mut senders) => {
				senders.push(sender);
				SlotEntries::Awaiting(senders)
			}
			SlotEntries::Pending(mut cs) => {
				let next_collation = cs.pop().expect("empty variant is always `Blank`; qed");
				let _ = sender.send(next_collation);

				if cs.is_empty() {
					SlotEntries::Blank
				} else {
					SlotEntries::Pending(cs)
				}
			}
		};
	}
}

struct ParachainCollators {
	primary: CollatorId,
	backup: Vec<CollatorId>,
}

/// Manages connected collators and role assignments from the perspective of a validator.
#[derive(Default)]
pub struct CollatorPool {
	collators: HashMap<CollatorId, (ParaId, PeerId)>,
	parachain_collators: HashMap<ParaId, ParachainCollators>,
	collations: HashMap<(Hash, ParaId), CollationSlot>,
}

impl CollatorPool {
	/// Create a new `CollatorPool` object.
	pub fn new() -> Self {
		CollatorPool {
			collators: HashMap::new(),
			parachain_collators: HashMap::new(),
			collations: HashMap::new(),
		}
	}

	/// Call when a new collator is authenticated. Returns the role.
	pub fn on_new_collator(&mut self, collator_id: CollatorId, para_id: ParaId, peer_id: PeerId) -> Role {
		self.collators.insert(collator_id.clone(), (para_id, peer_id));
		match self.parachain_collators.entry(para_id) {
			Entry::Vacant(vacant) => {
				vacant.insert(ParachainCollators {
					primary: collator_id,
					backup: Vec::new(),
				});

				Role::Primary
			},
			Entry::Occupied(mut occupied) => {
				occupied.get_mut().backup.push(collator_id);

				Role::Backup
			}
		}
	}

	/// Called when a collator disconnects. If it was the primary, returns a new primary for that
	/// parachain.
	pub fn on_disconnect(&mut self, collator_id: CollatorId) -> Option<CollatorId> {
		self.collators.remove(&collator_id).and_then(|(para_id, _)| match self.parachain_collators.entry(para_id) {
			Entry::Vacant(_) => None,
			Entry::Occupied(mut occ) => {
				if occ.get().primary == collator_id {
					if occ.get().backup.is_empty() {
						occ.remove();
						None
					} else {
						let mut collators = occ.get_mut();
						collators.primary = collators.backup.pop().expect("backup non-empty; qed");
						Some(collators.primary.clone())
					}
				} else {
					let pos = occ.get().backup.iter().position(|a| a == &collator_id)
						.expect("registered collator always present in backup if not primary; qed");

					occ.get_mut().backup.remove(pos);
					None
				}
			}
		})
	}

	/// Called when a collation is received.
	/// The collator should be registered for the parachain of the collation as a precondition of this function.
	/// The collation should have been checked for integrity of signature before passing to this function.
	pub fn on_collation(&mut self, collator_id: CollatorId, relay_parent: Hash, collation: Collation) {
		log::debug!(
			target: "collator-pool", "On collation from collator {} for relay parent {}",
			collator_id,
			relay_parent,
		);

		if let Some((para_id, _)) = self.collators.get(&collator_id) {
			debug_assert_eq!(para_id, &collation.info.parachain_index);

			// TODO: punish if not primary? (https://github.com/paritytech/polkadot/issues/213)

			self.collations.entry((relay_parent, para_id.clone()))
				.or_insert_with(CollationSlot::blank_now)
				.entries
				.received_collation(collation);
		}
	}

	/// Wait for a collation from a parachain.
	pub fn await_collation(&mut self, relay_parent: Hash, para_id: ParaId, sender: oneshot::Sender<Collation>) {
		self.collations.entry((relay_parent, para_id))
			.or_insert_with(CollationSlot::blank_now)
			.entries
			.await_with(sender);
	}

	/// Call periodically to perform collator set maintenance.
	/// Returns a set of actions to perform on the network level.
	pub fn maintain_peers(&mut self) -> Vec<Action> {
		// TODO: rearrange periodically to new primary, evaluate based on latency etc.
		// https://github.com/paritytech/polkadot/issues/214
		Vec::new()
	}

	/// called when a block with given hash has been imported.
	pub fn collect_garbage(&mut self, chain_head: Option<&Hash>) {
		let now = Instant::now();
		self.collations.retain(|&(ref h, _), slot| chain_head != Some(h) && slot.stay_alive(now));
	}

	/// Convert the given `CollatorId` to a `PeerId`.
	pub fn collator_id_to_peer_id(&self, collator_id: &CollatorId) -> Option<&PeerId> {
		self.collators.get(collator_id).map(|ids| &ids.1)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_core::crypto::UncheckedInto;
	use polkadot_primitives::v0::{CollationInfo, BlockData, PoVBlock};
	use futures::executor::block_on;

	fn make_pov(block_data: Vec<u8>) -> PoVBlock {
		PoVBlock {
			block_data: BlockData(block_data),
		}
	}

	#[test]
	fn disconnect_primary_gives_new_primary() {
		let mut pool = CollatorPool::new();
		let para_id: ParaId = 5.into();
		let bad_primary: CollatorId = [0; 32].unchecked_into();
		let good_backup: CollatorId = [1; 32].unchecked_into();

		assert_eq!(pool.on_new_collator(bad_primary.clone(), para_id.clone(), PeerId::random()), Role::Primary);
		assert_eq!(pool.on_new_collator(good_backup.clone(), para_id.clone(), PeerId::random()), Role::Backup);
		assert_eq!(pool.on_disconnect(bad_primary), Some(good_backup.clone()));
		assert_eq!(pool.on_disconnect(good_backup), None);
	}

	#[test]
	fn disconnect_backup_removes_from_pool() {
		let mut pool = CollatorPool::new();
		let para_id: ParaId = 5.into();
		let primary = [0; 32].unchecked_into();
		let backup: CollatorId = [1; 32].unchecked_into();

		assert_eq!(pool.on_new_collator(primary, para_id.clone(), PeerId::random()), Role::Primary);
		assert_eq!(pool.on_new_collator(backup.clone(), para_id.clone(), PeerId::random()), Role::Backup);
		assert_eq!(pool.on_disconnect(backup), None);
		assert!(pool.parachain_collators.get(&para_id).unwrap().backup.is_empty());
	}

	#[test]
	fn await_before_collation() {
		let mut pool = CollatorPool::new();
		let para_id: ParaId = 5.into();
		let peer_id = PeerId::random();
		let primary: CollatorId = [0; 32].unchecked_into();
		let relay_parent = [1; 32].into();

		assert_eq!(pool.on_new_collator(primary.clone(), para_id.clone(), peer_id.clone()), Role::Primary);
		let (tx1, rx1) = oneshot::channel();
		let (tx2, rx2) = oneshot::channel();
		pool.await_collation(relay_parent, para_id, tx1);
		pool.await_collation(relay_parent, para_id, tx2);
		let mut collation_info = CollationInfo::default();
		collation_info.parachain_index = para_id;
		collation_info.collator = primary.clone().into();
		pool.on_collation(primary.clone(), relay_parent, Collation {
			info: collation_info,
			pov: make_pov(vec![4, 5, 6]),
		});

		block_on(rx1).unwrap();
		block_on(rx2).unwrap();
		assert_eq!(pool.collators.get(&primary).map(|ids| &ids.1).unwrap(), &peer_id);
	}

	#[test]
	fn collate_before_await() {
		let mut pool = CollatorPool::new();
		let para_id: ParaId = 5.into();
		let primary: CollatorId = [0; 32].unchecked_into();
		let relay_parent = [1; 32].into();

		assert_eq!(pool.on_new_collator(primary.clone(), para_id.clone(), PeerId::random()), Role::Primary);

		let mut collation_info = CollationInfo::default();
		collation_info.parachain_index = para_id;
		collation_info.collator = primary.clone();
		pool.on_collation(primary.clone(), relay_parent, Collation {
			info: collation_info,
			pov: make_pov(vec![4, 5, 6]),
		});

		let (tx, rx) = oneshot::channel();
		pool.await_collation(relay_parent, para_id, tx);
		block_on(rx).unwrap();
	}

	#[test]
	fn slot_stay_alive() {
		let slot = CollationSlot::blank_now();
		let now = slot.live_at;

		assert!(slot.stay_alive(now));
		assert!(slot.stay_alive(now + Duration::from_secs(10)));
		assert!(!slot.stay_alive(now + COLLATION_LIFETIME));
		assert!(!slot.stay_alive(now + COLLATION_LIFETIME + Duration::from_secs(10)));
	}
}
