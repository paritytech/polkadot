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
use crate::finality::finalize_blocks;
use crate::validators::{Validators, ValidatorsConfiguration};
use crate::verification::{is_importable_header, verify_aura_header};
use crate::{AuraConfiguration, ChainTime, ChangeToEnact, PruningStrategy, Storage};
use bp_eth_poa::{AuraHeader, HeaderId, Receipt};
use sp_std::{collections::btree_map::BTreeMap, prelude::*};

/// Imports bunch of headers and updates blocks finality.
///
/// Transactions receipts must be provided if `header_import_requires_receipts()`
/// has returned true.
/// If successful, returns tuple where first element is the number of useful headers
/// we have imported and the second element is the number of useless headers (duplicate)
/// we have NOT imported.
/// Returns error if fatal error has occured during import. Some valid headers may be
/// imported in this case.
/// TODO: update me (https://github.com/paritytech/parity-bridges-common/issues/415)
#[allow(clippy::too_many_arguments)]
pub fn import_headers<S: Storage, PS: PruningStrategy, CT: ChainTime>(
	storage: &mut S,
	pruning_strategy: &mut PS,
	aura_config: &AuraConfiguration,
	validators_config: &ValidatorsConfiguration,
	submitter: Option<S::Submitter>,
	headers: Vec<(AuraHeader, Option<Vec<Receipt>>)>,
	chain_time: &CT,
	finalized_headers: &mut BTreeMap<S::Submitter, u64>,
) -> Result<(u64, u64), Error> {
	let mut useful = 0;
	let mut useless = 0;
	for (header, receipts) in headers {
		let import_result = import_header(
			storage,
			pruning_strategy,
			aura_config,
			validators_config,
			submitter.clone(),
			header,
			chain_time,
			receipts,
		);

		match import_result {
			Ok((_, finalized)) => {
				for (_, submitter) in finalized {
					if let Some(submitter) = submitter {
						*finalized_headers.entry(submitter).or_default() += 1;
					}
				}
				useful += 1;
			}
			Err(Error::AncientHeader) | Err(Error::KnownHeader) => useless += 1,
			Err(error) => return Err(error),
		}
	}

	Ok((useful, useless))
}

/// A vector of finalized headers and their submitters.
pub type FinalizedHeaders<S> = Vec<(HeaderId, Option<<S as Storage>::Submitter>)>;

/// Imports given header and updates blocks finality (if required).
///
/// Transactions receipts must be provided if `header_import_requires_receipts()`
/// has returned true.
///
/// Returns imported block id and list of all finalized headers.
/// TODO: update me (https://github.com/paritytech/parity-bridges-common/issues/415)
#[allow(clippy::too_many_arguments)]
pub fn import_header<S: Storage, PS: PruningStrategy, CT: ChainTime>(
	storage: &mut S,
	pruning_strategy: &mut PS,
	aura_config: &AuraConfiguration,
	validators_config: &ValidatorsConfiguration,
	submitter: Option<S::Submitter>,
	header: AuraHeader,
	chain_time: &CT,
	receipts: Option<Vec<Receipt>>,
) -> Result<(HeaderId, FinalizedHeaders<S>), Error> {
	// first check that we are able to import this header at all
	let (header_id, finalized_id) = is_importable_header(storage, &header)?;

	// verify header
	let import_context = verify_aura_header(storage, aura_config, submitter, &header, chain_time)?;

	// check if block schedules new validators
	let validators = Validators::new(validators_config);
	let (scheduled_change, enacted_change) = validators.extract_validators_change(&header, receipts)?;

	// check if block finalizes some other blocks and corresponding scheduled validators
	let validators_set = import_context.validators_set();
	let finalized_blocks = finalize_blocks(
		storage,
		finalized_id,
		(validators_set.enact_block, &validators_set.validators),
		header_id,
		import_context.submitter(),
		&header,
		aura_config.two_thirds_majority_transition,
	)?;
	let enacted_change = enacted_change
		.map(|validators| ChangeToEnact {
			signal_block: None,
			validators,
		})
		.or_else(|| validators.finalize_validators_change(storage, &finalized_blocks.finalized_headers));

	// NOTE: we can't return Err() from anywhere below this line
	// (because otherwise we'll have inconsistent storage if transaction will fail)

	// and finally insert the block
	let (best_id, best_total_difficulty) = storage.best_block();
	let total_difficulty = import_context.total_difficulty() + header.difficulty;
	let is_best = total_difficulty > best_total_difficulty;
	storage.insert_header(import_context.into_import_header(
		is_best,
		header_id,
		header,
		total_difficulty,
		enacted_change,
		scheduled_change,
		finalized_blocks.votes,
	));

	// compute upper border of updated pruning range
	let new_best_block_id = if is_best { header_id } else { best_id };
	let new_best_finalized_block_id = finalized_blocks.finalized_headers.last().map(|(id, _)| *id);
	let pruning_upper_bound = pruning_strategy.pruning_upper_bound(
		new_best_block_id.number,
		new_best_finalized_block_id
			.map(|id| id.number)
			.unwrap_or(finalized_id.number),
	);

	// now mark finalized headers && prune old headers
	storage.finalize_and_prune_headers(new_best_finalized_block_id, pruning_upper_bound);

	Ok((header_id, finalized_blocks.finalized_headers))
}

/// Returns true if transactions receipts are required to import given header.
pub fn header_import_requires_receipts<S: Storage>(
	storage: &S,
	validators_config: &ValidatorsConfiguration,
	header: &AuraHeader,
) -> bool {
	is_importable_header(storage, header)
		.map(|_| Validators::new(validators_config))
		.map(|validators| validators.maybe_signals_validators_change(header))
		.unwrap_or(false)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{
		run_test, secret_to_address, test_aura_config, test_validators_config, validator, validators_addresses,
		validators_change_receipt, HeaderBuilder, KeepSomeHeadersBehindBest, TestRuntime, GAS_LIMIT,
	};
	use crate::validators::ValidatorsSource;
	use crate::DefaultInstance;
	use crate::{BlocksToPrune, BridgeStorage, Headers, PruningRange};
	use frame_support::{StorageMap, StorageValue};
	use secp256k1::SecretKey;

	const TOTAL_VALIDATORS: usize = 3;

	#[test]
	fn rejects_finalized_block_competitors() {
		run_test(TOTAL_VALIDATORS, |_| {
			let mut storage = BridgeStorage::<TestRuntime>::new();
			storage.finalize_and_prune_headers(
				Some(HeaderId {
					number: 100,
					..Default::default()
				}),
				0,
			);
			assert_eq!(
				import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_aura_config(),
					&test_validators_config(),
					None,
					Default::default(),
					&(),
					None,
				),
				Err(Error::AncientHeader),
			);
		});
	}

	#[test]
	fn rejects_known_header() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();
			let header = HeaderBuilder::with_parent(&ctx.genesis).sign_by(&validator(1));
			assert_eq!(
				import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_aura_config(),
					&test_validators_config(),
					None,
					header.clone(),
					&(),
					None,
				)
				.map(|_| ()),
				Ok(()),
			);
			assert_eq!(
				import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_aura_config(),
					&test_validators_config(),
					None,
					header,
					&(),
					None,
				)
				.map(|_| ()),
				Err(Error::KnownHeader),
			);
		});
	}

	#[test]
	fn import_header_works() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let validators_config = ValidatorsConfiguration::Multi(vec![
				(0, ValidatorsSource::List(ctx.addresses.clone())),
				(1, ValidatorsSource::List(validators_addresses(2))),
			]);
			let mut storage = BridgeStorage::<TestRuntime>::new();
			let header = HeaderBuilder::with_parent(&ctx.genesis).sign_by(&validator(1));
			let hash = header.compute_hash();
			assert_eq!(
				import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_aura_config(),
					&validators_config,
					None,
					header,
					&(),
					None
				)
				.map(|_| ()),
				Ok(()),
			);

			// check that new validators will be used for next header
			let imported_header = Headers::<TestRuntime>::get(&hash).unwrap();
			assert_eq!(
				imported_header.next_validators_set_id,
				1, // new set is enacted from config
			);
		});
	}

	#[test]
	fn headers_are_pruned_during_import() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let validators_config =
				ValidatorsConfiguration::Single(ValidatorsSource::Contract([3; 20].into(), ctx.addresses.clone()));
			let validators = vec![validator(0), validator(1), validator(2)];
			let mut storage = BridgeStorage::<TestRuntime>::new();

			// header [0..11] are finalizing blocks [0; 9]
			// => since we want to keep 10 finalized blocks, we aren't pruning anything
			let mut latest_block_id = Default::default();
			for i in 1..11 {
				let header = HeaderBuilder::with_parent_number(i - 1).sign_by_set(&validators);
				let parent_id = header.parent_id().unwrap();

				let (rolling_last_block_id, finalized_blocks) = import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_aura_config(),
					&validators_config,
					Some(100),
					header,
					&(),
					None,
				)
				.unwrap();
				match i {
					2..=10 => assert_eq!(finalized_blocks, vec![(parent_id, Some(100))], "At {}", i,),
					_ => assert_eq!(finalized_blocks, vec![], "At {}", i),
				}
				latest_block_id = rolling_last_block_id;
			}
			assert!(storage.header(&ctx.genesis.compute_hash()).is_some());

			// header 11 finalizes headers [10] AND schedules change
			// => we prune header#0
			let header11 = HeaderBuilder::with_parent_number(10)
				.log_bloom((&[0xff; 256]).into())
				.receipts_root(
					"ead6c772ba0083bbff497ba0f4efe47c199a2655401096c21ab7450b6c466d97"
						.parse()
						.unwrap(),
				)
				.sign_by_set(&validators);
			let parent_id = header11.parent_id().unwrap();
			let (rolling_last_block_id, finalized_blocks) = import_header(
				&mut storage,
				&mut KeepSomeHeadersBehindBest::default(),
				&test_aura_config(),
				&validators_config,
				Some(101),
				header11.clone(),
				&(),
				Some(vec![validators_change_receipt(latest_block_id.hash)]),
			)
			.unwrap();
			assert_eq!(finalized_blocks, vec![(parent_id, Some(100))],);
			assert!(storage.header(&ctx.genesis.compute_hash()).is_none());
			latest_block_id = rolling_last_block_id;

			// and now let's say validators 1 && 2 went offline
			// => in the range 12-25 no blocks are finalized, but we still continue to prune old headers
			// until header#11 is met. we can't prune #11, because it schedules change
			let mut step = 56u64;
			let mut expected_blocks = vec![(header11.compute_id(), Some(101))];
			for i in 12..25 {
				let header = HeaderBuilder::with_parent_hash(latest_block_id.hash)
					.difficulty(i.into())
					.step(step)
					.sign_by_set(&validators);
				expected_blocks.push((header.compute_id(), Some(102)));
				let (rolling_last_block_id, finalized_blocks) = import_header(
					&mut storage,
					&mut KeepSomeHeadersBehindBest::default(),
					&test_aura_config(),
					&validators_config,
					Some(102),
					header,
					&(),
					None,
				)
				.unwrap();
				assert_eq!(finalized_blocks, vec![],);
				latest_block_id = rolling_last_block_id;
				step += 3;
			}
			assert_eq!(
				BlocksToPrune::<DefaultInstance>::get(),
				PruningRange {
					oldest_unpruned_block: 11,
					oldest_block_to_keep: 14,
				},
			);

			// now let's insert block signed by validator 1
			// => blocks 11..24 are finalized and blocks 11..14 are pruned
			step -= 2;
			let header = HeaderBuilder::with_parent_hash(latest_block_id.hash)
				.difficulty(25.into())
				.step(step)
				.sign_by_set(&validators);
			let (_, finalized_blocks) = import_header(
				&mut storage,
				&mut KeepSomeHeadersBehindBest::default(),
				&test_aura_config(),
				&validators_config,
				Some(103),
				header,
				&(),
				None,
			)
			.unwrap();
			assert_eq!(finalized_blocks, expected_blocks);
			assert_eq!(
				BlocksToPrune::<DefaultInstance>::get(),
				PruningRange {
					oldest_unpruned_block: 15,
					oldest_block_to_keep: 15,
				},
			);
		});
	}

	fn import_custom_block<S: Storage>(
		storage: &mut S,
		validators: &[SecretKey],
		header: AuraHeader,
	) -> Result<HeaderId, Error> {
		let id = header.compute_id();
		import_header(
			storage,
			&mut KeepSomeHeadersBehindBest::default(),
			&test_aura_config(),
			&ValidatorsConfiguration::Single(ValidatorsSource::Contract(
				[0; 20].into(),
				validators.iter().map(secret_to_address).collect(),
			)),
			None,
			header,
			&(),
			None,
		)
		.map(|_| id)
	}

	#[test]
	fn import_of_non_best_block_may_finalize_blocks() {
		run_test(TOTAL_VALIDATORS, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();

			// insert headers (H1, validator1), (H2, validator1), (H3, validator1)
			// making H3 the best header, without finalizing anything (we need 2 signatures)
			let mut expected_best_block = Default::default();
			for i in 1..4 {
				let step = 1 + i * TOTAL_VALIDATORS as u64;
				expected_best_block = import_custom_block(
					&mut storage,
					&ctx.validators,
					HeaderBuilder::with_parent_number(i - 1)
						.step(step)
						.sign_by_set(&ctx.validators),
				)
				.unwrap();
			}
			let (best_block, best_difficulty) = storage.best_block();
			assert_eq!(best_block, expected_best_block);
			assert_eq!(storage.finalized_block(), ctx.genesis.compute_id());

			// insert headers (H1', validator1), (H2', validator2), finalizing H2, even though H3
			// has better difficulty than H2' (because there are more steps involved)
			let mut expected_finalized_block = Default::default();
			let mut parent_hash = ctx.genesis.compute_hash();
			for i in 1..3 {
				let step = i;
				let id = import_custom_block(
					&mut storage,
					&ctx.validators,
					HeaderBuilder::with_parent_hash(parent_hash)
						.step(step)
						.gas_limit((GAS_LIMIT + 1).into())
						.sign_by_set(&ctx.validators),
				)
				.unwrap();
				parent_hash = id.hash;
				if i == 1 {
					expected_finalized_block = id;
				}
			}
			let (new_best_block, new_best_difficulty) = storage.best_block();
			assert_eq!(new_best_block, expected_best_block);
			assert_eq!(new_best_difficulty, best_difficulty);
			assert_eq!(storage.finalized_block(), expected_finalized_block);
		});
	}

	#[test]
	fn append_to_unfinalized_fork_fails() {
		const VALIDATORS: u64 = 5;
		run_test(VALIDATORS as usize, |ctx| {
			let mut storage = BridgeStorage::<TestRuntime>::new();

			// header1, authored by validator[2] is best common block between two competing forks
			let header1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(0)
					.step(2)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header1);
			assert_eq!(storage.finalized_block().number, 0);

			// validator[3] has authored header2 (nothing is finalized yet)
			let header2 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(1)
					.step(3)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header2);
			assert_eq!(storage.finalized_block().number, 0);

			// validator[4] has authored header3 (header1 is finalized)
			let header3 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(2)
					.step(4)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header3);
			assert_eq!(storage.finalized_block(), header1);

			// validator[4] has authored 4 blocks: header2'...header5' (header1 is still finalized)
			let header2_1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(1)
					.gas_limit((GAS_LIMIT + 1).into())
					.step(4)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			let header3_1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_hash(header2_1.hash)
					.step(4 + VALIDATORS)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			let header4_1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_hash(header3_1.hash)
					.step(4 + VALIDATORS * 2)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			let header5_1 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_hash(header4_1.hash)
					.step(4 + VALIDATORS * 3)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header5_1);
			assert_eq!(storage.finalized_block(), header1);

			// when we import header4 { parent = header3 }, authored by validator[0], header2 is finalized
			let header4 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_number(3)
					.step(5)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header5_1);
			assert_eq!(storage.finalized_block(), header2);

			// when we import header5 { parent = header4 }, authored by validator[1], header3 is finalized
			let header5 = import_custom_block(
				&mut storage,
				&ctx.validators,
				HeaderBuilder::with_parent_hash(header4.hash)
					.step(6)
					.sign_by_set(&ctx.validators),
			)
			.unwrap();
			assert_eq!(storage.best_block().0, header5);
			assert_eq!(storage.finalized_block(), header3);

			// import of header2'' { parent = header1 } fails, because it has number < best_finalized
			assert_eq!(
				import_custom_block(
					&mut storage,
					&ctx.validators,
					HeaderBuilder::with_parent_number(1)
						.gas_limit((GAS_LIMIT + 1).into())
						.step(3)
						.sign_by_set(&ctx.validators)
				),
				Err(Error::AncientHeader),
			);

			// import of header6' should also fail because we're trying to append to fork thas
			// has forked before finalized block
			assert_eq!(
				import_custom_block(
					&mut storage,
					&ctx.validators,
					HeaderBuilder::with_parent_number(5)
						.gas_limit((GAS_LIMIT + 1).into())
						.step(5 + VALIDATORS * 4)
						.sign_by_set(&ctx.validators),
				),
				Err(Error::TryingToFinalizeSibling),
			);
		});
	}
}
