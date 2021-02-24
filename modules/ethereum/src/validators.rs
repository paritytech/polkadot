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

use crate::error::Error;
use crate::{ChangeToEnact, Storage};
use bp_eth_poa::{Address, AuraHeader, HeaderId, LogEntry, Receipt, U256};
use sp_std::prelude::*;

/// The hash of InitiateChange event of the validators set contract.
pub(crate) const CHANGE_EVENT_HASH: &[u8; 32] = &[
	0x55, 0x25, 0x2f, 0xa6, 0xee, 0xe4, 0x74, 0x1b, 0x4e, 0x24, 0xa7, 0x4a, 0x70, 0xe9, 0xc1, 0x1f, 0xd2, 0xc2, 0x28,
	0x1d, 0xf8, 0xd6, 0xea, 0x13, 0x12, 0x6f, 0xf8, 0x45, 0xf7, 0x82, 0x5c, 0x89,
];

/// Where source of validators addresses come from. This covers the chain lifetime.
pub enum ValidatorsConfiguration {
	/// There's a single source for the whole chain lifetime.
	Single(ValidatorsSource),
	/// Validators source changes at given blocks. The blocks are ordered
	/// by the block number.
	Multi(Vec<(u64, ValidatorsSource)>),
}

/// Where validators addresses come from.
///
/// This source is valid within some blocks range. The blocks range could
/// cover multiple epochs - i.e. the validators that are authoring blocks
/// within this range could change, but the source itself can not.
#[cfg_attr(any(test, feature = "runtime-benchmarks"), derive(Debug, PartialEq))]
pub enum ValidatorsSource {
	/// The validators addresses are hardcoded and never change.
	List(Vec<Address>),
	/// The validators addresses are determined by the validators set contract
	/// deployed at given address. The contract must implement the `ValidatorSet`
	/// interface. Additionally, the initial validators set must be provided.
	Contract(Address, Vec<Address>),
}

/// A short hand for optional validators change.
pub type ValidatorsChange = Option<Vec<Address>>;

/// Validators manager.
pub struct Validators<'a> {
	config: &'a ValidatorsConfiguration,
}

impl<'a> Validators<'a> {
	/// Creates new validators manager using given configuration.
	pub fn new(config: &'a ValidatorsConfiguration) -> Self {
		Self { config }
	}

	/// Returns true if header (probabilistically) signals validators change and
	/// the caller needs to provide transactions receipts to import the header.
	pub fn maybe_signals_validators_change(&self, header: &AuraHeader) -> bool {
		let (_, _, source) = self.source_at(header.number);

		// if we are taking validators set from the fixed list, there's always
		// single epoch
		// => we never require transactions receipts
		let contract_address = match source {
			ValidatorsSource::List(_) => return false,
			ValidatorsSource::Contract(contract_address, _) => contract_address,
		};

		// else we need to check logs bloom and if it has required bits set, it means
		// that the contract has (probably) emitted epoch change event
		let expected_bloom = LogEntry {
			address: *contract_address,
			topics: vec![CHANGE_EVENT_HASH.into(), header.parent_hash],
			data: Vec::new(), // irrelevant for bloom.
		}
		.bloom();

		header.log_bloom.contains(&expected_bloom)
	}

	/// Extracts validators change signal from the header.
	///
	/// Returns tuple where first element is the change scheduled by this header
	/// (i.e. this change is only applied starting from the block that has finalized
	/// current block). The second element is the immediately applied change.
	pub fn extract_validators_change(
		&self,
		header: &AuraHeader,
		receipts: Option<Vec<Receipt>>,
	) -> Result<(ValidatorsChange, ValidatorsChange), Error> {
		// let's first check if new source is starting from this header
		let (source_index, _, source) = self.source_at(header.number);
		let (next_starts_at, next_source) = self.source_at_next_header(source_index, header.number);
		if next_starts_at == header.number {
			match *next_source {
				ValidatorsSource::List(ref new_list) => return Ok((None, Some(new_list.clone()))),
				ValidatorsSource::Contract(_, ref new_list) => return Ok((Some(new_list.clone()), None)),
			}
		}

		// else deal with previous source
		//
		// if we are taking validators set from the fixed list, there's always
		// single epoch
		// => we never require transactions receipts
		let contract_address = match source {
			ValidatorsSource::List(_) => return Ok((None, None)),
			ValidatorsSource::Contract(contract_address, _) => contract_address,
		};

		// else we need to check logs bloom and if it has required bits set, it means
		// that the contract has (probably) emitted epoch change event
		let expected_bloom = LogEntry {
			address: *contract_address,
			topics: vec![CHANGE_EVENT_HASH.into(), header.parent_hash],
			data: Vec::new(), // irrelevant for bloom.
		}
		.bloom();

		if !header.log_bloom.contains(&expected_bloom) {
			return Ok((None, None));
		}

		let receipts = receipts.ok_or(Error::MissingTransactionsReceipts)?;
		if header.check_receipts_root(&receipts).is_err() {
			return Err(Error::TransactionsReceiptsMismatch);
		}

		// iterate in reverse because only the _last_ change in a given
		// block actually has any effect
		Ok((
			receipts
				.iter()
				.rev()
				.filter(|r| r.log_bloom.contains(&expected_bloom))
				.flat_map(|r| r.logs.iter())
				.filter(|l| {
					l.address == *contract_address
						&& l.topics.len() == 2 && l.topics[0].as_fixed_bytes() == CHANGE_EVENT_HASH
						&& l.topics[1] == header.parent_hash
				})
				.filter_map(|l| {
					let data_len = l.data.len();
					if data_len < 64 {
						return None;
					}

					let new_validators_len_u256 = U256::from_big_endian(&l.data[32..64]);
					let new_validators_len = new_validators_len_u256.low_u64();
					if new_validators_len_u256 != new_validators_len.into() {
						return None;
					}

					if (data_len - 64) as u64 != new_validators_len.saturating_mul(32) {
						return None;
					}

					Some(
						l.data[64..]
							.chunks(32)
							.map(|chunk| {
								let mut new_validator = Address::default();
								new_validator.as_mut().copy_from_slice(&chunk[12..32]);
								new_validator
							})
							.collect(),
					)
				})
				.next(),
			None,
		))
	}

	/// Finalize changes when blocks are finalized.
	pub fn finalize_validators_change<S: Storage>(
		&self,
		storage: &S,
		finalized_blocks: &[(HeaderId, Option<S::Submitter>)],
	) -> Option<ChangeToEnact> {
		// if we haven't finalized any blocks, no changes may be finalized
		let newest_finalized_id = match finalized_blocks.last().map(|(id, _)| id) {
			Some(last_finalized_id) => last_finalized_id,
			None => return None,
		};
		let oldest_finalized_id = finalized_blocks
			.first()
			.map(|(id, _)| id)
			.expect("finalized_blocks is not empty; qed");

		// try to directly go to the header that has scheduled last change
		//
		// if we're unable to create import context for some block, it means
		// that the header has already been pruned => it and its ancestors had
		// no scheduled changes
		//
		// if we're unable to find scheduled changes for some block, it means
		// that these changes have been finalized already
		storage
			.import_context(None, &newest_finalized_id.hash)
			.and_then(|context| context.last_signal_block())
			.and_then(|signal_block| {
				if signal_block.number >= oldest_finalized_id.number {
					Some(signal_block)
				} else {
					None
				}
			})
			.and_then(|signal_block| {
				storage
					.scheduled_change(&signal_block.hash)
					.map(|change| ChangeToEnact {
						signal_block: Some(signal_block),
						validators: change.validators,
					})
			})
	}

	/// Returns source of validators that should author the header.
	fn source_at(&self, header_number: u64) -> (usize, u64, &ValidatorsSource) {
		match self.config {
			ValidatorsConfiguration::Single(ref source) => (0, 0, source),
			ValidatorsConfiguration::Multi(ref sources) => sources
				.iter()
				.rev()
				.enumerate()
				.find(|(_, &(begin, _))| begin < header_number)
				.map(|(i, (begin, source))| (sources.len() - 1 - i, *begin, source))
				.expect(
					"there's always entry for the initial block;\
					 we do not touch any headers with number < initial block number; qed",
				),
		}
	}

	/// Returns source of validators that should author the next header.
	fn source_at_next_header(&self, header_source_index: usize, header_number: u64) -> (u64, &ValidatorsSource) {
		match self.config {
			ValidatorsConfiguration::Single(ref source) => (0, source),
			ValidatorsConfiguration::Multi(ref sources) => {
				let next_source_index = header_source_index + 1;
				if next_source_index < sources.len() {
					let next_source = &sources[next_source_index];
					if next_source.0 < header_number + 1 {
						return (next_source.0, &next_source.1);
					}
				}

				let source = &sources[header_source_index];
				(source.0, &source.1)
			}
		}
	}
}

impl ValidatorsSource {
	/// Returns initial validators set.
	pub fn initial_epoch_validators(&self) -> Vec<Address> {
		match self {
			ValidatorsSource::List(ref list) => list.clone(),
			ValidatorsSource::Contract(_, ref list) => list.clone(),
		}
	}
}

#[cfg(test)]
pub(crate) mod tests {
	use super::*;
	use crate::mock::{run_test, validators_addresses, validators_change_receipt, TestRuntime};
	use crate::DefaultInstance;
	use crate::{AuraScheduledChange, BridgeStorage, Headers, ScheduledChanges, StoredHeader};
	use bp_eth_poa::compute_merkle_root;
	use frame_support::StorageMap;

	const TOTAL_VALIDATORS: usize = 3;

	#[test]
	fn source_at_works() {
		let config = ValidatorsConfiguration::Multi(vec![
			(0, ValidatorsSource::List(vec![[1; 20].into()])),
			(100, ValidatorsSource::List(vec![[2; 20].into()])),
			(200, ValidatorsSource::Contract([3; 20].into(), vec![[3; 20].into()])),
		]);
		let validators = Validators::new(&config);

		assert_eq!(
			validators.source_at(99),
			(0, 0, &ValidatorsSource::List(vec![[1; 20].into()])),
		);
		assert_eq!(
			validators.source_at_next_header(0, 99),
			(0, &ValidatorsSource::List(vec![[1; 20].into()])),
		);

		assert_eq!(
			validators.source_at(100),
			(0, 0, &ValidatorsSource::List(vec![[1; 20].into()])),
		);
		assert_eq!(
			validators.source_at_next_header(0, 100),
			(100, &ValidatorsSource::List(vec![[2; 20].into()])),
		);

		assert_eq!(
			validators.source_at(200),
			(1, 100, &ValidatorsSource::List(vec![[2; 20].into()])),
		);
		assert_eq!(
			validators.source_at_next_header(1, 200),
			(200, &ValidatorsSource::Contract([3; 20].into(), vec![[3; 20].into()])),
		);
	}

	#[test]
	fn maybe_signals_validators_change_works() {
		// when contract is active, but bloom has no required bits set
		let config = ValidatorsConfiguration::Single(ValidatorsSource::Contract(Default::default(), Vec::new()));
		let validators = Validators::new(&config);
		let mut header = AuraHeader {
			number: u64::max_value(),
			..Default::default()
		};
		assert!(!validators.maybe_signals_validators_change(&header));

		// when contract is active and bloom has required bits set
		header.log_bloom = (&[0xff; 256]).into();
		assert!(validators.maybe_signals_validators_change(&header));

		// when list is active and bloom has required bits set
		let config = ValidatorsConfiguration::Single(ValidatorsSource::List(vec![[42; 20].into()]));
		let validators = Validators::new(&config);
		assert!(!validators.maybe_signals_validators_change(&header));
	}

	#[test]
	fn extract_validators_change_works() {
		let config = ValidatorsConfiguration::Multi(vec![
			(0, ValidatorsSource::List(vec![[1; 20].into()])),
			(100, ValidatorsSource::List(vec![[2; 20].into()])),
			(200, ValidatorsSource::Contract([3; 20].into(), vec![[3; 20].into()])),
		]);
		let validators = Validators::new(&config);
		let mut header = AuraHeader {
			number: 100,
			..Default::default()
		};

		// when we're at the block that switches to list source
		assert_eq!(
			validators.extract_validators_change(&header, None),
			Ok((None, Some(vec![[2; 20].into()]))),
		);

		// when we're inside list range
		header.number = 150;
		assert_eq!(validators.extract_validators_change(&header, None), Ok((None, None)),);

		// when we're at the block that switches to contract source
		header.number = 200;
		assert_eq!(
			validators.extract_validators_change(&header, None),
			Ok((Some(vec![[3; 20].into()]), None)),
		);

		// when we're inside contract range and logs bloom signals change
		// but we have no receipts
		header.number = 250;
		header.log_bloom = (&[0xff; 256]).into();
		assert_eq!(
			validators.extract_validators_change(&header, None),
			Err(Error::MissingTransactionsReceipts),
		);

		// when we're inside contract range and logs bloom signals change
		// but there's no change in receipts
		header.receipts_root = "56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
			.parse()
			.unwrap();
		assert_eq!(
			validators.extract_validators_change(&header, Some(Vec::new())),
			Ok((None, None)),
		);

		// when we're inside contract range and logs bloom signals change
		// and there's change in receipts
		let receipts = vec![validators_change_receipt(Default::default())];
		header.receipts_root = compute_merkle_root(receipts.iter().map(|r| r.rlp()));
		assert_eq!(
			validators.extract_validators_change(&header, Some(receipts)),
			Ok((Some(vec![[7; 20].into()]), None)),
		);

		// when incorrect receipts root passed
		assert_eq!(
			validators.extract_validators_change(&header, Some(Vec::new())),
			Err(Error::TransactionsReceiptsMismatch),
		);
	}

	fn try_finalize_with_scheduled_change(scheduled_at: Option<HeaderId>) -> Option<ChangeToEnact> {
		run_test(TOTAL_VALIDATORS, |_| {
			let config = ValidatorsConfiguration::Single(ValidatorsSource::Contract(Default::default(), Vec::new()));
			let validators = Validators::new(&config);
			let storage = BridgeStorage::<TestRuntime>::new();

			// when we're finailizing blocks 10...100
			let id10 = HeaderId {
				number: 10,
				hash: [10; 32].into(),
			};
			let id100 = HeaderId {
				number: 100,
				hash: [100; 32].into(),
			};
			let finalized_blocks = vec![(id10, None), (id100, None)];
			let header100 = StoredHeader::<u64> {
				submitter: None,
				header: AuraHeader {
					number: 100,
					..Default::default()
				},
				total_difficulty: 0.into(),
				next_validators_set_id: 0,
				last_signal_block: scheduled_at,
			};
			let scheduled_change = AuraScheduledChange {
				validators: validators_addresses(1),
				prev_signal_block: None,
			};
			Headers::<TestRuntime>::insert(id100.hash, header100);
			if let Some(scheduled_at) = scheduled_at {
				ScheduledChanges::<DefaultInstance>::insert(scheduled_at.hash, scheduled_change);
			}

			validators.finalize_validators_change(&storage, &finalized_blocks)
		})
	}

	#[test]
	fn finalize_validators_change_finalizes_scheduled_change() {
		let id50 = HeaderId {
			number: 50,
			..Default::default()
		};
		assert_eq!(
			try_finalize_with_scheduled_change(Some(id50)),
			Some(ChangeToEnact {
				signal_block: Some(id50),
				validators: validators_addresses(1),
			}),
		);
	}

	#[test]
	fn finalize_validators_change_does_not_finalize_when_changes_are_not_scheduled() {
		assert_eq!(try_finalize_with_scheduled_change(None), None,);
	}

	#[test]
	fn finalize_validators_change_does_not_finalize_changes_when_they_are_outside_of_range() {
		let id5 = HeaderId {
			number: 5,
			..Default::default()
		};
		assert_eq!(try_finalize_with_scheduled_change(Some(id5)), None,);
	}
}
