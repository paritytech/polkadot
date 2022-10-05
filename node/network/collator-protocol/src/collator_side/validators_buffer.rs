// Copyright 2017-2022 Parity Technologies (UK) Ltd.
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

//! Validator groups buffer for connection managements.
//!
//! Solves 2 problems:
//! 	1. A collator may want to stay connected to multiple groups on rotation boundaries.
//! 	2. It's important to disconnect from validator when there're no collations to be fetched.
//!
//! We keep a simple FIFO buffer of N validator groups and a bitvec for each advertisement,
//! 1 indicating we want to be connected to i-th validator in a buffer, 0 otherwise.
//!
//! The bit is set to 1 for the whole **group** whenever it's inserted into the buffer. Given a relay
//! parent, one can reset a bit back to 0 for particular **validator**. For example, if a collation
//! was fetched or some timeout has been hit.
//!
//! The bitwise OR over known advertisements gives us validators indices for connection request.

use std::{
	collections::{HashMap, VecDeque},
	num::NonZeroUsize,
	ops::Range,
};

use bitvec::{bitvec, vec::BitVec};

use polkadot_primitives::v2::{AuthorityDiscoveryId, GroupIndex, Hash, SessionIndex};

/// The ring buffer stores at most this many unique validator groups.
///
/// This value should be chosen in way that all groups assigned to our para
/// in the view can fit into the buffer.
pub const VALIDATORS_BUFFER_CAPACITY: NonZeroUsize = match NonZeroUsize::new(3) {
	Some(cap) => cap,
	None => panic!("buffer capacity must be non-zero"),
};

/// Unique identifier of a validators group.
#[derive(Debug)]
struct ValidatorsGroupInfo {
	/// Number of validators in the group.
	len: usize,
	session_index: SessionIndex,
	group_index: GroupIndex,
}

/// Ring buffer of validator groups.
///
/// Tracks which peers we want to be connected to with respect to advertised collations.
#[derive(Debug)]
pub struct ValidatorGroupsBuffer {
	/// Validator groups identifiers we **had** advertisements for.
	group_infos: VecDeque<ValidatorsGroupInfo>,
	/// Continuous buffer of validators discovery keys.
	validators: VecDeque<AuthorityDiscoveryId>,
	/// Mapping from relay-parent to bit-vectors with bits for all `validators`.
	/// Invariants kept: All bit-vectors are guaranteed to have the same size.
	should_be_connected: HashMap<Hash, BitVec>,
	/// Buffer capacity, limits the number of **groups** tracked.
	cap: NonZeroUsize,
}

impl ValidatorGroupsBuffer {
	/// Creates a new buffer with a non-zero capacity.
	pub fn with_capacity(cap: NonZeroUsize) -> Self {
		Self {
			group_infos: VecDeque::new(),
			validators: VecDeque::new(),
			should_be_connected: HashMap::new(),
			cap,
		}
	}

	/// Returns discovery ids of validators we have at least one advertised-but-not-fetched
	/// collation for.
	pub fn validators_to_connect(&self) -> Vec<AuthorityDiscoveryId> {
		let validators_num = self.validators.len();
		let bits = self
			.should_be_connected
			.values()
			.fold(bitvec![0; validators_num], |acc, next| acc | next);

		self.validators
			.iter()
			.enumerate()
			.filter_map(|(idx, authority_id)| bits[idx].then_some(authority_id.clone()))
			.collect()
	}

	/// Note a new advertisement, marking that we want to be connected to validators
	/// from this group.
	///
	/// If max capacity is reached and the group is new, drops validators from the back
	/// of the buffer.
	pub fn note_collation_advertised(
		&mut self,
		relay_parent: Hash,
		session_index: SessionIndex,
		group_index: GroupIndex,
		validators: &[AuthorityDiscoveryId],
	) {
		if validators.is_empty() {
			return
		}

		match self.group_infos.iter().enumerate().find(|(_, group)| {
			group.session_index == session_index && group.group_index == group_index
		}) {
			Some((idx, group)) => {
				let group_start_idx = self.group_lengths_iter().take(idx).sum();
				self.set_bits(relay_parent, group_start_idx..(group_start_idx + group.len));
			},
			None => self.push(relay_parent, session_index, group_index, validators),
		}
	}

	/// Note that a validator is no longer interested in a given relay parent.
	pub fn reset_validator_interest(
		&mut self,
		relay_parent: Hash,
		authority_id: &AuthorityDiscoveryId,
	) {
		let bits = match self.should_be_connected.get_mut(&relay_parent) {
			Some(bits) => bits,
			None => return,
		};

		for (idx, auth_id) in self.validators.iter().enumerate() {
			if auth_id == authority_id {
				bits.set(idx, false);
			}
		}
	}

	/// Remove relay parent from the buffer.
	///
	/// The buffer will no longer track which validators are interested in a corresponding
	/// advertisement.
	pub fn remove_relay_parent(&mut self, relay_parent: &Hash) {
		self.should_be_connected.remove(relay_parent);
	}

	/// Removes all advertisements from the buffer.
	pub fn clear_advertisements(&mut self) {
		self.should_be_connected.clear();
	}

	/// Pushes a new group to the buffer along with advertisement, setting all validators
	/// bits to 1.
	///
	/// If the buffer is full, drops group from the tail.
	fn push(
		&mut self,
		relay_parent: Hash,
		session_index: SessionIndex,
		group_index: GroupIndex,
		validators: &[AuthorityDiscoveryId],
	) {
		let new_group_info =
			ValidatorsGroupInfo { len: validators.len(), session_index, group_index };

		let buf = &mut self.group_infos;
		let cap = self.cap.get();

		if buf.len() >= cap {
			let pruned_group = buf.pop_front().expect("buf is not empty; qed");
			self.validators.drain(..pruned_group.len);

			self.should_be_connected.values_mut().for_each(|bits| {
				bits.as_mut_bitslice().shift_left(pruned_group.len);
			});
		}

		self.validators.extend(validators.iter().cloned());
		buf.push_back(new_group_info);
		let buf_len = buf.len();
		let group_start_idx = self.group_lengths_iter().take(buf_len - 1).sum();

		let new_len = self.validators.len();
		self.should_be_connected
			.values_mut()
			.for_each(|bits| bits.resize(new_len, false));
		self.set_bits(relay_parent, group_start_idx..(group_start_idx + validators.len()));
	}

	/// Sets advertisement bits to 1 in a given range (usually corresponding to some group).
	/// If the relay parent is unknown, inserts 0-initialized bitvec first.
	///
	/// The range must be ensured to be within bounds.
	fn set_bits(&mut self, relay_parent: Hash, range: Range<usize>) {
		let bits = self
			.should_be_connected
			.entry(relay_parent)
			.or_insert_with(|| bitvec![0; self.validators.len()]);

		bits[range].fill(true);
	}

	/// Returns iterator over numbers of validators in groups.
	///
	/// Useful for getting an index of the first validator in i-th group.
	fn group_lengths_iter(&self) -> impl Iterator<Item = usize> + '_ {
		self.group_infos.iter().map(|group| group.len)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_keyring::Sr25519Keyring;

	#[test]
	fn one_capacity_buffer() {
		let cap = NonZeroUsize::new(1).unwrap();
		let mut buf = ValidatorGroupsBuffer::with_capacity(cap);

		let hash_a = Hash::repeat_byte(0x1);
		let hash_b = Hash::repeat_byte(0x2);

		let validators: Vec<_> = [
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		]
		.into_iter()
		.map(|key| AuthorityDiscoveryId::from(key.public()))
		.collect();

		assert!(buf.validators_to_connect().is_empty());

		buf.note_collation_advertised(hash_a, 0, GroupIndex(0), &validators[..2]);
		assert_eq!(buf.validators_to_connect(), validators[..2].to_vec());

		buf.reset_validator_interest(hash_a, &validators[1]);
		assert_eq!(buf.validators_to_connect(), vec![validators[0].clone()]);

		buf.note_collation_advertised(hash_b, 0, GroupIndex(1), &validators[2..]);
		assert_eq!(buf.validators_to_connect(), validators[2..].to_vec());

		for validator in &validators[2..] {
			buf.reset_validator_interest(hash_b, validator);
		}
		assert!(buf.validators_to_connect().is_empty());
	}

	#[test]
	fn buffer_works() {
		let cap = NonZeroUsize::new(3).unwrap();
		let mut buf = ValidatorGroupsBuffer::with_capacity(cap);

		let hashes: Vec<_> = (0..5).map(Hash::repeat_byte).collect();

		let validators: Vec<_> = [
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			Sr25519Keyring::Ferdie,
		]
		.into_iter()
		.map(|key| AuthorityDiscoveryId::from(key.public()))
		.collect();

		buf.note_collation_advertised(hashes[0], 0, GroupIndex(0), &validators[..2]);
		buf.note_collation_advertised(hashes[1], 0, GroupIndex(0), &validators[..2]);
		buf.note_collation_advertised(hashes[2], 0, GroupIndex(1), &validators[2..4]);
		buf.note_collation_advertised(hashes[2], 0, GroupIndex(1), &validators[2..4]);

		assert_eq!(buf.validators_to_connect(), validators[..4].to_vec());

		for validator in &validators[2..4] {
			buf.reset_validator_interest(hashes[2], validator);
		}

		buf.reset_validator_interest(hashes[1], &validators[0]);
		assert_eq!(buf.validators_to_connect(), validators[..2].to_vec());

		buf.reset_validator_interest(hashes[0], &validators[0]);
		assert_eq!(buf.validators_to_connect(), vec![validators[1].clone()]);

		buf.note_collation_advertised(hashes[3], 0, GroupIndex(1), &validators[2..4]);
		buf.note_collation_advertised(
			hashes[4],
			0,
			GroupIndex(2),
			std::slice::from_ref(&validators[4]),
		);

		buf.reset_validator_interest(hashes[3], &validators[2]);
		buf.note_collation_advertised(
			hashes[4],
			0,
			GroupIndex(3),
			std::slice::from_ref(&validators[0]),
		);

		assert_eq!(
			buf.validators_to_connect(),
			vec![validators[3].clone(), validators[4].clone(), validators[0].clone()]
		);
	}
}
