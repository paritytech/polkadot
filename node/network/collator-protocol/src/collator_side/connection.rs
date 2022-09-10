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

//! Utilities for connections management.

use std::{
	collections::{HashMap, VecDeque},
	ops::Range,
};

use bitvec::{bitvec, vec::BitVec};

use polkadot_primitives::v2::{AuthorityDiscoveryId, BlockNumber, GroupIndex, Hash};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ValidatorsGroupInfo {
	validators: Vec<AuthorityDiscoveryId>,
	session_start_block: BlockNumber,
	group_index: GroupIndex,
}

#[derive(Debug)]
pub struct ValidatorGroupsBuffer<const N: usize> {
	buf: VecDeque<ValidatorsGroupInfo>,
	should_be_connected: HashMap<Hash, BitVec>,
}

impl<const N: usize> ValidatorGroupsBuffer<N> {
	pub fn new() -> Self {
		assert!(N > 0);

		Self { buf: VecDeque::with_capacity(N), should_be_connected: HashMap::new() }
	}

	pub fn validators_to_connect(&self) -> Vec<AuthorityDiscoveryId> {
		let validators_num = self.validators_num();
		let bits = self
			.should_be_connected
			.values()
			.fold(bitvec![0; validators_num], |acc, next| acc | next);

		let validators_iter = self.buf.iter().flat_map(|group| &group.validators);
		validators_iter
			.enumerate()
			.filter_map(
				|(idx, authority_id)| if bits[idx] { Some(authority_id.clone()) } else { None },
			)
			.collect()
	}

	pub fn note_collation_distributed(
		&mut self,
		relay_parent: Hash,
		session_start_block: BlockNumber,
		group_index: GroupIndex,
		validators: &[AuthorityDiscoveryId],
	) {
		if validators.is_empty() {
			return
		}

		match self.buf.iter().enumerate().find(|(_, group)| {
			group.session_start_block == session_start_block && group.group_index == group_index
		}) {
			Some((idx, group)) => {
				let group_start_idx = self.validators_num_iter().take(idx).sum();
				let validators_num = self.validators_num();
				self.set_bits(
					relay_parent,
					validators_num,
					group_start_idx..(group_start_idx + group.validators.len()),
				);
			},
			None => self.push(relay_parent, session_start_block, group_index, validators),
		}
	}

	pub fn note_collation_sent(&mut self, relay_parent: Hash, authority_id: &AuthorityDiscoveryId) {
		let bits = match self.should_be_connected.get_mut(&relay_parent) {
			Some(bits) => bits,
			None => return,
		};
		let validators_iter = self.buf.iter().flat_map(|group| &group.validators);

		for (idx, auth_id) in validators_iter.enumerate() {
			if auth_id == authority_id {
				bits.set(idx, false);
			}
		}
	}

	pub fn remove_relay_parent(&mut self, relay_parent: Hash) {
		self.should_be_connected.remove(&relay_parent);
	}

	fn push(
		&mut self,
		relay_parent: Hash,
		session_start_block: BlockNumber,
		group_index: GroupIndex,
		validators: &[AuthorityDiscoveryId],
	) {
		let new_group_info = ValidatorsGroupInfo {
			validators: validators.to_owned(),
			session_start_block,
			group_index,
		};

		let buf = &mut self.buf;

		if buf.len() >= N {
			let pruned_group = buf.pop_front().expect("buf is not empty; qed");

			self.should_be_connected.values_mut().for_each(|bits| {
				bits.as_mut_bitslice().shift_left(pruned_group.validators.len());
			});
		}

		buf.push_back(new_group_info);
		let buf_len = buf.len();
		let last_group_idx = self.validators_num_iter().take(buf_len - 1).sum();

		let new_len = self.validators_num();
		self.should_be_connected
			.values_mut()
			.for_each(|bits| bits.resize(new_len, false));
		self.set_bits(relay_parent, new_len, last_group_idx..(last_group_idx + validators.len()));
	}

	fn set_bits(&mut self, relay_parent: Hash, bits_len: usize, range: Range<usize>) {
		let bits = self
			.should_be_connected
			.entry(relay_parent)
			.or_insert_with(|| bitvec![0; bits_len]);

		bits[range].fill(true);
	}

	fn validators_num_iter<'a>(&'a self) -> impl Iterator<Item = usize> + 'a {
		self.buf.iter().map(|group| group.validators.len())
	}

	fn validators_num(&self) -> usize {
		self.validators_num_iter().sum()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use sp_keyring::Sr25519Keyring;

	#[test]
	fn one_capacity_buffer() {
		let mut buf = ValidatorGroupsBuffer::<1>::new();

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

		buf.note_collation_distributed(hash_a, 0, GroupIndex(0), &validators[..2]);
		assert_eq!(buf.validators_to_connect(), validators[..2].to_vec());

		buf.note_collation_sent(hash_a, &validators[1]);
		assert_eq!(buf.validators_to_connect(), vec![validators[0].clone()]);

		buf.note_collation_distributed(hash_b, 0, GroupIndex(1), &validators[2..]);
		assert_eq!(buf.validators_to_connect(), validators[2..].to_vec());

		for validator in &validators[2..] {
			buf.note_collation_sent(hash_b, validator);
		}
		assert!(buf.validators_to_connect().is_empty());
	}

	#[test]
	fn buffer_works() {
		let mut buf = ValidatorGroupsBuffer::<3>::new();

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

		buf.note_collation_distributed(hashes[0], 0, GroupIndex(0), &validators[..2]);
		buf.note_collation_distributed(hashes[1], 0, GroupIndex(0), &validators[..2]);
		buf.note_collation_distributed(hashes[2], 0, GroupIndex(1), &validators[2..4]);
		buf.note_collation_distributed(hashes[2], 0, GroupIndex(1), &validators[2..4]);

		assert_eq!(buf.validators_to_connect(), validators[..4].to_vec());

		for validator in &validators[2..4] {
			buf.note_collation_sent(hashes[2], validator);
		}

		buf.note_collation_sent(hashes[1], &validators[0]);
		assert_eq!(buf.validators_to_connect(), validators[..2].to_vec());

		buf.note_collation_sent(hashes[0], &validators[0]);
		assert_eq!(buf.validators_to_connect(), vec![validators[1].clone()]);

		buf.note_collation_distributed(hashes[3], 0, GroupIndex(1), &validators[2..4]);
		buf.note_collation_distributed(
			hashes[4],
			0,
			GroupIndex(2),
			std::slice::from_ref(&validators[4]),
		);

		buf.note_collation_sent(hashes[3], &validators[2]);
		buf.note_collation_distributed(
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
