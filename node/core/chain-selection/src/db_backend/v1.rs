// Copyright (C) Parity Technologies (UK) Ltd.
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

//! A database [`Backend`][crate::backend::Backend] for the chain selection subsystem.
//!
//! This stores the following schema:
//!
//! ```ignore
//! ("CS_block_entry", Hash) -> BlockEntry;
//! ("CS_block_height", BigEndianBlockNumber) -> Vec<Hash>;
//! ("CS_stagnant_at", BigEndianTimestamp) -> Vec<Hash>;
//! ("CS_leaves") -> LeafEntrySet;
//! ```
//!
//! The big-endian encoding is used for creating iterators over the key-value DB which are
//! accessible by prefix, to find the earliest block number stored as well as the all stagnant
//! blocks.
//!
//! The `Vec`s stored are always non-empty. Empty `Vec`s are not stored on disk so there is no
//! semantic difference between `None` and an empty `Vec`.

use crate::{
	backend::{Backend, BackendWriteOp},
	Error,
};

use polkadot_node_primitives::BlockWeight;
use polkadot_primitives::{BlockNumber, Hash};

use parity_scale_codec::{Decode, Encode};
use polkadot_node_subsystem_util::database::{DBTransaction, Database};

use std::sync::Arc;

const BLOCK_ENTRY_PREFIX: &[u8; 14] = b"CS_block_entry";
const BLOCK_HEIGHT_PREFIX: &[u8; 15] = b"CS_block_height";
const STAGNANT_AT_PREFIX: &[u8; 14] = b"CS_stagnant_at";
const LEAVES_KEY: &[u8; 9] = b"CS_leaves";

type Timestamp = u64;

#[derive(Debug, Encode, Decode, Clone, PartialEq)]
enum Approval {
	#[codec(index = 0)]
	Approved,
	#[codec(index = 1)]
	Unapproved,
	#[codec(index = 2)]
	Stagnant,
}

impl From<crate::Approval> for Approval {
	fn from(x: crate::Approval) -> Self {
		match x {
			crate::Approval::Approved => Approval::Approved,
			crate::Approval::Unapproved => Approval::Unapproved,
			crate::Approval::Stagnant => Approval::Stagnant,
		}
	}
}

impl From<Approval> for crate::Approval {
	fn from(x: Approval) -> crate::Approval {
		match x {
			Approval::Approved => crate::Approval::Approved,
			Approval::Unapproved => crate::Approval::Unapproved,
			Approval::Stagnant => crate::Approval::Stagnant,
		}
	}
}

#[derive(Debug, Encode, Decode, Clone, PartialEq)]
struct ViabilityCriteria {
	explicitly_reverted: bool,
	approval: Approval,
	earliest_unviable_ancestor: Option<Hash>,
}

impl From<crate::ViabilityCriteria> for ViabilityCriteria {
	fn from(x: crate::ViabilityCriteria) -> Self {
		ViabilityCriteria {
			explicitly_reverted: x.explicitly_reverted,
			approval: x.approval.into(),
			earliest_unviable_ancestor: x.earliest_unviable_ancestor,
		}
	}
}

impl From<ViabilityCriteria> for crate::ViabilityCriteria {
	fn from(x: ViabilityCriteria) -> crate::ViabilityCriteria {
		crate::ViabilityCriteria {
			explicitly_reverted: x.explicitly_reverted,
			approval: x.approval.into(),
			earliest_unviable_ancestor: x.earliest_unviable_ancestor,
		}
	}
}

#[derive(Encode, Decode)]
struct LeafEntry {
	weight: BlockWeight,
	block_number: BlockNumber,
	block_hash: Hash,
}

impl From<crate::LeafEntry> for LeafEntry {
	fn from(x: crate::LeafEntry) -> Self {
		LeafEntry { weight: x.weight, block_number: x.block_number, block_hash: x.block_hash }
	}
}

impl From<LeafEntry> for crate::LeafEntry {
	fn from(x: LeafEntry) -> crate::LeafEntry {
		crate::LeafEntry {
			weight: x.weight,
			block_number: x.block_number,
			block_hash: x.block_hash,
		}
	}
}

#[derive(Encode, Decode)]
struct LeafEntrySet {
	inner: Vec<LeafEntry>,
}

impl From<crate::LeafEntrySet> for LeafEntrySet {
	fn from(x: crate::LeafEntrySet) -> Self {
		LeafEntrySet { inner: x.inner.into_iter().map(Into::into).collect() }
	}
}

impl From<LeafEntrySet> for crate::LeafEntrySet {
	fn from(x: LeafEntrySet) -> crate::LeafEntrySet {
		crate::LeafEntrySet { inner: x.inner.into_iter().map(Into::into).collect() }
	}
}

#[derive(Debug, Encode, Decode, Clone, PartialEq)]
struct BlockEntry {
	block_hash: Hash,
	block_number: BlockNumber,
	parent_hash: Hash,
	children: Vec<Hash>,
	viability: ViabilityCriteria,
	weight: BlockWeight,
}

impl From<crate::BlockEntry> for BlockEntry {
	fn from(x: crate::BlockEntry) -> Self {
		BlockEntry {
			block_hash: x.block_hash,
			block_number: x.block_number,
			parent_hash: x.parent_hash,
			children: x.children,
			viability: x.viability.into(),
			weight: x.weight,
		}
	}
}

impl From<BlockEntry> for crate::BlockEntry {
	fn from(x: BlockEntry) -> crate::BlockEntry {
		crate::BlockEntry {
			block_hash: x.block_hash,
			block_number: x.block_number,
			parent_hash: x.parent_hash,
			children: x.children,
			viability: x.viability.into(),
			weight: x.weight,
		}
	}
}

/// Configuration for the database backend.
#[derive(Debug, Clone, Copy)]
pub struct Config {
	/// The column where block metadata is stored.
	pub col_data: u32,
}

/// The database backend.
pub struct DbBackend {
	inner: Arc<dyn Database>,
	config: Config,
}

impl DbBackend {
	/// Create a new [`DbBackend`] with the supplied key-value store and
	/// config.
	pub fn new(db: Arc<dyn Database>, config: Config) -> Self {
		DbBackend { inner: db, config }
	}
}

impl Backend for DbBackend {
	fn load_block_entry(&self, hash: &Hash) -> Result<Option<crate::BlockEntry>, Error> {
		load_decode::<BlockEntry>(&*self.inner, self.config.col_data, &block_entry_key(hash))
			.map(|o| o.map(Into::into))
	}

	fn load_leaves(&self) -> Result<crate::LeafEntrySet, Error> {
		load_decode::<LeafEntrySet>(&*self.inner, self.config.col_data, LEAVES_KEY)
			.map(|o| o.map(Into::into).unwrap_or_default())
	}

	fn load_stagnant_at(&self, timestamp: crate::Timestamp) -> Result<Vec<Hash>, Error> {
		load_decode::<Vec<Hash>>(
			&*self.inner,
			self.config.col_data,
			&stagnant_at_key(timestamp.into()),
		)
		.map(|o| o.unwrap_or_default())
	}

	fn load_stagnant_at_up_to(
		&self,
		up_to: crate::Timestamp,
		max_elements: usize,
	) -> Result<Vec<(crate::Timestamp, Vec<Hash>)>, Error> {
		let stagnant_at_iter =
			self.inner.iter_with_prefix(self.config.col_data, &STAGNANT_AT_PREFIX[..]);

		let val = stagnant_at_iter
			.filter_map(|r| match r {
				Ok((k, v)) =>
					match (decode_stagnant_at_key(&mut &k[..]), <Vec<_>>::decode(&mut &v[..]).ok())
					{
						(Some(at), Some(stagnant_at)) => Some(Ok((at, stagnant_at))),
						_ => None,
					},
				Err(e) => Some(Err(e)),
			})
			.enumerate()
			.take_while(|(idx, r)| {
				r.as_ref().map_or(true, |(at, _)| *at <= up_to.into() && *idx < max_elements)
			})
			.map(|(_, v)| v)
			.collect::<Result<Vec<_>, _>>()?;

		Ok(val)
	}

	fn load_first_block_number(&self) -> Result<Option<BlockNumber>, Error> {
		let blocks_at_height_iter =
			self.inner.iter_with_prefix(self.config.col_data, &BLOCK_HEIGHT_PREFIX[..]);

		let val = blocks_at_height_iter
			.filter_map(|r| match r {
				Ok((k, _)) => decode_block_height_key(&k[..]).map(Ok),
				Err(e) => Some(Err(e)),
			})
			.next();

		val.transpose().map_err(Error::from)
	}

	fn load_blocks_by_number(&self, number: BlockNumber) -> Result<Vec<Hash>, Error> {
		load_decode::<Vec<Hash>>(&*self.inner, self.config.col_data, &block_height_key(number))
			.map(|o| o.unwrap_or_default())
	}

	/// Atomically write the list of operations, with later operations taking precedence over prior.
	fn write<I>(&mut self, ops: I) -> Result<(), Error>
	where
		I: IntoIterator<Item = BackendWriteOp>,
	{
		let mut tx = DBTransaction::new();
		for op in ops {
			match op {
				BackendWriteOp::WriteBlockEntry(block_entry) => {
					let block_entry: BlockEntry = block_entry.into();
					tx.put_vec(
						self.config.col_data,
						&block_entry_key(&block_entry.block_hash),
						block_entry.encode(),
					);
				},
				BackendWriteOp::WriteBlocksByNumber(block_number, v) =>
					if v.is_empty() {
						tx.delete(self.config.col_data, &block_height_key(block_number));
					} else {
						tx.put_vec(
							self.config.col_data,
							&block_height_key(block_number),
							v.encode(),
						);
					},
				BackendWriteOp::WriteViableLeaves(leaves) => {
					let leaves: LeafEntrySet = leaves.into();
					if leaves.inner.is_empty() {
						tx.delete(self.config.col_data, &LEAVES_KEY[..]);
					} else {
						tx.put_vec(self.config.col_data, &LEAVES_KEY[..], leaves.encode());
					}
				},
				BackendWriteOp::WriteStagnantAt(timestamp, stagnant_at) => {
					let timestamp: Timestamp = timestamp.into();
					if stagnant_at.is_empty() {
						tx.delete(self.config.col_data, &stagnant_at_key(timestamp));
					} else {
						tx.put_vec(
							self.config.col_data,
							&stagnant_at_key(timestamp),
							stagnant_at.encode(),
						);
					}
				},
				BackendWriteOp::DeleteBlocksByNumber(block_number) => {
					tx.delete(self.config.col_data, &block_height_key(block_number));
				},
				BackendWriteOp::DeleteBlockEntry(hash) => {
					tx.delete(self.config.col_data, &block_entry_key(&hash));
				},
				BackendWriteOp::DeleteStagnantAt(timestamp) => {
					let timestamp: Timestamp = timestamp.into();
					tx.delete(self.config.col_data, &stagnant_at_key(timestamp));
				},
			}
		}

		self.inner.write(tx).map_err(Into::into)
	}
}

fn load_decode<D: Decode>(
	db: &dyn Database,
	col_data: u32,
	key: &[u8],
) -> Result<Option<D>, Error> {
	match db.get(col_data, key)? {
		None => Ok(None),
		Some(raw) => D::decode(&mut &raw[..]).map(Some).map_err(Into::into),
	}
}

fn block_entry_key(hash: &Hash) -> [u8; 14 + 32] {
	let mut key = [0; 14 + 32];
	key[..14].copy_from_slice(BLOCK_ENTRY_PREFIX);
	hash.using_encoded(|s| key[14..].copy_from_slice(s));
	key
}

fn block_height_key(number: BlockNumber) -> [u8; 15 + 4] {
	let mut key = [0; 15 + 4];
	key[..15].copy_from_slice(BLOCK_HEIGHT_PREFIX);
	key[15..].copy_from_slice(&number.to_be_bytes());
	key
}

fn stagnant_at_key(timestamp: Timestamp) -> [u8; 14 + 8] {
	let mut key = [0; 14 + 8];
	key[..14].copy_from_slice(STAGNANT_AT_PREFIX);
	key[14..].copy_from_slice(&timestamp.to_be_bytes());
	key
}

fn decode_block_height_key(key: &[u8]) -> Option<BlockNumber> {
	if key.len() != 15 + 4 {
		return None
	}
	if !key.starts_with(BLOCK_HEIGHT_PREFIX) {
		return None
	}

	let mut bytes = [0; 4];
	bytes.copy_from_slice(&key[15..]);
	Some(BlockNumber::from_be_bytes(bytes))
}

fn decode_stagnant_at_key(key: &[u8]) -> Option<Timestamp> {
	if key.len() != 14 + 8 {
		return None
	}
	if !key.starts_with(STAGNANT_AT_PREFIX) {
		return None
	}

	let mut bytes = [0; 8];
	bytes.copy_from_slice(&key[14..]);
	Some(Timestamp::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[cfg(test)]
	fn test_db() -> Arc<dyn Database> {
		let db = kvdb_memorydb::create(1);
		let db = polkadot_node_subsystem_util::database::kvdb_impl::DbAdapter::new(db, &[0]);
		Arc::new(db)
	}

	#[test]
	fn block_height_key_decodes() {
		let key = block_height_key(5);
		assert_eq!(decode_block_height_key(&key), Some(5));
	}

	#[test]
	fn stagnant_at_key_decodes() {
		let key = stagnant_at_key(5);
		assert_eq!(decode_stagnant_at_key(&key), Some(5));
	}

	#[test]
	fn lower_block_height_key_lesser() {
		for i in 0..256 {
			for j in 1..=256 {
				let key_a = block_height_key(i);
				let key_b = block_height_key(i + j);

				assert!(key_a < key_b);
			}
		}
	}

	#[test]
	fn lower_stagnant_at_key_lesser() {
		for i in 0..256 {
			for j in 1..=256 {
				let key_a = stagnant_at_key(i);
				let key_b = stagnant_at_key(i + j);

				assert!(key_a < key_b);
			}
		}
	}

	#[test]
	fn write_read_block_entry() {
		let db = test_db();
		let config = Config { col_data: 0 };

		let mut backend = DbBackend::new(db, config);

		let block_entry = BlockEntry {
			block_hash: Hash::repeat_byte(1),
			block_number: 1,
			parent_hash: Hash::repeat_byte(0),
			children: vec![],
			viability: ViabilityCriteria {
				earliest_unviable_ancestor: None,
				explicitly_reverted: false,
				approval: Approval::Unapproved,
			},
			weight: 100,
		};

		backend
			.write(vec![BackendWriteOp::WriteBlockEntry(block_entry.clone().into())])
			.unwrap();

		assert_eq!(
			backend.load_block_entry(&block_entry.block_hash).unwrap().map(BlockEntry::from),
			Some(block_entry),
		);
	}

	#[test]
	fn delete_block_entry() {
		let db = test_db();
		let config = Config { col_data: 0 };

		let mut backend = DbBackend::new(db, config);

		let block_entry = BlockEntry {
			block_hash: Hash::repeat_byte(1),
			block_number: 1,
			parent_hash: Hash::repeat_byte(0),
			children: vec![],
			viability: ViabilityCriteria {
				earliest_unviable_ancestor: None,
				explicitly_reverted: false,
				approval: Approval::Unapproved,
			},
			weight: 100,
		};

		backend
			.write(vec![BackendWriteOp::WriteBlockEntry(block_entry.clone().into())])
			.unwrap();

		backend
			.write(vec![BackendWriteOp::DeleteBlockEntry(block_entry.block_hash)])
			.unwrap();

		assert!(backend.load_block_entry(&block_entry.block_hash).unwrap().is_none());
	}

	#[test]
	fn earliest_block_number() {
		let db = test_db();
		let config = Config { col_data: 0 };

		let mut backend = DbBackend::new(db, config);

		assert!(backend.load_first_block_number().unwrap().is_none());

		backend
			.write(vec![
				BackendWriteOp::WriteBlocksByNumber(2, vec![Hash::repeat_byte(0)]),
				BackendWriteOp::WriteBlocksByNumber(5, vec![Hash::repeat_byte(0)]),
				BackendWriteOp::WriteBlocksByNumber(10, vec![Hash::repeat_byte(0)]),
			])
			.unwrap();

		assert_eq!(backend.load_first_block_number().unwrap(), Some(2));

		backend
			.write(vec![
				BackendWriteOp::WriteBlocksByNumber(2, vec![]),
				BackendWriteOp::DeleteBlocksByNumber(5),
			])
			.unwrap();

		assert_eq!(backend.load_first_block_number().unwrap(), Some(10));
	}

	#[test]
	fn stagnant_at_up_to() {
		let db = test_db();
		let config = Config { col_data: 0 };

		let mut backend = DbBackend::new(db, config);

		// Prove that it's cheap
		assert!(backend
			.load_stagnant_at_up_to(Timestamp::max_value(), usize::MAX)
			.unwrap()
			.is_empty());

		backend
			.write(vec![
				BackendWriteOp::WriteStagnantAt(2, vec![Hash::repeat_byte(1)]),
				BackendWriteOp::WriteStagnantAt(5, vec![Hash::repeat_byte(2)]),
				BackendWriteOp::WriteStagnantAt(10, vec![Hash::repeat_byte(3)]),
			])
			.unwrap();

		assert_eq!(
			backend.load_stagnant_at_up_to(Timestamp::max_value(), usize::MAX).unwrap(),
			vec![
				(2, vec![Hash::repeat_byte(1)]),
				(5, vec![Hash::repeat_byte(2)]),
				(10, vec![Hash::repeat_byte(3)]),
			]
		);

		assert_eq!(
			backend.load_stagnant_at_up_to(10, usize::MAX).unwrap(),
			vec![
				(2, vec![Hash::repeat_byte(1)]),
				(5, vec![Hash::repeat_byte(2)]),
				(10, vec![Hash::repeat_byte(3)]),
			]
		);

		assert_eq!(
			backend.load_stagnant_at_up_to(9, usize::MAX).unwrap(),
			vec![(2, vec![Hash::repeat_byte(1)]), (5, vec![Hash::repeat_byte(2)]),]
		);

		assert_eq!(
			backend.load_stagnant_at_up_to(9, 1).unwrap(),
			vec![(2, vec![Hash::repeat_byte(1)]),]
		);

		backend.write(vec![BackendWriteOp::DeleteStagnantAt(2)]).unwrap();

		assert_eq!(
			backend.load_stagnant_at_up_to(5, usize::MAX).unwrap(),
			vec![(5, vec![Hash::repeat_byte(2)]),]
		);

		backend.write(vec![BackendWriteOp::WriteStagnantAt(5, vec![])]).unwrap();

		assert_eq!(
			backend.load_stagnant_at_up_to(10, usize::MAX).unwrap(),
			vec![(10, vec![Hash::repeat_byte(3)]),]
		);
	}

	#[test]
	fn write_read_blocks_at_height() {
		let db = test_db();
		let config = Config { col_data: 0 };

		let mut backend = DbBackend::new(db, config);

		backend
			.write(vec![
				BackendWriteOp::WriteBlocksByNumber(2, vec![Hash::repeat_byte(1)]),
				BackendWriteOp::WriteBlocksByNumber(5, vec![Hash::repeat_byte(2)]),
				BackendWriteOp::WriteBlocksByNumber(10, vec![Hash::repeat_byte(3)]),
			])
			.unwrap();

		assert_eq!(backend.load_blocks_by_number(2).unwrap(), vec![Hash::repeat_byte(1)]);

		assert_eq!(backend.load_blocks_by_number(3).unwrap(), vec![]);

		backend
			.write(vec![
				BackendWriteOp::WriteBlocksByNumber(2, vec![]),
				BackendWriteOp::DeleteBlocksByNumber(5),
			])
			.unwrap();

		assert_eq!(backend.load_blocks_by_number(2).unwrap(), vec![]);

		assert_eq!(backend.load_blocks_by_number(5).unwrap(), vec![]);

		assert_eq!(backend.load_blocks_by_number(10).unwrap(), vec![Hash::repeat_byte(3)]);
	}
}
