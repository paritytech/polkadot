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
//! The bit is set to 1 on new advertisements, and back to 0 when a collation is fetched
//! by a validator or the timeout has been hit.
//!
//! The bitwise OR over known advertisements gives us validators indices for connection request.

use std::{
	collections::{HashMap, HashSet, VecDeque},
	future::Future,
	ops::Range,
	pin::Pin,
	task::{Context, Poll},
	time::Duration,
};

use bitvec::{bitvec, vec::BitVec};
use futures::FutureExt;

use polkadot_primitives::v2::{AuthorityDiscoveryId, GroupIndex, Hash, SessionIndex};

pub const VALIDATORS_BUFFER_CAPACITY: usize = 3;

/// Validators bits are only reset after a delay, to mitigate
/// the risk of disconnecting from the same group throughout rotation.
pub const RESET_BIT_DELAY: Duration = Duration::from_secs(12);

/// Unique identifier of a validators group.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ValidatorsGroupInfo {
	len: usize,
	session_index: SessionIndex,
	group_index: GroupIndex,
}

#[derive(Debug)]
pub struct ValidatorGroupsBuffer<const N: usize> {
	buf: VecDeque<ValidatorsGroupInfo>,
	validators: VecDeque<AuthorityDiscoveryId>,
	should_be_connected: HashMap<Hash, BitVec>,
}

impl<const N: usize> ValidatorGroupsBuffer<N> {
	pub fn new() -> Self {
		assert!(N > 0);

		Self {
			buf: VecDeque::with_capacity(N),
			validators: VecDeque::new(),
			should_be_connected: HashMap::new(),
		}
	}

	pub fn validators_to_connect(&self) -> Vec<AuthorityDiscoveryId> {
		let validators_num = self.validators.len();
		let bits = self
			.should_be_connected
			.values()
			.fold(bitvec![0; validators_num], |acc, next| acc | next);

		self.validators
			.iter()
			.enumerate()
			.filter_map(
				|(idx, authority_id)| if bits[idx] { Some(authority_id.clone()) } else { None },
			)
			.collect()
	}

	pub fn note_collation_distributed(
		&mut self,
		relay_parent: Hash,
		session_index: SessionIndex,
		group_index: GroupIndex,
		validators: &[AuthorityDiscoveryId],
	) {
		if validators.is_empty() {
			return
		}

		match self.buf.iter().enumerate().find(|(_, group)| {
			group.session_index == session_index && group.group_index == group_index
		}) {
			Some((idx, group)) => {
				let group_start_idx = self.validators_num_iter().take(idx).sum();
				self.set_bits(relay_parent, group_start_idx..(group_start_idx + group.len));
			},
			None => self.push(relay_parent, session_index, group_index, validators),
		}
	}

	pub fn reset_validator_bit(&mut self, relay_parent: Hash, authority_id: &AuthorityDiscoveryId) {
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

	pub fn remove_relay_parent(&mut self, relay_parent: &Hash) {
		self.should_be_connected.remove(relay_parent);
	}

	fn push(
		&mut self,
		relay_parent: Hash,
		session_index: SessionIndex,
		group_index: GroupIndex,
		validators: &[AuthorityDiscoveryId],
	) {
		let new_group_info =
			ValidatorsGroupInfo { len: validators.len(), session_index, group_index };

		let buf = &mut self.buf;

		if buf.len() >= N {
			let pruned_group = buf.pop_front().expect("buf is not empty; qed");
			self.validators.drain(..pruned_group.len);

			self.should_be_connected.values_mut().for_each(|bits| {
				bits.as_mut_bitslice().shift_left(pruned_group.len);
			});
		}

		self.validators.extend(validators.iter().cloned());
		buf.push_back(new_group_info);
		let buf_len = buf.len();
		let group_start_idx = self.validators_num_iter().take(buf_len - 1).sum();

		let new_len = self.validators.len();
		self.should_be_connected
			.values_mut()
			.for_each(|bits| bits.resize(new_len, false));
		self.set_bits(relay_parent, group_start_idx..(group_start_idx + validators.len()));
	}

	fn set_bits(&mut self, relay_parent: Hash, range: Range<usize>) {
		let bits = self
			.should_be_connected
			.entry(relay_parent)
			.or_insert_with(|| bitvec![0; self.validators.len()]);

		bits[range].fill(true);
	}

	fn validators_num_iter<'a>(&'a self) -> impl Iterator<Item = usize> + 'a {
		self.buf.iter().map(|group| group.len)
	}
}

pub struct ResetBitDelay {
	fut: futures_timer::Delay,
	relay_parent: Hash,
	authority_ids: HashSet<AuthorityDiscoveryId>,
}

impl ResetBitDelay {
	pub fn new(
		relay_parent: Hash,
		authority_ids: HashSet<AuthorityDiscoveryId>,
		delay: Duration,
	) -> Self {
		Self { fut: futures_timer::Delay::new(delay), relay_parent, authority_ids }
	}
}

impl Future for ResetBitDelay {
	type Output = (Hash, HashSet<AuthorityDiscoveryId>);

	fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		self.fut
			.poll_unpin(cx)
			.map(|_| (self.relay_parent, std::mem::take(&mut self.authority_ids)))
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

		buf.reset_validator_bit(hash_a, &validators[1]);
		assert_eq!(buf.validators_to_connect(), vec![validators[0].clone()]);

		buf.note_collation_distributed(hash_b, 0, GroupIndex(1), &validators[2..]);
		assert_eq!(buf.validators_to_connect(), validators[2..].to_vec());

		for validator in &validators[2..] {
			buf.reset_validator_bit(hash_b, validator);
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
			buf.reset_validator_bit(hashes[2], validator);
		}

		buf.reset_validator_bit(hashes[1], &validators[0]);
		assert_eq!(buf.validators_to_connect(), validators[..2].to_vec());

		buf.reset_validator_bit(hashes[0], &validators[0]);
		assert_eq!(buf.validators_to_connect(), vec![validators[1].clone()]);

		buf.note_collation_distributed(hashes[3], 0, GroupIndex(1), &validators[2..4]);
		buf.note_collation_distributed(
			hashes[4],
			0,
			GroupIndex(2),
			std::slice::from_ref(&validators[4]),
		);

		buf.reset_validator_bit(hashes[3], &validators[2]);
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
