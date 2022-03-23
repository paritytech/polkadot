// Copyright 2019-2021 Parity Technologies (UK) Ltd.
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

//! Initialize Substrate -> Substrate headers bridge.
//!
//! Initialization is a transaction that calls `initialize()` function of the
//! `pallet-bridge-grandpa` pallet. This transaction brings initial header
//! and authorities set from source to target chain. The headers sync starts
//! with this header.

use crate::error::Error;

use bp_header_chain::{
	find_grandpa_authorities_scheduled_change,
	justification::{verify_justification, GrandpaJustification},
	InitializationData,
};
use codec::Decode;
use finality_grandpa::voter_set::VoterSet;
use num_traits::{One, Zero};
use relay_substrate_client::{
	BlockNumberOf, Chain, ChainWithGrandpa, Client, Error as SubstrateError, HashOf,
};
use sp_core::Bytes;
use sp_finality_grandpa::AuthorityList as GrandpaAuthoritiesSet;
use sp_runtime::traits::Header as HeaderT;

/// Submit headers-bridge initialization transaction.
pub async fn initialize<SourceChain: ChainWithGrandpa, TargetChain: Chain>(
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	target_transactions_signer: TargetChain::AccountId,
	prepare_initialize_transaction: impl FnOnce(
			TargetChain::Index,
			InitializationData<SourceChain::Header>,
		) -> Result<Bytes, SubstrateError>
		+ Send
		+ 'static,
) {
	let result = do_initialize(
		source_client,
		target_client,
		target_transactions_signer,
		prepare_initialize_transaction,
	)
	.await;

	match result {
		Ok(Some(tx_hash)) => log::info!(
			target: "bridge",
			"Successfully submitted {}-headers bridge initialization transaction to {}: {:?}",
			SourceChain::NAME,
			TargetChain::NAME,
			tx_hash,
		),
		Ok(None) => (),
		Err(err) => log::error!(
			target: "bridge",
			"Failed to submit {}-headers bridge initialization transaction to {}: {:?}",
			SourceChain::NAME,
			TargetChain::NAME,
			err,
		),
	}
}

/// Craft and submit initialization transaction, returning any error that may occur.
async fn do_initialize<SourceChain: ChainWithGrandpa, TargetChain: Chain>(
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	target_transactions_signer: TargetChain::AccountId,
	prepare_initialize_transaction: impl FnOnce(
			TargetChain::Index,
			InitializationData<SourceChain::Header>,
		) -> Result<Bytes, SubstrateError>
		+ Send
		+ 'static,
) -> Result<
	Option<TargetChain::Hash>,
	Error<SourceChain::Hash, <SourceChain::Header as HeaderT>::Number>,
> {
	let is_initialized = is_initialized::<SourceChain, TargetChain>(&target_client).await?;
	if is_initialized {
		log::info!(
			target: "bridge",
			"{}-headers bridge at {} is already initialized. Skipping",
			SourceChain::NAME,
			TargetChain::NAME,
		);
		return Ok(None)
	}

	let initialization_data = prepare_initialization_data(source_client).await?;
	log::info!(
		target: "bridge",
		"Prepared initialization data for {}-headers bridge at {}: {:?}",
		SourceChain::NAME,
		TargetChain::NAME,
		initialization_data,
	);

	let initialization_tx_hash = target_client
		.submit_signed_extrinsic(target_transactions_signer, move |_, transaction_nonce| {
			prepare_initialize_transaction(transaction_nonce, initialization_data)
		})
		.await
		.map_err(|err| Error::SubmitTransaction(TargetChain::NAME, err))?;
	Ok(Some(initialization_tx_hash))
}

/// Returns `Ok(true)` if bridge has already been initialized.
async fn is_initialized<SourceChain: ChainWithGrandpa, TargetChain: Chain>(
	target_client: &Client<TargetChain>,
) -> Result<bool, Error<HashOf<SourceChain>, BlockNumberOf<SourceChain>>> {
	Ok(target_client
		.raw_storage_value(
			bp_header_chain::storage_keys::best_finalized_hash_key(
				SourceChain::WITH_CHAIN_GRANDPA_PALLET_NAME,
			),
			None,
		)
		.await
		.map_err(|err| Error::RetrieveBestFinalizedHeaderHash(SourceChain::NAME, err))?
		.is_some())
}

/// Prepare initialization data for the GRANDPA verifier pallet.
async fn prepare_initialization_data<SourceChain: Chain>(
	source_client: Client<SourceChain>,
) -> Result<
	InitializationData<SourceChain::Header>,
	Error<SourceChain::Hash, <SourceChain::Header as HeaderT>::Number>,
> {
	// In ideal world we just need to get best finalized header and then to read GRANDPA authorities
	// set (`pallet_grandpa::CurrentSetId` + `GrandpaApi::grandpa_authorities()`) at this header.
	//
	// But now there are problems with this approach - `CurrentSetId` may return invalid value. So
	// here we're waiting for the next justification, read the authorities set and then try to
	// figure out the set id with bruteforce.
	let justifications = source_client
		.subscribe_justifications()
		.await
		.map_err(|err| Error::Subscribe(SourceChain::NAME, err))?;
	// Read next justification - the header that it finalizes will be used as initial header.
	let justification = justifications
		.next()
		.await
		.map_err(|e| Error::ReadJustification(SourceChain::NAME, e))
		.and_then(|justification| {
			justification.ok_or(Error::ReadJustificationStreamEnded(SourceChain::NAME))
		})?;

	// Read initial header.
	let justification: GrandpaJustification<SourceChain::Header> =
		Decode::decode(&mut &justification.0[..])
			.map_err(|err| Error::DecodeJustification(SourceChain::NAME, err))?;

	let (initial_header_hash, initial_header_number) =
		(justification.commit.target_hash, justification.commit.target_number);

	let initial_header = source_header(&source_client, initial_header_hash).await?;
	log::trace!(target: "bridge", "Selected {} initial header: {}/{}",
		SourceChain::NAME,
		initial_header_number,
		initial_header_hash,
	);

	// Read GRANDPA authorities set at initial header.
	let initial_authorities_set =
		source_authorities_set(&source_client, initial_header_hash).await?;
	log::trace!(target: "bridge", "Selected {} initial authorities set: {:?}",
		SourceChain::NAME,
		initial_authorities_set,
	);

	// If initial header changes the GRANDPA authorities set, then we need previous authorities
	// to verify justification.
	let mut authorities_for_verification = initial_authorities_set.clone();
	let scheduled_change = find_grandpa_authorities_scheduled_change(&initial_header);
	assert!(
		scheduled_change.as_ref().map(|c| c.delay.is_zero()).unwrap_or(true),
		"GRANDPA authorities change at {} scheduled to happen in {:?} blocks. We expect\
		regular hange to have zero delay",
		initial_header_hash,
		scheduled_change.as_ref().map(|c| c.delay),
	);
	let schedules_change = scheduled_change.is_some();
	if schedules_change {
		authorities_for_verification =
			source_authorities_set(&source_client, *initial_header.parent_hash()).await?;
		log::trace!(
			target: "bridge",
			"Selected {} header is scheduling GRANDPA authorities set changes. Using previous set: {:?}",
			SourceChain::NAME,
			authorities_for_verification,
		);
	}

	// Now let's try to guess authorities set id by verifying justification.
	let mut initial_authorities_set_id = 0;
	let mut min_possible_block_number = SourceChain::BlockNumber::zero();
	let authorities_for_verification = VoterSet::new(authorities_for_verification.clone())
		.ok_or(Error::ReadInvalidAuthorities(SourceChain::NAME, authorities_for_verification))?;
	loop {
		log::trace!(
			target: "bridge", "Trying {} GRANDPA authorities set id: {}",
			SourceChain::NAME,
			initial_authorities_set_id,
		);

		let is_valid_set_id = verify_justification::<SourceChain::Header>(
			(initial_header_hash, initial_header_number),
			initial_authorities_set_id,
			&authorities_for_verification,
			&justification,
		)
		.is_ok();

		if is_valid_set_id {
			break
		}

		initial_authorities_set_id += 1;
		min_possible_block_number += One::one();
		if min_possible_block_number > initial_header_number {
			// there can't be more authorities set changes than headers => if we have reached
			// `initial_block_number` and still have not found correct value of
			// `initial_authorities_set_id`, then something else is broken => fail
			return Err(Error::GuessInitialAuthorities(SourceChain::NAME, initial_header_number))
		}
	}

	Ok(InitializationData {
		header: Box::new(initial_header),
		authority_list: initial_authorities_set,
		set_id: if schedules_change {
			initial_authorities_set_id + 1
		} else {
			initial_authorities_set_id
		},
		is_halted: false,
	})
}

/// Read header by hash from the source client.
async fn source_header<SourceChain: Chain>(
	source_client: &Client<SourceChain>,
	header_hash: SourceChain::Hash,
) -> Result<SourceChain::Header, Error<SourceChain::Hash, <SourceChain::Header as HeaderT>::Number>>
{
	source_client
		.header_by_hash(header_hash)
		.await
		.map_err(|err| Error::RetrieveHeader(SourceChain::NAME, header_hash, err))
}

/// Read GRANDPA authorities set at given header.
async fn source_authorities_set<SourceChain: Chain>(
	source_client: &Client<SourceChain>,
	header_hash: SourceChain::Hash,
) -> Result<GrandpaAuthoritiesSet, Error<SourceChain::Hash, <SourceChain::Header as HeaderT>::Number>>
{
	let raw_authorities_set = source_client
		.grandpa_authorities_set(header_hash)
		.await
		.map_err(|err| Error::RetrieveAuthorities(SourceChain::NAME, header_hash, err))?;
	GrandpaAuthoritiesSet::decode(&mut &raw_authorities_set[..])
		.map_err(|err| Error::DecodeAuthorities(SourceChain::NAME, header_hash, err))
}
