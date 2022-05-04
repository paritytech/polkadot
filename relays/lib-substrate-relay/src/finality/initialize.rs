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

//! Initialize Substrate -> Substrate finality bridge.
//!
//! Initialization is a transaction that calls `initialize()` function of the
//! finality pallet (GRANDPA/BEEFY/...). This transaction brings initial header
//! and authorities set from source to target chain. The finality sync starts
//! with this header.

use crate::{error::Error, finality::engine::Engine};

use relay_substrate_client::{BlockNumberOf, Chain, Client, Error as SubstrateError, HashOf};
use sp_core::Bytes;
use sp_runtime::traits::Header as HeaderT;

/// Submit headers-bridge initialization transaction.
pub async fn initialize<E: Engine<SourceChain>, SourceChain: Chain, TargetChain: Chain, F>(
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	target_transactions_signer: TargetChain::AccountId,
	prepare_initialize_transaction: F,
) where
	F: FnOnce(TargetChain::Index, E::InitializationData) -> Result<Bytes, SubstrateError>
		+ Send
		+ 'static,
{
	let result = do_initialize::<E, _, _, _>(
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
async fn do_initialize<E: Engine<SourceChain>, SourceChain: Chain, TargetChain: Chain, F>(
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	target_transactions_signer: TargetChain::AccountId,
	prepare_initialize_transaction: F,
) -> Result<
	Option<TargetChain::Hash>,
	Error<SourceChain::Hash, <SourceChain::Header as HeaderT>::Number>,
>
where
	F: FnOnce(TargetChain::Index, E::InitializationData) -> Result<Bytes, SubstrateError>
		+ Send
		+ 'static,
{
	let is_initialized = is_initialized::<E, SourceChain, TargetChain>(&target_client).await?;
	if is_initialized {
		log::info!(
			target: "bridge",
			"{}-headers bridge at {} is already initialized. Skipping",
			SourceChain::NAME,
			TargetChain::NAME,
		);
		return Ok(None)
	}

	let initialization_data = E::prepare_initialization_data(source_client).await?;
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
async fn is_initialized<E: Engine<SourceChain>, SourceChain: Chain, TargetChain: Chain>(
	target_client: &Client<TargetChain>,
) -> Result<bool, Error<HashOf<SourceChain>, BlockNumberOf<SourceChain>>> {
	Ok(target_client
		.raw_storage_value(E::is_initialized_key(), None)
		.await
		.map_err(|err| Error::RetrieveBestFinalizedHeaderHash(SourceChain::NAME, err))?
		.is_some())
}
