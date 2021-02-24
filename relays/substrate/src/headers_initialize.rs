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

//! Initialize Substrate -> Substrate headers bridge.
//!
//! Initialization is a transaction that calls `initialize()` function of the
//! `pallet-substrate-bridge` pallet. This transaction brings initial header
//! and authorities set from source to target chain. The headers sync starts
//! with this header.

use codec::Decode;
use pallet_substrate_bridge::InitializationData;
use relay_substrate_client::{Chain, Client};
use sp_core::Bytes;
use sp_finality_grandpa::{AuthorityList as GrandpaAuthoritiesSet, SetId as GrandpaAuthoritiesSetId};

/// Submit headers-bridge initialization transaction.
pub async fn initialize<SourceChain: Chain, TargetChain: Chain>(
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	raw_initial_header: Option<Bytes>,
	raw_initial_authorities_set: Option<Bytes>,
	initial_authorities_set_id: Option<GrandpaAuthoritiesSetId>,
	prepare_initialize_transaction: impl FnOnce(InitializationData<SourceChain::Header>) -> Result<Bytes, String>,
) {
	let result = do_initialize(
		source_client,
		target_client,
		raw_initial_header,
		raw_initial_authorities_set,
		initial_authorities_set_id,
		prepare_initialize_transaction,
	)
	.await;

	match result {
		Ok(tx_hash) => log::info!(
			target: "bridge",
			"Successfully submitted {}-headers bridge initialization transaction to {}: {:?}",
			SourceChain::NAME,
			TargetChain::NAME,
			tx_hash,
		),
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
async fn do_initialize<SourceChain: Chain, TargetChain: Chain>(
	source_client: Client<SourceChain>,
	target_client: Client<TargetChain>,
	raw_initial_header: Option<Bytes>,
	raw_initial_authorities_set: Option<Bytes>,
	initial_authorities_set_id: Option<GrandpaAuthoritiesSetId>,
	prepare_initialize_transaction: impl FnOnce(InitializationData<SourceChain::Header>) -> Result<Bytes, String>,
) -> Result<TargetChain::Hash, String> {
	let initialization_data = prepare_initialization_data(
		source_client,
		raw_initial_header,
		raw_initial_authorities_set,
		initial_authorities_set_id,
	)
	.await?;
	let initialization_tx = prepare_initialize_transaction(initialization_data)?;
	let initialization_tx_hash = target_client
		.submit_extrinsic(initialization_tx)
		.await
		.map_err(|err| format!("Failed to submit {} transaction: {:?}", TargetChain::NAME, err))?;
	Ok(initialization_tx_hash)
}

/// Prepare initialization data for the headers-bridge pallet.
async fn prepare_initialization_data<SourceChain: Chain>(
	source_client: Client<SourceChain>,
	raw_initial_header: Option<Bytes>,
	raw_initial_authorities_set: Option<Bytes>,
	initial_authorities_set_id: Option<GrandpaAuthoritiesSetId>,
) -> Result<InitializationData<SourceChain::Header>, String> {
	let source_genesis_hash = *source_client.genesis_hash();

	let initial_header = match raw_initial_header {
		Some(raw_initial_header) => SourceChain::Header::decode(&mut &raw_initial_header.0[..])
			.map_err(|err| format!("Failed to decode {} initial header: {:?}", SourceChain::NAME, err))?,
		None => source_client
			.header_by_hash(source_genesis_hash)
			.await
			.map_err(|err| format!("Failed to retrive {} genesis header: {:?}", SourceChain::NAME, err))?,
	};

	let raw_initial_authorities_set = match raw_initial_authorities_set {
		Some(raw_initial_authorities_set) => raw_initial_authorities_set.0,
		None => source_client
			.grandpa_authorities_set(source_genesis_hash)
			.await
			.map_err(|err| {
				format!(
					"Failed to retrive {} authorities set at genesis header: {:?}",
					SourceChain::NAME,
					err
				)
			})?,
	};
	let initial_authorities_set =
		GrandpaAuthoritiesSet::decode(&mut &raw_initial_authorities_set[..]).map_err(|err| {
			format!(
				"Failed to decode {} initial authorities set: {:?}",
				SourceChain::NAME,
				err
			)
		})?;

	Ok(InitializationData {
		header: initial_header,
		authority_list: initial_authorities_set,
		set_id: initial_authorities_set_id.unwrap_or(0),
		// There may be multiple scheduled changes, so on real chains we should select proper
		// moment, when there's nothing scheduled. On ephemeral (temporary) chains, it is ok to
		// start with genesis.
		scheduled_change: None,
		is_halted: false,
	})
}
